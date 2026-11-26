//! The connections to hosts: one for each, made when a file there is
//! first asked for, shared by everything that reaches files there, and
//! made again when it is lost.

use std::collections::HashMap;
use std::io;
use std::process::Command;
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use crate::location::{Hop, Remote};
use crate::methods::command_line;
use crate::sftp::{Sftp, patience};

/// A host that could not be reached is not tried again for this long:
/// what asks, asks often.
const RETRY_AFTER: Duration = Duration::from_secs(3);

enum State {
    Connecting,
    Ready(Arc<Sftp>),
    Failed {
        kind: io::ErrorKind,
        why: String,
        at: Instant,
    },
}

struct Host {
    state: Mutex<State>,
    changed: Condvar,
}

/// The connections of this process, by host.
#[derive(Default)]
pub struct Connections {
    hosts: Mutex<HashMap<String, Arc<Host>>>,
}

impl Connections {
    /// The connections that everything in this process shares.
    pub fn global() -> &'static Self {
        static GLOBAL: OnceLock<Connections> = OnceLock::new();
        GLOBAL.get_or_init(Self::default)
    }

    /// The connection to the host of a file, made now if there is none.
    /// Making it takes as long as the program that makes it; the wait is
    /// at most the patience of this thread (see
    /// [`crate::with_patience`]), after which the call fails as timed out
    /// while the connection goes on being made for whoever asks next.
    pub fn sftp(&self, remote: &Remote) -> io::Result<Arc<Sftp>> {
        let name = remote.host();
        let host = {
            let mut hosts = self.hosts.lock().unwrap_or_else(PoisonError::into_inner);
            Arc::clone(hosts.entry(name.clone()).or_insert_with(|| {
                Arc::new(Host {
                    // As good as never tried.
                    state: Mutex::new(State::Failed {
                        kind: io::ErrorKind::NotConnected,
                        why: String::new(),
                        at: Instant::now() - RETRY_AFTER,
                    }),
                    changed: Condvar::new(),
                })
            }))
        };
        let deadline = Instant::now() + patience();
        let mut state = host.state.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            match &*state {
                State::Ready(sftp) if !sftp.is_lost() => return Ok(Arc::clone(sftp)),
                State::Failed { kind, why, at } if at.elapsed() < RETRY_AFTER => {
                    return Err(io::Error::new(*kind, why.clone()));
                }
                State::Connecting => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            format!("still connecting to {name}"),
                        ));
                    }
                    state = host
                        .changed
                        .wait_timeout(state, left)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                }
                State::Ready(_) | State::Failed { .. } => {
                    *state = State::Connecting;
                    let (host, hops) = (Arc::clone(&host), remote.hops.clone());
                    std::thread::Builder::new()
                        .name("sftp-connect".to_owned())
                        .spawn(move || {
                            let made = program(&hops).and_then(start).map_err(explained);
                            let mut state =
                                host.state.lock().unwrap_or_else(PoisonError::into_inner);
                            *state = match made {
                                Ok(sftp) => State::Ready(Arc::new(sftp)),
                                Err(error) => State::Failed {
                                    kind: error.kind(),
                                    why: error.to_string(),
                                    at: Instant::now(),
                                },
                            };
                            host.changed.notify_all();
                        })?;
                }
            }
        }
    }

    /// The connection to the host of a file if there is one that works,
    /// as far as is known without asking the host. None is made.
    pub fn connected(&self, remote: &Remote) -> Option<Arc<Sftp>> {
        let hosts = self.hosts.lock().unwrap_or_else(PoisonError::into_inner);
        let host = hosts.get(&remote.host())?;
        let state = host.state.lock().unwrap_or_else(PoisonError::into_inner);
        match &*state {
            State::Ready(sftp) if !sftp.is_lost() => Some(Arc::clone(sftp)),
            _ => None,
        }
    }

    /// Whether there is a connection to the host of a file that works,
    /// as far as is known without asking the host.
    pub fn is_connected(&self, remote: &Remote) -> bool {
        let hosts = self.hosts.lock().unwrap_or_else(PoisonError::into_inner);
        hosts.get(&remote.host()).is_some_and(|host| {
            let state = host.state.lock().unwrap_or_else(PoisonError::into_inner);
            matches!(&*state, State::Ready(sftp) if !sftp.is_lost())
        })
    }

    /// The hosts that have been asked for, as the start of the names of
    /// their files, each with how its connection is: `connected`,
    /// `connecting`, or why it is not.
    pub fn hosts(&self) -> Vec<(String, String)> {
        let hosts = self.hosts.lock().unwrap_or_else(PoisonError::into_inner);
        let mut listed: Vec<(String, String)> = hosts
            .iter()
            .map(|(name, host)| {
                let state = host.state.lock().unwrap_or_else(PoisonError::into_inner);
                let how = match &*state {
                    State::Ready(sftp) if !sftp.is_lost() => "connected".to_owned(),
                    State::Ready(_) => "the connection to the host is lost".to_owned(),
                    State::Connecting => "connecting".to_owned(),
                    State::Failed { why, .. } => why.clone(),
                };
                (name.clone(), how)
            })
            .collect();
        listed.sort();
        listed
    }

    /// Ends the connection to a host, given as [`Remote::host`] gives it.
    /// What is under way there fails; the next file asked for connects
    /// again. Returns whether the host was known.
    pub fn disconnect(&self, host: &str) -> bool {
        let mut hosts = self.hosts.lock().unwrap_or_else(PoisonError::into_inner);
        hosts.remove(host).is_some()
    }
}

