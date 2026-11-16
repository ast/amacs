//! The Wayland clipboard, through the `wl-copy` and `wl-paste` programs.
//!
//! The server itself has no connection to the compositor, so that it can
//! outlive one. These programs are what gives it a clipboard anyway, for
//! terminal and graphical frames alike.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use amacs_core::Clipboard;

const TEXT: &str = "text/plain;charset=utf-8";
/// A clipboard owner that does not answer must not hang the editor.
const PASTE_TIMEOUT: Duration = Duration::from_millis(500);

pub struct WaylandClipboard;

impl WaylandClipboard {
    /// The clipboard, if there is a compositor and the programs are installed.
    pub fn detect() -> Option<Self> {
        std::env::var_os("WAYLAND_DISPLAY")?;
        let found = |program: &str| {
            Command::new(program)
                .arg("--version")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok()
        };
        (found("wl-copy") && found("wl-paste")).then_some(Self)
    }
}

/// Runs `wl-paste` and returns what it prints, or `None` if it fails, as it
/// does when the clipboard is empty or holds no text.
fn paste(args: &[&str]) -> Option<String> {
    let mut child = Command::new("wl-paste")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let (sender, output) = mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        let read = stdout.read_to_string(&mut text).is_ok();
        let _ = sender.send(read.then_some(text));
    });
    match output.recv_timeout(PASTE_TIMEOUT) {
        Ok(text) => {
            let succeeded = child.wait().is_ok_and(|status| status.success());
            text.filter(|_| succeeded)
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            None
        }
    }
}

impl Clipboard for WaylandClipboard {
    fn set(&mut self, text: &str) {
        let Ok(mut child) = Command::new("wl-copy")
            .args(["--type", TEXT])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            return;
        };
        // wl-copy reads the text and then stays around to serve it, so it is
        // fed and reaped from a thread of its own.
        let text = text.to_owned();
        std::thread::spawn(move || {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
        });
    }

    fn get(&mut self) -> Option<String> {
        paste(&["--no-newline", "--type", TEXT]).or_else(|| paste(&["--no-newline"]))
    }
}
