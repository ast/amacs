//! What is known of a file without reading it, on this host or another.

use std::ffi::OsString;
use std::time::SystemTime;

/// What kind of thing a name in a directory is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    File,
    Directory,
    Symlink,
    /// A socket, a device, a pipe; or the host did not say.
    Other,
}

impl FileType {
    /// The kind that the bits of a Unix `st_mode` give.
    pub fn from_mode(mode: u32) -> Self {
        match mode & 0o170_000 {
            0o100_000 => Self::File,
            0o040_000 => Self::Directory,
            0o120_000 => Self::Symlink,
            _ => Self::Other,
        }
    }
}

/// The size, kind, age and permissions of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Metadata {
    pub file_type: FileType,
    /// Size in bytes.
    pub len: u64,
    /// When the contents last changed, if the host said.
    pub modified: Option<SystemTime>,
    /// The permission bits of a Unix mode, without the bits of the kind.
    pub mode: u32,
}

impl Metadata {
    pub fn is_dir(&self) -> bool {
        self.file_type == FileType::Directory
    }

    pub fn is_file(&self) -> bool {
        self.file_type == FileType::File
    }

    pub fn is_symlink(&self) -> bool {
        self.file_type == FileType::Symlink
    }

    /// Whether anyone may run the file as a program.
    pub fn is_executable(&self) -> bool {
        !self.is_dir() && self.mode & 0o111 != 0
    }
}

impl From<&std::fs::Metadata> for Metadata {
    fn from(meta: &std::fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            file_type: FileType::from_mode(meta.mode()),
            len: meta.len(),
            modified: meta.modified().ok(),
            mode: meta.mode() & 0o7777,
        }
    }
}

/// One name in a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: OsString,
    /// What the name itself is: of a link, the link and not what it leads
    /// to. `None` if that could not be found out.
    pub metadata: Option<Metadata>,
}
