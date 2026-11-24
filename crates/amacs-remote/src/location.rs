//! Where a file is: on this host, or on another that is reached in hops.
//!
//! The syntax is tramp's, since hands and language models know it:
//!
//! ```text
//! /ssh:user@host#2222:/path       a host, with a user and a port
//! /sudo::/etc/hosts               this host, as root
//! /ssh:host|sudo::/etc/hosts      root on a host
//! /podman:name:/path              a container
//! /:/ssh:odd:/name                a local file of that odd name
//! ```

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// A way to get from one place to the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method {
    /// Another host, through the user's `ssh`.
    Ssh,
    /// Another user on the same host, root unless one is named.
    Sudo,
    /// A container of that name on the host.
    Podman,
}

impl Method {
    const ALL: [Self; 3] = [Self::Ssh, Self::Sudo, Self::Podman];

    /// The name that file names carry.
    pub fn name(self) -> &'static str {
        match self {
            Self::Ssh => "ssh",
            Self::Sudo => "sudo",
            Self::Podman => "podman",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|method| method.name() == name)
    }
}

/// One step on the way to a file: a method and whom to be where.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Hop {
    pub method: Method,
    /// Whom to be there; `None` leaves it to the method.
    pub user: Option<String>,
    /// The host, or the container's name. Empty for the host that the hop
    /// starts from, which only `sudo` allows. An IPv6 address is held
    /// without its brackets.
    pub host: String,
    pub port: Option<u16>,
}

impl fmt::Display for Hop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:", self.method.name())?;
        if let Some(user) = &self.user {
            write!(f, "{user}@")?;
        }
        if self.host.contains(':') {
            write!(f, "[{}]", self.host)?;
        } else {
            f.write_str(&self.host)?;
        }
        if let Some(port) = self.port {
            write!(f, "#{port}")?;
        }
        Ok(())
    }
}

/// A file that is not on this host, or not as this user.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Remote {
    /// The steps from here to there, never empty.
    pub hops: Vec<Hop>,
    /// The path at the far end. One that is not absolute is taken from
    /// the home directory there.
    pub path: PathBuf,
}

impl Remote {
    /// The far end without a path: what two files share when one
    /// connection serves both.
    pub fn host(&self) -> String {
        let hops: Vec<String> = self.hops.iter().map(Hop::to_string).collect();
        format!("/{}:", hops.join("|"))
    }
}

/// Where a file is.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Location {
    Local(PathBuf),
    Remote(Remote),
}

/// Why a file name that starts as a remote one is not one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// The name ends before the path: `/ssh:host`.
    Incomplete,
    /// A hop after the first names a method that does not exist.
    UnknownMethod(String),
    /// A method that needs a host has none: `/ssh::/path`.
    NoHost(Method),
    /// The host cannot be one: it holds a slash, or brackets do not close.
    BadHost(String),
    /// What follows `#` is not a port number.
    BadPort(String),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Incomplete => {
                f.write_str("the remote file name has no path: a colon ends the host")
            }
            Self::UnknownMethod(name) => write!(f, "{name} is not a way to reach a host"),
            Self::NoHost(method) => write!(f, "{} needs a host", method.name()),
            Self::BadHost(host) => write!(f, "{host} cannot name a host"),
            Self::BadPort(port) => write!(f, "{port} is not a port number"),
        }
    }
}

impl std::error::Error for ParseError {}

impl Location {
    /// Reads a file name. One that does not start with `/` and a method
    /// that exists is a local file, whatever else it holds; one that does
    /// has to be a whole remote name. `/:` in front makes any name local.
    pub fn parse(name: &str) -> Result<Self, ParseError> {
        if let Some(quoted) = name.strip_prefix("/:") {
            return Ok(Self::Local(PathBuf::from(quoted)));
        }
        let local = || Ok(Self::Local(PathBuf::from(name)));
        let Some(rest) = name.strip_prefix('/') else {
            return local();
        };
        let Some((first, mut rest)) = rest.split_once(':') else {
            return local();
        };
        let Some(mut method) = Method::from_name(first) else {
            return local();
        };
        let mut hops = Vec::new();
        loop {
            let (end, last) = hop_end(rest)?;
            hops.push(parse_hop(method, &rest[..end])?);
            rest = &rest[end + 1..];
            if last {
                break;
            }
            let (name, after) = rest.split_once(':').ok_or(ParseError::Incomplete)?;
            method = Method::from_name(name)
                .ok_or_else(|| ParseError::UnknownMethod(name.to_owned()))?;
            rest = after;
        }
        Ok(Self::Remote(Remote {
            hops,
            path: PathBuf::from(rest),
        }))
    }

