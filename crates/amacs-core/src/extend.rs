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
use crate::editor::Editor;
use crate::env::EnvState;
use crate::error::{CmdError, CmdResult};
use crate::frame::FrameId;
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

    /// The windows that show a buffer, in every frame, those of the
    /// current frame first. What happens to a buffer on its own, as when a
    /// program writes to it, happens in whatever frame is current, which
    /// need not be one that shows the buffer.
    pub fn buffer_windows(&self, buffer: BufferId) -> Vec<WindowId> {
        let current = self.current_frame();
        let mut frames: Vec<FrameId> = self.frames.keys().collect();
        frames.sort_by_key(|frame| *frame != current);
        frames
            .into_iter()
            .flat_map(|frame| self.frames[frame].layout.leaves())
            .filter(|window| self.windows[*window].buffer == buffer)
            .collect()
    }

    /// Whether a window is still there, in any frame.
    pub fn window_live(&self, id: WindowId) -> bool {
        self.windows.contains_key(id)
    }

    /// The frame of a window. The minibuffer window is a real window of its
    /// frame but not a leaf of its layout; it is selected while a prompt is
    /// open, and `with-selected-window` must be able to restore it.
    fn window_frame(&self, id: WindowId) -> CmdResult<FrameId> {
        self.frames
            .iter()
            .find(|(_, frame)| frame.minibuffer_window == id || frame.layout.leaves().contains(&id))
            .map(|(frame, _)| frame)
            .ok_or_else(|| CmdError::msg("That window is no longer there"))
    }

    /// The selected window of the frame that a window is in.
    pub fn frame_selected_window(&self, id: WindowId) -> CmdResult<WindowId> {
        Ok(self.frames[self.window_frame(id)?].selected)
    }

    /// Selects a window. A window of another frame makes that frame the
    /// current one for the rest of the script, or until a window of the
    /// first frame is selected again: code that runs for a program, not
    /// for a key, reaches the windows of every frame this way.
    pub fn select_window(&mut self, id: WindowId) -> CmdResult {
        let frame = self.window_frame(id)?;
        if frame != self.current_frame() {
            if self.script.restore_frame.is_none() {
                self.script.restore_frame = Some(self.current_frame());
            }
            self.select_frame(frame);
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

    /// Hides lines of a buffer, as [`Buffer::hide_lines`] has it. Point in
    /// a window that shows the buffer moves out of what is hidden, to the
    /// end of the line before it: else the lines would be shown again at
    /// once, since a command that leaves point in hidden lines opens them.
    pub fn hide_lines(&mut self, buffer: BufferId, range: std::ops::Range<usize>) {
        if self.buffers[buffer].hide_lines(range).is_none() {
            return;
        }
        let points: Vec<_> = self
            .windows
            .values()
            .filter(|window| window.buffer == buffer)
            .map(|window| window.point)
            .collect();
        let buffer = &mut self.buffers[buffer];
        for point in points {
            let shown = buffer.folds().visible(buffer.marker(point));
            buffer.set_marker(point, shown);
        }
    }

    /// Shows the hidden lines that point is on, in every window: the folds
    /// around point are opened and those beside it stay. This is what
    /// makes a jump into hidden lines, by a search or to an error, land on
    /// text that is seen.
    pub(crate) fn settle_points(&mut self) {
        let hidden: Vec<_> = self
            .windows
            .values()
            .filter_map(|window| {
                let buffer = &self.buffers[window.buffer];
                let point = buffer.marker(window.point);
                buffer
                    .folds()
                    .hidden(point)
                    .then_some((window.buffer, point))
            })
            .collect();
        for (buffer, point) in hidden {
            self.buffers[buffer].reveal(point);
        }
    }

    /// How the text of a window's buffer flows into the rows of that
    /// window: as [`Buffer::flow`] has it, with nothing left out of the
    /// line that the window's point is on.
    pub(crate) fn window_flow(&self, id: WindowId) -> crate::display::Flow<'_> {
        let window = &self.windows[id];
        let buffer = &self.buffers[window.buffer];
        let mut flow = buffer.flow();
        if !flow.headings.is_empty() {
            let step = self.option_for(window.buffer, crate::display::ORG_INDENT_STEP);
            if let Some(crate::script::Value::Int(step)) = step {
                flow.indent_step = usize::try_from(step).unwrap_or(0);
            }
        }
        if !flow.hidden.is_empty() {
            let text = buffer.text();
            let line = text.line_of(buffer.marker(window.point));
            flow.reveal = (text.line_start(line), text.line_end(line) + 1);
        }
        flow
    }

    /// The chars a window shows: from its start to the end of its last row.
    pub fn visible_range(&self, id: WindowId) -> std::ops::Range<usize> {
        let window = &self.windows[id];
        let buffer = &self.buffers[window.buffer];
        let width = self.text_width(id);
        let start = buffer.marker(window.start);
        let shown = crate::display::shown_rows(
            buffer.text(),
            (start, window.skip),
            window.text_rows(),
            width,
            self.window_flow(id),
        );
        match shown.last().map(|shown| shown.row.next) {
            Some(Some(next)) => start..next,
            Some(None) => start..buffer.len(),
            None => start..start,
        }
    }

    /// Makes another buffer the one that the selected window shows, without
    /// what goes with the user switching to it: no hooks, no change to the
    /// order of buffers. For code that works in a buffer for a moment.
    pub fn set_current_buffer(&mut self, buffer: BufferId) {
        self.set_window_buffer(self.selected(), buffer);
    }

    /// The buffer of the current frame's minibuffer, if that is its name.
    /// It is in no list of buffers, and code that works in another buffer
    /// for a moment while a prompt is open has to come back to it.
    pub fn minibuffer_named(&self, name: &str) -> Option<BufferId> {
        let buffer = self.frame().minibuffer_buffer;
        (self.buffers[buffer].name == name).then_some(buffer)
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

    /// Starts a program and leaves it to itself: nothing is written to it,
    /// what it writes is thrown away, and when it ends is not known. For
    /// programs that show something of their own, such as a viewer. It runs
    /// in the working directory with the environment of its project, in a
    /// process group of its own, so that what is done to the editor's is
    /// not done to it.
    pub fn start_process(&mut self, program: &str, args: &[String]) -> CmdResult {
        use std::os::unix::process::CommandExt;
        let directory = self.local_directory()?;
        let environment = match self.environment(&directory) {
            Some(EnvState::Ready(environment)) => environment,
            _ => Vec::new(),
        };
        let mut command = Command::new(program);
        command.args(args).process_group(0);
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
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = command
            .spawn()
            .map_err(|error| CmdError::msg(format!("cannot run {program}: {error}")))?;
        // Someone has to wait for it, or it stays as a zombie when it ends.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }

    /// Runs a program without waiting for it: returns a promise that
    /// resolves to a list of its exit status, what it wrote to standard
    /// output, and what it wrote to standard error. The status is `#f` if
    /// the program could not be run or was stopped by a signal. The program
    /// runs in the working directory with the environment of its project.
    pub fn process_promise(
        &mut self,
        program: &str,
        args: &[String],
        input: String,
        variables: &[(String, String)],
    ) -> u64 {
        let id = self.new_promise();
        self.await_program(id);
        let directory = match self.local_directory() {
            Ok(directory) => directory,
            // A program that could not be run, to those who wait for it.
            Err(error) => {
                let _ = self.extend.sender.send(Finished {
                    promise: id,
                    status: None,
                    stdout: String::new(),
                    stderr: error.to_string(),
                });
                (self.waker)();
                return id;
            }
        };
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
        // What the caller asks for comes over what the project has.
        for (name, value) in variables {
            command.env(name, value);
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
