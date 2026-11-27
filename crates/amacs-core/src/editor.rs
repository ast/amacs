use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use amacs_proto::{InputEvent, Key, KeyCode, Mods, NamedKey};
use amacs_text::{MarkerId, Text};
use slotmap::SlotMap;

use crate::buffer::{Buffer, BufferId};
use crate::commands;
use crate::complete::CompletionState;
use crate::completion::Waker;
use crate::decorate::Labels;
use crate::env::{EnvState, Environments};
use crate::error::{CmdError, CmdResult};
use crate::extend::ExtendState;
use crate::faces::Faces;
use crate::format::Formatter;
use crate::frame::{Frame, FrameId};
use crate::history::History;
use crate::keymap::{Keymap, Lookup, lookup_in};
use crate::kill_ring::KillRing;
use crate::lsp::LspState;
use crate::minibuffer::MinibufferState;
use crate::mouse::Mouse;
use crate::process::ProcessState;
use crate::script::{ScriptCall, ScriptFn, ScriptState, ScriptTarget, Value};
use crate::snippet::ActiveSnippet;
use crate::undo_view::UndoView;
use crate::window::{Rect, Window, WindowId};

/// The input in progress in a frame that is not the current one.
#[derive(Default)]
pub(crate) struct InputState {
    pending_keys: Vec<Key>,
    esc_pending: bool,
    prefix_arg: Option<Prefix>,
    minibuffer: Option<MinibufferState>,
    minibuffer_hint: Option<String>,
    key_prompt: Option<KeyPrompt>,
    labels: Option<Labels>,
    key_hint_due: Option<Instant>,
}

/// What running a command does.
#[derive(Clone)]
pub enum CommandBody {
    Native(fn(&mut Editor) -> CmdResult),
    /// A Scheme procedure of no arguments, run by the script host.
    Script(ScriptFn),
}

/// An interactive command, run by key binding or by name with `M-x`.
#[derive(Clone)]
pub struct Command {
    pub name: String,
    pub doc: String,
    pub body: CommandBody,
}

pub const GLOBAL_MAP: &str = "global";
pub const MINIBUFFER_MAP: &str = "minibuffer";

/// The prefix argument given with `C-u` or `M-<digit>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prefix {
    /// `C-u` pressed one or more times: 4, 16, 64, ...
    Universal(i64),
    Number(i64),
    /// `C-u -` with no digits yet.
    Negative,
}

impl Prefix {
    pub fn value(self) -> i64 {
        match self {
            Prefix::Universal(n) | Prefix::Number(n) => n,
            Prefix::Negative => -1,
        }
    }

    pub(crate) fn push_digit(self, digit: i64) -> Prefix {
        match self {
            Prefix::Universal(_) => Prefix::Number(digit),
            Prefix::Negative => Prefix::Number(-digit),
            Prefix::Number(n) if n < 0 => {
                Prefix::Number(n.saturating_mul(10).saturating_sub(digit))
            }
            Prefix::Number(n) => Prefix::Number(n.saturating_mul(10).saturating_add(digit)),
        }
    }
}

type KeyAnswer = Box<dyn FnOnce(&mut Editor, Key) -> CmdResult>;

/// What a key sequence does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyMeaning {
    Command(String),
    /// It is the start of longer sequences.
    Prefix,
    Unbound,
}

/// How often files are checked for changes made by other programs.
const AUTO_REVERT_INTERVAL: Duration = Duration::from_secs(5);

/// A question waiting for a single key.
pub(crate) struct KeyPrompt {
    pub prompt: String,
    answer: KeyAnswer,
    /// Promise to reject if the question is abandoned, when Scheme waits.
    pub(crate) promise: Option<u64>,
}

/// The system clipboard, as far as the kill ring is concerned. Kills are
/// copied to it, and text that another program put there can be yanked.
pub trait Clipboard {
    fn set(&mut self, text: &str);
    /// The clipboard's text, or `None` if it has none or cannot be read.
    fn get(&mut self) -> Option<String>;
    /// Puts a secret, such as a password, on the clipboard, with the hint
    /// to programs that keep what was on it that this is not to be kept,
    /// if the clipboard can give one.
    fn set_secret(&mut self, text: &str) {
        self.set(text);
    }
    /// Makes the clipboard hold nothing.
    fn clear(&mut self) {
        self.set("");
    }
}

/// Something the editor did that its host has to act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorEvent {
    /// A client waiting for a buffer to be finished can stop waiting. The
    /// number is the token the waiter was registered with.
    WaiterReleased(u64),
    /// A frame was closed by a command, so its frontend should go away.
    FrameClosed(FrameId),
}

const SCRATCH: &str = "*scratch*";
/// How long a prefix key waits before the keys that can follow are shown.
const KEY_HINT_DELAY: Duration = Duration::from_millis(600);

/// All editor state. It is driven by [`Editor::handle_event`] and observed
/// through [`Editor::redisplay`].
pub struct Editor {
    pub(crate) buffers: SlotMap<BufferId, Buffer>,
    /// Buffers in most-recently-selected order.
    pub(crate) buffer_order: Vec<BufferId>,
    pub(crate) windows: SlotMap<WindowId, Window>,
    pub(crate) frames: SlotMap<FrameId, Frame>,
    pub(crate) current_frame: FrameId,
    pub(crate) base_frame: FrameId,
    /// The frame whose echo area shows the active prompt, if there is one.
    pub(crate) prompt_frame: Option<FrameId>,
    pub(crate) minibuffer: Option<MinibufferState>,
    /// Text shown after the minibuffer input until the next key.
    pub(crate) minibuffer_hint: Option<String>,
    pub(crate) key_prompt: Option<KeyPrompt>,
    /// Whether frames belong to clients of a long-running server. Closing
    /// the terminal then closes a frame instead of exiting the editor.
    pub(crate) daemon: bool,
    events: Vec<EditorEvent>,

