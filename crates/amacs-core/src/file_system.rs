//! The one place where the files that a person works on are reached.
//!
//! Everything that visits, saves, lists, renames or deletes a file by a
//! name that the user or a program gave goes through the functions of
//! this module, and they through a [`FileSystem`]: [`Local`] for the files
//! of this machine, an SFTP connection for those of another. What the
//! editor keeps for itself on this machine does not come here: auto-save
//! data, histories, dictionaries, the Scheme that it loads.
//!
//! A name in tramp's syntax, `/ssh:host:/path`, is a file on another
//! host, and the functions here take and give such names. They are for
//! the thread that owns the editor, so they wait for a host a few seconds
//! at most and then fail as timed out. Work that may take the time it
//! takes, on a thread of its own, is wrapped in [`patiently`]; reading
//! and writing the contents of a remote file belongs there.

use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use std::cell::Cell;
use std::time::{Duration, SystemTime};

use amacs_remote::{Connections, Location, Remote, Sftp};
pub use amacs_remote::{DirEntry, FileType, Metadata};

/// Files somewhere, by their paths there.
pub trait FileSystem {
    /// What a file is, following links.
    fn metadata(&self, path: &Path) -> io::Result<Metadata>;

    /// What a name is: of a link, the link itself.
    fn symlink_metadata(&self, path: &Path) -> io::Result<Metadata>;

    /// Where a link points.
    fn read_link(&self, path: &Path) -> io::Result<PathBuf>;

    /// The absolute path of a file, with links followed. Of a file that
    /// is not there, this machine gives an error and other hosts may give
    /// a path all the same; callers take the name as it is on an error.
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf>;

    /// Opens a file for reading from its start.
    fn open(&self, path: &Path) -> io::Result<Box<dyn Read + '_>>;

    /// The contents of a file.
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        let mut contents = Vec::new();
        self.open(path)?.read_to_end(&mut contents)?;
        Ok(contents)
    }

    /// Makes a file hold `contents` and nothing else, writing over what
    /// it held.
    fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()>;

    /// Replaces a file by what `content` writes, so that a crash cannot
    /// leave a half-written file: the bytes go to a temporary file that
    /// is then renamed over the target. The new file has the permissions
    /// of the old, and a link is written through, not replaced.
    fn write_atomically(
        &self,
        path: &Path,
        content: &mut dyn FnMut(&mut dyn Write) -> io::Result<()>,
    ) -> io::Result<()>;

    /// What a directory holds, without `.` and `..`, in no order.
    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>>;

    /// Gives a file or directory another name, in place of whatever file
    /// has that name already.
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;

    /// Deletes a file or a link; a directory with everything in it.
    fn delete(&self, path: &Path) -> io::Result<()>;

    /// Creates a directory, and the directories above it that are
    /// missing.
    fn mkdir(&self, path: &Path) -> io::Result<()>;

    /// Makes a link at `link` that points to `target`, taken as written.
    fn symlink(&self, target: &Path, link: &Path) -> io::Result<()>;

    /// Makes a file that is not there yet, with what `content` writes,
    /// permissions and, if given, the time it was last changed: for a
    /// copy. It is whole under its name or not there.
    fn write_new(
        &self,
        path: &Path,
        mode: u32,
        modified: Option<SystemTime>,
        content: &mut dyn FnMut(&mut dyn Write) -> io::Result<()>,
    ) -> io::Result<()> {
        self.write_atomically(path, content)?;
        self.set_attributes(path, Some(mode), modified)
    }

    /// What each of some files is, following links. Other hosts are
    /// asked about all of them at once; an error is then that of the
    /// connection, and what is wrong with one file is its answer.
    fn metadata_many(&self, paths: &[PathBuf]) -> io::Result<Vec<io::Result<Metadata>>> {
        Ok(paths.iter().map(|path| self.metadata(path)).collect())
    }

    /// Where each of some links points, asked like
    /// [`FileSystem::metadata_many`].
    fn read_link_many(&self, paths: &[PathBuf]) -> io::Result<Vec<io::Result<PathBuf>>> {
        Ok(paths.iter().map(|path| self.read_link(path)).collect())
    }

    /// Gives a file permissions, the time it was last changed, or both.
    /// A link is followed.
    fn set_attributes(
        &self,
        path: &Path,
        mode: Option<u32>,
        modified: Option<SystemTime>,
    ) -> io::Result<()>;
}