    pub fn is_remote(&self) -> bool {
        matches!(self, Self::Remote(_))
    }

    /// The path on the host that holds the file.
    pub fn path(&self) -> &Path {
        match self {
            Self::Local(path) => path,
            Self::Remote(remote) => &remote.path,
        }
    }

    /// Another path in the same place.
    pub fn with_path(&self, path: impl Into<PathBuf>) -> Self {
        match self {
            Self::Local(_) => Self::Local(path.into()),
            Self::Remote(remote) => Self::Remote(Remote {
                hops: remote.hops.clone(),
                path: path.into(),
            }),
        }
    }
}

/// Where the hop that `text` starts with ends, and whether the path
/// follows it rather than another hop.
fn hop_end(text: &str) -> Result<(usize, bool), ParseError> {
    let mut bracketed = false;
    for (at, c) in text.char_indices() {
        match c {
            '[' => bracketed = true,
            ']' => bracketed = false,
            ':' if !bracketed => return Ok((at, true)),
            '|' if !bracketed => return Ok((at, false)),
            '/' => return Err(ParseError::BadHost(text[..=at].to_owned())),
            _ => {}
        }
    }
    Err(ParseError::Incomplete)
}

/// Reads `user@host#port`, each part of which may be missing.
fn parse_hop(method: Method, text: &str) -> Result<Hop, ParseError> {
    let (user, rest) = match text.rsplit_once('@') {
        Some((user, rest)) => (Some(user), rest),
        None => (None, text),
    };
    let (host, port) = if let Some(inner) = rest.strip_prefix('[') {
        let (host, after) = inner
            .split_once(']')
            .ok_or_else(|| ParseError::BadHost(rest.to_owned()))?;
        match after.strip_prefix('#') {
            Some(port) => (host, Some(port)),
            None if after.is_empty() => (host, None),
            None => return Err(ParseError::BadHost(rest.to_owned())),
        }
    } else {
        match rest.split_once('#') {
            Some((host, port)) => (host, Some(port)),
            None => (rest, None),
        }
    };
    if host.is_empty() && method != Method::Sudo {
        return Err(ParseError::NoHost(method));
    }
    let port = port
        .map(|port| {
            port.parse::<u16>()
                .map_err(|_| ParseError::BadPort(port.to_owned()))
        })
        .transpose()?;
    Ok(Hop {
        method,
        user: user.filter(|user| !user.is_empty()).map(str::to_owned),
        host: host.to_owned(),
        port,
    })
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local(path) => {
                let name = path.to_string_lossy();
                // A local name that reads as something else says that it
                // is local.
                if Self::parse(&name).as_ref() != Ok(self) {
                    f.write_str("/:")?;
                }
                f.write_str(&name)
            }
            Self::Remote(remote) => {
                write!(f, "{}{}", remote.host(), remote.path.to_string_lossy())
            }
        }
    }
}

impl FromStr for Location {
    type Err = ParseError;

