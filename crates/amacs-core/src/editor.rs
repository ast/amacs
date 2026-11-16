use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;

use amacs_proto::{InputEvent, Key, KeyCode, Mods, NamedKey};
use amacs_text::{MarkerId, Text};
use slotmap::SlotMap;

use crate::buffer::{Buffer, BufferId};
use crate::commands;
use crate::error::{CmdError, CmdResult};
use crate::keymap::{Keymap, Lookup, lookup_in};
use crate::kill_ring::KillRing;
use crate::minibuffer::MinibufferState;
use crate::redisplay::RedisplayState;
use crate::script::{ScriptCall, ScriptFn, ScriptState, ScriptTarget, Value};
use crate::window::{Layout, Rect, Window, WindowId};

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

type KeyPrompt = Box<dyn FnOnce(&mut Editor, Key) -> CmdResult>;

const SCRATCH: &str = "*scratch*";

/// All editor state. It is driven by [`Editor::handle_event`] and observed
/// through [`Editor::redisplay`].
pub struct Editor {
    pub(crate) buffers: SlotMap<BufferId, Buffer>,
    /// Buffers in most-recently-selected order.
    pub(crate) buffer_order: Vec<BufferId>,
    pub(crate) windows: SlotMap<WindowId, Window>,
    pub(crate) layout: Layout,
    pub(crate) selected: WindowId,
    pub(crate) minibuffer_window: WindowId,
    pub(crate) minibuffer: Option<MinibufferState>,
    /// Text shown after the minibuffer input until the next key.
    pub(crate) minibuffer_hint: Option<String>,
    /// A question waiting for a single key, with its prompt.
    pub(crate) key_prompt: Option<(String, KeyPrompt)>,
    pub(crate) size: (u16, u16),

    commands: HashMap<String, Command>,
    /// Keymaps by name: [`GLOBAL_MAP`], [`MINIBUFFER_MAP`], and one per mode,
    /// named after the mode.
    keymaps: HashMap<String, Keymap>,
    pub(crate) script: ScriptState,
    pending_keys: Vec<Key>,
    esc_pending: bool,
    /// Prefix argument for the next command.
    pub(crate) prefix_arg: Option<Prefix>,
    /// Prefix argument of the command that is running.
    current_prefix: Option<Prefix>,
    pub(crate) last_key: Option<Key>,
    this_command: Option<String>,
    last_command: Option<String>,

    /// Directory that file prompts start in for buffers without a file.
    pub(crate) default_directory: PathBuf,
    pub(crate) kill_ring: KillRing,
    pub(crate) echo: Option<String>,
    /// Set by vertical motion commands to keep the goal column.
    pub(crate) keep_goal: bool,
    exit: bool,
    next_serial: u32,
    pub(crate) redisplay: RedisplayState,
}

impl Editor {
    /// Creates an editor with a `*scratch*` buffer in a frame of the given size.
    pub fn new(cols: u16, rows: u16) -> Self {
        let mut buffers = SlotMap::with_key();
        let scratch = buffers.insert(Buffer::new(SCRATCH));
        let minibuffer = buffers.insert(Buffer::new(" *Minibuf*"));
        let mut windows = SlotMap::with_key();
        let selected = windows.insert(new_window(&mut buffers[scratch], scratch, 1));
        let minibuffer_window = windows.insert(new_window(&mut buffers[minibuffer], minibuffer, 0));

        let mut editor = Self {
            buffers,
            buffer_order: vec![scratch],
            windows,
            layout: Layout::Leaf(selected),
            selected,
            minibuffer_window,
            minibuffer: None,
            minibuffer_hint: None,
            key_prompt: None,
            size: (cols, rows),
            commands: HashMap::new(),
            keymaps: HashMap::new(),
            script: ScriptState::new(),
            pending_keys: Vec::new(),
            esc_pending: false,
            prefix_arg: None,
            current_prefix: None,
            last_key: None,
            this_command: None,
            last_command: None,
            default_directory: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
            kill_ring: KillRing::default(),
            echo: None,
            keep_goal: false,
            exit: false,
            next_serial: 2,
            redisplay: RedisplayState::default(),
        };
        commands::install(&mut editor);
        editor.relayout();
        editor
    }

    // Commands

    /// Defines or redefines a command. Keys bound to its name run the new
    /// definition.
    pub fn define_command(&mut self, name: &str, doc: &str, body: CommandBody) {
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
        self.current_prefix = self.prefix_arg.take();
        self.this_command = Some(name.to_owned());
        self.keep_goal = false;
        let run = match body {
            CommandBody::Native(run) => run,
            CommandBody::Script(func) => {
                // The host runs the body and then finishes the command.
                let queued = self.queue_script(ScriptCall {
                    target: ScriptTarget::Func(func),
                    args: Vec::new(),
                    command: Some(name.to_owned()),
                    echo_result: false,
                });
                self.report(queued);
                return;
            }
        };
        let buffer = self.window().buffer;
        let tick = self.buffers[buffer].tick();
        let point = self.point();
        self.buffer_mut().begin_command(point);

        let result = run(self);

        self.finish_command(buffer, tick, true);
        self.report(result);
    }

