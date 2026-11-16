//! Visiting and saving files.

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use amacs_text::Text;

use crate::buffer::{Buffer, BufferId};
use crate::editor::Editor;
use crate::error::{CmdError, CmdResult};

/// Expands a leading `~` and resolves a relative path against `base`,
/// without touching the file system.
pub fn expand_path(path: &str, base: &Path) -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let expanded = match (path.strip_prefix('~'), home) {
        (Some(""), Some(home)) => home,
        (Some(rest), Some(home)) if rest.starts_with('/') => home.join(&rest[1..]),
        _ => PathBuf::from(path),
    };
    base.join(expanded)
}

/// Shortens a path under the home directory to `~/...` for display.
pub fn abbreviate_path(path: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match home.and_then(|home| path.strip_prefix(home).ok().map(Path::to_owned)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// Reads a file as UTF-8. Text that is not valid UTF-8 is decoded lossily;
/// the flag reports that, since saving such text would change the file.
fn read_text(path: &Path) -> io::Result<(Text, bool)> {
    match Text::from_reader(File::open(path)?) {
        Ok(text) => Ok((text, false)),
        Err(error) if error.kind() == io::ErrorKind::InvalidData => {
            let bytes = fs::read(path)?;
            Ok((Text::from(&*String::from_utf8_lossy(&bytes)), true))
        }
        Err(error) => Err(error),
    }
}

/// Writes `text` to `path` so that a crash cannot leave a half-written file:
/// the text goes to a temporary file that is then renamed over the target.
fn write_atomically(path: &Path, text: &Text) -> io::Result<()> {
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
        text.write_to(&mut writer)?;
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

pub fn modified_time(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|meta| meta.modified()).ok()
}

impl Editor {
    /// The directory that file names typed by the user are relative to: the
    /// current buffer's directory, or the editor's default directory.
    pub fn working_directory(&self) -> &Path {
        self.buffer()
            .file
            .as_deref()
            .and_then(Path::parent)
            .unwrap_or(&self.default_directory)
    }

    /// Shows `path` in the selected window, reading it if no buffer visits it
    /// yet. A path that does not exist gives an empty buffer for a new file.
    pub fn find_file(&mut self, path: &Path) -> CmdResult {
        let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        if let Some(id) = self.buffer_visiting(&path) {
            self.switch_to_buffer(id);
            return Ok(());
        }
        if path.is_dir() {
            return Err(CmdError::msg(format!(
                "{} is a directory",
                abbreviate_path(&path)
            )));
        }
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );

        let (text, lossy, is_new) = match read_text(&path) {
            Ok((text, lossy)) => (text, lossy, false),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (Text::new(), false, true),
            Err(error) => {
                return Err(CmdError::msg(format!(
                    "Cannot open {}: {error}",
                    abbreviate_path(&path)
                )));
            }
        };
        let mut buffer = Buffer::with_text(name, text);
        buffer.disk_mtime = modified_time(&path);
        buffer.file = Some(path);
        buffer.read_only = lossy;
        let id = self.add_buffer(buffer);
        self.switch_to_buffer(id);
        if lossy {
            self.message("File is not valid UTF-8; opened read-only with replacement characters");
        } else if is_new {
            self.message("(New file)");
        }
        self.run_hook("find-file-hook", Vec::new());
        Ok(())
    }

    fn buffer_visiting(&self, path: &Path) -> Option<BufferId> {
        self.buffer_ids()
            .find(|id| self.buffers[*id].file.as_deref() == Some(path))
    }

    /// Writes the current buffer to `path` and makes it the buffer's file.
    pub fn write_buffer(&mut self, path: &Path) -> CmdResult {
        write_atomically(path, self.text()).map_err(|error| {
            CmdError::msg(format!("Cannot write {}: {error}", abbreviate_path(path)))
        })?;
        let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        let shown = abbreviate_path(&path);
        let renamed = self.buffer().file.as_deref() != Some(&path);
        if renamed {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| shown.clone());
            // Clear the old name first so that it does not count as taken.
            self.buffer_mut().name.clear();
            let unique = (1..)
                .map(|n| {
                    if n == 1 {
                        name.clone()
                    } else {
                        format!("{name}<{n}>")
                    }
                })
                .find(|candidate| self.find_buffer(candidate).is_none())
                .expect("an unused name exists");
            self.buffer_mut().name = unique;
        }
        let buffer = self.buffer_mut();
        buffer.disk_mtime = modified_time(&path);
        buffer.file = Some(path);
        buffer.mark_saved();
        self.message(format!("Wrote {shown}"));
        self.run_hook("after-save-hook", Vec::new());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_expands_only_as_a_whole_component() {
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME is set"));
        let base = Path::new("/base");
        assert_eq!(expand_path("~", base), home);
        assert_eq!(expand_path("~/a/b", base), home.join("a/b"));
        assert_eq!(expand_path("/tmp/~x", base), PathBuf::from("/tmp/~x"));
        assert_eq!(
            expand_path("sub/file", base),
            PathBuf::from("/base/sub/file")
        );
        assert_eq!(expand_path("", base), PathBuf::from("/base/"));
        assert_eq!(abbreviate_path(&home.join("a")), "~/a");
        assert_eq!(abbreviate_path(&home), "~");
        assert_eq!(abbreviate_path(Path::new("/etc/hosts")), "/etc/hosts");
    }
}