    fn from_str(name: &str) -> Result<Self, ParseError> {
        Self::parse(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(name: &str) -> Remote {
        match Location::parse(name) {
            Ok(Location::Remote(remote)) => remote,
            other => panic!("{name} is not remote: {other:?}"),
        }
    }

    fn hop(method: Method, user: Option<&str>, host: &str, port: Option<u16>) -> Hop {
        Hop {
            method,
            user: user.map(str::to_owned),
            host: host.to_owned(),
            port,
        }
    }

    #[test]
    fn local_names_stay_local() {
        for name in [
            "",
            "/",
            "/etc/hosts",
            "relative/file",
            "~/notes",
            "ssh:host:/path",
            "/tmp/a:b:c",
            "/scp",
            "/ssh",
            "/unknown:host:/path",
            "/ssh/host:/path",
        ] {
            assert_eq!(
                Location::parse(name),
                Ok(Location::Local(PathBuf::from(name))),
                "{name}"
            );
        }
    }

    #[test]
    fn a_host_with_user_and_port() {
        let file = remote("/ssh:albin@evo#2222:/home/albin/notes.org");
        assert_eq!(
            file.hops,
            [hop(Method::Ssh, Some("albin"), "evo", Some(2222))]
        );
        assert_eq!(file.path, Path::new("/home/albin/notes.org"));
        assert_eq!(file.host(), "/ssh:albin@evo#2222:");

        let file = remote("/ssh:evo:/etc");
        assert_eq!(file.hops, [hop(Method::Ssh, None, "evo", None)]);
    }

    #[test]
    fn sudo_needs_no_host() {
        let file = remote("/sudo::/etc/nixos/configuration.nix");
        assert_eq!(file.hops, [hop(Method::Sudo, None, "", None)]);
        assert_eq!(file.path, Path::new("/etc/nixos/configuration.nix"));

        let file = remote("/sudo:postgres@:/var/lib");
        assert_eq!(file.hops, [hop(Method::Sudo, Some("postgres"), "", None)]);
    }

    #[test]
    fn hops_follow_each_other() {
        let file = remote("/ssh:jump|ssh:albin@inner|sudo::/root/.profile");
        assert_eq!(
            file.hops,
            [
                hop(Method::Ssh, None, "jump", None),
                hop(Method::Ssh, Some("albin"), "inner", None),
                hop(Method::Sudo, None, "", None),
            ]
        );
        assert_eq!(file.path, Path::new("/root/.profile"));
    }

    #[test]
    fn containers_and_addresses() {
        let file = remote("/podman:builder:/src/main.rs");
        assert_eq!(file.hops, [hop(Method::Podman, None, "builder", None)]);

        let file = remote("/ssh:root@[fe80::1]#22:/var/log");
        assert_eq!(
            file.hops,
            [hop(Method::Ssh, Some("root"), "fe80::1", Some(22))]
        );
        assert_eq!(file.path, Path::new("/var/log"));
    }

    #[test]
    fn the_path_may_be_relative_to_home() {
        assert_eq!(remote("/ssh:evo:").path, Path::new(""));
        assert_eq!(remote("/ssh:evo:src/amacs").path, Path::new("src/amacs"));
        // Colons after the host are part of the path.
        assert_eq!(remote("/ssh:evo:/a:b|c").path, Path::new("/a:b|c"));
    }

    #[test]
    fn what_is_printed_reads_back_the_same() {
        for name in [
            "/etc/hosts",
            "relative",
            "/ssh:evo:/etc/hosts",
            "/ssh:albin@evo#2222:/home/albin",
            "/sudo::/etc/shadow",
            "/ssh:evo|sudo::/etc/shadow",
            "/ssh:[::1]:/tmp",
            "/ssh:[::1]#2222:",
            "/podman:root@builder:/src",
            "/:/ssh:odd:/name",
        ] {
            let location = Location::parse(name).expect(name);
            assert_eq!(location.to_string(), name);
            assert_eq!(location.to_string().parse(), Ok(location));
        }
        // The mark of a local name is dropped when nothing needs it.
        let plain: Location = "/:/etc/hosts".parse().unwrap();
        assert_eq!(plain.to_string(), "/etc/hosts");
    }

    #[test]
    fn another_path_in_the_same_place() {
        let file = Location::parse("/ssh:evo|sudo::/etc/hosts").unwrap();
        assert_eq!(
            file.with_path("/etc/passwd").to_string(),
            "/ssh:evo|sudo::/etc/passwd"
        );
        assert_eq!(file.path(), Path::new("/etc/hosts"));
        assert!(file.is_remote());
    }

    #[test]
    fn names_that_start_remote_have_to_be_whole() {
        let error = |name: &str| Location::parse(name).unwrap_err();
        assert_eq!(error("/ssh:"), ParseError::Incomplete);
        assert_eq!(error("/ssh:evo"), ParseError::Incomplete);
        assert_eq!(error("/ssh:evo|sudo"), ParseError::Incomplete);
        assert_eq!(error("/ssh:evo|sudo:"), ParseError::Incomplete);
        assert_eq!(error("/ssh::/etc"), ParseError::NoHost(Method::Ssh));
        assert_eq!(error("/podman::/etc"), ParseError::NoHost(Method::Podman));
        assert_eq!(
            error("/ssh:evo|telnet:old:/etc"),
            ParseError::UnknownMethod("telnet".to_owned())
        );
        assert_eq!(
            error("/ssh:evo#http:/etc"),
            ParseError::BadPort("http".to_owned())
        );
        assert_eq!(
            error("/ssh:evo#70000:/etc"),
            ParseError::BadPort("70000".to_owned())
        );
        assert_eq!(
            error("/ssh:evo/etc:/x"),
            ParseError::BadHost("evo/".to_owned())
        );
        assert_eq!(
            error("/ssh:[::1]x:/etc"),
            ParseError::BadHost("[::1]x".to_owned())
        );
        // Brackets that do not close swallow the colon that would end
        // the host.
        assert_eq!(
            error("/ssh:[::1:/etc"),
            ParseError::BadHost("[::1:/".to_owned())
        );
    }
}
