//! What the core offers to the Scheme code that extends it, beyond the
//! commands: waiting for a key, for a time or for a program, windows by
//! number, and labels over the text.
//!
//! Scheme never blocks the editor. What takes time gives a promise, which
//! the Scheme side waits for with `await`.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use amacs_proto::Key;
use slotmap::{Key as _, KeyData};

use crate::buffer::BufferId;
use crate::decorate::{Label, Labels};
use crate::display::walk_row;
use crate::editor::Editor;
use crate::env::EnvState;
use crate::error::{CmdError, CmdResult};
use crate::script::Value;
use crate::window::WindowId;

/// What a program that was run for Scheme code came to.
struct Finished {
    promise: u64,
    /// The exit status, or `None` if the program could not be run or was
    /// stopped by a signal.
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

pub(crate) struct ExtendState {
    /// When to resolve which promise.
    timers: Vec<(Instant, u64)>,
    sender: mpsc::Sender<Finished>,
    finished: mpsc::Receiver<Finished>,
}

impl Default for ExtendState {
    fn default() -> Self {
        let (sender, finished) = mpsc::channel();
        Self {
            timers: Vec::new(),
            sender,
            finished,
        }
    }
}

/// A window as a number that Scheme code can hold.
pub fn window_number(id: WindowId) -> i64 {
    id.data().as_ffi() as i64
}

impl Editor {
    /// The window that a number from [`window_number`] stands for, if it
    /// still exists.
    pub fn window_by_number(&self, number: i64) -> CmdResult<WindowId> {
        let id = WindowId::from(KeyData::from_ffi(number as u64));
        if self.windows.contains_key(id) {
            Ok(id)
        } else {
            Err(CmdError::msg(format!("There is no window {number}")))
        }
    }

    /// The windows of the current frame that show buffers, the selected
    /// one first.
    pub fn window_list(&self) -> Vec<WindowId> {
        let selected = self.selected();
        let mut windows = self.frame().layout.leaves();
        windows.sort_by_key(|id| *id != selected);
        windows
    }

    pub fn selected_window(&self) -> WindowId {
        self.selected()
    }

    /// Selects a window of the current frame.
    pub fn select_window(&mut self, id: WindowId) -> CmdResult {
        if !self.frame().layout.leaves().contains(&id) {
            return Err(CmdError::msg("That window is not in this frame"));
        }
        self.frame_mut().selected = id;
        Ok(())
    }

    /// Name of the buffer a window shows, and its point.
    pub fn window_state(&self, id: WindowId) -> (&str, usize) {
        let window = &self.windows[id];
        let buffer = &self.buffers[window.buffer];
        (buffer.name.as_str(), buffer.marker(window.point))
    }

    /// The chars a window shows: from its start to the end of its last row.
    pub fn visible_range(&self, id: WindowId) -> std::ops::Range<usize> {
        let window = &self.windows[id];
        let buffer = &self.buffers[window.buffer];
        let width = self.text_width(id);
        let start = buffer.marker(window.start);
        let mut pos = start;
        for _ in 0..window.text_rows() {
            let row = walk_row(buffer.text(), pos, width, buffer.tab_width, |_, _, _, _| {});
            match row.next {
                Some(next) => pos = next,
                None => return start..buffer.len(),
            }
        }
        start..pos
    }

    /// Makes another buffer the one that the selected window shows, without
    /// what goes with the user switching to it: no hooks, no change to the
    /// order of buffers. For code that works in a buffer for a moment.
    pub fn set_current_buffer(&mut self, buffer: BufferId) {
        self.set_window_buffer(self.selected(), buffer);
    }

    /// Shows lines under the echo area until the next key, as the keys
    /// that can follow a prefix are shown.
    pub fn show_hint(&mut self, lines: Vec<String>) {
        self.frame_mut().hint = lines;
    }

    // Labels

