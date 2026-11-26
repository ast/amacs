//! Where the questions of programs come in: a socket of its own, on which
//! `amacsclient --askpass` hands over what `ssh`, `sudo` or git asks, and
//! waits for the answer. See [`amacs_core::askpass`] for the whole of it.
//!
//! The socket is apart from the one that clients connect to, so that the
//! editor in a terminal, which has no clients, is asked the same way as
//! a server. Beside it is a link to the helper under a name that says
//! what it is for: `ssh` and `sudo` take the path of a program to run,
//! with no arguments of the editor's choosing.

use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use amacs_core::Waker;
use amacs_proto::{Answer, Ask, read_msg, socket_path, write_msg};

/// The socket and the link, removed again on drop.
pub struct Askpass {
    made: [PathBuf; 2],
}

impl Drop for Askpass {
    fn drop(&mut self) {
        for path in &self.made {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// `amacsclient`: the one beside this program, or else the one that the
/// path leads to.
fn helper() -> Option<PathBuf> {
    const NAME: &str = "amacsclient";
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join(NAME)))
        .filter(|path| path.exists());
    beside.or_else(|| {
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|dir| dir.join(NAME))
            .find(|path| path.is_file())
    })
}

/// Removes what editors that are gone left behind in a directory.
fn sweep(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let pid = name
            .to_str()
            .and_then(|name| name.strip_prefix("askpass-"))
            .map(|rest| rest.strip_suffix(".sock").unwrap_or(rest))
            .and_then(|pid| pid.parse::<u32>().ok());
        if pid.is_some_and(|pid| !Path::new(&format!("/proc/{pid}")).exists()) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Starts to take questions, and tells the editor's core that they are
/// answered: `waker` makes the editor look at what came in. If that
/// cannot be, for want of the helper or of a place for the socket, the
/// error says why; then programs are told to ask nothing, as before.
pub fn start(waker: Waker) -> Result<Askpass, String> {
    let helper = helper().ok_or("amacsclient was not found beside amacs or on the path")?;
    let dir = socket_path()
        .parent()
        .ok_or("there is no directory for the socket")?
        .to_owned();
    fn failed(what: &'static str, path: &Path) -> impl Fn(std::io::Error) -> String {
        let path = path.display().to_string();
        move |error| format!("cannot {what} {path}: {error}")
    }
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .map_err(failed("create", &dir))?;
    sweep(&dir);
    let pid = std::process::id();
    let link = dir.join(format!("askpass-{pid}"));
    let socket = dir.join(format!("askpass-{pid}.sock"));
    for path in [&link, &socket] {
        let _ = std::fs::remove_file(path);
    }
    std::os::unix::fs::symlink(&helper, &link).map_err(failed("make the link", &link))?;
    let made = Askpass {
        made: [link.clone(), socket.clone()],
    };
    let listener = UnixListener::bind(&socket).map_err(failed("listen on", &socket))?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))
        .map_err(failed("protect", &socket))?;
    std::thread::Builder::new()
        .name("askpass".to_owned())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                // Each question waits for the user on a thread of its
                // own, so that one does not hold up the next.
                let _ = std::thread::Builder::new()
                    .name("askpass-question".to_owned())
                    .spawn(move || question(&stream));
            }
        })
        .map_err(|error| format!("cannot start a thread: {error}"))?;
    amacs_core::askpass::serve(link, socket, waker);
    Ok(made)
}

/// Takes one question from a helper and writes the answer back.
fn question(stream: &UnixStream) {
    let Ok(Some(Ask {
        ticket,
        prompt,
        kind,
    })) = read_msg::<_, Ask>(stream)
    else {
        return;
    };
    let answer = match amacs_core::askpass::ask(&ticket, &prompt, &kind) {
        // The editor going away before it answers is no answer.
        Ok(answers) => match answers.recv() {
            Ok(Some(answer)) => Answer::Given(answer),
            Ok(None) | Err(_) => Answer::None,
        },
        Err(why) => Answer::Refused(why),
    };
    let _ = write_msg(stream, &answer);
}
