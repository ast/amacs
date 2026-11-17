//! `amacsclient`: asks a running amacs server to visit files, evaluate
//! Scheme, or open a frame in this terminal. Starts the server if needed.

use std::ffi::OsString;
use std::io::IsTerminal;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use amacs_client::{Connection, parse_position, sibling_binary};
use amacs_proto::{ClientMsg, FileSpec, FrameRequest, ServerMsg, read_msg, socket_path, write_msg};
use amacs_tty::Tty;
use anyhow::{Context, Result, anyhow, bail};

const USAGE: &str = "Usage: amacsclient [OPTION]... [+LINE[:COLUMN]] [FILE]...

Visit FILEs in the amacs server and wait until they are finished with C-x #.
The server is started if it is not running.

Options:
  -t, --tty               Open a frame in this terminal
  -c, --create-frame      Open a frame in a new window
  -n, --no-wait           Return at once instead of waiting for the files
  -e, --eval EXPR         Evaluate a Scheme expression and print its value
      --kill-server       Stop the server, saving what it can for recovery
  -s, --socket-name PATH  Use this socket instead of the default
  -h, --help              Show this help
  -V, --version           Show the version

Without -t, a frame is opened in this terminal only if no client has one.";

#[derive(Default)]
struct Options {
    files: Vec<FileSpec>,
    evals: Vec<String>,
    tty: bool,
    create_frame: bool,
    no_wait: bool,
    kill_server: bool,
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
            "-c" | "--create-frame" => options.create_frame = true,
            "-n" | "--no-wait" => options.no_wait = true,
            "--kill-server" => options.kill_server = true,
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
    if options.tty && options.create_frame {
        bail!("--tty and --create-frame cannot be combined");
    }
    let wants_frame = options.tty || options.create_frame;
    if options.kill_server {
        return Ok(Some(options));
    }
    if options.files.is_empty() && options.evals.is_empty() && !wants_frame {
        bail!("nothing to do: give a file, --eval, --tty or --create-frame\n{USAGE}");
    }
    Ok(Some(options))
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
    if options.kill_server {
        return kill_server(&socket);
    }
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
    if options.create_frame {
        // The window is a program of its own, which with --no-wait has
        // nowhere to say that the server would not have it. Ask something
        // here first: a server of another build answers by refusing.
        connection.send(&ClientMsg::Eval {
            source: "#t".to_owned(),
        })?;
        if let ServerMsg::Exit { message, status } = connection.receive()? {
            return Ok(exit(message, status.max(1)));
        }
        return open_window(&options, &socket);
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
                FrameRequest::New { cols, rows }
            } else {
                FrameRequest::NewIfNone { cols, rows }
            }
        }
    };
    connection.send(&ClientMsg::Edit {
        files: options.files,
        wait: !options.no_wait,
        frame,
    })?;

    match connection.receive()? {
        ServerMsg::Attached { .. } => run_frame(connection),
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

/// Stops the server that listens on `socket`, without talking to it, so
/// that a server of another build can be stopped too. It is asked to
/// terminate, which makes it save what is unsaved for recovery first.
fn kill_server(socket: &Path) -> Result<ExitCode> {
    use std::os::unix::net::UnixStream;

    let listening = || UnixStream::connect(socket).is_ok();
    let pid_file = amacs_proto::pid_path(socket);
    let pid = std::fs::read_to_string(&pid_file)
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok());
    let Some(pid) = pid else {
        if listening() {
            bail!(
                "a server is running, but it is from a build that did not record its \
                 process id; stop it with `pkill -x amacs`"
            );
        }
        println!("No server is running");
        return Ok(ExitCode::SUCCESS);
    };
    // The id may be left over from a server that is gone, and belong to
    // something else by now.
    let name = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
    if name.trim() != "amacs" {
        let _ = std::fs::remove_file(&pid_file);
        println!("No server is running");
        return Ok(ExitCode::SUCCESS);
    }
    let asked = Command::new("kill")
        .arg(pid.to_string())
        .status()
        .context("cannot run kill")?;
    if !asked.success() {
        bail!("could not stop the server (process {pid})");
    }
    let started = std::time::Instant::now();
    while listening() {
        if started.elapsed() > std::time::Duration::from_secs(10) {
            bail!("the server (process {pid}) was asked to stop but is still running");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    println!("Server stopped");
    Ok(ExitCode::SUCCESS)
}

/// Hands over to the graphical client, which opens a window for the files.
/// With `--no-wait` it runs on its own and this program returns at once.
fn open_window(options: &Options, socket: &Path) -> Result<ExitCode> {
    let binary = sibling_binary("amacs-gui");
    let mut command = Command::new(&binary);
    command.arg("--socket-name").arg(socket);
    if options.no_wait {
        command.arg("--no-wait");
    }
    for file in &options.files {
        if let Some(line) = file.line {
            let column = file
                .column
                .map(|column| format!(":{column}"))
                .unwrap_or_default();
            command.arg(format!("+{line}{column}"));
        }
        command.arg(&file.path);
    }
    if options.no_wait {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .with_context(|| format!("cannot start {}", binary.display()))?;
        return Ok(ExitCode::SUCCESS);
    }
    // Only returns if the program could not be started.
    Err(command.exec()).with_context(|| format!("cannot start {}", binary.display()))
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