    commands: HashMap<String, Command>,
    /// The built-in commands. A command of the same name defined in Scheme
    /// replaces the entry above but not this one, so that it can call what
    /// it replaces.
    natives: HashMap<String, fn(&mut Editor) -> CmdResult>,
    pub(crate) extend: ExtendState,
    pub(crate) file_jobs: crate::files::FileJobs,
    pub(crate) spell: crate::spell::SpellState,
    pub(crate) processes: ProcessState,
    pub(crate) websockets: crate::websocket::WebSocketState,
    pub(crate) terminals: crate::terminal::Terminals,
    /// Keymaps by name: [`GLOBAL_MAP`], [`MINIBUFFER_MAP`], and one per mode,
    /// named after the mode.
    keymaps: HashMap<String, Keymap>,
    pub(crate) script: ScriptState,
    pub(crate) faces: Faces,
    pub(crate) lsp: LspState,
    pub(crate) environments: Environments,
    pub(crate) completion: CompletionState,
    pub(crate) snippet: Option<ActiveSnippet>,
    /// Snippets by the mode they belong to, each a name and its template.
    pub(crate) snippets: HashMap<String, Vec<(String, Vec<Value>)>>,
    pub(crate) formatters: HashMap<String, Formatter>,
    /// Labels of places to jump to, while the user picks one.
    pub(crate) labels: Option<Labels>,
    /// Where the pointer is and what was last done with it.
    pub(crate) mouse: Mouse,
    /// The undo history being shown as a tree.
    pub(crate) undo_view: Option<UndoView>,
    /// The rectangle last killed or copied, a string per line.
    pub(crate) killed_rectangle: Vec<String>,
    /// Set by commands that edit and want the region to stay active.
    pub(crate) keep_mark: bool,
    /// When to look again for files that changed on disk.
    auto_revert_due: Option<Instant>,
    pending_keys: Vec<Key>,
    esc_pending: bool,
    /// Prefix argument for the next command.
    pub(crate) prefix_arg: Option<Prefix>,
    /// Prefix argument of the command that is running.
    current_prefix: Option<Prefix>,
    pub(crate) last_key: Option<Key>,
    /// The keyboard macro being defined, the last one and the one that runs.
    pub(crate) macros: crate::kmacro::Macros,
    /// The text last pasted into a buffer that binds `<paste>`.
    last_paste: Option<String>,
    /// What each window showed and how much of it, at the last layout, to
    /// tell when a size changes.
    window_sizes: HashMap<WindowId, (BufferId, usize, usize)>,
    this_command: Option<String>,
    last_command: Option<String>,

    /// Directory that file prompts start in for buffers without a file.
    pub(crate) default_directory: PathBuf,
    /// Directory for auto-save files and other persistent state.
    pub(crate) state_directory: Option<PathBuf>,
    pub(crate) kill_ring: KillRing,
    pub(crate) history: History,
    /// What ran and how long it took.
    pub(crate) telemetry: crate::telemetry::Telemetry,
    /// Wakes the host's event loop from worker threads.
    pub(crate) waker: Waker,
    /// When to show the keys that can follow the pending prefix.
    key_hint_due: Option<Instant>,
    clipboard: Option<Box<dyn Clipboard>>,
    /// The kill ring head when it and the clipboard last agreed.
    clipboard_synced: Option<String>,
    /// The secret that the editor put on the clipboard, until it is
    /// cleared from there.
    clipboard_secret: Option<String>,
    /// The ends of the names of the files that Scheme reads and writes:
    /// their bytes are not their text. See `define-file-handler`.
    pub(crate) file_handler_suffixes: Vec<String>,
    /// Set by vertical motion commands to keep the goal column.
    pub(crate) keep_goal: bool,
    exit: bool,
    next_serial: u32,
}

impl Editor {
    /// Creates an editor with a `*scratch*` buffer in a frame of the given size.
    pub fn new(cols: u16, rows: u16) -> Self {
        let mut buffers = SlotMap::with_key();
        let scratch = buffers.insert(Buffer::new(SCRATCH));

        let mut editor = Self {
            buffers,
            buffer_order: vec![scratch],
            windows: SlotMap::with_key(),
            frames: SlotMap::with_key(),
            current_frame: FrameId::default(),
            base_frame: FrameId::default(),
            prompt_frame: None,
            minibuffer: None,
            minibuffer_hint: None,
            key_prompt: None,
            daemon: false,
            events: Vec::new(),
            commands: HashMap::new(),
            keymaps: HashMap::new(),
            natives: HashMap::new(),
            extend: ExtendState::default(),
            file_jobs: crate::files::FileJobs::default(),
            spell: crate::spell::SpellState::default(),
            processes: ProcessState::default(),
            websockets: Default::default(),
            terminals: crate::terminal::Terminals::default(),
            script: ScriptState::new(),
            faces: Faces::default(),
            lsp: LspState::default(),
            environments: Environments::default(),
            completion: CompletionState::default(),
            snippet: None,
            snippets: HashMap::new(),
            formatters: HashMap::new(),
            labels: None,
            mouse: Mouse::default(),
            undo_view: None,
            killed_rectangle: Vec::new(),
            keep_mark: false,
            auto_revert_due: None,
            pending_keys: Vec::new(),
            esc_pending: false,
            prefix_arg: None,
            current_prefix: None,
            last_key: None,
            macros: crate::kmacro::Macros::default(),
            last_paste: None,
            window_sizes: HashMap::new(),
            this_command: None,
            last_command: None,
            default_directory: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
            state_directory: None,
            kill_ring: KillRing::default(),
            history: History::default(),
            telemetry: crate::telemetry::Telemetry::default(),
            waker: Arc::new(|| {}),
            key_hint_due: None,
            clipboard: None,
            clipboard_synced: None,
            clipboard_secret: None,
            file_handler_suffixes: Vec::new(),
            keep_goal: false,
            exit: false,
            next_serial: 1,
        };
        editor.base_frame = editor.new_frame(scratch, cols, rows);
        editor.current_frame = editor.base_frame;
        commands::install(&mut editor);
        editor.relayout();
        editor
    }

    // Commands

    /// Defines or redefines a command. Keys bound to its name run the new
    /// definition.
    pub fn define_command(&mut self, name: &str, doc: &str, body: CommandBody) {
        if let CommandBody::Native(run) = &body {
            self.natives.insert(name.to_owned(), *run);
        }
        let command = Command {
            name: name.to_owned(),
            doc: doc.to_owned(),
            body,
        };
        self.commands.insert(command.name.clone(), command);
    }

    pub fn command(&self, name: &str) -> Option<&Command> {
        self.commands.get(name)
    }

    /// The built-in command of a name, whether or not a command defined in
    /// Scheme has taken the name.
    pub fn native_command(&self, name: &str) -> Option<fn(&mut Editor) -> CmdResult> {
        self.natives.get(name).copied()
    }

    pub fn command_names(&self) -> impl Iterator<Item = &str> {
        self.commands.keys().map(String::as_str)
    }

    /// The keymap with the given name, created empty if it does not exist.
    pub fn keymap_mut(&mut self, name: &str) -> &mut Keymap {
        self.keymaps.entry(name.to_owned()).or_default()
    }

    pub fn keymap(&self, name: &str) -> Option<&Keymap> {
        self.keymaps.get(name)
    }

    /// The numeric prefix argument of the running command, 1 if none was given.
    pub fn arg(&self) -> i64 {
        self.current_prefix.map_or(1, Prefix::value)
    }

    pub fn raw_prefix(&self) -> Option<Prefix> {
        self.current_prefix
    }

    pub(crate) fn set_current_prefix(&mut self, prefix: Prefix) {
        self.current_prefix = Some(prefix);
    }

    /// Name of the command that ran before the current one.
    pub fn last_command(&self) -> Option<&str> {
        self.last_command.as_deref()
    }