    /// The bookkeeping after a command or script call. `is_command` is false
    /// for script callbacks, which must not count as the user's last command.
    pub(crate) fn finish_command(&mut self, buffer: BufferId, tick: u64, is_command: bool) {
        const SELF_INSERT: &str = "self-insert-command";
        let amalgamate = self.this_command.as_deref() == Some(SELF_INSERT)
            && self.last_command.as_deref() == Some(SELF_INSERT);
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
        if let Some(buf) = self.buffers.get_mut(buffer)
            && buf.tick() != tick
        {
            buf.mark_active = false;
        }
        if !self.keep_goal {
            self.window_mut().goal_col = None;
        }
        self.kill_ring.end_command();
        // A command run from within another, as with M-x, has already been
        // recorded as the last command.
        if let Some(name) = self.this_command.take() {
            self.last_command = Some(name);
        }
        self.current_prefix = None;
        if is_command {
            self.run_hook("post-command-hook", Vec::new());
        }
    }

    fn report(&mut self, result: CmdResult) {
        if let Err(error) = result {
            self.message(error.to_string());
        }
    }

    // Input

    pub fn handle_event(&mut self, event: InputEvent) {
        match event {
            InputEvent::Key(key) => self.handle_key(key),
            InputEvent::Resize { cols, rows } => self.resize(cols, rows),
            InputEvent::Paste(text) => self.handle_paste(&text),
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.size = (cols, rows);
        self.relayout();
        self.redisplay.invalidate();
    }

    pub fn handle_key(&mut self, key: Key) {
        self.relayout();
        self.echo = None;
        self.minibuffer_hint = None;

        if let Some((_, answer)) = self.key_prompt.take() {
            let result = answer(self, key);
            self.report(result);
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
                if self.prefix_arg.is_some() && self.echo.is_none() {
                    self.show_pending();
                }
            }
            Some(None) => self.show_pending(),
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

    fn handle_paste(&mut self, text: &str) {
        self.relayout();
        self.echo = None;
        self.minibuffer_hint = None;
        self.cancel_pending();
        self.key_prompt = None;
        self.execute_with("yank", |ed| ed.insert(text));
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

    fn cancel_pending(&mut self) {
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
        self.echo = Some(format!("{}-", parts.join(" ")));
    }

    /// Asks a question that is answered with a single key.
    pub fn ask_key(
        &mut self,
        prompt: impl Into<String>,
        answer: impl FnOnce(&mut Editor, Key) -> CmdResult + 'static,
    ) {
        self.key_prompt = Some((prompt.into(), Box::new(answer)));
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
    }

    // Messages and exit

    pub fn message(&mut self, text: impl Into<String>) {
        self.echo = Some(text.into());
    }

    pub fn echo(&self) -> Option<&str> {
        self.echo.as_deref()
    }

    pub fn request_exit(&mut self) {
        self.exit = true;
    }

    pub fn should_exit(&self) -> bool {
        self.exit
    }

    // Current window and buffer

    pub fn in_minibuffer(&self) -> bool {
        self.selected == self.minibuffer_window
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
        &self.windows[self.selected]
    }

    pub fn window_mut(&mut self) -> &mut Window {
        &mut self.windows[self.selected]
    }

    pub fn buffer(&self) -> &Buffer {
        &self.buffers[self.window().buffer]
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

    /// Puts text on the kill ring as a new entry.
    pub fn kill_new(&mut self, text: String) {
        self.kill_ring.push(text);
    }

    /// The newest kill ring entry.
    pub fn current_kill(&mut self) -> Option<String> {
        self.kill_ring.current().map(str::to_owned)
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
        self.set_window_buffer(self.selected, id);
        self.buffer_order.retain(|other| *other != id);
        self.buffer_order.insert(0, id);
    }

    /// The most recently selected buffer other than the current one.
    pub fn other_buffer(&self) -> Option<BufferId> {
        let current = self.window().buffer;
        self.buffer_ids().find(|id| *id != current)
    }

    /// Removes a buffer. Windows showing it switch to another buffer.
    pub fn kill_buffer(&mut self, id: BufferId) {
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
        self.buffers.remove(id);
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
        let (cols, rows) = self.size;
        let area = Rect {
            x: 0,
            y: 0,
            w: cols,
            h: rows.saturating_sub(1),
        };
        let mut placed = Vec::new();
        self.layout.arrange(area, false, &mut placed);
        for placement in placed {
            let window = &mut self.windows[placement.window];
            window.rect = placement.rect;
            window.border = placement.border;
        }
        self.windows[self.minibuffer_window].rect = Rect {
            x: 0,
            y: rows.saturating_sub(1),
            w: cols,
            h: rows.min(1),
        };
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

fn describe_keys(keys: &[Key]) -> String {
    keys.iter()
        .map(Key::to_string)
        .collect::<Vec<_>>()
        .join(" ")
}