    /// Shows key sequences over places in windows: a position in the
    /// window's buffer, or `None` for the window itself. `dim` fades the
    /// text under them.
    pub fn show_labels(&mut self, labels: Vec<(WindowId, Option<usize>, String)>, dim: bool) {
        let entries = labels
            .into_iter()
            .map(|(window, pos, keys)| Label { window, pos, keys })
            .collect();
        self.labels = Some(Labels { entries, dim });
    }

    pub fn hide_labels(&mut self) {
        self.labels = None;
    }

    // Promises

    /// Waits for the next key for Scheme code: returns a promise that
    /// resolves to the key as text, such as `"a"` or `"C-x"`. `C-g` rejects
    /// it, which abandons the code that waits.
    pub fn read_key_promise(&mut self, prompt: impl Into<String>) -> u64 {
        let id = self.new_promise();
        self.ask_key(prompt, move |ed, key| {
            if key == Key::ctrl('g') {
                ed.hide_labels();
                ed.reject_promise(id);
                return Err(CmdError::Quit);
            }
            ed.resolve_promise(id, Value::Str(key.to_string()));
            Ok(())
        });
        if let Some(question) = &mut self.key_prompt {
            question.promise = Some(id);
        }
        id
    }

    /// A promise that resolves after some milliseconds.
    pub fn timer_promise(&mut self, milliseconds: u64) -> u64 {
        let id = self.new_promise();
        let due = Instant::now() + Duration::from_millis(milliseconds);
        self.extend.timers.push((due, id));
        id
    }

    /// Runs a program without waiting for it: returns a promise that
    /// resolves to a list of its exit status, what it wrote to standard
    /// output, and what it wrote to standard error. The status is `#f` if
    /// the program could not be run or was stopped by a signal. The program
    /// runs in the working directory with the environment of its project.
    pub fn process_promise(&mut self, program: &str, args: &[String], input: String) -> u64 {
        let id = self.new_promise();
        let directory = self.working_directory().to_owned();
        let environment = match self.environment(&directory) {
            Some(EnvState::Ready(environment)) => environment,
            _ => Vec::new(),
        };
        let mut command = Command::new(program);
        command.args(args);
        if directory.is_dir() {
            command.current_dir(&directory);
        }
        for (name, value) in &environment {
            match value {
                Some(value) => command.env(name, value),
                None => command.env_remove(name),
            };
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let (sender, waker) = (self.extend.sender.clone(), self.waker.clone());
        let program = program.to_owned();
        std::thread::spawn(move || {
            let outcome = command.spawn().and_then(|mut child| {
                if let Some(mut stdin) = child.stdin.take() {
                    // Writing and reading happen at once, or a program that
                    // writes while it reads would block on a full pipe.
                    std::thread::spawn(move || {
                        let _ = stdin.write_all(input.as_bytes());
                    });
                }
                child.wait_with_output()
            });
            let finished = match outcome {
                Ok(output) => Finished {
                    promise: id,
                    status: output.status.code(),
                    stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                },
                Err(error) => Finished {
                    promise: id,
                    status: None,
                    stdout: String::new(),
                    stderr: format!("cannot run {program}: {error}"),
                },
            };
            if sender.send(finished).is_ok() {
                waker();
            }
        });
        id
    }

    /// Resolves the promises whose time has come or whose program is done.
    pub(crate) fn extend_poll(&mut self, now: Instant) {
        let due: Vec<u64> = self
            .extend
            .timers
            .iter()
            .filter(|(at, _)| *at <= now)
            .map(|(_, id)| *id)
            .collect();
        self.extend.timers.retain(|(at, _)| *at > now);
        for id in due {
            self.resolve_promise(id, Value::Void);
        }
        while let Ok(finished) = self.extend.finished.try_recv() {
            let status = finished.status.map(i64::from);
            self.resolve_promise(
                finished.promise,
                Value::List(vec![
                    status.into(),
                    finished.stdout.into(),
                    finished.stderr.into(),
                ]),
            );
        }
    }

    pub(crate) fn extend_deadline(&self) -> Option<Instant> {
        self.extend.timers.iter().map(|(at, _)| *at).min()
    }
}
