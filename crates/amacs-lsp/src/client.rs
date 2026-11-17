//! A running language server and the pipes to it.

use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::transport::{read_message, write_message};

/// Lines of the server's standard error that are kept.
const STDERR_LINES: usize = 200;

/// What arrives from a server.
#[derive(Debug)]
pub enum Incoming {
    Message(Value),
    /// The server's output ended: it exited or closed the pipe. The text
    /// says why, if anything is known.
    Closed(String),
}

/// A language server process.
///
/// Messages to it are queued and written by a thread, so that a server that
/// is slow to read never blocks the editor. Messages from it are handed to
/// a callback on another thread.
pub struct Client {
    child: Child,
    outgoing: Sender<Value>,
    next_id: i64,
    stderr: Arc<Mutex<VecDeque<String>>>,
}

impl Client {
    /// Starts `program` in `directory`. `environment` lists variables to set
    /// (`Some`) or remove (`None`) relative to the editor's own. Everything
    /// the server sends is passed to `on_incoming`, which runs on a thread
    /// of its own.
    pub fn spawn(
        program: &str,
        args: &[String],
        directory: &Path,
        environment: &[(String, Option<String>)],
        on_incoming: impl Fn(Incoming) + Send + 'static,
    ) -> io::Result<Self> {
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (name, value) in environment {
            match value {
                Some(value) => command.env(name, value),
                None => command.env_remove(name),
            };
        }
        let mut child = command.spawn()?;
        let pipe = || io::Error::other("the server's pipes could not be opened");
        let mut stdin = child.stdin.take().ok_or_else(pipe)?;
        let stdout = child.stdout.take().ok_or_else(pipe)?;
        let stderr_pipe = child.stderr.take().ok_or_else(pipe)?;

        let (outgoing, queue) = mpsc::channel::<Value>();
        std::thread::spawn(move || {
            for message in queue {
                if write_message(&mut stdin, &message).is_err() {
                    break;
                }
            }
        });

        let stderr = Arc::new(Mutex::new(VecDeque::new()));
        let kept = stderr.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr_pipe).lines() {
                let Ok(line) = line else { break };
                if let Ok(mut kept) = kept.lock() {
                    if kept.len() == STDERR_LINES {
                        kept.pop_front();
                    }
                    kept.push_back(line);
                }
            }
        });

        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let reason = loop {
                match read_message(&mut reader) {
                    Ok(Some(message)) => on_incoming(Incoming::Message(message)),
                    Ok(None) => break "the server exited".to_owned(),
                    Err(error) => break format!("cannot read from the server: {error}"),
                }
            };
            on_incoming(Incoming::Closed(reason));
        });

        Ok(Self {
            child,
            outgoing,
            next_id: 0,
            stderr,
        })
    }

    fn send(&self, message: Value) {
        // If the writer has gone, the reader reports the server as closed.
        let _ = self.outgoing.send(message);
    }

    /// Sends a request and returns its id, which the response will carry.
    pub fn request(&mut self, method: &str, params: Value) -> i64 {
        self.next_id += 1;
        self.send(json!({
            "jsonrpc": "2.0",
            "id": self.next_id,
            "method": method,
            "params": params,
        }));
        self.next_id
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    /// Answers a request that the server made.
    pub fn respond(&self, id: Value, result: Value) {
        self.send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    /// Refuses a request that the server made.
    pub fn respond_error(&self, id: Value, code: i64, message: &str) {
        self.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": code, "message": message},
        }));
    }

    /// The last lines the server wrote to its standard error.
    pub fn stderr_tail(&self) -> Vec<String> {
        self.stderr
            .lock()
            .map(|kept| kept.iter().cloned().collect())
            .unwrap_or_default()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // A server that is still running after being told to exit, or that
        // was never told, must not outlive the editor.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