/// The files of this machine.
pub struct Local;

impl FileSystem for Local {
    fn metadata(&self, path: &Path) -> io::Result<Metadata> {
        fs::metadata(path).map(|meta| Metadata::from(&meta))
    }

    fn symlink_metadata(&self, path: &Path) -> io::Result<Metadata> {
        fs::symlink_metadata(path).map(|meta| Metadata::from(&meta))
    }

    fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        fs::read_link(path)
    }

    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        fs::canonicalize(path)
    }

    fn open(&self, path: &Path) -> io::Result<Box<dyn Read + '_>> {
        Ok(Box::new(File::open(path)?))
    }

    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        fs::read(path)
    }

    fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        fs::write(path, contents)
    }

    fn write_atomically(
        &self,
        path: &Path,
        content: &mut dyn FnMut(&mut dyn Write) -> io::Result<()>,
    ) -> io::Result<()> {
        // Write through a symlink rather than replacing it with a regular file.
        let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        let dir = target.parent().unwrap_or(Path::new("."));
        let name = target
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
        let mut temp_name = std::ffi::OsString::from(".#");
        temp_name.push(name);
        temp_name.push(format!(".{}.tmp", std::process::id()));
        let temp = dir.join(temp_name);

        let result = (|| {
            let file = File::create(&temp)?;
            if let Ok(meta) = fs::metadata(&target) {
                file.set_permissions(meta.permissions())?;
            }
            let mut writer = BufWriter::new(&file);
            content(&mut writer)?;
            writer.flush()?;
            drop(writer);
            file.sync_all()?;
            fs::rename(&temp, &target)
        })();
        if result.is_err() {
            // Best effort: the write error is what the user needs to see.
            let _ = fs::remove_file(&temp);
        }
        result
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        fs::read_dir(path)?
            .map(|entry| {
                let entry = entry?;
                Ok(DirEntry {
                    name: entry.file_name(),
                    metadata: entry.metadata().ok().map(|meta| Metadata::from(&meta)),
                })
            })
            .collect()
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        fs::rename(from, to)
    }

    fn delete(&self, path: &Path) -> io::Result<()> {
        if fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir()) {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        }
    }

    fn mkdir(&self, path: &Path) -> io::Result<()> {
        fs::create_dir_all(path)
    }

    fn symlink(&self, target: &Path, link: &Path) -> io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    fn set_attributes(
        &self,
        path: &Path,
        mode: Option<u32>,
        modified: Option<SystemTime>,
    ) -> io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        if let Some(mode) = mode {
            fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
        }
        if let Some(modified) = modified {
            // Reading is enough to set the time of what one owns, and
            // all that a directory can be opened for.
            File::open(path)?.set_modified(modified)?;
        }
        Ok(())
    }
}

/// The files of the host at the other end of an SFTP connection.
impl FileSystem for Sftp {
    fn metadata(&self, path: &Path) -> io::Result<Metadata> {
        Sftp::metadata(self, path)
    }

    fn symlink_metadata(&self, path: &Path) -> io::Result<Metadata> {
        Sftp::symlink_metadata(self, path)
    }

    fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        Sftp::read_link(self, path)
    }

    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        Sftp::canonicalize(self, path)
    }

    fn open(&self, path: &Path) -> io::Result<Box<dyn Read + '_>> {
        Ok(Box::new(Sftp::open(self, path)?))
    }

    fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        Sftp::write(self, path, contents)
    }

    fn write_atomically(
        &self,
        path: &Path,
        content: &mut dyn FnMut(&mut dyn Write) -> io::Result<()>,
    ) -> io::Result<()> {
        Sftp::write_atomically(self, path, content)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        Sftp::read_dir(self, path)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        Sftp::rename(self, from, to)
    }

    fn delete(&self, path: &Path) -> io::Result<()> {
        if Sftp::symlink_metadata(self, path).is_ok_and(|meta| meta.is_dir()) {
            self.remove_dir_all(path)
        } else {
            self.remove_file(path)
        }
    }

    fn mkdir(&self, path: &Path) -> io::Result<()> {
        self.create_dir_all(path)
    }

    fn symlink(&self, target: &Path, link: &Path) -> io::Result<()> {
        Sftp::symlink(self, target, link)
    }

    fn write_new(
        &self,
        path: &Path,
        mode: u32,
        modified: Option<SystemTime>,
        content: &mut dyn FnMut(&mut dyn Write) -> io::Result<()>,
    ) -> io::Result<()> {
        Sftp::write_new(self, path, mode, modified, content)
    }

    fn metadata_many(&self, paths: &[PathBuf]) -> io::Result<Vec<io::Result<Metadata>>> {
        Sftp::metadata_many(self, paths)
    }

    fn read_link_many(&self, paths: &[PathBuf]) -> io::Result<Vec<io::Result<PathBuf>>> {
        Sftp::read_link_many(self, paths)
    }

    fn set_attributes(
        &self,
        path: &Path,
        mode: Option<u32>,
        modified: Option<SystemTime>,
    ) -> io::Result<()> {
        Sftp::set_attributes(self, path, mode, modified)
    }
}

/// How long the thread that owns the editor waits for a host.
const HURRY: Duration = Duration::from_secs(3);

thread_local! {
    static PATIENT: Cell<bool> = const { Cell::new(false) };
}

/// Runs `action` with what it asks of other hosts waiting as long as a
/// slow line needs, not the few seconds that the editor's own thread can
/// spare. For threads that do work in the background.
pub fn patiently<T>(action: impl FnOnce() -> T) -> T {
    let before = PATIENT.replace(true);
    let result = action();
    PATIENT.set(before);
    result
}

/// The host and path that a name stands for, if it is the name of a file
/// on another host; `None` for a local name. A name that starts as a
/// remote one and is not whole is an error.
pub fn remote(path: &Path) -> io::Result<Option<Remote>> {
    // What is not UTF-8 is not tramp's syntax.
    let Some(name) = path.to_str() else {
        return Ok(None);
    };
    match Location::parse(name) {
        Ok(Location::Remote(remote)) => Ok(Some(remote)),
        Ok(Location::Local(_)) => Ok(None),
        Err(error) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            error.to_string(),
        )),
    }
}

/// Whether a name is that of a file on another host, or starts as one.
pub fn is_remote(path: &Path) -> bool {
    !matches!(remote(path), Ok(None))
}

/// Whether a name is that of a file on a host that there is a working
/// connection to, as far as is known without asking the host.
pub fn is_connected(path: &Path) -> bool {
    matches!(remote(path), Ok(Some(remote)) if Connections::global().is_connected(&remote))
}

/// What a file on another host is, asked over the connection that there
/// is: `None` if there is none, and none is made for this. For what
/// looks at files by itself, which must not call a host that was left.
pub fn metadata_if_connected(path: &Path) -> Option<io::Result<Metadata>> {
    let remote = remote(path).ok()??;
    let sftp = Connections::global().connected(&remote)?;
    Some(sftp.metadata(remote.host_path()))
}

/// The hosts that files have been asked of, each as the start of the
/// names of its files, with how its connection is.
pub fn hosts() -> Vec<(String, String)> {
    Connections::global().hosts()
}

/// Ends the connection to a host, named as [`hosts`] names it. Returns
/// whether the host was known.
pub fn disconnect(host: &str) -> bool {
    Connections::global().disconnect(host)
}

/// Runs `action` with the files of a host, connecting to it if need be.
fn on_host<T>(remote: &Remote, action: impl FnOnce(&Sftp) -> io::Result<T>) -> io::Result<T> {
    let run = || action(&*Connections::global().sftp(remote)?);
    if PATIENT.get() {
        run()
    } else {
        amacs_remote::with_patience(HURRY, run)
    }
}

