//! Visiting and saving files.

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use amacs_text::Text;

use crate::buffer::{Buffer, BufferId};
use crate::editor::Editor;
use crate::error::{CmdError, CmdResult};
use crate::script::Value;

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

/// Writes auto-save data, creating the directory for the user only.
fn write_auto_save(path: &Path, text: &Text) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    if let Some(dir) = path.parent() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }
    write_atomically(path, text)
}

pub fn modified_time(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|meta| meta.modified()).ok()
}

/// File name under the auto-save directory for a visited file: the path
/// with `/` written as `!`, and `!` doubled so that the mapping is unique.
fn auto_save_name(file: &Path) -> String {
    file.to_string_lossy().replace('!', "!!").replace('/', "!")
}

impl Editor {
    /// Sets the directory for data that outlives the process, such as
    /// auto-save files. Without one, nothing is auto-saved.
    pub fn set_state_directory(&mut self, dir: Option<PathBuf>) {
        if let Some(dir) = &dir {
            self.history = crate::history::History::load(dir);
        }
        self.state_directory = dir;
    }

    /// Writes input history and the list of recent files to the state
    /// directory, if they have changed.
    pub fn save_state(&mut self) {
        if let Some(dir) = self.state_directory.clone()
            && let Err(error) = self.history.save(&dir)
        {
            self.message(format!("Cannot save history: {error}"));
        }
    }

    fn auto_save_path(&self, file: &Path) -> Option<PathBuf> {
        let dir = self.state_directory.as_ref()?;
        Some(dir.join("auto-save").join(auto_save_name(file)))
    }

    pub(crate) fn remove_auto_save(&self, file: &Path) {
        if let Some(path) = self.auto_save_path(file) {
            // It usually does not exist.
            let _ = fs::remove_file(path);
        }
    }

    /// Writes the modified file-visiting buffers that have changed since
    /// their last auto-save to the auto-save directory, and removes the
    /// auto-save files of buffers that are no longer modified. With `force`,
    /// writes every modified buffer. Returns the number of files written.
    ///
    /// The visited files themselves are never touched.
    pub fn auto_save(&mut self, force: bool) -> usize {
        let ids: Vec<BufferId> = self.buffers.keys().collect();
        let mut written = 0;
        let mut failure = None;
        for id in ids {
            let buffer = &self.buffers[id];
            let Some(path) = buffer
                .file
                .as_deref()
                .and_then(|file| self.auto_save_path(file))
            else {
                continue;
            };
            let tick = buffer.tick();
            if !buffer.is_modified() {
                if buffer.auto_saved_tick.is_some() {
                    let _ = fs::remove_file(&path);
                    self.buffers[id].auto_saved_tick = None;
                }
                continue;
            }
            if !force && buffer.auto_saved_tick == Some(tick) {
                continue;
            }
            match write_auto_save(&path, buffer.text()) {
                Ok(()) => {
                    self.buffers[id].auto_saved_tick = Some(tick);
                    written += 1;
                }
                Err(error) => failure = Some(format!("Auto-save failed: {error}")),
            }
        }
        if let Some(message) = failure {
            self.message(message);
        }
        written
    }

    /// The auto-save file for the current buffer, if it exists and is not
    /// older than the visited file.
    fn newer_auto_save(&self) -> Option<PathBuf> {
        let file = self.buffer().file.as_deref()?;
        let path = self.auto_save_path(file)?;
        let saved = modified_time(&path)?;
        // Timestamps are coarse, so data written just after the file can
        // carry the same time. It is removed on save, so it is still wanted.
        let current = modified_time(file).is_none_or(|on_disk| saved >= on_disk);
        current.then_some(path)
    }

    /// Replaces the current buffer's text with its auto-save data. The
    /// change can be undone, and nothing is written to the visited file.
    pub fn recover_this_file(&mut self) -> CmdResult {
        let path = self
            .newer_auto_save()
            .ok_or_else(|| CmdError::msg("No auto-save data for this buffer"))?;
        let (text, _) = read_text(&path)?;
        let len = self.buffer().len();
        self.delete(0..len)?;
        self.buffer_mut().insert(0, &text.to_string())?;
        self.set_point(0);
        self.message("Recovered from auto-save data; save the buffer to keep it");
        Ok(())
    }

    /// Reads a buffer's file again, replacing the text and forgetting its
    /// undo history. Windows keep their place by line and column.
    pub fn revert_buffer(&mut self, id: BufferId) -> CmdResult {
        let file = self.buffers[id]
            .file
            .clone()
            .ok_or_else(|| CmdError::msg("This buffer does not visit a file"))?;
        let (text, lossy) = read_text(&file).map_err(|error| {
            CmdError::msg(format!("Cannot read {}: {error}", abbreviate_path(&file)))
        })?;
        // Where each window showing the buffer is, as lines and a column.
        let places: Vec<(crate::window::WindowId, usize, usize, usize)> = self
            .windows
            .iter()
            .filter(|(_, window)| window.buffer == id)
            .map(|(window_id, window)| {
                let buffer = &self.buffers[id];
                let old = buffer.text();
                let point = buffer.marker(window.point);
                let line = old.line_of(point);
                let start_line = old.line_of(buffer.marker(window.start));
                (window_id, line, point - old.line_start(line), start_line)
            })
            .collect();
        let buffer = &mut self.buffers[id];
        buffer.reset(text);
        buffer.read_only = lossy;
        buffer.disk_mtime = modified_time(&file);
        buffer.auto_saved_tick = None;
        let place = |text: &Text, line: usize, column: usize| {
            let line = line.min(text.len_lines() - 1);
            (text.line_start(line) + column).min(text.line_end(line))
        };
        for (window_id, line, column, start_line) in places {
            let window = &self.windows[window_id];
            let (point, start) = (window.point, window.start);
            let buffer = &mut self.buffers[id];
            let pos = place(buffer.text(), line, column);
            buffer.set_marker(point, pos);
            let pos = place(buffer.text(), start_line, 0);
            buffer.set_marker(start, pos);
        }
        Ok(())
    }

