//! The ways to reach a place, as a table that is filled from outside.
//!
//! A method is the word that a step of a remote name starts with: `ssh`
//! in `/ssh:evo:/etc`, `sudo` in `/ssh:evo|sudo::/etc`. What each one
//! does is not written here. It is data: a command line with holes for
//! the host, the user and the port of the step, and a few facts about
//! how the command behaves. The editor's Scheme defines `ssh`, `sudo`
//! and the rest with `define-remote-method`, and a user can define
//! more. This module keeps the table and makes one program out of the
//! steps of a name.
//!
//! The program is built from the last step inwards. At the far end
//! something has to speak SFTP: an `sftp-server` that a line of `sh`
//! finds and runs, or the method's own way to one (`ssh -s host sftp`).
//! Each step before that runs the command of the next one in its place:
//! `sudo` takes it as words, `ssh` as one line for the shell there.

use std::io;
use std::sync::{OnceLock, PoisonError, RwLock};

use crate::location::Hop;
use crate::sftp::sftp_server;

/// A part of a command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    /// A word as it stands.
    Text(String),
    /// The host of the step.
    Host,
    /// Whom to be there: the user of the step, or the method's own.
    User,
    /// The port of the step.
    Port,
    /// The steps before this one that it reaches through by itself,
    /// each as the method's `jump` writes it, with commas between.
    Jumps,
    /// Words that are there only if all of what they need is: `-l` and
    /// the user, when a user is named.
    Group(Vec<Piece>),
}

/// A way to reach a place.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MethodDef {
    /// The word that names it in a file name.
    pub name: String,
    /// The program that enters the place, and its arguments. What is to
    /// run there follows them.
    pub command: Vec<Piece>,
    /// The whole command when this is the last step and the method has a
    /// way of its own to an SFTP server, as `ssh` has.
    pub sftp: Option<Vec<Piece>>,
    /// How a step is written when the next step, of the same method,
    /// reaches through it by itself: the parts are joined into one word.
    /// Without this, each step runs the next.
    pub jump: Option<Vec<Piece>>,
    /// What is to run in the place is handed over as one line for a
    /// shell there, not as words.
    pub line: bool,
    /// A step may leave the host out.
    pub host_optional: bool,
    /// Whom to be when the step names nobody.
    pub user: Option<String>,
    /// The place has the files of the place before it: the same host as
    /// another user.
    pub stays: bool,
}

fn table() -> &'static RwLock<Vec<MethodDef>> {
    static TABLE: OnceLock<RwLock<Vec<MethodDef>>> = OnceLock::new();
    TABLE.get_or_init(|| {
        // This machine through its own `sftp-server`, as the user that
        // the editor runs as: what the tests of remote files visit.
        let mut methods = vec![MethodDef {
            name: "sftp-test".to_owned(),
            host_optional: true,
            stays: true,
            ..MethodDef::default()
        }];
        if cfg!(test) {
            methods.extend(fixtures());
        }
        RwLock::new(methods)
    })
}

/// Adds a method to the table, in place of one of the same name.
pub fn define_method(method: MethodDef) {
    let mut methods = table().write().unwrap_or_else(PoisonError::into_inner);
    match methods.iter_mut().find(|known| known.name == method.name) {
        Some(known) => *known = method,
        None => methods.push(method),
    }
}

/// The method of a name, if there is one.
pub fn method(name: &str) -> Option<MethodDef> {
    let methods = table().read().unwrap_or_else(PoisonError::into_inner);
    methods.iter().find(|method| method.name == name).cloned()
}

/// The names of the methods, in the order that they were defined.
pub fn method_names() -> Vec<String> {
    let methods = table().read().unwrap_or_else(PoisonError::into_inner);
    methods.iter().map(|method| method.name.clone()).collect()
}