/// Runs `action` with the file system that holds `path`, and the path
/// there.
fn on<T>(
    path: &Path,
    action: impl FnOnce(&dyn FileSystem, &Path) -> io::Result<T>,
) -> io::Result<T> {
    match remote(path)? {
        Some(remote) => on_host(&remote, |sftp| action(sftp, remote.host_path())),
        // `/:` in front of a local name is not part of it.
        None => match path.to_str().and_then(|name| name.strip_prefix("/:")) {
            Some(quoted) => action(&Local, Path::new(quoted)),
            None => action(&Local, path),
        },
    }
}

/// What a file is, following links.
pub fn metadata(path: &Path) -> io::Result<Metadata> {
    on(path, |files, path| files.metadata(path))
}

/// What a name is: of a link, the link itself.
pub fn symlink_metadata(path: &Path) -> io::Result<Metadata> {
    on(path, |files, path| files.symlink_metadata(path))
}

/// Whether the name leads to something: a link that leads nowhere does
/// not.
pub fn exists(path: &Path) -> bool {
    metadata(path).is_ok()
}

/// Whether the name is that of a directory, or of a link to one.
pub fn is_dir(path: &Path) -> bool {
    metadata(path).is_ok_and(|meta| meta.is_dir())
}

/// Where a link points.
pub fn read_link(path: &Path) -> io::Result<PathBuf> {
    on(path, |files, path| files.read_link(path))
}

/// The absolute path of a file, with links followed. See
/// [`FileSystem::canonicalize`] for a file that is not there.
pub fn canonicalize(path: &Path) -> io::Result<PathBuf> {
    match remote(path)? {
        Some(remote) => on_host(&remote, |sftp| {
            Ok(remote.name_of(&sftp.canonicalize(remote.host_path())?))
        }),
        None => on(path, |files, path| files.canonicalize(path)),
    }
}

/// Hands a file, open for reading from its start, to `reader`.
pub fn read_with<T>(
    path: &Path,
    reader: impl FnOnce(&mut dyn Read) -> io::Result<T>,
) -> io::Result<T> {
    on(path, |files, path| reader(&mut *files.open(path)?))
}

/// The contents of a file.
pub fn read(path: &Path) -> io::Result<Vec<u8>> {
    on(path, |files, path| files.read(path))
}

/// The contents of a file that holds UTF-8.
pub fn read_to_string(path: &Path) -> io::Result<String> {
    String::from_utf8(read(path)?).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "stream did not contain valid UTF-8",
        )
    })
}

/// Makes a file hold `contents` and nothing else.
pub fn write(path: &Path, contents: &[u8]) -> io::Result<()> {
    on(path, |files, path| files.write(path, contents))
}

/// Replaces a file by what `content` writes, all of it or none. See
/// [`FileSystem::write_atomically`].
pub fn write_atomically(
    path: &Path,
    mut content: impl FnMut(&mut dyn Write) -> io::Result<()>,
) -> io::Result<()> {
    on(path, |files, path| {
        files.write_atomically(path, &mut content)
    })
}

/// What a directory holds, without `.` and `..`, in no order.
pub fn read_dir(path: &Path) -> io::Result<Vec<DirEntry>> {
    on(path, |files, path| files.read_dir(path))
}

/// Gives a file or directory another name on the same file system, in
/// place of whatever file has that name already.
pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
    let to = match (remote(from)?, remote(to)?) {
        (Some(from), Some(to)) if from.hops == to.hops => to.host_path().to_owned(),
        (None, None) => to.to_owned(),
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::CrossesDevices,
                "a file is not renamed from one host to another",
            ));
        }
    };
    on(from, |files, from| files.rename(from, &to))
}

/// Deletes a file or a link; a directory with everything in it.
pub fn delete(path: &Path) -> io::Result<()> {
    on(path, |files, path| files.delete(path))
}

/// Creates a directory, and the directories above it that are missing.
pub fn mkdir(path: &Path) -> io::Result<()> {
    on(path, |files, path| files.mkdir(path))
}

/// Makes a link at `link` that points to `target`, taken as written.
pub fn symlink(target: &Path, link: &Path) -> io::Result<()> {
    on(link, |files, link| files.symlink(target, link))
}

/// Gives a file permissions, the time it was last changed, or both.
pub fn set_attributes(
    path: &Path,
    mode: Option<u32>,
    modified: Option<SystemTime>,
) -> io::Result<()> {
    on(path, |files, path| {
        files.set_attributes(path, mode, modified)
    })
}