    /// Runs a command by name, with the bookkeeping that surrounds every
    /// command: prefix argument, undo boundaries, mark deactivation.
    pub fn execute(&mut self, name: &str) {
        let Some(body) = self.commands.get(name).map(|c| c.body.clone()) else {
            self.message(format!("Unknown command: {name}"));
            return;
        };
        // A program does not run commands in a buffer that is secret: a
        // command can copy the text to where the program can read it.
        // It can leave the buffer.
        const LEAVING: [&str; 7] = [
            "switch-to-buffer",
            "kill-buffer",
            "other-window",
            "next-buffer",
            "previous-buffer",
            "keyboard-quit",
            "execute-extended-command",
        ];
        if !LEAVING.contains(&name)
            && let Err(error) = self.keep_from_program(self.window().buffer)
        {
            self.message(error.to_string());
            return;
        }
        self.current_prefix = self.prefix_arg.take();
        self.this_command = Some(name.to_owned());
        self.keep_goal = false;
        self.keep_mark = false;
        // With functions on the hook that runs before commands, Scheme runs
        // them and then the command.
        if self.hook_active("pre-command-hook") {
            let queued = self.queue_script(ScriptCall {
                target: ScriptTarget::Named("%run-command"),
                args: vec![Value::sym(name)],
                command: Some(name.to_owned()),
                echo_result: false,
                frame: None,
            });
            self.report(queued);
            return;
        }
        let run = match body {
            CommandBody::Native(run) => run,
            CommandBody::Script(func) => {
                self.script.queued_prefix =
                    self.current_prefix.map(|prefix| (name.to_owned(), prefix));
                // The host runs the body and then finishes the command.
                let queued = self.queue_script(ScriptCall {
                    target: ScriptTarget::Func(func),
                    args: Vec::new(),
                    command: Some(name.to_owned()),
                    echo_result: false,
                    frame: None,
                });
                self.report(queued);
                return;
            }
        };
        let buffer = self.window().buffer;
        let tick = self.buffers[buffer].tick();
        let point = self.point();
        self.buffer_mut().begin_command(point);

        let started = Instant::now();
        let result = run(self);

        self.finish_command(buffer, tick, true);
        self.report(result);
        self.note_time("command", name, started.elapsed());
    }

    /// The bookkeeping after a command or script call. `is_command` is false
    /// for script callbacks, which must not count as the user's last command.
    pub(crate) fn finish_command(&mut self, buffer: BufferId, tick: u64, is_command: bool) {
        const SELF_INSERT: &str = "self-insert-command";
        let amalgamate = self.this_command.as_deref() == Some(SELF_INSERT)
            && self.last_command.as_deref() == Some(SELF_INSERT);
        self.settle_points();
        self.end_undo_step(amalgamate);
        let keep_mark = std::mem::take(&mut self.keep_mark);
        if let Some(buf) = self.buffers.get_mut(buffer) {
            if buf.tick() != tick && !keep_mark {
                buf.mark_active = false;
            }
            if !buf.mark_active {
                buf.rectangle_mark = false;
            }
        }
        if !self.keep_goal {
            self.window_mut().goal_col = None;
        }
        self.kill_ring.end_command();
        self.export_kill();
        self.sync_minibuffer(Instant::now());
        // A command run from within another, as with M-x, has already been
        // recorded as the last command.
        let command = self.this_command.take();
        if is_command {
            self.undo_view_after_command(command.as_deref());
            self.snippet_after_command(command.as_deref());
            self.completion_after_command(command.as_deref());
        }
        self.lsp_after_command();
        if let Some(name) = command {
            self.last_command = Some(name);
        }
        self.current_prefix = None;
        if is_command {
            self.run_hook("post-command-hook", Vec::new());
        }
    }

    /// Closes the undo record of what has been done to every buffer. With
    /// `amalgamate`, the current buffer's joins its previous record.
    fn end_undo_step(&mut self, amalgamate: bool) {
        let current = self.window().buffer;
        // Point in each buffer, as its undo record should remember it: that
        // of the selected window, else of any window showing the buffer.
        let mut points: HashMap<BufferId, MarkerId> = HashMap::new();
        for window in self.windows.values() {
            points.entry(window.buffer).or_insert(window.point);
        }
        points.insert(current, self.window().point);
        for (id, buf) in &mut self.buffers {
            let point = match points.get(&id) {
                Some(marker) => buf.marker(*marker),
                None => buf.saved_point(),
            };
            buf.end_command(point, amalgamate && id == current);
        }
    }

    /// Makes the edits done outside any command, such as those a language
    /// server asked for, an undo step of their own.
    pub(crate) fn commit_undo(&mut self) {
        self.end_undo_step(false);
        self.lsp_flush_all();
    }

    /// The environment for programs run for files in `dir`, from its
    /// `.envrc`, or `None` if there is none or direnv is not used.
    pub(crate) fn environment(&mut self, dir: &std::path::Path) -> Option<EnvState> {
        if matches!(self.option("use-direnv"), Ok(Value::Bool(false))) {
            return None;
        }
        let program = match self.option("direnv-program") {
            Ok(Value::Str(program)) => program,
            _ => "direnv".to_owned(),
        };
        let waker = self.waker.clone();
        self.environments.lookup(dir, &program, &waker)
    }

    /// Takes in the environments that have been worked out and starts what
    /// was waiting for them.
    pub(crate) fn environment_poll(&mut self) {
        let finished = self.environments.poll();
        if finished.is_empty() {
            return;
        }
        for (root, problem) in finished {
            if let Some(problem) = problem {
                self.message(format!(
                    "direnv: {problem} ({})",
                    crate::files::abbreviate_path(&root)
                ));
            }
        }
        self.lsp_start_waiting();
    }

    /// Forgets the environments worked out so far, so that a changed
    /// `.envrc` or flake is read again.
    pub fn reload_environments(&mut self) {
        self.environments.clear();
    }

    pub(crate) fn report(&mut self, result: CmdResult) {
        if let Err(error) = result {
            // A keyboard macro ends where one of its commands fails.
            self.macro_stop();
            self.message(error.to_string());
        }
    }

    // Input

    /// Whether the user is asked something in the current frame: a
    /// prompt in the minibuffer, or a question that one key answers.
    pub fn prompt_open(&self) -> bool {
        self.key_prompt.is_some()
            || (self.minibuffer.is_some() && self.prompt_frame == Some(self.current_frame()))
    }