/// Methods like those that the editor's Scheme defines, for tests of
/// Rust that run without it: enough of `ssh`, `sudo` and `podman` to
/// read names and to see how commands are put together.
pub fn fixtures() -> Vec<MethodDef> {
    let text = |word: &str| Piece::Text(word.to_owned());
    let ssh: Vec<Piece> = vec![
        text("ssh"),
        Piece::Group(vec![text("-J"), Piece::Jumps]),
        Piece::Group(vec![text("-l"), Piece::User]),
        Piece::Group(vec![text("-p"), Piece::Port]),
    ];
    let with = |end: &[Piece]| ssh.iter().chain(end).cloned().collect::<Vec<_>>();
    vec![
        MethodDef {
            name: "ssh".to_owned(),
            command: with(&[text("--"), Piece::Host]),
            sftp: Some(with(&[text("-s"), text("--"), Piece::Host, text("sftp")])),
            jump: Some(vec![
                Piece::Group(vec![Piece::User, text("@")]),
                Piece::Host,
                Piece::Group(vec![text(":"), Piece::Port]),
            ]),
            line: true,
            ..MethodDef::default()
        },
        MethodDef {
            name: "sudo".to_owned(),
            command: vec![
                text("sudo"),
                text("-n"),
                text("-u"),
                Piece::User,
                text("--"),
            ],
            host_optional: true,
            user: Some("root".to_owned()),
            stays: true,
            ..MethodDef::default()
        },
        MethodDef {
            name: "podman".to_owned(),
            command: vec![
                text("podman"),
                text("exec"),
                text("-i"),
                Piece::Group(vec![text("-u"), Piece::User]),
                Piece::Host,
            ],
            ..MethodDef::default()
        },
    ]
}

/// Defines the methods of [`fixtures`], for the tests of other crates.
pub fn define_fixtures() {
    for method in fixtures() {
        if self::method(&method.name).is_none() {
            define_method(method);
        }
    }
}

/// How a place is asked where its `sftp-server` is and to run it. The
/// configuration of the host's own `sshd` says where the program is,
/// which is the one place to find it on a system that keeps programs
/// where NixOS does.
const FIND_SFTP: &str = r#"for s in "$(sed -n "s/^[Ss]ubsystem[[:space:]]*sftp[[:space:]]*\([^[:space:]]*\).*/\1/p" /etc/ssh/sshd_config 2>/dev/null)" /usr/lib/openssh/sftp-server /usr/libexec/openssh/sftp-server /usr/lib/ssh/sftp-server /usr/libexec/sftp-server "$(command -v sftp-server)"; do if [ -x "$s" ]; then exec "$s"; fi; done; echo "no sftp-server was found there" >&2; exit 127"#;

/// A word as a shell reads it back, whatever is in it.
fn quoted(word: &str) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || "-_./:=@%+,".contains(c);
    if !word.is_empty() && word.chars().all(plain) {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// A step, with the steps before it that it reaches through by itself.
struct Step<'a> {
    hop: &'a Hop,
    method: MethodDef,
    jumps: Vec<String>,
}

impl Step<'_> {
    /// What a hole in a command line holds for this step, if anything.
    fn value(&self, piece: &Piece, in_jump: bool) -> Option<String> {
        match piece {
            Piece::Text(text) => Some(text.clone()),
            // An address with colons stands in brackets where a colon
            // means something else.
            Piece::Host if in_jump && self.hop.host.contains(':') => {
                Some(format!("[{}]", self.hop.host))
            }
            Piece::Host => Some(self.hop.host.clone()).filter(|host| !host.is_empty()),
            Piece::User => self.hop.user.clone().or_else(|| self.method.user.clone()),
            Piece::Port => self.hop.port.map(|port| port.to_string()),
            Piece::Jumps => Some(self.jumps.join(",")).filter(|jumps| !jumps.is_empty()),
            Piece::Group(_) => None,
        }
    }

    /// The words of a command line. A hole with nothing for it is left
    /// out, and takes its group with it.
    fn words(&self, pieces: &[Piece], in_jump: bool) -> Vec<String> {
        let mut words = Vec::new();
        for piece in pieces {
            match piece {
                Piece::Group(inner) => {
                    let whole = inner.iter().all(|piece| match piece {
                        Piece::Group(_) => true,
                        piece => self.value(piece, in_jump).is_some(),
                    });
                    if whole {
                        words.extend(self.words(inner, in_jump));
                    }
                }
                piece => words.extend(self.value(piece, in_jump)),
            }
        }
        words
    }
}

