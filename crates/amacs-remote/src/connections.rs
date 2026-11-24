//! The connections to hosts: one for each, made when a file there is
//! first asked for, shared by everything that reaches files there, and
//! made again when it is lost.

use std::collections::HashMap;
use std::io;
use std::process::Command;
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use crate::location::{Hop, Method, Remote};
use crate::sftp::{Sftp, patience, sftp_server};

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
                            let made = program(&hops).and_then(Sftp::spawn);
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

/// The program whose pipes speak SFTP with the far end of some hops.
fn program(hops: &[Hop]) -> io::Result<Command> {
    match hops {
        [hop] if hop.method == Method::Ssh => Ok(ssh(hop)),
        [hop] if hop.method == Method::Test => {
            let server = sftp_server().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "this machine has no sftp-server")
            })?;
            Ok(Command::new(server))
        }
        _ => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "only /ssh:host: is reached so far: sudo, containers and several hops come later",
        )),
    }
}

/// The user's own `ssh`, asked for the SFTP subsystem of a host. It
/// authenticates as it always does, with the user's configuration, agent
/// and jump hosts, and shares one connection per host with every other
/// `ssh` that this editor starts, which stays for ten minutes after its
/// last use.
fn ssh(hop: &Hop) -> Command {
    let mut command = Command::new("ssh");
    // Nobody is there to answer a question yet, so none is asked.
    command.args(["-o", "BatchMode=yes"]);
    // A host that is not there, or no longer there, is given up on by
    // `ssh` itself.
    command.args(["-o", "ConnectTimeout=10"]);
    command.args([
        "-o",
        "ServerAliveInterval=10",
        "-o",
        "ServerAliveCountMax=3",
    ]);
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        let directory = std::path::Path::new(&runtime).join("amacs");
        if std::fs::create_dir_all(&directory).is_ok() {
            command.args(["-o", "ControlMaster=auto", "-o", "ControlPersist=600"]);
            command.arg("-o");
            command.arg(format!("ControlPath={}/ssh-%C", directory.display()));
        }
    }
    if let Some(user) = &hop.user {
        command.args(["-l", user]);
    }
    if let Some(port) = hop.port {
        command.args(["-p", &port.to_string()]);
    }
    command.args(["-s", "--", &hop.host, "sftp"]);
    command
}

/// The hosts that the user's `ssh` knows by name: those that
/// `~/.ssh/config` names outright, without patterns, in the order of the
/// file.
pub fn ssh_hosts() -> Vec<String> {
    std::env::var_os("HOME")
        .and_then(|home| {
            std::fs::read_to_string(std::path::Path::new(&home).join(".ssh/config")).ok()
        })
        .map_or_else(Vec::new, |config| hosts_in(&config))
}

fn hosts_in(config: &str) -> Vec<String> {
    let mut hosts: Vec<String> = Vec::new();
    for line in config.lines() {
        // `Host a b`, or `Host=a b`, in any case.
        let mut words = line
            .split(|c: char| c.is_whitespace() || c == '=')
            .filter(|word| !word.is_empty());
        if !words
            .next()
            .is_some_and(|key| key.eq_ignore_ascii_case("host"))
        {
            continue;
        }
        for name in words {
            let name = name.trim_matches('"');
            let plain = !name.is_empty() && !name.contains(['*', '?', '!', ',']);
            if plain && !hosts.iter().any(|host| host == name) {
                hosts.push(name.to_owned());
            }
        }
    }
    hosts
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

    fn arguments(command: &Command) -> Vec<String> {
        command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn ssh_is_asked_for_the_sftp_subsystem() {
        let command = program(&remote("/ssh:albin@evo#2222:/etc").hops).unwrap();
        assert_eq!(command.get_program(), "ssh");
        let arguments = arguments(&command);
        assert!(arguments.ends_with(
            &["-l", "albin", "-p", "2222", "-s", "--", "evo", "sftp"].map(str::to_owned)
        ));
        assert!(arguments.contains(&"BatchMode=yes".to_owned()));

        let command = program(&remote("/ssh:evo:/etc").hops).unwrap();
        assert!(arguments_end(&command, &["-s", "--", "evo", "sftp"]));
    }

    fn arguments_end(command: &Command, end: &[&str]) -> bool {
        let end: Vec<String> = end.iter().map(|part| (*part).to_owned()).collect();
        let arguments = arguments(command);
        arguments.ends_with(&end) && !arguments.contains(&"-l".to_owned())
    }

    #[test]
    fn hosts_are_read_from_the_configuration_of_ssh() {
        let config = "# mine\nHost evo\n  HostName evo.example.org\n\nhost  pi  nas \"quoted\"\n\
                      Host *.example.org !bad *\n  User albin\nHost=equals\nHostName not-a-host\nHost evo\n";
        assert_eq!(hosts_in(config), ["evo", "pi", "nas", "quoted", "equals"]);
        assert_eq!(hosts_in(""), [] as [&str; 0]);
    }

    #[test]
    fn what_is_not_reached_yet_says_so() {
        for name in ["/sudo::/etc", "/ssh:a|ssh:b:/etc", "/podman:box:/etc"] {
            let error = program(&remote(name).hops).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::Unsupported, "{name}");
        }
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
        let nowhere = remote("/sudo::/etc");
        let error = connections.sftp(&nowhere).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
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