/// The program whose pipes speak SFTP with the far end of some hops:
/// see [`crate::methods`].
fn program(hops: &[Hop]) -> io::Result<Command> {
    let line = command_line(hops)?;
    let mut command = Command::new(&line[0]);
    command.args(&line[1..]);
    Ok(command)
}

/// Starts the program that reaches a place. A program that is not on
/// this machine is said as that: the error for it is else the one for a
/// file that is not there, which a new file is.
fn start(command: Command) -> io::Result<Sftp> {
    let name = command.get_program().to_string_lossy().into_owned();
    Sftp::spawn(command).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => io::Error::new(
            io::ErrorKind::Unsupported,
            format!("the program {name} was not found on this machine"),
        ),
        _ => error,
    })
}

/// Says in the editor's words what a program that was to reach a host
/// said in its own, where that helps.
fn explained(error: io::Error) -> io::Error {
    let said = error.to_string();
    if said.contains("a password is required") || said.contains("a terminal is required") {
        return io::Error::new(
            io::ErrorKind::PermissionDenied,
            "sudo needs a password on that host. The editor cannot give a password now. \
             Use a host where sudo needs no password, or run sudo in a terminal first.",
        );
    }
    error
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::location::Location;

    fn remote(name: &str) -> Remote {
        match Location::parse(name) {
            Ok(Location::Remote(remote)) => remote,
            other => panic!("{name} is not remote: {other:?}"),
        }
    }

    #[test]
    fn what_a_program_said_is_said_in_plain_words() {
        let asked = explained(io::Error::other("sudo: a password is required"));
        assert_eq!(asked.kind(), io::ErrorKind::PermissionDenied);
        assert!(
            asked
                .to_string()
                .starts_with("sudo needs a password on that host.")
        );
        assert_eq!(
            explained(io::Error::other("no route")).to_string(),
            "no route"
        );
        assert_eq!(
            program(&remote("/ssh:evo:/etc").hops)
                .unwrap()
                .get_program(),
            "ssh"
        );
    }

    #[test]
    fn a_host_is_connected_once_and_again_when_lost() {
        let connections = Connections::default();
        let here = remote("/sftp-test::/");
        assert!(!connections.is_connected(&here));
        assert_eq!(connections.hosts(), []);

        let first = connections.sftp(&here).unwrap();
        assert!(first.metadata(here.host_path()).unwrap().is_dir());
        let again = connections.sftp(&remote("/sftp-test::/etc")).unwrap();
        assert!(Arc::ptr_eq(&first, &again));
        assert!(connections.is_connected(&here));
        assert_eq!(
            connections.hosts(),
            [("/sftp-test::".to_owned(), "connected".to_owned())]
        );

        assert!(connections.disconnect("/sftp-test::"));
        assert!(!connections.disconnect("/sftp-test::"));
        assert!(!connections.is_connected(&here));
        let anew = connections.sftp(&here).unwrap();
        assert!(!Arc::ptr_eq(&first, &anew));
        assert!(anew.metadata(here.host_path()).unwrap().is_dir());
    }

    #[test]
    fn a_host_that_cannot_be_reached_says_why_to_all_who_ask() {
        let connections = Connections::default();
        let nowhere = remote("/ssh:evo|sudo:pi:/etc");
        let error = connections.sftp(&nowhere).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        // Soon after, the same answer without another try.
        let again = connections.sftp(&nowhere).err().unwrap();
        assert_eq!(again.to_string(), error.to_string());
        assert_eq!(connections.hosts()[0].1, error.to_string());
        assert!(!connections.is_connected(&nowhere));
    }

    #[test]
    fn nobody_waits_longer_for_a_connection_than_they_have_patience() {
        let connections = Connections::default();
        let here = remote("/sftp-test::/");
        let error = crate::with_patience(Duration::ZERO, || connections.sftp(&here))
            .err()
            .unwrap();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(error.to_string(), "still connecting to /sftp-test::");
        // It goes on being made meanwhile.
        assert!(connections.sftp(&here).is_ok());
    }
}
