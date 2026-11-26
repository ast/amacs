//! Files on other hosts.
//!
//! A remote file has a name in tramp's syntax, `/ssh:user@host:/path`,
//! which [`Location`] parses and prints. Its contents are reached over
//! SFTP: [`Sftp`] is a client of version 3 of that protocol, which speaks
//! through the pipes of a program that it does not look inside. That
//! program is the user's own `ssh`, or an `sftp-server` run by `sudo` or
//! inside a container, so there is no cryptography and no network code
//! here, and nothing to install on the host. [`Connections`] keeps one
//! such program for each host.

mod connections;
mod hosts;
mod location;
mod metadata;
mod methods;
mod sftp;

pub use connections::Connections;
pub use hosts::{SshHost, ssh_hosts, ssh_hosts_in};
pub use location::{Hop, Location, ParseError, Remote};
pub use metadata::{DirEntry, FileType, Metadata};
pub use methods::{
    MethodDef, Piece, command_line, define_fixtures, define_method, method, method_names,
};
pub use sftp::{FileReader, Sftp, sftp_server, with_patience};
