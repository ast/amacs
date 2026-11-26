//! Visiting and saving files.

use std::collections::HashSet;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use amacs_text::Text;

use crate::buffer::{Buffer, BufferId};
use crate::editor::Editor;
use crate::error::{CmdError, CmdResult};
use crate::file_system::{self, FileSystem};
use crate::script::{LARGE_FILE_THRESHOLD, Value};

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

/// Expands a file name that a person typed, as [`expand_path`] does, after
/// dropping what they typed over: as in Emacs, `//` and `/~` start the
/// name anew, so that `~/src//ssh:host:/etc` is `/ssh:host:/etc` and
/// nothing has to be deleted first.
pub fn expand_typed_path(path: &str, base: &Path) -> PathBuf {
    expand_path(typed_name(path), base)
}

/// What is left of a file name that a person typed when what they typed
/// over is dropped: see [`expand_typed_path`].
pub fn typed_name(path: &str) -> &str {
    let root = path.rfind("//").map(|at| at + 1);
    let home = path
        .rmatch_indices("/~")
        .map(|(at, _)| at + 1)
        .find(|at| matches!(path[at + 1..].chars().next(), None | Some('/')));
    &path[root.max(home).unwrap_or(0)..]
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

/// A size in bytes as a person reads it: `812`, `4.2K`, `700M`.
pub fn format_size(bytes: u64) -> String {
    let mut size = bytes as f64;
    let mut units = ["", "K", "M", "G", "T"].into_iter();
    let mut unit = units.next().unwrap_or("");
    while size >= 1024.0 {
        match units.next() {
            Some(next) => unit = next,
            None => break,
        }
        size /= 1024.0;
    }
    if unit.is_empty() || size >= 10.0 {
        format!("{size:.0}{unit}")
    } else {
        format!("{size:.1}{unit}")
    }
}

/// What a file is, as far as reading it into a buffer goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// Text of a size that is read without a thought, or no file yet.
    Text,
    /// Not text: reading it would show nothing of use.
    Binary,
    /// Text of so many bytes that reading it holds the editor up.
    Large(u64),
}

/// Whether a file is something other than text, by what git and grep go
/// by: a NUL byte near its start. Only the start is read.
pub fn is_binary(path: &Path) -> io::Result<bool> {
    let mut start = Vec::with_capacity(8192);
    file_system::read_with(path, |file| file.take(8192).read_to_end(&mut start))?;
    Ok(start.contains(&0))
}

/// Reads a file as UTF-8. Text that is not valid UTF-8 is decoded lossily;
/// the flag reports that, since saving such text would change the file.
fn read_text(path: &Path) -> io::Result<(Text, bool)> {
    match file_system::read_with(path, |file| Text::from_reader(file)) {
        Ok(text) => Ok((text, false)),
        Err(error) if error.kind() == io::ErrorKind::InvalidData => {
            let bytes = file_system::read(path)?;
            Ok((Text::from(&*String::from_utf8_lossy(&bytes)), true))
        }
        Err(error) => Err(error),
    }
}

/// What a file is, from what a look at it gave. Text up to `limit` bytes
/// is read without a thought.
fn kind_of(path: &Path, meta: Option<&file_system::Metadata>, limit: Option<u64>) -> FileKind {
    let Some(meta) = meta.filter(|meta| meta.is_file()) else {
        return FileKind::Text;
    };
    if is_binary(path).unwrap_or(false) {
        return FileKind::Binary;
    }
    match limit {
        Some(limit) if meta.len > limit => FileKind::Large(meta.len),
        _ => FileKind::Text,
    }
}

/// What there was to visit under a name.
pub(crate) enum Fetched {
    /// A directory, which the file manager shows.
    Directory(PathBuf),
    /// Text of so many bytes that reading it has to be asked for.
    Large(PathBuf, u64),
    /// A file and its text, or a name with no file yet.
    File {
        path: PathBuf,
        text: Text,
        /// The file is not valid UTF-8, and the text is not what it holds.
        lossy: bool,
        is_new: bool,
        modified: Option<SystemTime>,
    },
}