/// What the links of a directory lead to: for each name, in order, where
/// the link points and what is there, or `None` for what cannot be
/// found out. A host is asked about all of them at once.
pub fn links(
    directory: &Path,
    names: &[std::ffi::OsString],
) -> io::Result<Vec<(Option<PathBuf>, Option<Metadata>)>> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    on(directory, |files, directory| {
        let paths: Vec<PathBuf> = names.iter().map(|name| directory.join(name)).collect();
        let targets = files.read_link_many(&paths)?;
        let followed = files.metadata_many(&paths)?;
        Ok(targets
            .into_iter()
            .zip(followed)
            .map(|(target, meta)| (target.ok(), meta.ok()))
            .collect())
    })
}

/// A name taken apart: the start that names the host, such as
/// `/ssh:evo:`, and the path there as it is written. `None` for a local
/// name, and for one that starts as a remote one and is not whole.
pub fn split(path: &Path) -> Option<(String, PathBuf)> {
    match remote(path) {
        Ok(Some(remote)) => Some((remote.host(), remote.path)),
        _ => None,
    }
}

/// Whether two names are of files that one connection, or this machine,
/// holds both.
fn same_host(a: &Path, b: &Path) -> io::Result<bool> {
    Ok(remote(a)?.map(|remote| remote.hops) == remote(b)?.map(|remote| remote.hops))
}

/// Copies a file to a new name, on the same host or another: a directory
/// with everything in it, a link as a link. Permissions and times go
/// along; owners do not. What is copied between two places of one host
/// still travels through this machine. A file is whole under its new
/// name or not there, but a directory that could not be copied whole is
/// left as far as it got. It is an error if something has the new name.
pub fn copy(from: &Path, to: &Path) -> io::Result<()> {
    if symlink_metadata(to).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} exists already", to.display()),
        ));
    }
    if same_host(from, to)? && to.starts_with(from) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a directory is not copied into itself",
        ));
    }
    copy_tree(from, to, None)
}

/// Copies what [`copy`] has found may be copied. `known` is what the
/// source is, of a link the link itself, where the listing of its
/// directory has said so already.
fn copy_tree(from: &Path, to: &Path, known: Option<Metadata>) -> io::Result<()> {
    let meta = match known {
        Some(meta) => meta,
        None => symlink_metadata(from)?,
    };
    if meta.is_symlink() {
        symlink(&read_link(from)?, to)
    } else if meta.is_dir() {
        mkdir(to)?;
        for entry in read_dir(from)? {
            copy_tree(
                &from.join(&entry.name),
                &to.join(&entry.name),
                entry.metadata,
            )?;
        }
        // Last, so that a directory that may not be written to can be
        // filled, and its time is not that of the filling.
        set_attributes(to, Some(meta.mode), meta.modified)
    } else if meta.is_file() {
        read_with(from, |reader| {
            on(to, |files, to| {
                files.write_new(to, meta.mode, meta.modified, &mut |out| {
                    io::copy(reader, out).map(drop)
                })
            })
        })
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is neither a file nor a directory", from.display()),
        ))
    }
}