    /// Reads again the files that another program has changed, for buffers
    /// that have no changes of their own to lose.
    pub(crate) fn auto_revert(&mut self) {
        if !matches!(
            self.option("auto-revert"),
            Ok(value) if value != crate::script::Value::Bool(false)
        ) {
            return;
        }
        let changed: Vec<BufferId> = self
            .buffers
            .iter()
            .filter(|(_, buffer)| {
                let Some(file) = buffer.file.as_deref() else {
                    return false;
                };
                let on_disk = modified_time(file);
                !buffer.is_modified() && on_disk.is_some() && on_disk != buffer.disk_mtime
            })
            .map(|(id, _)| id)
            .collect();
        for id in changed {
            let name = self.buffers[id].name.clone();
            match self.revert_buffer(id) {
                Ok(()) => self.message(format!("Reverted {name}: its file changed on disk")),
                Err(error) => self.message(error.to_string()),
            }
        }
    }

    /// The directory that file names typed by the user are relative to: the
    /// current buffer's directory, or the editor's default directory.
    pub fn working_directory(&self) -> &Path {
        let buffer = self.buffer();
        buffer
            .directory
            .as_deref()
            .or_else(|| buffer.file.as_deref().and_then(Path::parent))
            .unwrap_or(&self.default_directory)
    }

    /// Shows `path` in the selected window, reading it if no buffer visits it
    /// yet. A path that does not exist gives an empty buffer for a new file.
    pub fn find_file(&mut self, path: &Path) -> CmdResult {
        // Directories are shown by the file manager, which is Scheme.
        if path.is_dir() && self.has_script_host() {
            let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
            self.queue_named(
                "%visit-directory",
                vec![Value::Str(path.display().to_string())],
            );
            return Ok(());
        }
        let (id, note) = self.visit_file(path)?;
        self.switch_to_buffer(id);
        if let Some(note) = note {
            self.message(note);
        }
        if self.buffer().file.is_some() && self.newer_auto_save().is_some() {
            self.message(format!(
                "{} has auto-save data; use M-x recover-this-file",
                self.buffer().name
            ));
        }
        Ok(())
    }

    /// The buffer that visits `path`, reading the file if no buffer does
    /// yet, without showing it. Also returns something to tell the user
    /// about a file just read, if there is anything. The hooks that choose
    /// the buffer's mode run when it is first shown.
    pub(crate) fn visit_file(
        &mut self,
        path: &Path,
    ) -> CmdResult<(BufferId, Option<&'static str>)> {
        let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        if let Some(id) = self.buffer_visiting(&path) {
            return Ok((id, None));
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
        self.history.visit(&path);
        self.note_project(&path);
        buffer.file = Some(path);
        buffer.read_only = lossy;
        buffer.needs_file_hook = true;
        let id = self.add_buffer(buffer);
        let note = if lossy {
            Some("File is not valid UTF-8; opened read-only with replacement characters")
        } else if is_new {
            Some("(New file)")
        } else {
            None
        };
        Ok((id, note))
    }

    fn buffer_visiting(&self, path: &Path) -> Option<BufferId> {
        self.buffer_ids()
            .find(|id| self.buffers[*id].file.as_deref() == Some(path))
    }

    /// Saves the current buffer to `path` and makes it the buffer's file.
    pub fn write_buffer(&mut self, path: &Path) -> CmdResult {
        self.save_to(path, false)
    }

    /// Saves the current buffer to `path` and kills it once it is written.
    pub fn write_buffer_and_kill(&mut self, path: &Path) -> CmdResult {
        self.save_to(path, true)
    }

    fn save_to(&mut self, path: &Path, kill: bool) -> CmdResult {
        // With functions on the hook that runs before saving, Scheme runs
        // them and then asks for the write. They may wait for something, so
        // the buffer is named: another may be current by then.
        if self.hook_active("before-save-hook") && self.has_script_host() {
            let name = self.buffer().name.as_str().into();
            let path = path.display().to_string().into();
            self.queue_named("%save-buffer", vec![name, path, kill.into()]);
            return Ok(());
        }
        self.write_buffer_now(path)?;
        if kill {
            let id = self.window().buffer;
            self.kill_buffer(id);
        }
        Ok(())
    }

    /// Writes the current buffer to `path`, without the hook that runs
    /// before saving.
    pub fn write_buffer_now(&mut self, path: &Path) -> CmdResult {
        let unformatted = self.format_before_save();
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
        self.remove_auto_save(&path);
        let point = self.point();
        let buffer = self.buffer_mut();
        // What a formatter or a hook changed just now is part of what was
        // written.
        buffer.commit_edits(point);
        buffer.disk_mtime = modified_time(&path);
        buffer.file = Some(path);
        buffer.auto_saved_tick = None;
        buffer.mark_saved();
        match unformatted {
            Some(problem) => self.message(format!("Wrote {shown}, not formatted: {problem}")),
            None => self.message(format!("Wrote {shown}")),
        }
        let id = self.window().buffer;
        self.lsp_did_save(id);
        // The hook runs later, when another buffer may be current.
        if self.hook_active("after-save-hook") {
            let name = self.buffer().name.as_str().into();
            self.queue_named("%run-hook-in", vec![name, Value::sym("after-save-hook")]);
        }
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
