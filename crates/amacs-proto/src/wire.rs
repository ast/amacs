//! The protocol between the amacs server and its clients.
//!
//! Messages travel over a Unix socket. Each is a little-endian `u32` length
//! followed by that many bytes of postcard-encoded data. A client starts with
//! [`ClientMsg::Hello`] and then makes one request.

use std::io::{self, Read, Write};
use std::path::PathBuf;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::{DrawOp, InputEvent};

/// Changes whenever a message changes shape. Client and server must agree.
pub const PROTOCOL_VERSION: u32 = 9;

/// Larger messages are refused, so that a corrupt length cannot make the
/// reader allocate without bound.
const MAX_MESSAGE: usize = 64 * 1024 * 1024;

/// An error from evaluating Scheme, in parts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvalFailure {
    /// One line: what went wrong.
    pub message: String,
    /// Where, as `file:line:column`.
    pub location: Option<String>,
    /// The calls that were in progress, innermost first: the procedure's
    /// name and where it had got to. Either can be unknown.
    pub trace: Vec<(Option<String>, Option<String>)>,
}

/// A file to visit, with an optional position to go to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSpec {
    /// An absolute path.
    pub path: PathBuf,
    /// Line to go to, counting from 1.
    pub line: Option<u32>,
    /// Column on that line, counting from 0.
    pub column: Option<u32>,
}

/// Whether an edit request comes with somewhere to show a frame: a terminal
/// or a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FrameRequest {
    /// Use whatever frame is current. The client shows nothing.
    None,
    /// Open a frame for the client, of this size in character cells.
    New { cols: u16, rows: u16 },
    /// Open a frame for the client only if no client has one.
    NewIfNone { cols: u16, rows: u16 },
    /// Open a frame that no person looks at, for a program that drives the
    /// editor and reads what is drawn. It does not count as a frame when
    /// others ask whether there is one, and does not become the frame that
    /// clients without one act in.
    Headless { cols: u16, rows: u16 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMsg {
    /// Always the first message.
    Hello { version: u32 },
    /// Ask to be told when everything sent before this has been handled
    /// and what it changed on the client's frame has been sent. Answered
    /// with [`ServerMsg::Synced`], after the draw.
    Sync,
    /// Evaluate Scheme source. Answered with [`ServerMsg::EvalResult`].
    Eval { source: String },
    /// Visit files. Answered with [`ServerMsg::Attached`] if the client is to
    /// show a frame, else [`ServerMsg::Opened`]. With `wait`, the server
    /// sends [`ServerMsg::Exit`] once every file's buffer has been finished.
    Edit {
        files: Vec<FileSpec>,
        wait: bool,
        frame: FrameRequest,
    },
    /// Input for the client's frame.
    Input(InputEvent),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerMsg {
    /// The client has a frame: it must send input and apply draw operations.
    /// The font is what the user configured, for clients that draw text
    /// themselves. The size is in pixels.
    Attached {
        font_family: String,
        font_size: u16,
    },
    /// The files were opened in an existing frame.
    Opened,
    Draw(Vec<DrawOp>),
    /// The printed value of an [`ClientMsg::Eval`], or the error report.
    EvalResult {
        ok: bool,
        text: String,
        /// What the code wrote with `display` and its like.
        output: String,
        /// The error taken apart, for programs.
        failure: Option<EvalFailure>,
    },
    /// The answer to [`ClientMsg::Sync`].
    Synced,
    /// The client is done and should exit with this status.
    Exit {
        message: Option<String>,
        status: u8,
    },
}

/// Writes one message and flushes.
pub fn write_msg<W: Write, T: Serialize>(mut writer: W, message: &T) -> io::Result<()> {
    let bytes = postcard::to_stdvec(message).map_err(io::Error::other)?;
    let len = u32::try_from(bytes.len())
        .ok()
        .filter(|len| *len as usize <= MAX_MESSAGE)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "message too large"))?;
    writer.write_all(&len.to_le_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()
}

/// Reads one message. Returns `None` if the stream ended cleanly before one.
pub fn read_msg<R: Read, T: DeserializeOwned>(mut reader: R) -> io::Result<Option<T>> {
    let mut len = [0; 4];
    // Distinguish a clean end of stream from one in the middle of a message.
    match reader.read(&mut len)? {
        0 => return Ok(None),
        read => reader.read_exact(&mut len[read..])?,
    }
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_MESSAGE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "message too large",
        ));
    }
    let mut bytes = vec![0; len];
    reader.read_exact(&mut bytes)?;
    postcard::from_bytes(&bytes)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Where the server listens: `$AMACS_SOCKET` if set, else
