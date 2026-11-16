//! `amacsclient`: asks a running amacs server to visit files, evaluate
//! Scheme, or open a frame in this terminal. Starts the server if needed.

use std::ffi::OsString;
use std::io::{BufReader, BufWriter, ErrorKind, IsTerminal};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use amacs_proto::{
    ClientMsg, FileSpec, FrameRequest, PROTOCOL_VERSION, ServerMsg, read_msg, socket_path,
    write_msg,
};
use amacs_tty::Tty;
use anyhow::{Context, Result, anyhow, bail};

const USAGE: &str = "Usage: amacsclient [OPTION]... [+LINE[:COLUMN]] [FILE]...

Visit FILEs in the amacs server and wait until they are finished with C-x #.
The server is started if it is not running.

Options:
  -t, --tty               Open a frame in this terminal
  -n, --no-wait           Return at once instead of waiting for the files
  -e, --eval EXPR         Evaluate a Scheme expression and print its value
  -s, --socket-name PATH  Use this socket instead of the default
  -h, --help              Show this help
  -V, --version           Show the version

Without -t, a frame is opened in this terminal only if no client has one.";

/// How long to wait for a server that was just started.
const START_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Default)]
struct Options {
    files: Vec<FileSpec>,
    evals: Vec<String>,
    tty: bool,
    no_wait: bool,
    socket: Option<PathBuf>,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("amacsclient: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Parses `LINE` or `LINE:COLUMN`.
fn parse_position(text: &str) -> Option<(u32, Option<u32>)> {
    match text.split_once(':') {
        Some((line, column)) => Some((line.parse().ok()?, Some(column.parse().ok()?))),
        None => Some((text.parse().ok()?, None)),
    }
}

fn parse_args() -> Result<Option<Options>> {
    let mut options = Options::default();
    let mut args = std::env::args_os().skip(1);
    let mut options_done = false;
    // A `+LINE` argument applies to the file that follows it.
    let mut position = None;
    while let Some(arg) = args.next() {
        let text = arg.to_str().filter(|_| !options_done);
        if let Some(parsed) = text
            .and_then(|s| s.strip_prefix('+'))
            .and_then(parse_position)
        {
            position = Some(parsed);
            continue;
        }
        let Some(flag) = text.filter(|s| s.starts_with('-') && *s != "-") else {
            let path = std::path::absolute(Path::new(&arg)).context("cannot resolve file name")?;
            let (line, column) = position.take().unzip();
            options.files.push(FileSpec {
                path,
                line,
                column: column.flatten(),
            });
            continue;
        };
        let mut value = |what: &str| -> Result<OsString> {
            args.next()
                .ok_or_else(|| anyhow!("{flag} needs {what}\n{USAGE}"))
        };
        match flag {
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("amacsclient {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--" => options_done = true,
            "-t" | "--tty" => options.tty = true,
            "-n" | "--no-wait" => options.no_wait = true,
            "-s" | "--socket-name" => options.socket = Some(value("a path")?.into()),
            "-e" | "--eval" => {
                let expr = value("an expression")?
                    .into_string()
                    .map_err(|_| anyhow!("--eval needs valid UTF-8"))?;
                options.evals.push(expr);
            }
            _ => bail!("unknown option `{flag}`\n{USAGE}"),
        }
    }
    if options.files.is_empty() && options.evals.is_empty() && !options.tty {
        bail!("nothing to do: give a file, --eval or --tty\n{USAGE}");
    }
    Ok(Some(options))
}

/// The `amacs` binary to start: the one installed beside this program, or
/// else whichever is on the path.
fn server_binary() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join("amacs")))
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from("amacs"))
}

/// Connects to the server, starting one first if none is listening.
fn connect(socket: &Path) -> Result<UnixStream> {
    let refused = |error: &std::io::Error| {
        matches!(
            error.kind(),
            ErrorKind::NotFound | ErrorKind::ConnectionRefused
        )
    };
    match UnixStream::connect(socket) {
        Ok(stream) => return Ok(stream),
        Err(error) if refused(&error) => {}
        Err(error) => {
            return Err(error).with_context(|| format!("cannot connect to {}", socket.display()));
        }
    }

    let binary = server_binary();
    Command::new(&binary)
        .arg("--daemon")
        .env("AMACS_SOCKET", socket)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // Its own process group, so that C-c in this terminal does not
        // reach the server.
        .process_group(0)
        .spawn()
        .with_context(|| format!("cannot start the server ({})", binary.display()))?;

    let started = Instant::now();
    loop {
        match UnixStream::connect(socket) {
            Ok(stream) => return Ok(stream),
            Err(error) if refused(&error) && started.elapsed() < START_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("the server did not start listening on {}", socket.display())
                });
            }
        }
    }
}

