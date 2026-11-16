//! Connecting to the amacs server, shared by `amacsclient` and the
//! graphical client.

use std::io::{BufReader, BufWriter, ErrorKind};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use amacs_proto::{ClientMsg, PROTOCOL_VERSION, ServerMsg, read_msg, write_msg};
use anyhow::{Context, Result, anyhow};

/// How long to wait for a server that was just started.
const START_TIMEOUT: Duration = Duration::from_secs(10);

/// A program installed beside the running one, or else whichever of that
/// name is on the path.
pub fn sibling_binary(name: &str) -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join(name)))
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from(name))
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

    let binary = sibling_binary("amacs");
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

/// A greeted connection to the server.
pub struct Connection {
    pub reader: BufReader<UnixStream>,
    pub writer: BufWriter<UnixStream>,
}

impl Connection {
    /// Connects, starting the server if needed, and sends the greeting.
    pub fn open(socket: &Path) -> Result<Self> {
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

    pub fn send(&mut self, message: &ClientMsg) -> Result<()> {
        write_msg(&mut self.writer, message).context("cannot write to the server")
    }

    /// The next message. The connection closing counts as an error.
    pub fn receive(&mut self) -> Result<ServerMsg> {
        read_msg(&mut self.reader)
            .context("cannot read from the server")?
            .ok_or_else(|| anyhow!("lost the connection to the server"))
    }
}

/// Parses `LINE` or `LINE:COLUMN`, as given after `+` on the command line.
pub fn parse_position(text: &str) -> Option<(u32, Option<u32>)> {
    match text.split_once(':') {
        Some((line, column)) => Some((line.parse().ok()?, Some(column.parse().ok()?))),
        None => Some((text.parse().ok()?, None)),
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