/// `$XDG_RUNTIME_DIR/amacs/server.sock`, else a per-user directory under the
/// temporary directory.
pub fn socket_path() -> PathBuf {
    if let Some(path) = std::env::var_os("AMACS_SOCKET").filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
    let dir = match std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty()) {
        Some(runtime) => PathBuf::from(runtime).join("amacs"),
        None => {
            let user = std::env::var("USER").unwrap_or_else(|_| "user".to_owned());
            std::env::temp_dir().join(format!("amacs-{user}"))
        }
    };
    dir.join("server.sock")
}

/// Where a server listening on `socket` writes its process id, so that it
/// can be stopped by a client that cannot talk to it.
pub fn pid_path(socket: &std::path::Path) -> PathBuf {
    let mut name = socket.as_os_str().to_owned();
    name.push(".pid");
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DisplayLine, FaceId, Key, PaneId};
    use proptest::prelude::*;

    fn round_trip<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(message: &T) {
        let mut bytes = Vec::new();
        write_msg(&mut bytes, message).unwrap();
        let mut reader = bytes.as_slice();
        assert_eq!(
            read_msg::<_, T>(&mut reader).unwrap().as_ref(),
            Some(message)
        );
        assert_eq!(
            read_msg::<_, T>(&mut reader).unwrap(),
            None,
            "then a clean end"
        );
    }

    #[test]
    fn messages_round_trip() {
        round_trip(&ClientMsg::Hello {
            version: PROTOCOL_VERSION,
        });
        round_trip(&ClientMsg::Input(InputEvent::Key(
            "C-M-s-ö".parse::<Key>().unwrap(),
        )));
        round_trip(&ClientMsg::Edit {
            files: vec![FileSpec {
                path: "/tmp/日本.txt".into(),
                line: Some(3),
                column: None,
            }],
            wait: true,
            frame: FrameRequest::NewIfNone { cols: 80, rows: 24 },
        });
        round_trip(&ServerMsg::Draw(vec![
            DrawOp::Lines {
                pane: PaneId(3),
                first_row: 1,
                lines: vec![DisplayLine::plain("text", FaceId(2))],
            },
            DrawOp::Flush,
        ]));
        round_trip(&ServerMsg::Exit {
            message: Some("bye".into()),
            status: 1,
        });
    }

    #[test]
    fn a_truncated_message_is_an_error_not_an_end() {
        let mut bytes = Vec::new();
        write_msg(
            &mut bytes,
            &ClientMsg::Eval {
                source: "(+ 1 2)".into(),
            },
        )
        .unwrap();
        for cut in 1..bytes.len() {
            assert!(
                read_msg::<_, ClientMsg>(&bytes[..cut]).is_err(),
                "cut at {cut}"
            );
        }
    }

    #[test]
    fn an_absurd_length_is_refused_without_allocating() {
        let bytes = u32::MAX.to_le_bytes();
        let error = read_msg::<_, ClientMsg>(bytes.as_slice()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    proptest! {
        #[test]
        fn arbitrary_bytes_never_panic_the_decoder(bytes in prop::collection::vec(any::<u8>(), 0..200)) {
            let mut framed = (bytes.len() as u32).to_le_bytes().to_vec();
            framed.extend(&bytes);
            let _ = read_msg::<_, ClientMsg>(framed.as_slice());
            let _ = read_msg::<_, ServerMsg>(framed.as_slice());
            let _ = read_msg::<_, ClientMsg>(bytes.as_slice());
        }
    }
}