    pub fn handle_event(&mut self, event: InputEvent) {
        // What is typed while a command waits for a program is for what
        // the command does next. Not so while a prompt is open here: the
        // command did not open it, since it waits, and what is typed is
        // for the prompt. The program may be the one that asks, as ssh
        // asks for a password before it gives the file that a command
        // waits for.
        if self.holds_input()
            && !self.prompt_open()
            && matches!(event, InputEvent::Key(_) | InputEvent::Paste(_))
        {
            if event == InputEvent::Key(Key::ctrl('g')) {
                self.stop_waiting();
            } else {
                self.hold_input(event);
            }
            return;
        }
        match event {
            InputEvent::Key(key) => self.handle_key(key),
            InputEvent::Resize { cols, rows } => self.resize(cols, rows),
            InputEvent::Paste(text) => self.handle_paste(&text),
            InputEvent::Mouse(event) => self.handle_mouse(event),
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.frame_mut().size = (cols, rows);
        self.relayout();
        self.frame_mut().redisplay.invalidate();
    }

    pub fn handle_key(&mut self, key: Key) {
        let starts_sequence =
            self.pending_keys.is_empty() && !self.esc_pending && self.key_prompt.is_none();
        self.macro_note_key(key, starts_sequence);
        self.frame_mut().echo = None;
        self.frame_mut().hint.clear();
        self.key_hint_due = None;
        self.minibuffer_hint = None;
        self.relayout();

        if let Some(question) = self.key_prompt.take() {
            self.prompt_frame = None;
            // What the answer does is a command like any other: an undo
            // step of its own, and the servers told of its edits.
            self.execute_with("answer-question", |ed| (question.answer)(ed, key));
            return;
        }

        let quit = Key::ctrl('g');
        if key == quit && (self.esc_pending || !self.pending_keys.is_empty()) {
            self.cancel_pending();
            self.message("Quit");
            return;
        }

        // ESC is a prefix that adds Meta to the next key, for terminals that
        // cannot send Meta directly.
        let key = if std::mem::take(&mut self.esc_pending) {
            Key::new(key.code, key.mods | Mods::META)
        } else if key == Key::named(NamedKey::Escape) {
            self.esc_pending = true;
            self.show_pending();
            return;
        } else {
            key
        };

        if self.pending_keys.is_empty()
            && let Some(prefix) = self.prefix_arg
            && let Some(c) = key.self_insert_char()
        {
            if let Some(digit) = c.to_digit(10) {
                self.prefix_arg = Some(prefix.push_digit(i64::from(digit)));
                self.show_pending();
                return;
            }
            if c == '-' && matches!(prefix, Prefix::Universal(4)) {
                self.prefix_arg = Some(Prefix::Negative);
                self.show_pending();
                return;
            }
        }

        self.pending_keys.push(key);
        let lookup = {
            let maps: Vec<&Keymap> = self
                .active_keymaps()
                .into_iter()
                .filter_map(|name| self.keymaps.get(name))
                .collect();
            match lookup_in(&maps, &self.pending_keys) {
                Lookup::Command(name) => Some(Some(name.to_owned())),
                Lookup::Prefix => Some(None),
                Lookup::Unbound => None,
            }
        };
        match lookup {
            Some(Some(name)) => {
                self.pending_keys.clear();
                self.last_key = Some(key);
                self.execute(&name);
                if self.prefix_arg.is_some() && self.frame().echo.is_none() {
                    self.show_pending();
                }
            }
            Some(None) => {
                self.show_pending();
                self.key_hint_due = Some(Instant::now() + KEY_HINT_DELAY);
            }
            None if self.pending_keys.len() == 1 && key.self_insert_char().is_some() => {
                self.pending_keys.clear();
                self.last_key = Some(key);
                self.execute("self-insert-command");
            }
            None => {
                let keys = describe_keys(&self.pending_keys);
                self.cancel_pending();
                self.message(format!("{keys} is undefined"));
            }
        }
    }

    /// The window that shows a cell of the frame, and the cell's row and
    /// column in it. The mode line is no part of that.
    pub(crate) fn window_at(&self, col: u16, row: u16) -> Option<(WindowId, usize, usize)> {
        self.frame().layout.leaves().into_iter().find_map(|id| {
            let rect = self.windows[id].rect;
            let inside = (rect.x..rect.x + rect.w).contains(&col)
                && (rect.y..rect.y + rect.h.saturating_sub(1)).contains(&row);
            inside.then(|| (id, usize::from(row - rect.y), usize::from(col - rect.x)))
        })
    }

    /// The position in a window's buffer that is shown at a row and column
    /// of the window, or the nearest there is.
    pub(crate) fn position_at(&self, id: WindowId, row: usize, col: usize) -> usize {
        let window = &self.windows[id];
        let buffer = &self.buffers[window.buffer];
        let (gutter, width) = crate::redisplay::text_area(window, buffer);
        let col = col.saturating_sub(gutter);
        let mut start = buffer.marker(window.start);
        for _ in 0..row {
            let walked = crate::display::walk_row(
                buffer.text(),
                start,
                width,
                self.window_flow(id),
                |_, _, _, _| {},
            );
            match walked.next {
                Some(next) => start = next,
                // Below the end of the text.
                None => return buffer.len(),
            }
        }
        let mut found = None;
        let walked = crate::display::walk_row(
            buffer.text(),
            start,
            width,
            self.window_flow(id),
            |pos, _, column, cells| {
                if found.is_none() && col < column + cells {
                    found = Some(pos);
                }
            },
        );
        found.unwrap_or(walked.end)
    }

    fn handle_paste(&mut self, text: &str) {
        self.relayout();
        self.frame_mut().echo = None;
        self.minibuffer_hint = None;
        self.cancel_pending();
        self.cancel_key_prompt();
        // A mode that wants the text itself binds `<paste>`; a terminal
        // hands it to its program.
        let key = Key::named(NamedKey::Paste);
        if let Some(command) = self.key_binding(&[key]) {
            self.last_paste = Some(text.to_owned());
            self.last_key = Some(key);
            self.execute(&command);
            return;
        }
        self.execute_with("yank", |ed| ed.insert(text));
    }

    /// The name of the command that is running, if one is.
    pub fn this_command(&self) -> Option<&str> {
        self.this_command.as_deref()
    }

    /// The key that ran the command that is running, as `last-key` gives
    /// it, or the last one pressed.
    pub fn last_key(&self) -> Option<Key> {
        self.last_key
    }

    /// The text last pasted into a buffer that binds `<paste>`.
    pub fn last_paste(&self) -> Option<&str> {
        self.last_paste.as_deref()
    }

    /// Columns of a window that show text: without the line numbers.
    pub fn window_width(&self, id: WindowId) -> usize {
        self.text_width(id)
    }

    /// Rows of a window that show text: without the mode line.
    pub fn window_height(&self, id: WindowId) -> usize {
        self.windows[id].text_rows()
    }

    /// Runs `body` with the bookkeeping of a command called `name`.
    fn execute_with(&mut self, name: &str, body: impl FnOnce(&mut Editor) -> CmdResult) {
        self.this_command = Some(name.to_owned());
        let buffer = self.window().buffer;
        let tick = self.buffers[buffer].tick();
        let point = self.point();
        self.buffer_mut().begin_command(point);
        let result = body(self);
        self.finish_command(buffer, tick, true);
        self.report(result);
    }

    /// Takes out the input that is in progress in the current frame: a key
    /// sequence, a prefix argument, a prompt. Each frame has its own, and
    /// the editor's fields hold that of the current frame.
    pub(crate) fn stash_input(&mut self) -> InputState {
        InputState {
            pending_keys: std::mem::take(&mut self.pending_keys),
            esc_pending: std::mem::take(&mut self.esc_pending),
            prefix_arg: self.prefix_arg.take(),
            minibuffer: self.minibuffer.take(),
            minibuffer_hint: self.minibuffer_hint.take(),
            key_prompt: self.key_prompt.take(),
            labels: self.labels.take(),
            key_hint_due: self.key_hint_due.take(),
        }
    }

    /// Puts back input that was in progress in the frame that is now current.
    pub(crate) fn restore_input(&mut self, input: InputState) {
        self.pending_keys = input.pending_keys;
        self.esc_pending = input.esc_pending;
        self.prefix_arg = input.prefix_arg;
        self.minibuffer = input.minibuffer;
        self.minibuffer_hint = input.minibuffer_hint;
        self.key_prompt = input.key_prompt;
        self.labels = input.labels;
        self.key_hint_due = input.key_hint_due;
        let prompting = self.minibuffer.is_some() || self.key_prompt.is_some();
        self.prompt_frame = prompting.then_some(self.current_frame);
    }

    pub(crate) fn cancel_pending(&mut self) {
        self.pending_keys.clear();
        self.esc_pending = false;
        self.prefix_arg = None;
    }

    /// Echoes the unfinished key sequence, as in `C-u 4 C-x-`.
    fn show_pending(&mut self) {
        let mut parts = Vec::new();
        match self.prefix_arg {
            Some(Prefix::Universal(n)) => {
                parts.extend(std::iter::repeat_n("C-u".to_owned(), n.ilog(4) as usize));
            }
            Some(Prefix::Number(n)) => parts.push(format!("C-u {n}")),
            Some(Prefix::Negative) => parts.push("C-u -".to_owned()),
            None => {}
        }
        parts.extend(self.pending_keys.iter().map(Key::to_string));
        if self.esc_pending {
            parts.push("ESC".to_owned());
        }
        self.frame_mut().echo = Some(format!("{}-", parts.join(" ")));
    }

    /// Asks a question that is answered with a single key.
    pub fn ask_key(
        &mut self,
        prompt: impl Into<String>,
        answer: impl FnOnce(&mut Editor, Key) -> CmdResult + 'static,
    ) {
        self.key_prompt = Some(KeyPrompt {
            prompt: prompt.into(),
            answer: Box::new(answer),
            promise: None,
        });
        self.prompt_frame = Some(self.current_frame);
    }

    fn cancel_key_prompt(&mut self) {
        if let Some(question) = self.key_prompt.take() {
            self.prompt_frame = None;
            if let Some(id) = question.promise {
                self.reject_promise(id);
            }
        }
        // Labels are only there for a question about them.
        self.labels = None;
    }

    /// Abandons all input in progress: a key sequence, a prefix argument, a
    /// single-key question, the minibuffer.
    pub(crate) fn cancel_input(&mut self) {
        self.cancel_pending();
        self.cancel_key_prompt();
        // Quitting is how the minibuffer reports being abandoned.
        let _ = crate::minibuffer::abort_minibuffer(self);
    }

    /// Sets how worker threads wake the host's event loop when they have
    /// results, so that it calls [`Editor::poll`] and redraws.
    pub fn set_waker(&mut self, waker: Waker) {
        self.waker = waker;
    }

    /// Does the work that is due at `now` and takes in results from worker
    /// threads. The host calls this when woken and when a deadline passes.
    pub fn poll(&mut self, now: Instant) {
        self.sync_minibuffer(now);
        self.environment_poll();
        self.lsp_poll(now);
        self.extend_poll(now);
        self.file_jobs_poll();
        self.askpass_poll();
        self.spell_poll();
        self.process_poll();
        self.websocket_poll();
        self.completion_poll(now);
        if self.auto_revert_due.is_none_or(|due| due <= now) {
            self.auto_revert_due = Some(now + AUTO_REVERT_INTERVAL);
            self.auto_revert();
        }
        if self.key_hint_due.is_some_and(|due| due <= now) {
            self.key_hint_due = None;
            self.show_key_hint();
        }
        self.held_notice_poll(now);
    }

    /// When [`Editor::poll`] next has something to do, if it does.
    pub fn next_deadline(&self) -> Option<Instant> {
        [
            self.key_hint_due,
            self.minibuffer_deadline(),
            self.completion.due,
            self.lsp.idle_due,
            self.auto_revert_due,
            self.extend_deadline(),
            self.held_notice_deadline(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Shows the keys that can follow the pending prefix, which-key style.
    fn show_key_hint(&mut self) {
        if self.pending_keys.is_empty() {
            return;
        }
        let mut entries: Vec<(Key, String)> = Vec::new();
        for name in self.active_keymaps() {
            let Some(map) = self.keymaps.get(name) else {
                continue;
            };
            for (key, binding) in map.entries(&self.pending_keys) {
                // An earlier keymap's binding hides a later one's.
                if !entries.iter().any(|(seen, _)| *seen == key) {
                    entries.push((key, binding));
                }
            }
        }
        entries.sort_by_key(|(key, _)| key.to_string());
        let cells: Vec<String> = entries
            .iter()
            .map(|(key, binding)| format!("{key} → {binding}"))
            .collect();

        // Fill columns of equal width, row by row.
        let width = usize::from(self.frame().size.0).max(1);
        let cell = cells.iter().map(|c| c.chars().count()).max().unwrap_or(0) + 2;
        let per_row = (width / cell.max(1)).max(1);
        let lines = cells
            .chunks(per_row)
            .map(|row| row.iter().map(|c| format!("{c:cell$}")).collect::<String>())
            .collect();
        self.frame_mut().hint = lines;
    }

    /// Marks the editor as a server whose frames belong to clients.
    pub fn set_daemon(&mut self, daemon: bool) {
        self.daemon = daemon;
    }

    pub(crate) fn emit(&mut self, event: EditorEvent) {
        self.events.push(event);
    }

    /// The events since the last call, for the host to act on.
    pub fn take_events(&mut self) -> Vec<EditorEvent> {
        std::mem::take(&mut self.events)
    }

    /// Asks a yes-or-no question and runs `on_yes` if the answer is `y`.
    pub fn ask_yes_no(
        &mut self,
        prompt: impl Into<String>,
        on_yes: impl FnOnce(&mut Editor) -> CmdResult + 'static,
    ) {
        let prompt = prompt.into();
        self.ask_key(format!("{prompt} (y or n) "), move |ed, key| {
            match key.code {
                KeyCode::Char('y' | 'Y') if key.mods.is_empty() => on_yes(ed),
                KeyCode::Char('n' | 'N') if key.mods.is_empty() => Ok(()),
                KeyCode::Char('g') if key.mods == Mods::CTRL => Err(CmdError::Quit),
                _ => {
                    ed.ask_yes_no(prompt, on_yes);
                    Ok(())
                }
            }
        });
    }

    /// Sets the directory that file prompts start in for buffers that do not
    /// visit a file. It defaults to the working directory of the process.
    pub fn set_default_directory(&mut self, dir: impl Into<PathBuf>) {
        self.default_directory = dir.into();
    }

    /// Asks a yes-or-no question for Scheme code: returns a promise that
    /// resolves to a boolean, or is rejected if the user quits.
    pub fn ask_yes_no_promise(&mut self, prompt: impl Into<String>) -> u64 {
        let id = self.new_promise();
        self.ask_bool(prompt.into(), id);
        id
    }

    fn ask_bool(&mut self, prompt: String, id: u64) {
        self.ask_key(format!("{prompt} (y or n) "), move |ed, key| {
            match key.code {
                KeyCode::Char('y' | 'Y') if key.mods.is_empty() => {
                    ed.resolve_promise(id, Value::Bool(true));
                }
                KeyCode::Char('n' | 'N') if key.mods.is_empty() => {
                    ed.resolve_promise(id, Value::Bool(false));
                }
                KeyCode::Char('g') if key.mods == Mods::CTRL => {
                    ed.reject_promise(id);
                    return Err(CmdError::Quit);
                }
                _ => ed.ask_bool(prompt, id),
            }
            Ok(())
        });
        if let Some(question) = &mut self.key_prompt {
            question.promise = Some(id);
        }
    }

    // Faces and themes

    pub fn faces(&self) -> &Faces {
        &self.faces
    }

    /// The faces, for changing them. Frames are redrawn afterwards.
    pub fn faces_mut(&mut self) -> &mut Faces {
        &mut self.faces
    }

    /// Sets the language of the current buffer by name, or clears it.
    pub fn set_language(&mut self, name: Option<&str>) -> CmdResult {
        let language = match name {
            Some(name) => Some(
                amacs_treesit::language(name)
                    .ok_or_else(|| CmdError::msg(format!("Unknown language: {name}")))?,
            ),
            None => None,
        };
        self.buffer_mut().set_language(language);
        Ok(())
    }

    /// Indents the line that point is on as its language says.
    pub fn indent_current_line(&mut self) -> CmdResult {
        let line = self.text().line_of(self.point());
        crate::commands::indent::indent_line(self, line)
    }

    // Messages and exit

    pub fn message(&mut self, text: impl Into<String>) {
        self.frame_mut().echo = Some(text.into());
    }

    pub fn echo(&self) -> Option<&str> {
        self.frame().echo.as_deref()
    }

    pub fn request_exit(&mut self) {
        self.lsp_shutdown_all();
        self.kill_all_processes();
        self.exit = true;
    }

    pub fn should_exit(&self) -> bool {
        self.exit
    }

    // Current window and buffer

    pub fn in_minibuffer(&self) -> bool {
        self.selected() == self.minibuffer_window()
    }

    /// Fails for commands that make no sense while the minibuffer is selected.
    pub(crate) fn forbid_in_minibuffer(&self) -> CmdResult {
        if self.in_minibuffer() {
            Err(CmdError::msg("Not available in the minibuffer"))
        } else {
            Ok(())
        }
    }

    pub fn window(&self) -> &Window {
        &self.windows[self.selected()]
    }

    pub fn window_mut(&mut self) -> &mut Window {
        let selected = self.selected();
        &mut self.windows[selected]
    }

    pub fn buffer(&self) -> &Buffer {
        &self.buffers[self.window().buffer]
    }

    /// Gives a buffer a default face of its own; see [`Buffer::set_default_face`].
    pub fn set_buffer_default_face(&mut self, id: BufferId, face: Option<String>) {
        if let Some(buffer) = self.buffers.get_mut(id) {
            buffer.set_default_face(face);
        }
    }

    pub fn buffer_mut(&mut self) -> &mut Buffer {
        let id = self.window().buffer;
        &mut self.buffers[id]
    }

    pub fn text(&self) -> &Text {
        self.buffer().text()
    }

    pub fn point(&self) -> usize {
        self.buffer().marker(self.window().point)
    }

    /// Moves point, clamping to the buffer.
    pub fn set_point(&mut self, pos: usize) {
        let marker = self.window().point;
        self.buffer_mut().set_marker(marker, pos);
    }

    /// Inserts at point and leaves point after the inserted text.
    pub fn insert(&mut self, s: &str) -> CmdResult {
        let point = self.point();
        self.buffer_mut().insert(point, s)?;
        self.set_point(point + s.chars().count());
        Ok(())
    }

    /// Deletes a range of the current buffer and returns the deleted text.
    pub fn delete(&mut self, range: Range<usize>) -> CmdResult<String> {
        self.buffer_mut().delete(range)
    }

    /// The region between point and mark, in buffer order.
    pub fn region(&self) -> CmdResult<Range<usize>> {
        let mark = self
            .buffer()
            .mark()
            .ok_or_else(|| CmdError::msg("The mark is not set now, so there is no region"))?;
        let point = self.point();
        Ok(mark.min(point)..mark.max(point))
    }

    /// Connects the kill ring to the system clipboard.
    pub fn set_clipboard(&mut self, clipboard: Box<dyn Clipboard>) {
        self.clipboard = Some(clipboard);
    }

    /// Copies the newest kill to the clipboard if it is new since the last
    /// time the two agreed.
    fn export_kill(&mut self) {
        let Some(clipboard) = &mut self.clipboard else {
            return;
        };
        let head = self.kill_ring.head();
        if head.is_some() && head != self.clipboard_synced.as_deref() {
            clipboard.set(head.unwrap_or_default());
            self.clipboard_synced = head.map(str::to_owned);
        }
    }

    /// Puts the clipboard's text on the kill ring if another program put it
    /// there, so that the next yank inserts it.
    pub(crate) fn import_clipboard(&mut self) {
        let Some(text) = self
            .clipboard
            .as_mut()
            .and_then(|clipboard| clipboard.get())
        else {
            return;
        };
        if !text.is_empty() && self.kill_ring.head() != Some(text.as_str()) {
            self.kill_ring.push(text.clone());
        }
        self.clipboard_synced = Some(text);
    }

    /// Puts a secret, such as a password, on the clipboard and not on the
    /// kill ring. Returns whether there is a clipboard to put it on.
    pub fn set_clipboard_secret(&mut self, text: &str) -> bool {
        let Some(clipboard) = &mut self.clipboard else {
            return false;
        };
        clipboard.set_secret(text);
        // The two agree, as far as kills are concerned: the secret is not
        // overwritten by the newest kill at the end of the command.
        self.clipboard_synced = self.kill_ring.head().map(str::to_owned);
        self.clipboard_secret = Some(text.to_owned());
        true
    }

    /// Takes the secret that [`Editor::set_clipboard_secret`] put on the
    /// clipboard off it, if it is still what the clipboard holds, and off
    /// the kill ring, where a yank in the editor has put it. Returns
    /// whether the clipboard was cleared.
    pub fn clear_clipboard_secret(&mut self) -> bool {
        let Some(secret) = self.clipboard_secret.take() else {
            return false;
        };
        self.kill_ring.forget(&secret);
        let Some(clipboard) = &mut self.clipboard else {
            return false;
        };
        if clipboard.get().as_deref() != Some(secret.as_str()) {
            return false;
        }
        clipboard.clear();
        self.clipboard_synced = self.kill_ring.head().map(str::to_owned);
        true
    }

    /// Puts text on the kill ring as a new entry.
    pub fn kill_new(&mut self, text: String) {
        self.kill_ring.push(text);
    }

    /// The newest kill ring entry.
    pub fn current_kill(&mut self) -> Option<String> {
        self.import_clipboard();
        self.kill_ring.current().map(str::to_owned)
    }

    /// What a key sequence does in the current buffer.
    pub fn key_lookup(&self, keys: &[Key]) -> KeyMeaning {
        let maps: Vec<&Keymap> = self
            .active_keymaps()
            .into_iter()
            .filter_map(|name| self.keymaps.get(name))
            .collect();
        match lookup_in(&maps, keys) {
            Lookup::Command(name) => KeyMeaning::Command(name.to_owned()),
            Lookup::Prefix => KeyMeaning::Prefix,
            Lookup::Unbound => KeyMeaning::Unbound,
        }
    }

    /// Every key sequence that a keymap binds, with the command it runs,
    /// in the order of the keys as they are written.
    pub fn keymap_bindings(&self, name: &str) -> Vec<(String, String)> {
        fn collect(map: &Keymap, prefix: &mut Vec<Key>, out: &mut Vec<(String, String)>) {
            for (key, shown) in map.entries(prefix) {
                prefix.push(key);
                if shown == "+prefix" {
                    collect(map, prefix, out);
                } else {
                    out.push((describe_keys(prefix), shown));
                }
                prefix.pop();
            }
        }
        let mut found = Vec::new();
        if let Some(map) = self.keymaps.get(name) {
            collect(map, &mut Vec::new(), &mut found);
        }
        found.sort();
        found
    }

    /// The keys that run each command in the current buffer: what
    /// [`Editor::keys_for`] gives, for all commands at once.
    pub fn keys_by_command(&self) -> HashMap<String, Vec<String>> {
        let mut seen = std::collections::HashSet::new();
        let mut keys: HashMap<String, Vec<String>> = HashMap::new();
        for name in self.active_keymaps() {
            for (sequence, command) in self.keymap_bindings(name) {
                // An earlier keymap's binding hides a later one's.
                if seen.insert(sequence.clone()) {
                    keys.entry(command).or_default().push(sequence);
                }
            }
        }
        keys
    }

    /// The key sequences that run a command in the current buffer.
    pub fn keys_for(&self, command: &str) -> Vec<String> {
        fn collect(map: &Keymap, prefix: &mut Vec<Key>, command: &str, out: &mut Vec<Vec<Key>>) {
            for (key, shown) in map.entries(prefix) {
                prefix.push(key);
                if shown == command {
                    out.push(prefix.clone());
                } else if shown == "+prefix" {
                    collect(map, prefix, command, out);
                }
                prefix.pop();
            }
        }
        let mut found = Vec::new();
        for name in self.active_keymaps() {
            if let Some(map) = self.keymaps.get(name) {
                collect(map, &mut Vec::new(), command, &mut found);
            }
        }
        // Only those that an earlier keymap does not give another meaning.
        let mut keys: Vec<String> = found
            .into_iter()
            .filter(|keys| self.key_lookup(keys) == KeyMeaning::Command(command.to_owned()))
            .map(|keys| describe_keys(&keys))
            .collect();
        keys.sort();
        keys.dedup();
        keys
    }

    /// The command that a key sequence runs in the current buffer, if it is
    /// bound to one.
    pub fn key_binding(&self, keys: &[Key]) -> Option<String> {
        let maps: Vec<&Keymap> = self
            .active_keymaps()
            .into_iter()
            .filter_map(|name| self.keymaps.get(name))
            .collect();
        match lookup_in(&maps, keys) {
            Lookup::Command(name) => Some(name.to_owned()),
            Lookup::Prefix | Lookup::Unbound => None,
        }
    }

    // Buffers

    pub fn buffer_ids(&self) -> impl Iterator<Item = BufferId> + '_ {
        self.buffer_order.iter().copied()
    }

    pub fn buffer_by_id(&self, id: BufferId) -> Option<&Buffer> {
        self.buffers.get(id)
    }

    pub fn find_buffer(&self, name: &str) -> Option<BufferId> {
        self.buffer_ids().find(|id| self.buffers[*id].name == name)
    }

    /// Adds a buffer, renaming it `name<2>` and so on if the name is taken.
    pub fn add_buffer(&mut self, mut buffer: Buffer) -> BufferId {
        if self.find_buffer(&buffer.name).is_some() {
            let base = buffer.name.clone();
            buffer.name = (2..)
                .map(|n| format!("{base}<{n}>"))
                .find(|name| self.find_buffer(name).is_none())
                .expect("an unused name exists");
        }
        let id = self.buffers.insert(buffer);
        self.buffer_order.push(id);
        self.apply_options();
        id
    }

    /// Shows a buffer in the selected window.
    pub fn switch_to_buffer(&mut self, id: BufferId) {
        self.set_window_buffer(self.selected(), id);
        self.buffer_order.retain(|other| *other != id);
        self.buffer_order.insert(0, id);
        // A file read without being shown gets its mode now that it is.
        if std::mem::take(&mut self.buffers[id].needs_file_hook) {
            self.run_hook("find-file-hook", Vec::new());
        }
    }

    /// Shows a buffer in a window other than the selected one, making one
    /// if the frame has only one. The selection stays where it is.
    pub(crate) fn show_in_other_window(&mut self, id: BufferId) -> CmdResult {
        let leaves = self.frame().layout.leaves();
        let selected = self.selected();
        if let Some(other) = leaves.iter().find(|leaf| **leaf != selected)
            && leaves.contains(&selected)
        {
            self.set_window_buffer(*other, id);
            return Ok(());
        }
        let target = leaves[0];
        if self.windows[target].rect.h < 6 {
            return Err(CmdError::msg("No room for another window"));
        }
        let new = self.make_window(id);
        self.frame_mut()
            .layout
            .split(target, crate::window::SplitDir::Below, new);
        self.relayout();
        Ok(())
    }

    /// The most recently selected buffer other than the current one.
    pub fn other_buffer(&self) -> Option<BufferId> {
        let current = self.window().buffer;
        self.buffer_ids().find(|id| *id != current)
    }

    /// Removes a buffer. Windows showing it switch to another buffer.
    pub fn kill_buffer(&mut self, id: BufferId) {
        if !self.buffers.contains_key(id) {
            return;
        }
        self.lsp_detach(id);
        self.terminal_detach(id);
        self.spell_buffer_killed(id);
        self.buffer_order.retain(|other| *other != id);
        let replacement = match self.buffer_order.first() {
            Some(first) => *first,
            None => self.add_buffer(Buffer::new(SCRATCH)),
        };
        let showing: Vec<WindowId> = self
            .windows
            .iter()
            .filter(|(_, window)| window.buffer == id)
            .map(|(window, _)| window)
            .collect();
        for window in showing {
            self.set_window_buffer(window, replacement);
        }
        if let Some(mut buffer) = self.buffers.remove(id) {
            // The history stays on disk for the next visit, without the
            // changes that the user gave up by this.
            if buffer.is_modified() {
                buffer.give_up_changes();
            }
            if let Err(error) = buffer.write_undo_journal() {
                self.message(format!("Cannot write the undo history: {error}"));
            }
            for token in std::mem::take(&mut buffer.waiters) {
                self.emit(EditorEvent::WaiterReleased(token));
            }
            // The user chose to discard the buffer, so its auto-save data
            // is not wanted either.
            if let Some(file) = &buffer.file {
                self.remove_auto_save(file);
            }
            // The buffer is gone, so the hook gets its name.
            self.run_hook("kill-buffer-hook", vec![Value::str(buffer.name)]);
        }
    }

    /// Registers a client that waits for a buffer to be finished with
    /// `server-edit` or killed. `token` comes back in
    /// [`EditorEvent::WaiterReleased`].
    pub fn add_waiter(&mut self, buffer: BufferId, token: u64) {
        if let Some(buffer) = self.buffers.get_mut(buffer) {
            buffer.waiters.push(token);
        }
    }

    /// Forgets a client's waiters without notifying it, as when it has gone.
    pub fn remove_waiters(&mut self, token: u64) {
        for buffer in self.buffers.values_mut() {
            buffer.waiters.retain(|waiter| *waiter != token);
        }
    }

    /// Closes a frame at the user's request and tells the host.
    pub(crate) fn close_frame(&mut self, id: FrameId) -> CmdResult {
        self.delete_frame(id)?;
        self.emit(EditorEvent::FrameClosed(id));
        Ok(())
    }

    /// Moves point to the start of a line, counting from 1, and optionally
    /// to a column on it, counting from 0.
    pub fn goto_line(&mut self, line: usize, column: usize) {
        let text = self.text();
        let line = line.clamp(1, text.len_lines()) - 1;
        let pos = (text.line_start(line) + column).min(text.line_end(line));
        self.set_point(pos);
    }

    // Windows

    pub(crate) fn make_window(&mut self, buffer: BufferId) -> WindowId {
        let serial = self.next_serial;
        self.next_serial += 1;
        self.windows
            .insert(new_window(&mut self.buffers[buffer], buffer, serial))
    }

    pub(crate) fn remove_window(&mut self, id: WindowId) {
        if let Some(window) = self.windows.remove(id) {
            let buffer = &mut self.buffers[window.buffer];
            let (point, start) = (buffer.marker(window.point), buffer.marker(window.start));
            buffer.save_view(point, start);
            buffer.remove_marker(window.point);
            buffer.remove_marker(window.start);
        }
    }

    pub(crate) fn set_window_buffer(&mut self, id: WindowId, buffer: BufferId) {
        let window = &self.windows[id];
        if window.buffer == buffer {
            return;
        }
        let old = &mut self.buffers[window.buffer];
        let (point, start) = (old.marker(window.point), old.marker(window.start));
        old.save_view(point, start);
        old.remove_marker(window.point);
        old.remove_marker(window.start);

        let new = &mut self.buffers[buffer];
        let (point, start) = (new.saved_point(), new.saved_start());
        let (point, start) = (new.create_marker(point), new.create_marker(start));
        let window = &mut self.windows[id];
        window.buffer = buffer;
        window.point = point;
        window.start = start;
        window.goal_col = None;
    }

    /// Recomputes every window's area from the layout tree and frame size.
    pub(crate) fn relayout(&mut self) {
        let (cols, rows) = self.frame().size;
        let echo_rows = self.echo_rows().min(rows);
        let area = Rect {
            x: 0,
            y: 0,
            w: cols,
            h: rows - echo_rows,
        };
        let mut placed = Vec::new();
        self.frame().layout.arrange(area, false, &mut placed);
        for placement in placed {
            let window = &mut self.windows[placement.window];
            window.rect = placement.rect;
            window.border = placement.border;
        }
        let minibuffer_window = self.minibuffer_window();
        self.windows[minibuffer_window].rect = Rect {
            x: 0,
            y: rows - echo_rows,
            w: cols,
            h: echo_rows,
        };
        self.note_window_sizes();
    }

    /// Runs `window-size-change-hook` when the text area of a window of the
    /// frame is not the size it was, or shows another buffer.
    fn note_window_sizes(&mut self) {
        let mut changed = false;
        for id in self.frame().layout.leaves() {
            let window = &self.windows[id];
            let (_, width) = crate::redisplay::text_area(window, &self.buffers[window.buffer]);
            let now = (window.buffer, width, window.text_rows());
            if self.window_sizes.insert(id, now) != Some(now) {
                changed = true;
            }
        }
        let windows = &self.windows;
        self.window_sizes.retain(|id, _| windows.contains_key(*id));
        if changed {
            self.run_hook("window-size-change-hook", Vec::new());
        }
    }
}

fn new_window(buffer: &mut Buffer, id: BufferId, serial: u32) -> Window {
    let (point, start) = (buffer.saved_point(), buffer.saved_start());
    Window {
        buffer: id,
        point: buffer.create_marker(point),
        start: buffer.create_marker(start),
        goal_col: None,
        rect: Rect::default(),
        border: false,
        serial,
    }
}

/// What help shows of a command's documentation: its first sentence, on
/// one line. A sentence ends with a full stop before a space or at the end
/// of a line; documentation without one is shown whole.
pub fn doc_summary(doc: &str) -> String {
    let flat = doc.split_whitespace().collect::<Vec<_>>().join(" ");
    match flat.find(". ") {
        Some(end) => flat[..=end].to_owned(),
        None => flat,
    }
}

fn describe_keys(keys: &[Key]) -> String {
    keys.iter()
        .map(Key::to_string)
        .collect::<Vec<_>>()
        .join(" ")
}
