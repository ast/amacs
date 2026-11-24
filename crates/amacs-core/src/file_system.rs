//! The one place where the files that a person works on are reached.
//!
//! Everything that visits, saves, lists, renames or deletes a file by a
//! name that the user or a program gave goes through the functions of
//! this module, and they through a [`FileSystem`]: [`Local`] for the files
//! of this machine, an SFTP connection for those of another. What the
//! editor keeps for itself on this machine does not come here: auto-save
//! data, histories, dictionaries, the Scheme that it loads.
//!
//! For now every path is a local one.

use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use amacs_remote::Sftp;
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
}

/// Runs `action` with the file system that holds `path`, and the path
/// there. This is where a name of a file on another host will find its
/// connection.
fn on<T>(
    path: &Path,
    action: impl FnOnce(&dyn FileSystem, &Path) -> io::Result<T>,
) -> io::Result<T> {
    action(&Local, path)
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
    on(path, |files, path| files.canonicalize(path))
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
    on(from, |files, from| files.rename(from, to))
}

/// Deletes a file or a link; a directory with everything in it.
pub fn delete(path: &Path) -> io::Result<()> {
    on(path, |files, path| files.delete(path))
}

/// Creates a directory, and the directories above it that are missing.
pub fn mkdir(path: &Path) -> io::Result<()> {
    on(path, |files, path| files.mkdir(path))
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