/// Looks at what a name leads to, and reads it if it is text that is not
/// larger than `limit` bytes, or of any size with `anyway`. An error is
/// what to tell the user. This touches no editor state: a file on another
/// host is fetched on a thread of its own.
pub(crate) fn fetch(path: &Path, anyway: bool, limit: Option<u64>) -> Result<Fetched, String> {
    let path = file_system::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let cannot_open = |error: io::Error| format!("Cannot open {}: {error}", abbreviate_path(&path));
    let meta = match file_system::metadata(&path) {
        Ok(meta) => Some(meta),
        // Of a local file, reading it says what is wrong. Of a remote
        // one, what is wrong may be the way there.
        Err(error) if error.kind() != io::ErrorKind::NotFound && file_system::is_remote(&path) => {
            return Err(cannot_open(error));
        }
        Err(_) => None,
    };
    if meta.is_some_and(|meta| meta.is_dir()) {
        return Ok(Fetched::Directory(path));
    }
    match kind_of(&path, meta.as_ref(), limit) {
        FileKind::Binary => {
            return Err(format!(
                "{} is not text ({})",
                abbreviate_path(&path),
                format_size(meta.map_or(0, |meta| meta.len))
            ));
        }
        FileKind::Large(size) if !anyway => return Ok(Fetched::Large(path, size)),
        FileKind::Large(_) | FileKind::Text => {}
    }
    let (text, lossy, is_new) = match read_text(&path) {
        Ok((text, lossy)) => (text, lossy, false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => (Text::new(), false, true),
        Err(error) => return Err(cannot_open(error)),
    };
    let modified = if is_new {
        None
    } else {
        meta.and_then(|meta| meta.modified)
    };
    Ok(Fetched::File {
        path,
        text,
        lossy,
        is_new,
        modified,
    })
}

/// Files on other hosts that buffers visit are looked at this often, to
/// see whether another program has changed them. Local ones are looked at
/// far more often; this costs an answer from the host each time.
const REMOTE_REVERT_INTERVAL: Duration = Duration::from_secs(30);

/// What a directory holds, for the file manager: see `directory-entries`
/// in the Scheme that is part of the editor.
pub struct Listed {
    name: String,
    is_dir: bool,
    size: u64,
    /// When it was last changed, in seconds since 1970.
    changed: u64,
    link: Option<PathBuf>,
    runs: bool,
}

impl From<Listed> for Value {
    fn from(entry: Listed) -> Self {
        Value::List(vec![
            entry.name.into(),
            Value::sym(if entry.is_dir { "directory" } else { "file" }),
            Value::Int(entry.size as i64),
            Value::Int(entry.changed as i64),
            entry.link.map(|target| target.display().to_string()).into(),
            entry.runs.into(),
        ])
    }
}

/// What a directory holds, in order of name. A link counts as what it
/// leads to.
pub fn list_directory(directory: &Path) -> io::Result<Vec<Listed>> {
    let mut entries = Vec::new();
    for entry in file_system::read_dir(directory)? {
        let path = directory.join(&entry.name);
        let own = entry.metadata;
        let is_link = own.is_some_and(|own| own.is_symlink());
        let link = is_link
            .then(|| file_system::read_link(&path).ok())
            .flatten();
        // What a link leads to says what it is, if it leads anywhere;
        // nothing else needs a second look.
        let followed = match own {
            Some(own) if !is_link => Some(own),
            _ => file_system::metadata(&path).ok().or(own),
        };
        let (is_dir, size, changed, runs) = followed.map_or((false, 0, 0, false), |meta| {
            let changed = meta
                .modified
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |since| since.as_secs());
            (meta.is_dir(), meta.len, changed, meta.is_executable())
        });
        entries.push(Listed {
            name: entry.name.to_string_lossy().into_owned(),
            is_dir,
            size,
            changed,
            link,
            runs,
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

/// Work on files that Scheme starts and waits for: see `%start-file-job`.
pub enum FileJob {
    /// See [`file_system::copy`].
    Copy(PathBuf, PathBuf),
    /// See [`file_system::move_to`].
    Move(PathBuf, PathBuf),
    /// Renaming on one host, to a name that nothing has.
    Rename(PathBuf, PathBuf),
    Delete(PathBuf),
    /// See [`list_directory`].
    List(PathBuf),
    /// Finding a directory: its name, with the path made whole if it was
    /// one from the home directory there.
    Directory(PathBuf),
}

/// What a [`FileJob`] came to.
enum Outcome {
    Nothing,
    Name(PathBuf),
    Entries(Vec<Listed>),
}

impl FileJob {
    fn run(self) -> Result<Outcome, String> {
        let said = |verb: &str, path: &Path| {
            let about = format!("cannot {verb} {}", abbreviate_path(path));
            move |error: io::Error| format!("{about}: {error}")
        };
        match self {
            Self::Copy(from, to) => file_system::copy(&from, &to)
                .map(|()| Outcome::Nothing)
                .map_err(said("copy", &from)),
            Self::Move(from, to) => file_system::move_to(&from, &to)
                .map(|()| Outcome::Nothing)
                .map_err(said("move", &from)),
            Self::Rename(from, to) => {
                if file_system::symlink_metadata(&to).is_ok() {
                    return Err(format!("{} exists already", to.display()));
                }
                file_system::rename(&from, &to)
                    .map(|()| Outcome::Nothing)
                    .map_err(said("rename", &from))
            }
            Self::Delete(path) => file_system::delete(&path)
                .map(|()| Outcome::Nothing)
                .map_err(said("delete", &path)),
            Self::List(path) => list_directory(&path)
                .map(Outcome::Entries)
                .map_err(said("read", &path)),
            Self::Directory(path) => {
                // A path that does not start at the top starts at the
                // home directory there, which only the host can name.
                let whole = match file_system::split(&path) {
                    Some((_, at)) if !at.is_absolute() => {
                        file_system::canonicalize(&path).map_err(said("find", &path))?
                    }
                    _ => path,
                };
                match file_system::metadata(&whole) {
                    Ok(meta) if meta.is_dir() => Ok(Outcome::Name(whole)),
                    Ok(_) => Err(format!("{} is not a directory", whole.display())),
                    Err(error) => Err(said("read", &whole)(error)),
                }
            }
        }
    }
}

/// Something that a thread in the background has finished.
enum Done {
    Job {
        promise: u64,
        outcome: Result<Outcome, String>,
    },
    Fetched {
        promise: u64,
        outcome: Result<Fetched, String>,
    },
    Written {
        promise: u64,
        buffer: BufferId,
        /// The state of the buffer that was written.
        tick: u64,
        unformatted: Option<String>,
        /// The name of the file as the host has it, and its time there.
        outcome: Result<(PathBuf, Option<SystemTime>), String>,
    },
    Reread {
        buffer: BufferId,
        file: PathBuf,
        /// Nobody asked: the file was only looked at for changes.
        unasked: bool,
        /// `None` if the file is as it was.
        outcome: Result<Option<(Text, bool, Option<SystemTime>)>, String>,
    },
}

/// The work on files that goes on in the background: reading and writing
/// those on other hosts.
pub(crate) struct FileJobs {
    sender: mpsc::Sender<Done>,
    done: mpsc::Receiver<Done>,
    /// Buffers whose files are being read again.
    rereading: HashSet<BufferId>,
    remote_revert_due: Option<Instant>,
}

impl Default for FileJobs {
    fn default() -> Self {
        let (sender, done) = mpsc::channel();
        Self {
            sender,
            done,
            rereading: HashSet::new(),
            remote_revert_due: None,
        }
    }
}

/// Writes `text` to `path` so that a crash cannot leave a half-written file.
fn write_atomically(path: &Path, text: &Text) -> io::Result<()> {
    file_system::write_atomically(path, |out| text.write_to(out))
}

/// Writes auto-save data, creating the directory for the user only.
/// Whatever the file, its auto-save data is on this machine.
fn write_auto_save(path: &Path, text: &Text) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    if let Some(dir) = path.parent() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }
    file_system::Local.write_atomically(path, &mut |out| text.write_to(out))
}

pub fn modified_time(path: &Path) -> Option<SystemTime> {
    file_system::metadata(path).ok()?.modified
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
        self.newer_auto_save_of(self.window().buffer)
    }

    fn newer_auto_save_of(&self, id: BufferId) -> Option<PathBuf> {
        let buffer = &self.buffers[id];
        let file = buffer.file.as_deref()?;
        let path = self.auto_save_path(file)?;
        let saved = modified_time(&path)?;
        // The time that the file had when it was read or written will do
        // for one on another host, which is not asked again for this.
        let on_disk = if file_system::is_remote(file) {
            buffer.disk_mtime
        } else {
            modified_time(file)
        };
        // Timestamps are coarse, so data written just after the file can
        // carry the same time. It is removed on save, so it is still wanted.
        let current = on_disk.is_none_or(|on_disk| saved >= on_disk);
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
    /// undo history. Windows keep their place by line and column. A file
    /// on another host is read in the background, and the buffer changes
    /// when that is done.
    pub fn revert_buffer(&mut self, id: BufferId) -> CmdResult {
        let file = self.buffers[id]
            .file
            .clone()
            .ok_or_else(|| CmdError::msg("This buffer does not visit a file"))?;
        if file_system::is_remote(&file) {
            self.reread(id, file, None);
            return Ok(());
        }
        let (text, lossy) = read_text(&file).map_err(|error| {
            CmdError::msg(format!("Cannot read {}: {error}", abbreviate_path(&file)))
        })?;
        let modified = modified_time(&file);
        self.reverted(id, text, lossy, modified);
        Ok(())
    }

    /// Makes a buffer hold what was read from its file just now.
    fn reverted(&mut self, id: BufferId, text: Text, lossy: bool, modified: Option<SystemTime>) {
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
        buffer.disk_mtime = modified;
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
    }

    /// Runs `work` on a thread of its own, where it may wait for a host
    /// as long as a slow line takes, and takes in what it comes to on the
    /// next [`Editor::poll`].
    fn in_background(&self, work: impl FnOnce() -> Done + Send + 'static) {
        let (sender, waker) = (self.file_jobs.sender.clone(), self.waker.clone());
        std::thread::spawn(move || {
            if sender.send(file_system::patiently(work)).is_ok() {
                waker();
            }
        });
    }

    /// Reads a buffer's file again in the background. With the time that
    /// the file is known to have, only if it has another now, and nothing
    /// is said if that cannot be found out.
    fn reread(&mut self, buffer: BufferId, file: PathBuf, known: Option<Option<SystemTime>>) {
        if !self.file_jobs.rereading.insert(buffer) {
            return;
        }
        let unasked = known.is_some();
        self.in_background(move || {
            let now = file_system::metadata(&file).map(|meta| meta.modified);
            let changed = match (&now, known) {
                (_, None) => true,
                (Ok(now), Some(known)) => now.is_some() && *now != known,
                (Err(_), Some(_)) => false,
            };
            let outcome = if changed {
                read_text(&file)
                    .map(|(text, lossy)| Some((text, lossy, now.ok().flatten())))
                    .map_err(|error| format!("Cannot read {}: {error}", abbreviate_path(&file)))
            } else {
                Ok(None)
            };
            Done::Reread {
                buffer,
                file,
                unasked,
                outcome,
            }
        });
    }

    /// Starts visiting a file in the background, as files on other hosts
    /// are visited: returns a promise that resolves, once the file has
    /// been read into a buffer, to a list that starts with `buffer`,
    /// `directory`, `large` or `error`. See `%visit-remote` in the Scheme
    /// that is part of the editor, which shows what was found. What is
    /// typed meanwhile waits for the file if a command asked for it, or
    /// with `hold`, when a person did at a prompt.
    pub fn fetch_file_promise(&mut self, path: PathBuf, anyway: bool, hold: bool) -> u64 {
        let promise = self.new_promise();
        if hold {
            self.hold_input_for(promise, "find-file");
        } else {
            self.await_program(promise);
        }
        let limit = self.large_file_limit();
        self.in_background(move || Done::Fetched {
            promise,
            outcome: fetch(&path, anyway, limit),
        });
        promise
    }

    /// Starts writing the current buffer to a file in the background, as
    /// files on other hosts are written, and makes that its file when it
    /// is done: returns a promise that resolves to `#t`, or to a string
    /// that says what went wrong. The hook that runs before saving is not
    /// run.
    pub fn send_buffer_promise(&mut self, path: PathBuf) -> u64 {
        let promise = self.new_promise();
        self.await_program(promise);
        let unformatted = self.format_before_save();
        // What a formatter or a hook changed just now is part of what is
        // written.
        let point = self.point();
        self.buffer_mut().commit_edits(point);
        let (buffer, tick, text) = (
            self.window().buffer,
            self.buffer().tick(),
            self.text().clone(),
        );
        self.in_background(move || {
            let outcome = write_atomically(&path, &text)
                .map(|()| {
                    let path = file_system::canonicalize(&path).unwrap_or(path.clone());
                    let modified = modified_time(&path);
                    (path, modified)
                })
                .map_err(|error| format!("Cannot write {}: {error}", abbreviate_path(&path)));
            Done::Written {
                promise,
                buffer,
                tick,
                unformatted,
                outcome,
            }
        });
        promise
    }

    /// Starts work on files in the background, as what is on other hosts
    /// needs: returns a promise that resolves to a list of `#t` and what
    /// the work gives, if anything, or of `#f` and what went wrong. What
    /// is typed meanwhile waits, if a command asked for the work.
    pub fn file_job_promise(&mut self, job: FileJob) -> u64 {
        let promise = self.new_promise();
        self.await_program(promise);
        self.in_background(move || Done::Job {
            promise,
            outcome: job.run(),
        });
        promise
    }

    /// Takes in what the threads in the background have finished.
    pub(crate) fn file_jobs_poll(&mut self) {
        while let Ok(done) = self.file_jobs.done.try_recv() {
            match done {
                Done::Job { promise, outcome } => {
                    let result = match outcome {
                        Ok(Outcome::Nothing) => vec![true.into()],
                        Ok(Outcome::Name(path)) => {
                            vec![true.into(), path.display().to_string().into()]
                        }
                        Ok(Outcome::Entries(entries)) => vec![
                            true.into(),
                            Value::List(entries.into_iter().map(Value::from).collect()),
                        ],
                        Err(problem) => vec![false.into(), problem.into()],
                    };
                    self.resolve_promise(promise, Value::List(result));
                }
                Done::Fetched { promise, outcome } => {
                    let found = match outcome {
                        Ok(Fetched::Directory(path)) => {
                            vec![Value::sym("directory"), path.display().to_string().into()]
                        }
                        Ok(Fetched::Large(path, size)) => vec![
                            Value::sym("large"),
                            format!("{} is {}", abbreviate_path(&path), format_size(size)).into(),
                        ],
                        Ok(file @ Fetched::File { .. }) => {
                            let (id, note) = self.adopt(file);
                            let recover = self.newer_auto_save_of(id).map(|_| {
                                format!(
                                    "{} has auto-save data; use M-x recover-this-file",
                                    self.buffers[id].name
                                )
                            });
                            vec![
                                Value::sym("buffer"),
                                self.buffers[id].name.as_str().into(),
                                recover.or(note.map(str::to_owned)).into(),
                            ]
                        }
                        Err(problem) => vec![Value::sym("error"), problem.into()],
                    };
                    self.resolve_promise(promise, Value::List(found));
                }
                Done::Written {
                    promise,
                    buffer,
                    tick,
                    unformatted,
                    outcome,
                } => {
                    let result = match outcome {
                        Ok((path, modified)) => {
                            if self.buffers.contains_key(buffer) {
                                self.written(buffer, path, modified, Some(tick), unformatted);
                            }
                            Value::Bool(true)
                        }
                        Err(problem) => problem.into(),
                    };
                    self.resolve_promise(promise, result);
                }
                Done::Reread {
                    buffer,
                    file,
                    unasked,
                    outcome,
                } => {
                    self.file_jobs.rereading.remove(&buffer);
                    let still = self.buffers.get(buffer).is_some_and(|buffer| {
                        buffer.file.as_deref() == Some(&file) && !(unasked && buffer.is_modified())
                    });
                    match outcome {
                        Ok(Some((text, lossy, modified))) if still => {
                            self.reverted(buffer, text, lossy, modified);
                            let name = &self.buffers[buffer].name;
                            self.message(if unasked {
                                format!("Reverted {name}: its file changed on disk")
                            } else {
                                format!("Reverted {name}")
                            });
                        }
                        Err(problem) if !unasked => self.message(problem),
                        _ => {}
                    }
                }
            }
        }
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
                let Some(file) = buffer
                    .file
                    .as_deref()
                    .filter(|file| !file_system::is_remote(file))
                else {
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
        // Files on other hosts are asked about less often, in the
        // background, and only over a connection that is there: a host
        // that was left is not called again for this.
        let now = Instant::now();
        if self
            .file_jobs
            .remote_revert_due
            .is_some_and(|due| due > now)
        {
            return;
        }
        self.file_jobs.remote_revert_due = Some(now + REMOTE_REVERT_INTERVAL);
        let remote: Vec<(BufferId, PathBuf, Option<SystemTime>)> = self
            .buffers
            .iter()
            .filter(|(_, buffer)| !buffer.is_modified())
            .filter_map(|(id, buffer)| {
                let file = buffer.file.as_ref()?;
                file_system::is_connected(file).then(|| (id, file.clone(), buffer.disk_mtime))
            })
            .collect();
        for (id, file, known) in remote {
            self.reread(id, file, Some(known));
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

    /// The working directory, for running a program in: an error if it
    /// is on another host, where programs are not run yet.
    pub fn local_directory(&self) -> CmdResult<PathBuf> {
        let directory = self.working_directory();
        if file_system::is_remote(directory) {
            return Err(CmdError::msg(format!(
                "Programs are not run on other hosts yet, and this buffer is in {}",
                directory.display()
            )));
        }
        Ok(directory.to_owned())
    }

    /// Shows `path` in the selected window, reading it if no buffer visits it
    /// yet. A path that does not exist gives an empty buffer for a new file.
    pub fn find_file(&mut self, path: &Path) -> CmdResult {
        self.find_file_with(path, false)
    }

    /// Visits a file as [`Editor::find_file`] does, also if it is so large
    /// that reading it takes a while.
    pub fn find_file_anyway(&mut self, path: &Path) -> CmdResult {
        self.find_file_with(path, true)
    }

    /// Visits a file for a person who can be asked: about a large one, a
    /// question comes first.
    pub fn find_file_asking(&mut self, path: &Path) -> CmdResult {
        // Of a file on another host, what it is comes with reading it,
        // and the question after that.
        if file_system::is_remote(path) {
            return self.visit_remote(path, false, true);
        }
        let resolved = file_system::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        if self.buffer_visiting(&resolved).is_none()
            && let FileKind::Large(size) = self.file_kind(&resolved)
        {
            let name = abbreviate_path(&resolved);
            self.ask_yes_no(
                format!("{name} is {}; open it anyway?", format_size(size)),
                move |ed| ed.find_file_anyway(&resolved),
            );
            return Ok(());
        }
        self.find_file(path)
    }

    /// What a file is, as far as reading it into a buffer goes. A file
    /// that cannot be looked at counts as text: reading it says why.
    pub fn file_kind(&self, path: &Path) -> FileKind {
        let meta = file_system::metadata(path).ok();
        kind_of(path, meta.as_ref(), self.large_file_limit())
    }

    /// The size in bytes above which text is read only when asked to.
    fn large_file_limit(&self) -> Option<u64> {
        match self.option(LARGE_FILE_THRESHOLD) {
            Ok(Value::Int(megabytes)) => Some((megabytes.max(0) as u64) << 20),
            _ => None,
        }
    }

    fn find_file_with(&mut self, path: &Path, anyway: bool) -> CmdResult {
        // A file on another host is read in the background, and shown by
        // Scheme when it is there.
        if file_system::is_remote(path) {
            return self.visit_remote(path, anyway, false);
        }
        // Directories are shown by the file manager, which is Scheme.
        if file_system::is_dir(path) && self.has_script_host() {
            let path = file_system::canonicalize(path).unwrap_or_else(|_| path.to_owned());
            self.queue_named(
                "%visit-directory",
                vec![Value::Str(path.display().to_string())],
            );
            return Ok(());
        }
        let (id, note) = self.visit_file(path, anyway)?;
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

    /// Shows a file on another host: at once if a buffer visits it under
    /// this name, and else when Scheme has had it read in the background.
    /// With `ask`, a large file is asked about; without, it is an error
    /// unless `anyway`.
    fn visit_remote(&mut self, path: &Path, anyway: bool, ask: bool) -> CmdResult {
        if let Some(id) = self.buffer_visiting(path) {
            self.switch_to_buffer(id);
            return Ok(());
        }
        if !self.has_script_host() {
            return Err(CmdError::msg(
                "Files on other hosts are not visited without Scheme",
            ));
        }
        let name = path.display().to_string();
        self.queue_named(
            "%visit-remote",
            vec![name.into(), anyway.into(), ask.into()],
        );
        Ok(())
    }

    /// The buffer that visits `path`, reading the file if no buffer does
    /// yet, without showing it. Also returns something to tell the user
    /// about a file just read, if there is anything. The hooks that choose
    /// the buffer's mode run when it is first shown.
    ///
    /// A file that is not text is not read, and one above the size of the
    /// `large-file-threshold` option only if `anyway`: reading happens in
    /// one go and would hold the editor up, for nothing in the first case.
    pub(crate) fn visit_file(
        &mut self,
        path: &Path,
        anyway: bool,
    ) -> CmdResult<(BufferId, Option<&'static str>)> {
        if file_system::is_remote(path) {
            return Err(CmdError::msg(format!(
                "{} is on another host, and is visited with find-file",
                path.display()
            )));
        }
        let path = file_system::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        if let Some(id) = self.buffer_visiting(&path) {
            return Ok((id, None));
        }
        match fetch(&path, anyway, self.large_file_limit()).map_err(CmdError::msg)? {
            Fetched::Directory(path) => Err(CmdError::msg(format!(
                "{} is a directory",
                abbreviate_path(&path)
            ))),
            Fetched::Large(path, size) => Err(CmdError::msg(format!(
                "{} is {}, above large-file-threshold",
                abbreviate_path(&path),
                format_size(size)
            ))),
            file @ Fetched::File { .. } => Ok(self.adopt(file)),
        }
    }

    /// Makes a buffer of a file that was read, unless one visits it
    /// already, without showing it. Also returns something to tell the
    /// user about the file, if there is anything.
    fn adopt(&mut self, file: Fetched) -> (BufferId, Option<&'static str>) {
        let Fetched::File {
            path,
            text,
            lossy,
            is_new,
            modified,
        } = file
        else {
            unreachable!("only a file that was read becomes a buffer");
        };
        if let Some(id) = self.buffer_visiting(&path) {
            return (id, None);
        }
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let mut buffer = Buffer::with_text(name, text);
        buffer.disk_mtime = modified;
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
        (id, note)
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
        // So does writing to another host, which takes a while.
        let remote = file_system::is_remote(path);
        if remote && !self.has_script_host() {
            return Err(CmdError::msg(
                "Files on other hosts are not written without Scheme",
            ));
        }
        if (remote || self.hook_active("before-save-hook")) && self.has_script_host() {
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
    /// before saving. A file on another host is not written this way but
    /// with [`Editor::send_buffer_promise`].
    pub fn write_buffer_now(&mut self, path: &Path) -> CmdResult {
        if file_system::is_remote(path) {
            return Err(CmdError::msg(format!(
                "{} is on another host, and is written in the background by save-buffer and write-file",
                path.display()
            )));
        }
        let unformatted = self.format_before_save();
        write_atomically(path, self.text()).map_err(|error| {
            CmdError::msg(format!("Cannot write {}: {error}", abbreviate_path(path)))
        })?;
        let path = file_system::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        let modified = modified_time(&path);
        let id = self.window().buffer;
        self.written(id, path, modified, None, unformatted);
        Ok(())
    }

    /// Notes that a buffer has been written to a file, which is its file
    /// from now on. With the state of the buffer that was written, if
    /// that was a while ago: a buffer that has changed since is not saved.
    fn written(
        &mut self,
        id: BufferId,
        path: PathBuf,
        modified: Option<SystemTime>,
        tick: Option<u64>,
        unformatted: Option<String>,
    ) {
        let shown = abbreviate_path(&path);
        let renamed = self.buffers[id].file.as_deref() != Some(&path);
        if renamed {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| shown.clone());
            // Clear the old name first so that it does not count as taken.
            self.buffers[id].name.clear();
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
            self.buffers[id].name = unique;
        }
        self.remove_auto_save(&path);
        let point = self.point();
        let buffer = &mut self.buffers[id];
        let current = tick.is_none_or(|tick| tick == buffer.tick());
        if tick.is_none() {
            // What a formatter or a hook changed just now is part of what
            // was written.
            buffer.commit_edits(point);
        }
        buffer.disk_mtime = modified;
        buffer.file = Some(path);
        if current {
            buffer.auto_saved_tick = None;
            buffer.mark_saved();
        }
        match (unformatted, current) {
            (Some(problem), _) => self.message(format!("Wrote {shown}, not formatted: {problem}")),
            (None, true) => self.message(format!("Wrote {shown}")),
            (None, false) => self.message(format!(
                "Wrote {shown} as it was; the buffer has changed since"
            )),
        }
        self.lsp_did_save(id);
        // The hook runs later, when another buffer may be current.
        if self.hook_active("after-save-hook") {
            let name = self.buffers[id].name.as_str().into();
            self.queue_named("%run-hook-in", vec![name, Value::sym("after-save-hook")]);
        }
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

    /// Lets the threads in the background finish what `done` waits for.
    fn settle(ed: &mut Editor, done: impl Fn(&Editor) -> bool) {
        let start = Instant::now();
        while !done(ed) {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "nothing happened"
            );
            std::thread::sleep(Duration::from_millis(2));
            ed.file_jobs_poll();
        }
    }

    #[test]
    fn a_remote_file_that_another_program_changed_is_read_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = fs::canonicalize(dir.path()).unwrap().join("shared.txt");
        fs::write(&path, "first").unwrap();
        let name = PathBuf::from(format!("/sftp-test::{}", path.display()));
        let change = |text: &str, seconds: u64| {
            fs::write(&path, text).unwrap();
            let file = fs::File::options().write(true).open(&path).unwrap();
            file.set_modified(SystemTime::now() + Duration::from_secs(seconds))
                .unwrap();
        };

        let mut ed = Editor::new(80, 10);
        let (id, _) = ed.adopt(fetch(&name, false, None).unwrap());
        let known = ed.buffers[id].disk_mtime;
        assert!(known.is_some());

        // Nothing has changed: nothing is read.
        ed.reread(id, name.clone(), Some(known));
        settle(&mut ed, |ed| ed.file_jobs.rereading.is_empty());
        assert_eq!(ed.buffers[id].text().to_string(), "first");
        assert_eq!(ed.buffers[id].disk_mtime, known);

        change("second", 5);
        ed.reread(id, name.clone(), Some(known));
        settle(&mut ed, |ed| ed.file_jobs.rereading.is_empty());
        assert_eq!(ed.buffers[id].text().to_string(), "second");
        assert_ne!(ed.buffers[id].disk_mtime, known);
        assert!(!ed.buffers[id].is_modified());

        // A buffer with changes of its own keeps them.
        let known = ed.buffers[id].disk_mtime;
        change("third", 10);
        ed.reread(id, name.clone(), Some(known));
        ed.buffers[id].insert(0, "mine ").unwrap();
        settle(&mut ed, |ed| ed.file_jobs.rereading.is_empty());
        assert_eq!(ed.buffers[id].text().to_string(), "mine second");

        // A file that is gone, or a host, is no reason to say anything.
        fs::remove_file(&path).unwrap();
        ed.reread(id, name, Some(known));
        settle(&mut ed, |ed| ed.file_jobs.rereading.is_empty());
        assert_eq!(ed.buffers[id].text().to_string(), "mine second");
    }

    #[test]
    fn what_is_typed_over_is_dropped() {
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME is set"));
        let base = Path::new("/base");
        let typed = |path| expand_typed_path(path, base);
        assert_eq!(typed("sub/file"), Path::new("/base/sub/file"));
        assert_eq!(typed("~/src//etc/hosts"), Path::new("/etc/hosts"));
        assert_eq!(typed("~/src//ssh:evo:/etc"), Path::new("/ssh:evo:/etc"));
        assert_eq!(typed("/a//b//c"), Path::new("/c"));
        assert_eq!(typed("/etc/~/notes"), home.join("notes"));
        assert_eq!(typed("/ssh:evo:/etc/~"), home);
        assert_eq!(typed("/tmp/~x/y"), Path::new("/tmp/~x/y"));
        // The home directory of the host is not that of this machine.
        assert_eq!(typed("/ssh:evo:~/src"), Path::new("/ssh:evo:~/src"));
        assert_eq!(typed("/a//~/b"), home.join("b"));
        assert_eq!(typed("/a/~//b"), Path::new("/b"));
    }
}