fn refused(kind: io::ErrorKind, why: String) -> io::Error {
    io::Error::new(kind, why)
}

/// The command whose pipes speak SFTP with the far end of some steps:
/// the program and its arguments.
pub fn command_line(hops: &[Hop]) -> io::Result<Vec<String>> {
    let mut steps: Vec<Step> = Vec::new();
    for (index, hop) in hops.iter().enumerate() {
        let method = method(&hop.method).ok_or_else(|| {
            refused(
                io::ErrorKind::Unsupported,
                format!("{}: is not a method that is defined", hop.method),
            )
        })?;
        // The host of a step that stays is the one before it, which
        // tramp lets one write again. With no step before it, the host
        // is this machine under any name.
        let before = index.checked_sub(1).map(|at| hops[at].host.as_str());
        if method.stays && !hop.host.is_empty() && before.is_some_and(|before| hop.host != before) {
            return Err(refused(
                io::ErrorKind::InvalidInput,
                format!(
                    "{name} is for the host before it, not for {}: write {name}::",
                    hop.host,
                    name = method.name
                ),
            ));
        }
        // A step that the next one reaches through by itself is no
        // step of its own.
        let mut jumps = Vec::new();
        if let Some(previous) = steps.last()
            && previous.method.name == method.name
            && let Some(jump) = &method.jump
        {
            jumps.clone_from(&previous.jumps);
            jumps.push(previous.words(jump, true).concat());
            steps.pop();
        }
        steps.push(Step { hop, method, jumps });
    }

    // What speaks SFTP at the far end. Where no step leaves this
    // machine, its program is known here.
    let mut line: Vec<String> = if steps.iter().all(|step| step.method.stays) {
        let server = sftp_server().ok_or_else(|| {
            refused(
                io::ErrorKind::NotFound,
                "this machine has no sftp-server".to_owned(),
            )
        })?;
        vec![server.to_string_lossy().into_owned()]
    } else {
        ["sh", "-c", FIND_SFTP].map(str::to_owned).into()
    };
    for (index, step) in steps.iter().enumerate().rev() {
        let last = index == steps.len() - 1;
        line = match &step.method.sftp {
            Some(sftp) if last => step.words(sftp, false),
            _ => {
                let mut words = step.words(&step.method.command, false);
                if step.method.line && !words.is_empty() {
                    let inner: Vec<String> = line.iter().map(|word| quoted(word)).collect();
                    words.push(inner.join(" "));
                } else {
                    words.extend(line);
                }
                words
            }
        };
    }
    if line.is_empty() {
        return Err(refused(
            io::ErrorKind::Unsupported,
            "a file name with no host".to_owned(),
        ));
    }
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::location::{Location, Remote};

    fn remote(name: &str) -> Remote {
        match Location::parse(name) {
            Ok(Location::Remote(remote)) => remote,
            other => panic!("{name} is not remote: {other:?}"),
        }
    }

    fn line(name: &str) -> Vec<String> {
        command_line(&remote(name).hops).unwrap()
    }

    #[test]
    fn a_method_asks_for_its_own_sftp_where_it_has_one() {
        assert_eq!(
            line("/ssh:albin@evo#2222:/etc"),
            [
                "ssh", "-l", "albin", "-p", "2222", "-s", "--", "evo", "sftp"
            ]
        );
        assert_eq!(line("/ssh:evo:/etc"), ["ssh", "-s", "--", "evo", "sftp"]);
        // Hosts in a row are reached through by the last of them.
        assert_eq!(
            line("/ssh:me@gate#2222|ssh:[::1]|ssh:evo:/etc"),
            ["ssh", "-J", "me@gate:2222,[::1]", "-s", "--", "evo", "sftp"]
        );
    }

    #[test]
    fn steps_run_the_next_one_as_words_or_as_a_line() {
        // Here, the sftp-server of this machine, as words.
        let here = line("/sudo::/etc");
        assert_eq!(here[..5], ["sudo", "-n", "-u", "root", "--"]);
        assert!(
            here[5].ends_with("sftp-server") && here.len() == 6,
            "{here:?}"
        );
        assert_eq!(line("/sudo:postgres@:/etc")[3], "postgres");
        assert_eq!(line("/sftp-test::/etc").len(), 1);

        // On a host, sudo and what finds the sftp-server there, as one
        // line for the shell of the host.
        let there = line("/ssh:evo|sudo::/etc");
        assert_eq!(there[..3], ["ssh", "--", "evo"]);
        assert_eq!(there.len(), 4);
        let handed = &there[3];
        assert!(
            handed.starts_with("sudo -n -u root -- sh -c 'for s in "),
            "{handed}"
        );
        assert!(handed.ends_with("exit 127'"), "{handed}");
        // tramp's way of writing it, with the host again.
        assert_eq!(line("/ssh:evo|sudo:root@evo:/etc"), there);

        // A line within a line is quoted once more.
        let deep = line("/ssh:evo|podman:box|ssh:inner|sudo::/etc");
        assert_eq!(deep[..3], ["ssh", "--", "evo"]);
        assert!(
            deep[3].starts_with(
                "podman exec -i box ssh -- inner 'sudo -n -u root -- sh -c '\\''for s"
            )
        );
    }

    #[test]
    fn the_line_for_a_place_finds_its_sftp_server() {
        // The same line with echo in place of exec, run by the shell of
        // this machine: it names a program that is there.
        let Some(server) = sftp_server() else { return };
        let script = FIND_SFTP.replace("exec \"$s\"", "echo \"$s\"; exit");
        assert_ne!(script, FIND_SFTP);
        let own = std::env::var_os("PATH").unwrap_or_default();
        let path = std::env::split_paths(&own).chain([server.parent().unwrap().to_owned()]);
        // Through the quoting that a line for a shell gets.
        let output = std::process::Command::new("sh")
            .args(["-c", &format!("sh -c {}", quoted(&script))])
            .env("PATH", std::env::join_paths(path).unwrap())
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&output.stdout);
        assert!(said.trim().ends_with("sftp-server"), "{output:?}");
        assert_eq!(quoted("plain-1.2_3/x:y"), "plain-1.2_3/x:y");
        assert_eq!(quoted("it's"), r"'it'\''s'");
        assert_eq!(quoted(""), "''");
    }

    #[test]
    fn what_cannot_be_reached_says_so() {
        let stays = command_line(&remote("/ssh:evo|sudo:pi:/etc").hops).unwrap_err();
        assert_eq!(stays.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            stays.to_string(),
            "sudo is for the host before it, not for pi: write sudo::"
        );
        let hop = Hop {
            method: "gone".to_owned(),
            user: None,
            host: "x".to_owned(),
            port: None,
        };
        let unknown = command_line(&[hop]).unwrap_err();
        assert_eq!(unknown.kind(), io::ErrorKind::Unsupported);
        assert_eq!(unknown.to_string(), "gone: is not a method that is defined");
    }

    #[test]
    fn a_method_is_defined_and_defined_again() {
        let method = |user: &str| MethodDef {
            name: "doas-test".to_owned(),
            command: vec![
                Piece::Text("doas".to_owned()),
                Piece::Text("-u".to_owned()),
                Piece::User,
            ],
            user: Some(user.to_owned()),
            host_optional: true,
            stays: true,
            ..MethodDef::default()
        };
        assert!(self::method("doas-test").is_none());
        define_method(method("root"));
        assert_eq!(line("/doas-test::/etc")[..3], ["doas", "-u", "root"]);
        define_method(method("admin"));
        assert_eq!(line("/doas-test::/etc")[2], "admin");
        let names = method_names();
        assert_eq!(names.iter().filter(|name| *name == "doas-test").count(), 1);
        assert_eq!(names[0], "sftp-test");
    }
}