struct Connection {
    reader: BufReader<UnixStream>,
    writer: BufWriter<UnixStream>,
}

impl Connection {
    fn open(socket: &Path) -> Result<Self> {
        let stream = connect(socket)?;
        let mut connection = Self {
            reader: BufReader::new(stream.try_clone()?),
            writer: BufWriter::new(stream),
        };
        connection.send(&ClientMsg::Hello {
            version: PROTOCOL_VERSION,
        })?;
        Ok(connection)
    }

    fn send(&mut self, message: &ClientMsg) -> Result<()> {
        write_msg(&mut self.writer, message).context("cannot write to the server")
    }

    fn receive(&mut self) -> Result<ServerMsg> {
        read_msg(&mut self.reader)
            .context("cannot read from the server")?
            .ok_or_else(|| anyhow!("lost the connection to the server"))
    }
}

/// Prints what the server said on exit and turns its status into ours.
fn exit(message: Option<String>, status: u8) -> ExitCode {
    if let Some(message) = message {
        eprintln!("{message}");
    }
    ExitCode::from(status)
}

fn run() -> Result<ExitCode> {
    let Some(options) = parse_args()? else {
        return Ok(ExitCode::SUCCESS);
    };
    let socket = options.socket.clone().unwrap_or_else(socket_path);
    let mut connection = Connection::open(&socket)?;

    for source in &options.evals {
        connection.send(&ClientMsg::Eval {
            source: source.clone(),
        })?;
        match connection.receive()? {
            ServerMsg::EvalResult { ok: true, text } => println!("{text}"),
            ServerMsg::EvalResult { ok: false, text } => {
                eprintln!("{text}");
                return Ok(ExitCode::FAILURE);
            }
            ServerMsg::Exit { message, status } => return Ok(exit(message, status)),
            other => bail!("unexpected reply from the server: {other:?}"),
        }
    }
    if options.files.is_empty() && !options.tty {
        return Ok(ExitCode::SUCCESS);
    }

    let on_terminal = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let frame = match (options.tty, on_terminal) {
        (true, false) => bail!("--tty needs standard input and output to be a terminal"),
        (_, false) => FrameRequest::None,
        (tty, true) => {
            let (cols, rows) =
                amacs_tty::terminal_size().context("cannot read the terminal size")?;
            if tty {
                FrameRequest::Tty { cols, rows }
            } else {
                FrameRequest::TtyIfNone { cols, rows }
            }
        }
    };
    connection.send(&ClientMsg::Edit {
        files: options.files,
        wait: !options.no_wait,
        frame,
    })?;

    match connection.receive()? {
        ServerMsg::Attached => run_frame(connection),
        ServerMsg::Opened => loop {
            // Nothing to show: wait to be told that the files are finished.
            if let ServerMsg::Exit { message, status } = connection.receive()? {
                return Ok(exit(message, status));
            }
        },
        ServerMsg::Exit { message, status } => Ok(exit(message, status)),
        other => bail!("unexpected reply from the server: {other:?}"),
    }
}

/// Shows the server's frame in this terminal until the server ends it.
fn run_frame(connection: Connection) -> Result<ExitCode> {
    let Connection {
        mut reader,
        mut writer,
    } = connection;
    let mut tty = Tty::new().context("cannot set up the terminal")?;

    // Input goes to the server from its own thread. It ends with the
    // process, or when the server stops listening.
    std::thread::spawn(move || {
        while let Ok(event) = amacs_tty::read_event() {
            if write_msg(&mut writer, &ClientMsg::Input(event)).is_err() {
                break;
            }
        }
    });

    loop {
        match read_msg::<_, ServerMsg>(&mut reader) {
            Ok(Some(ServerMsg::Draw(ops))) => tty.apply(&ops)?,
            Ok(Some(ServerMsg::Exit { message, status })) => {
                // Restore the terminal before printing to it.
                drop(tty);
                return Ok(exit(message, status));
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {
                drop(tty);
                bail!("lost the connection to the server");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_position;

    #[test]
    fn positions_are_a_line_and_an_optional_column() {
        assert_eq!(parse_position("12"), Some((12, None)));
        assert_eq!(parse_position("12:4"), Some((12, Some(4))));
        assert_eq!(parse_position("x"), None);
        assert_eq!(parse_position("12:"), None);
        assert_eq!(parse_position(""), None);
    }
}