/// Moves a file or directory to a new name: by renaming where that can
/// be, and else, as between hosts, by copying it and deleting the
/// original once the copy is whole. It is an error if something has the
/// new name.
pub fn move_to(from: &Path, to: &Path) -> io::Result<()> {
    if symlink_metadata(to).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} exists already", to.display()),
        ));
    }
    match rename(from, to) {
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
            copy(from, to)?;
            delete(from)
        }
        result => result,
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{PermissionsExt, symlink};

    use super::*;

    /// What every file system does alike, tried in a directory that is
    /// empty and also a directory of this machine.
    fn behaves(files: &dyn FileSystem, root: &Path) {
        let at = |name: &str| root.join(name);

        // Writing, whole or not at all.
        files
            .write_atomically(&at("file"), &mut |out| out.write_all(b"first"))
            .unwrap();
        assert_eq!(files.read(&at("file")).unwrap(), b"first");
        fs::set_permissions(at("file"), fs::Permissions::from_mode(0o640)).unwrap();
        symlink("file", at("link")).unwrap();
        files
            .write_atomically(&at("link"), &mut |out| {
                out.write_all(b"second ")?;
                out.write_all(b"time")
            })
            .unwrap();
        assert_eq!(fs::read(at("file")).unwrap(), b"second time");
        assert!(fs::symlink_metadata(at("link")).unwrap().is_symlink());
        let failed = files.write_atomically(&at("file"), &mut |out| {
            out.write_all(b"half")?;
            Err(io::Error::other("stop"))
        });
        assert_eq!(failed.unwrap_err().to_string(), "stop");
        assert_eq!(fs::read(at("file")).unwrap(), b"second time");

        // Looking.
        let meta = files.metadata(&at("link")).unwrap();
        assert!(meta.is_file() && !meta.is_executable());
        assert_eq!((meta.len, meta.mode), (11, 0o640));
        let on_disk = fs::metadata(at("file")).unwrap().modified().unwrap();
        let age = on_disk.duration_since(meta.modified.unwrap()).unwrap();
        assert!(age.as_secs() == 0, "the time is the file's, to the second");
        assert!(files.symlink_metadata(&at("link")).unwrap().is_symlink());
        assert_eq!(files.read_link(&at("link")).unwrap(), Path::new("file"));
        assert_eq!(
            files.canonicalize(&at("link")).unwrap(),
            fs::canonicalize(root).unwrap().join("file")
        );
        for missing in [
            files.metadata(&at("none")).err(),
            files.symlink_metadata(&at("none")).err(),
            files.read(&at("none")).err(),
            files.open(&at("none")).err(),
            files.read_dir(&at("none")).err(),
            files.delete(&at("none")).err(),
            files.rename(&at("none"), &at("other")).err(),
        ] {
            let error = missing.expect("nothing is there");
            assert_eq!(error.kind(), io::ErrorKind::NotFound);
            assert_eq!(
                error.to_string().trim_end_matches(" (os error 2)"),
                "No such file or directory"
            );
        }

        // Reading from the start.
        let mut start = [0; 6];
        files
            .open(&at("file"))
            .unwrap()
            .read_exact(&mut start)
            .unwrap();
        assert_eq!(&start, b"second");

        // Directories.
        files.mkdir(&at("a/b/c")).unwrap();
        files.mkdir(&at("a/b")).unwrap();
        files.write(&at("a/b/file"), b"inner").unwrap();
        files.write(&at("a/b/file"), b"in").unwrap();
        assert_eq!(fs::read(at("a/b/file")).unwrap(), b"in");
        symlink(at("a/b"), at("a/b/c/up")).unwrap();
        let mut entries = files.read_dir(root).unwrap();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let kinds: Vec<(&str, FileType)> = entries
            .iter()
            .map(|entry| {
                (
                    entry.name.to_str().unwrap(),
                    entry.metadata.unwrap().file_type,
                )
            })
            .collect();
        assert_eq!(
            kinds,
            [
                ("a", FileType::Directory),
                ("file", FileType::File),
                ("link", FileType::Symlink),
            ]
        );

        // Links and attributes.
        files.symlink(Path::new("a/b"), &at("made")).unwrap();
        assert_eq!(fs::read_link(at("made")).unwrap(), Path::new("a/b"));
        fs::remove_file(at("made")).unwrap();
        let then = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        for name in ["a/b/file", "a/b"] {
            files
                .set_attributes(&at(name), Some(0o750), Some(then))
                .unwrap();
            let meta = fs::metadata(at(name)).unwrap();
            assert_eq!(meta.permissions().mode() & 0o7777, 0o750);
            assert_eq!(meta.modified().unwrap(), then);
        }
        files.set_attributes(&at("a/b/file"), None, None).unwrap();
        files
            .set_attributes(&at("a/b/file"), Some(0o600), None)
            .unwrap();
        let meta = fs::metadata(at("a/b/file")).unwrap();
        assert_eq!(meta.permissions().mode() & 0o7777, 0o600);
        assert_eq!(meta.modified().unwrap(), then);

        // Many at once, each answered for itself.
        let asked = [at("a/b"), at("none"), at("link"), at("file")];
        let looked = files.metadata_many(&asked).unwrap();
        assert!(looked[0].as_ref().unwrap().is_dir());
        assert_eq!(
            looked[1].as_ref().unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert!(looked[2].as_ref().unwrap().is_file());
        let targets = files.read_link_many(&asked).unwrap();
        assert_eq!(targets[2].as_ref().unwrap(), Path::new("file"));
        assert!(targets[0].is_err() && targets[1].is_err() && targets[3].is_err());

        // A new file, with its mode and time.
        files
            .write_new(&at("new"), 0o640, Some(then), &mut |out| {
                out.write_all(b"new")
            })
            .unwrap();
        let meta = fs::metadata(at("new")).unwrap();
        assert_eq!(fs::read(at("new")).unwrap(), b"new");
        assert_eq!(meta.permissions().mode() & 0o7777, 0o640);
        assert_eq!(meta.modified().unwrap(), then);
        fs::remove_file(at("new")).unwrap();

        // Renaming, also onto a name that is taken.
        files.rename(&at("a/b/file"), &at("moved")).unwrap();
        files.rename(&at("moved"), &at("file")).unwrap();
        assert_eq!(fs::read(at("file")).unwrap(), b"in");
        files.rename(&at("a"), &at("z")).unwrap();

        // Deleting: a link, not what it leads to; a directory with what
        // is in it.
        files.delete(&at("link")).unwrap();
        assert!(at("file").exists());
        fs::create_dir(at("kept")).unwrap();
        symlink(at("kept"), at("z/kept-link")).unwrap();
        files.delete(&at("z")).unwrap();
        files.delete(&at("file")).unwrap();
        let left: Vec<_> = files.read_dir(root).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].name, "kept");
    }

    #[test]
    fn names_of_remote_files_reach_their_host() {
        // The methods are Scheme's to define; these stand in for them.
        amacs_remote::define_fixtures();
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let there = |name: &str| PathBuf::from(format!("/sftp-test::{}/{name}", root.display()));

        assert!(is_remote(&there("file")));
        assert!(is_remote(Path::new("/ssh:half")));
        assert!(!is_remote(&root));
        assert!(!is_remote(Path::new("/:/ssh:odd:/name")));

        write(&there("file"), b"over there").unwrap();
        assert_eq!(fs::read(root.join("file")).unwrap(), b"over there");
        assert!(is_connected(&there("file")));
        assert!(exists(&there("file")) && !is_dir(&there("file")));
        assert_eq!(read_to_string(&there("file")).unwrap(), "over there");
        write_atomically(&there("file"), |out| out.write_all(b"again")).unwrap();
        assert_eq!(metadata(&there("file")).unwrap().len, 5);

        // Names that come back are names of files on that host.
        symlink("file", root.join("link")).unwrap();
        assert_eq!(canonicalize(&there("link")).unwrap(), there("file"));
        assert_eq!(read_link(&there("link")).unwrap(), Path::new("file"));

        mkdir(&there("sub/deeper")).unwrap();
        rename(&there("file"), &there("sub/moved")).unwrap();
        assert!(root.join("sub/moved").exists());
        let across = rename(&there("sub/moved"), &root.join("back")).unwrap_err();
        assert_eq!(across.kind(), io::ErrorKind::CrossesDevices);
        let names: Vec<_> = read_dir(&there("")).unwrap();
        assert_eq!(names.len(), 2);
        delete(&there("sub")).unwrap();
        assert!(!root.join("sub").exists());

        // A local name that looks remote, marked as local.
        let odd = PathBuf::from(format!("/:{}/ssh:odd", root.display()));
        write(&odd, b"here").unwrap();
        assert_eq!(fs::read(root.join("ssh:odd")).unwrap(), b"here");

        let half = metadata(Path::new("/ssh:half")).unwrap_err();
        assert_eq!(half.kind(), io::ErrorKind::InvalidInput);
        let unreached = metadata(Path::new("/ssh:evo|sudo:pi:/etc")).unwrap_err();
        assert_eq!(unreached.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            unreached.to_string(),
            "sudo is for the host before it, not for pi: write sudo::"
        );
    }

    #[test]
    fn files_are_copied_and_moved_between_here_and_there() {
        // The methods are Scheme's to define; these stand in for them.
        amacs_remote::define_fixtures();
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let here = |name: &str| root.join(name);
        let there = |name: &str| PathBuf::from(format!("/sftp-test::{}/{name}", root.display()));
        let then = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);

        // More than one piece of a file, and all that a directory holds.
        let big: Vec<u8> = (0..300_000u32).map(|n| (n % 251) as u8).collect();
        fs::create_dir_all(here("tree/inner")).unwrap();
        fs::write(here("tree/big"), &big).unwrap();
        fs::write(here("tree/inner/run"), "#!/bin/sh\n").unwrap();
        fs::set_permissions(here("tree/inner/run"), fs::Permissions::from_mode(0o755)).unwrap();
        symlink("inner/run", here("tree/link")).unwrap();
        symlink("nowhere", here("tree/broken")).unwrap();
        File::open(here("tree/big"))
            .unwrap()
            .set_modified(then)
            .unwrap();
        File::open(here("tree/inner"))
            .unwrap()
            .set_modified(then)
            .unwrap();

        let same = |copy: &Path| {
            assert_eq!(fs::read(copy.join("big")).unwrap(), big);
            let run = fs::metadata(copy.join("inner/run")).unwrap();
            assert_eq!(run.permissions().mode() & 0o7777, 0o755);
            assert_eq!(
                fs::read_link(copy.join("link")).unwrap(),
                Path::new("inner/run")
            );
            assert_eq!(
                fs::read_link(copy.join("broken")).unwrap(),
                Path::new("nowhere")
            );
            let modified = |name: &str| fs::metadata(copy.join(name)).unwrap().modified().unwrap();
            assert_eq!(modified("big"), then);
            assert_eq!(modified("inner"), then);
        };
        copy(&here("tree"), &there("up")).unwrap();
        same(&here("up"));
        copy(&there("up"), &here("down")).unwrap();
        same(&here("down"));
        copy(&there("up"), &there("beside")).unwrap();
        same(&here("beside"));
        copy(&here("tree"), &here("local")).unwrap();
        same(&here("local"));

        // Nothing is written over, and nothing goes into itself.
        let taken = copy(&here("tree"), &there("up")).unwrap_err();
        assert_eq!(taken.kind(), io::ErrorKind::AlreadyExists);
        let inside = copy(&there("up"), &there("up/inner/again")).unwrap_err();
        assert_eq!(inside.to_string(), "a directory is not copied into itself");
        assert!(!here("up/inner/again").exists());
        // A name that only starts alike is another place.
        copy(&there("up"), &there("upper")).unwrap();

        // Moving: a rename on one host, a copy and a deletion between two.
        move_to(&there("up"), &there("renamed")).unwrap();
        assert!(!here("up").exists());
        same(&here("renamed"));
        move_to(&there("renamed"), &here("fetched")).unwrap();
        assert!(!here("renamed").exists());
        same(&here("fetched"));
        let taken = move_to(&here("fetched"), &there("down")).unwrap_err();
        assert_eq!(taken.kind(), io::ErrorKind::AlreadyExists);
        assert!(here("fetched").exists());

        assert_eq!(
            split(&there("x")),
            Some(("/sftp-test::".to_owned(), root.join("x")))
        );
        assert_eq!(
            split(Path::new("/ssh:me@evo:")),
            Some(("/ssh:me@evo:".to_owned(), PathBuf::new()))
        );
        assert_eq!(split(&root), None);
    }

    #[test]
    fn local_files_behave() {
        let dir = tempfile::tempdir().unwrap();
        behaves(&Local, dir.path());
    }

    #[test]
    fn files_over_sftp_behave_as_local_ones() {
        let dir = tempfile::tempdir().unwrap();
        let server = amacs_remote::sftp_server().expect("OpenSSH's sftp-server is on this machine");
        let sftp = Sftp::spawn(std::process::Command::new(server)).unwrap();
        behaves(&sftp, dir.path());
    }
}
