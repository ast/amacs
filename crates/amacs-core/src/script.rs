//! The core's side of the Scheme interface.
//!
//! The core never runs Scheme itself and does not know the interpreter.
//! When it wants Scheme code to run, such as a command defined in Scheme, a
//! hook, or a waiting `read-string`, it queues a [`ScriptCall`]. The script
//! host takes calls off the queue between input events and runs them while it
//! holds no borrow of the editor, so the Scheme code is free to call editor
//! primitives.

use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use amacs_proto::SubpixelOrder;

use crate::buffer::{BufferId, LineNumbers};
use crate::editor::Editor;
use crate::error::{CmdError, CmdResult};

/// A Scheme procedure, opaque to the core. Only the script host that created
/// it can call it.
#[derive(Clone)]
pub struct ScriptFn(pub Rc<dyn Any>);

impl fmt::Debug for ScriptFn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("#<procedure>")
    }
}

impl PartialEq for ScriptFn {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

/// A value passed between the core and Scheme.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Void,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Sym(String),
    List(Vec<Value>),
    Func(ScriptFn),
}

impl Value {
    pub fn sym(name: &str) -> Self {
        Value::Sym(name.to_owned())
    }

    pub fn str(text: impl Into<String>) -> Self {
        Value::Str(text.into())
    }

    /// The text of a string or symbol, for arguments that accept either.
    pub fn as_name(&self) -> Option<&str> {
        match self {
            Value::Str(s) | Value::Sym(s) => Some(s),
            _ => None,
        }
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    /// `None` becomes `#f`, as is conventional for a missing result.
    fn from(value: Option<T>) -> Self {
        value.map_or(Value::Bool(false), Into::into)
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Value::Bool(value)
    }
}

impl From<usize> for Value {
    fn from(value: usize) -> Self {
        Value::Int(value as i64)
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Value::Int(value)
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Value::Str(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Value::Str(value.to_owned())
    }
}

/// What a [`ScriptCall`] runs.
#[derive(Debug, Clone)]
pub enum ScriptTarget {
    Func(ScriptFn),
    /// A procedure of the Scheme runtime, looked up by its global name.
    Named(&'static str),
    /// Source text evaluated at top level.
    Eval(String),
}

/// A request for the script host to run Scheme code.
#[derive(Debug, Clone)]
pub struct ScriptCall {
    pub target: ScriptTarget,
    pub args: Vec<Value>,
    /// Set when the call is the body of this command, so that it gets a
    /// command's bookkeeping: undo boundary, mark deactivation, hooks.
    pub command: Option<String>,
    /// Show the result in the echo area, as `eval-expression` does.
    pub echo_result: bool,
    /// The frame to run the call in, if it matters: code that waited goes
    /// on in the frame it was started from, whichever is current by then.
    pub frame: Option<crate::frame::FrameId>,
}

/// How a script call ended, as reported by the host.
pub enum ScriptOutcome {
    /// The printed form of the result.
    Returned(String),
    Failed {
        /// One line for the echo area.
        summary: String,
        /// The full report, for the `*Messages*` buffer.
        report: String,
    },
}

pub(crate) struct ModeDef {
    pub parent: Option<String>,
    /// Name shown in the mode line.
    pub pretty: String,
}

struct OptionDef {
    default: Value,
    doc: String,
}

/// Script-related editor state.
#[derive(Default)]
pub(crate) struct ScriptState {
    /// Whether a host is attached to run queued calls.
    attached: bool,
    queue: std::collections::VecDeque<ScriptCall>,
    /// Buffer and its tick when the running call started.
    frame: Option<(BufferId, u64)>,
    next_promise: u64,
    /// The frame each promise was made in.
    promise_frames: HashMap<u64, crate::frame::FrameId>,
    /// The frame to go back to when the running call is done.
    pub(crate) restore_frame: Option<crate::frame::FrameId>,
    /// The command that the running call is part of, also when the call
    /// is code that waited and goes on.
    running_command: Option<String>,
    /// Promises that commands wait for, with the command: the code that
    /// goes on when one is settled is still that command.
    command_promises: HashMap<u64, String>,
    /// Those of them that are for a program to finish. While there are
    /// any, what is typed waits: it was typed for what the command will
    /// show or ask when the program is done.
    awaited_programs: std::collections::HashSet<u64>,
    /// What was typed meanwhile, with the frame it was typed in.
    held_input: std::collections::VecDeque<(crate::frame::FrameId, amacs_proto::InputEvent)>,
    /// When to say that what is typed is waiting.
    held_notice_due: Option<std::time::Instant>,
    /// The prefix argument of a command in Scheme that is queued, with the
    /// command's name: the command that ran it, as M-x does, is finished
    /// before it runs, and takes the prefix argument in force with it.
    pub(crate) queued_prefix: Option<(String, crate::editor::Prefix)>,
    /// Hooks that have functions, so that others cost nothing to run.
    active_hooks: std::collections::HashSet<String>,
    options: HashMap<String, OptionDef>,
    option_values: HashMap<String, Value>,
    pub modes: HashMap<String, ModeDef>,
    pub global_minor_modes: Vec<String>,
}

pub const FUNDAMENTAL_MODE: &str = "fundamental-mode";
const TAB_WIDTH: &str = "tab-width";
pub(crate) const LARGE_FILE_THRESHOLD: &str = "large-file-threshold";
const FONT_FAMILY: &str = "font-family";
const FONT_SIZE: &str = "font-size";
const SUBPIXEL_ORDER: &str = "subpixel-order";
const GPU_RENDERER: &str = "gpu-renderer";
const LINE_NUMBERS: &str = "line-numbers";
const INDENT_OFFSET: &str = "indent-offset";
const INDENT_TABS: &str = "indent-tabs-mode";
const MESSAGES: &str = "*Messages*";

/// What a value of the `line-numbers` option asks for.
fn line_numbers(value: &Value) -> Option<LineNumbers> {
    match value {
        Value::Bool(false) => Some(LineNumbers::Off),
        Value::Bool(true) => Some(LineNumbers::Absolute),
        _ => match value.as_name()? {
            "absolute" => Some(LineNumbers::Absolute),
            "relative" => Some(LineNumbers::Relative),
            _ => None,
        },
    }
}

impl ScriptState {
    pub fn new() -> Self {
        let mut state = Self::default();
        state.modes.insert(
            FUNDAMENTAL_MODE.to_owned(),
            ModeDef {
                parent: None,
                pretty: "Fundamental".to_owned(),
            },
        );
        state.options.insert(
            TAB_WIDTH.to_owned(),
            OptionDef {
                default: Value::Int(8),
                doc: "Distance between tab stops, in columns.".to_owned(),
            },
        );
        state.options.insert(
            LINE_NUMBERS.to_owned(),
            OptionDef {
                default: Value::Bool(false),
                doc: "Line numbers beside the text: #f, 'absolute or 'relative.".to_owned(),
            },
        );
        state.options.insert(
            INDENT_OFFSET.to_owned(),
            OptionDef {
                default: Value::Int(4),
                doc: "Columns per level of indentation.".to_owned(),
            },
        );
        state.options.insert(
            INDENT_TABS.to_owned(),
            OptionDef {
                default: Value::Bool(false),
                doc: "Whether indentation is made of tabs rather than spaces.".to_owned(),
            },
        );
        for (name, default, doc) in [
            (
                "use-direnv",
                Value::Bool(true),
                "Whether programs run for a file get the environment of its .envrc.",
            ),
            (
                "direnv-program",
                Value::str("direnv"),
                "The direnv program.",
            ),
            (
                LARGE_FILE_THRESHOLD,
                Value::Int(50),
                "The size in megabytes above which a file is read only after a question, since reading it holds the editor up; #f for no limit.",
            ),
            (
                "formatter",
                Value::Bool(false),
                "Name of the formatter for the buffer's text, or #f.",
            ),
            (
                "format-on-save",
                Value::Bool(false),
                "Whether saving a buffer formats it first.",
            ),
            (
                "project-root-markers",
                Value::List(vec![Value::str(".git"), Value::str(".projectile")]),
                "Names of the files and directories that mark the root of a project. The nearest one above a file decides.",
            ),
            (
                "auto-revert",
                Value::Bool(true),
                "Whether a buffer without changes is read again when its file changes on disk.",
            ),
            (
                crate::spell::LANGUAGES,
                Value::List(vec![Value::str("en_US")]),
                "The languages that spelling is checked in: a list of names of Hunspell dictionaries, such as \"en_US\" and \"sv_SE\". A word is right if one of them knows it. (spell-dictionaries) lists those there are.",
            ),
            (
                crate::spell::DICTIONARY_PATH,
                Value::List(Vec::new()),
                "The directories that dictionaries are looked for in, each an .aff and a .dic file named by its language. When empty, they are those of DICPATH and then hunspell and myspell/dicts in the system's data directories.",
            ),
            (
                crate::spell::ONLY_IN,
                Value::Bool(false),
                "The kinds of syntax that spelling is checked in, a list of names of faces such as (\"comment\" \"doc\" \"string\"), or #f for all of the text. Code modes set it for their buffers.",
            ),
            (
                crate::spell::NOT_IN,
                Value::List(vec![
                    Value::str("code"),
                    Value::str("link"),
                    Value::str("escape"),
                ]),
                "The kinds of syntax that spelling is not checked in when spell-only-in is #f: a list of names of faces.",
            ),
            (
                "whitespace-line-column",
                Value::Int(80),
                "The column that whitespace-mode marks text beyond.",
            ),
            (
                "lsp-idle-delay",
                Value::Int(300),
                "Milliseconds point rests before what is under it is described; negative for never.",
            ),
            (
                "completion-delay",
                Value::Int(100),
                "Milliseconds of rest after typing before completions appear; negative for never.",
            ),
            (
                "completion-min-prefix",
                Value::Int(2),
                "Chars of a word that must be typed before completions appear.",
            ),
        ] {
            state.options.insert(
                name.to_owned(),
                OptionDef {
                    default,
                    doc: doc.to_owned(),
                },
            );
        }
        state.options.insert(
            FONT_FAMILY.to_owned(),
            OptionDef {
                default: Value::str("monospace"),
                doc: "Font family for graphical frames.".to_owned(),
            },
        );
        state.options.insert(
            FONT_SIZE.to_owned(),
            OptionDef {
                default: Value::Int(13),
                doc: "Font size for graphical frames, in pixels.".to_owned(),
            },
        );
        state.options.insert(
            SUBPIXEL_ORDER.to_owned(),
            OptionDef {
                default: Value::str("none"),
                doc: "How graphical frames draw text: \"none\" in shades of grey; \
                      \"rgb\" or \"bgr\", the order of the parts of the screen's \
                      pixels from left to right, for each part by itself, which is \
                      three times the resolution across and sharper on a screen of \
                      low density."
                    .to_owned(),
            },
        );
        state.options.insert(
            GPU_RENDERER.to_owned(),
            OptionDef {
                default: Value::Bool(false),
                doc: "Whether graphical frames draw with the GPU, through Vulkan, \
                      and not on the CPU. Read when a window opens; `--renderer` \
                      and AMACS_RENDERER say otherwise for one window, and a \
                      window draws on the CPU after all where there is no Vulkan."
                    .to_owned(),
            },
        );
        state
    }
}

impl Editor {
    // The call queue

    /// Declares that a script host will run queued calls.
    pub fn attach_script_host(&mut self) {
        self.script.attached = true;
    }

    pub fn has_script_host(&self) -> bool {
        self.script.attached
    }

    pub(crate) fn queue_script(&mut self, call: ScriptCall) -> CmdResult {
        if !self.script.attached {
            return Err(CmdError::msg("Scheme is not available in this session"));
        }
        self.script.queue.push_back(call);
        Ok(())
    }

    /// Queues a call to a procedure of the Scheme runtime.
    pub fn queue_named(&mut self, name: &'static str, args: Vec<Value>) {
        // Without a host there is nobody to call, which is not an error.
        let _ = self.queue_script(ScriptCall {
            target: ScriptTarget::Named(name),
            args,
            command: None,
            echo_result: false,
            frame: None,
        });
    }

    /// Queues source text for evaluation at top level.
    pub fn queue_eval(&mut self, source: impl Into<String>, echo_result: bool) -> CmdResult {
        self.queue_script(ScriptCall {
            target: ScriptTarget::Eval(source.into()),
            args: Vec::new(),
            command: None,
            echo_result,
            frame: None,
        })
    }

    /// The next call for the host to run. The host must call
    /// [`Editor::begin_script`] before running it and [`Editor::end_script`]
    /// after.
    pub fn take_script_call(&mut self) -> Option<ScriptCall> {
        let call = self.script.queue.pop_front()?;
        if let Some(command) = &call.command
            && let Some((_, prefix)) = self
                .script
                .queued_prefix
                .take_if(|(name, _)| name == command)
        {
            self.set_current_prefix(prefix);
        }
        // Code that goes on after waiting is part of the command that
        // waited.
        let settled = match (&call.target, call.args.first()) {
            (ScriptTarget::Named("%promise-resolve" | "%promise-reject"), Some(Value::Int(id))) => {
                let id = *id as u64;
                self.script.awaited_programs.remove(&id);
                self.script.command_promises.remove(&id)
            }
            _ => None,
        };
        self.script.running_command = call.command.clone().or(settled);
        if let Some(frame) = call.frame
            && frame != self.current_frame()
            && self.has_frame(frame)
        {
            self.script.restore_frame = Some(self.current_frame());
            self.select_frame(frame);
        }
        Some(call)
    }

    pub fn begin_script(&mut self) {
        let buffer = self.window().buffer;
        self.script.frame = Some((buffer, self.buffers[buffer].tick()));
        let point = self.point();
        self.buffer_mut().begin_command(point);
    }

    pub fn end_script(&mut self, call: &ScriptCall, outcome: ScriptOutcome) {
        self.script.running_command = None;
        if let Some((buffer, tick)) = self.script.frame.take() {
            self.finish_command(buffer, tick, call.command.is_some());
        }
        if let Some(frame) = self.script.restore_frame.take()
            && self.has_frame(frame)
        {
            self.select_frame(frame);
        }
        match outcome {
            ScriptOutcome::Returned(printed) if call.echo_result => self.message(printed),
            ScriptOutcome::Returned(_) => {}
            ScriptOutcome::Failed { summary, report } => {
                self.macro_stop();
                self.log(&report);
                self.message(summary);
            }
        }
    }

    // Promises

    /// Allocates an id that Scheme code can `await`.
    pub fn new_promise(&mut self) -> u64 {
        self.script.next_promise += 1;
        let frame = self.current_frame();
        self.script
            .promise_frames
            .insert(self.script.next_promise, frame);
        if let Some(command) = self.script.running_command.clone() {
            self.script
                .command_promises
                .insert(self.script.next_promise, command);
        }
        self.script.next_promise
    }

    // Typing ahead
    //
    // A command written in Scheme that runs a program gives up control
    // while the program runs. What is typed then is meant for what the
    // command does next, a question it asks or a buffer it shows, and so
    // it waits until the command gets there, as it would if the command
    // had blocked. C-g stops the waiting.

    /// Marks a promise as one for a program to finish, if a command made
    /// it: what is typed until it is settled waits.
    pub(crate) fn await_program(&mut self, promise: u64) {
        if self.script.command_promises.contains_key(&promise) {
            self.script.awaited_programs.insert(promise);
        }
    }

    /// Keeps what is typed waiting until a promise is settled, as if the
    /// named command had made it: for what a command of the core hands to
    /// Scheme to wait for.
    pub(crate) fn hold_input_for(&mut self, promise: u64, command: &str) {
        self.script
            .command_promises
            .entry(promise)
            .or_insert_with(|| command.to_owned());
        self.script.awaited_programs.insert(promise);
    }

    /// The running command no longer keeps what is typed waiting, whatever
    /// programs it goes on to run: for work that takes long and asks
    /// nothing after it.
    pub fn release_input(&mut self) {
        self.script.running_command = None;
    }

    pub(crate) fn holds_input(&self) -> bool {
        !self.script.awaited_programs.is_empty()
    }

    /// Keeps something typed until the command that is waiting for a
    /// program has gone on.
    pub(crate) fn hold_input(&mut self, event: amacs_proto::InputEvent) {
        let frame = self.current_frame();
        self.script.held_input.push_back((frame, event));
        if self.script.held_notice_due.is_none() {
            self.script.held_notice_due =
                Some(std::time::Instant::now() + std::time::Duration::from_secs(1));
        }
    }

    /// Gives up on the programs that commands wait for, as C-g asks: the
    /// commands are left the way quitting a prompt leaves them, and what
    /// was typed for them is dropped.
    pub(crate) fn stop_waiting(&mut self) {
        let awaited: Vec<u64> = self.script.awaited_programs.drain().collect();
        for promise in awaited {
            self.script.command_promises.remove(&promise);
            self.reject_promise(promise);
        }
        self.script.held_input.clear();
        self.script.held_notice_due = None;
        self.message("Quit");
    }

    /// The next thing that was typed while a command waited, once nothing
    /// waits any more and nothing is queued. The host handles it as if it
    /// had been typed now, and asks again.
    pub fn take_held_input(&mut self) -> Option<(crate::frame::FrameId, amacs_proto::InputEvent)> {
        if self.holds_input() || !self.script.queue.is_empty() {
            return None;
        }
        // The keys of a keyboard macro that runs come the same way, before
        // what was typed meanwhile.
        if let Some(key) = self.macro_take_key() {
            return Some((self.current_frame(), amacs_proto::InputEvent::Key(key)));
        }
        let held = self.script.held_input.pop_front();
        if self.script.held_input.is_empty() {
            self.script.held_notice_due = None;
        }
        held
    }

    pub(crate) fn held_notice_deadline(&self) -> Option<std::time::Instant> {
        self.script.held_notice_due
    }

    /// Says, when typing has waited a while, what it waits for.
    pub(crate) fn held_notice_poll(&mut self, now: std::time::Instant) {
        if self.script.held_notice_due.is_none_or(|due| due > now) {
            return;
        }
        self.script.held_notice_due = None;
        let command = self
            .script
            .awaited_programs
            .iter()
            .find_map(|promise| self.script.command_promises.get(promise))
            .cloned();
        if let Some(command) = command
            && !self.script.held_input.is_empty()
        {
            self.message(format!("Waiting for {command}; C-g stops waiting"));
        }
    }

    fn settle_promise(&mut self, name: &'static str, id: u64, mut args: Vec<Value>) {
        args.insert(0, Value::Int(id as i64));
        let frame = self.script.promise_frames.remove(&id);
        // Without a host there is nobody to call, which is not an error.
        let _ = self.queue_script(ScriptCall {
            target: ScriptTarget::Named(name),
            args,
            command: None,
            echo_result: false,
            frame,
        });
    }

    /// Resumes the Scheme code waiting on a promise with its value.
    pub fn resolve_promise(&mut self, id: u64, value: Value) {
        self.settle_promise("%promise-resolve", id, vec![value]);
    }

    /// Abandons the Scheme code waiting on a promise, as when the user quits
    /// a prompt.
    pub fn reject_promise(&mut self, id: u64) {
        self.settle_promise("%promise-reject", id, Vec::new());
    }

    // Hooks

    /// Records whether a hook has any functions. The functions themselves
    /// live in the Scheme runtime.
    pub fn set_hook_active(&mut self, name: &str, active: bool) {
        if active {
            self.script.active_hooks.insert(name.to_owned());
        } else {
            self.script.active_hooks.remove(name);
        }
    }

    pub(crate) fn hook_active(&self, name: &str) -> bool {
        self.script.active_hooks.contains(name)
    }

    /// Queues a run of a hook's functions, if it has any.
    pub fn run_hook(&mut self, name: &str, mut args: Vec<Value>) {
        if self.script.active_hooks.contains(name) {
            args.insert(0, Value::sym(name));
            self.queue_named("%run-hook", args);
        }
    }

    // Options

    /// Defines a user option. Redefining one keeps the value it has been set to.
    pub fn define_option(&mut self, name: &str, default: Value, doc: &str) {
        self.script.options.insert(
            name.to_owned(),
            OptionDef {
                default,
                doc: doc.to_owned(),
            },
        );
        self.apply_options();
    }

    pub fn option_doc(&self, name: &str) -> Option<&str> {
        self.script.options.get(name).map(|def| def.doc.as_str())
    }

    /// The value of an option in the current buffer: its buffer-local value,
    /// else the global value, else the default.
    pub fn option(&self, name: &str) -> CmdResult<Value> {
        self.option_in(self.window().buffer, name)
            .ok_or_else(|| CmdError::msg(format!("Unknown option: {name}")))
    }

    /// The value of an option in a given buffer.
    pub(crate) fn option_for(&self, buffer: BufferId, name: &str) -> Option<Value> {
        self.option_in(buffer, name)
    }

    fn option_in(&self, buffer: BufferId, name: &str) -> Option<Value> {
        let def = self.script.options.get(name)?;
        let value = self.buffers[buffer]
            .locals
            .get(name)
            .or_else(|| self.script.option_values.get(name))
            .unwrap_or(&def.default);
        Some(value.clone())
    }

    fn check_option(&self, name: &str, value: &Value) -> CmdResult {
        if !self.script.options.contains_key(name) {
            return Err(CmdError::msg(format!("Unknown option: {name}")));
        }
        if name == TAB_WIDTH && !matches!(value, Value::Int(1..=64)) {
            return Err(CmdError::msg("tab-width must be an integer from 1 to 64"));
        }
        if name == FONT_SIZE && !matches!(value, Value::Int(4..=200)) {
            return Err(CmdError::msg("font-size must be an integer from 4 to 200"));
        }
        if name == FONT_FAMILY && !matches!(value, Value::Str(_)) {
            return Err(CmdError::msg("font-family must be a string"));
        }
        if name == SUBPIXEL_ORDER
            && !matches!(value, Value::Str(order) if SubpixelOrder::parse(order).is_some())
        {
            return Err(CmdError::msg(
                "subpixel-order must be \"none\", \"rgb\" or \"bgr\"",
            ));
        }
        if name == GPU_RENDERER && !matches!(value, Value::Bool(_)) {
            return Err(CmdError::msg("gpu-renderer must be #t or #f"));
        }
        if name == INDENT_OFFSET && !matches!(value, Value::Int(1..=16)) {
            return Err(CmdError::msg(
                "indent-offset must be an integer from 1 to 16",
            ));
        }
        if name == LINE_NUMBERS && line_numbers(value).is_none() {
            return Err(CmdError::msg(
                "line-numbers must be #f, 'absolute or 'relative",
            ));
        }
        Ok(())
    }

    /// Sets an option's global value.
    pub fn set_option(&mut self, name: &str, value: Value) -> CmdResult {
        self.check_option(name, &value)?;
        self.script.option_values.insert(name.to_owned(), value);
        self.apply_options();
        Ok(())
    }

    /// Sets an option's value in the current buffer only.
    pub fn set_local_option(&mut self, name: &str, value: Value) -> CmdResult {
        self.check_option(name, &value)?;
        self.buffer_mut().locals.insert(name.to_owned(), value);
        self.apply_options();
        Ok(())
    }

    /// The configured font for graphical frames: family, size in pixels,
    /// and the order of subpixels to draw for.
    pub fn frame_font(&self) -> (String, u16, SubpixelOrder) {
        let family = match self.script.option_values.get(FONT_FAMILY) {
            Some(Value::Str(family)) => family.clone(),
            _ => "monospace".to_owned(),
        };
        let size = match self.script.option_values.get(FONT_SIZE) {
            Some(Value::Int(size)) => *size as u16,
            _ => 13,
        };
        let subpixel = match self.script.option_values.get(SUBPIXEL_ORDER) {
            Some(Value::Str(order)) => SubpixelOrder::parse(order).unwrap_or_default(),
            _ => SubpixelOrder::None,
        };
        (family, size, subpixel)
    }

    /// Whether graphical frames are to draw with the GPU, as configured.
    pub fn frame_gpu(&self) -> bool {
        matches!(
            self.script.option_values.get(GPU_RENDERER),
            Some(Value::Bool(true))
        )
    }

    /// Copies the options that the core itself uses into the buffers.
    pub(crate) fn apply_options(&mut self) {
        let ids: Vec<BufferId> = self.buffers.keys().collect();
        for id in ids {
            if let Some(Value::Int(width)) = self.option_in(id, TAB_WIDTH) {
                self.buffers[id].tab_width = width as usize;
            }
            if let Some(Value::Int(offset)) = self.option_in(id, INDENT_OFFSET) {
                self.buffers[id].indent_offset = offset as usize;
            }
            if let Some(value) = self.option_in(id, INDENT_TABS) {
                self.buffers[id].indent_tabs = value != Value::Bool(false);
            }
            if let Some(numbers) = self
                .option_in(id, LINE_NUMBERS)
                .and_then(|value| line_numbers(&value))
            {
                self.buffers[id].line_numbers = numbers;
            }
        }
    }

    // Modes

    pub fn define_mode(&mut self, name: &str, parent: Option<&str>, pretty: &str) {
        self.script.modes.insert(
            name.to_owned(),
            ModeDef {
                parent: parent.map(str::to_owned),
                pretty: pretty.to_owned(),
            },
        );
    }

    pub fn mode_parent(&self, name: &str) -> Option<&str> {
        self.script.modes.get(name)?.parent.as_deref()
    }

    fn check_mode(&self, name: &str) -> CmdResult {
        if self.script.modes.contains_key(name) {
            Ok(())
        } else {
            Err(CmdError::msg(format!("Unknown mode: {name}")))
        }
    }

    /// Makes `name` the current buffer's major mode. Buffer-local option
    /// values belong to the old mode and are dropped.
    pub fn set_major_mode(&mut self, name: &str) -> CmdResult {
        self.check_mode(name)?;
        let buffer = self.buffer_mut();
        buffer.major_mode = name.to_owned();
        buffer.locals.clear();
        buffer.set_language(None);
        self.apply_options();
        Ok(())
    }

    /// Turns a minor mode on or off, in the current buffer or globally.
    pub fn set_minor_mode(&mut self, name: &str, global: bool, enabled: bool) -> CmdResult {
        self.check_mode(name)?;
        let modes = if global {
            &mut self.script.global_minor_modes
        } else {
            &mut self.buffer_mut().minor_modes
        };
        modes.retain(|mode| mode != name);
        if enabled {
            modes.push(name.to_owned());
        }
        Ok(())
    }

    pub fn minor_mode_enabled(&self, name: &str) -> bool {
        self.script
            .global_minor_modes
            .iter()
            .any(|mode| mode == name)
            || self.buffer().minor_modes.iter().any(|mode| mode == name)
    }

    /// Names of the keymaps that apply in the current buffer, most specific
    /// first: minor modes (newest first), the major mode and its ancestors,
    /// then the global map.
    pub(crate) fn active_keymaps(&self) -> Vec<&str> {
        let buffer = self.buffer();
        let mut names: Vec<&str> = Vec::new();
        if self.in_minibuffer() {
            names.push(crate::editor::MINIBUFFER_MAP);
        }
        // What is in progress at point comes before any mode.
        if self.completion_active() {
            names.push(crate::complete::POPUP_MAP);
        }
        if self.snippet_active() {
            names.push(crate::snippet::SNIPPET_MAP);
        }
        if self.undo_view_active() {
            names.push(crate::undo_view::UNDO_MAP);
        }
        names.extend(buffer.minor_modes.iter().rev().map(String::as_str));
        names.extend(
            self.script
                .global_minor_modes
                .iter()
                .rev()
                .map(String::as_str),
        );
        let mut mode = Some(buffer.major_mode.as_str());
        // The depth limit guards against a cycle of parents.
        for _ in 0..16 {
            let Some(name) = mode else { break };
            names.push(name);
            mode = self.mode_parent(name);
        }
        names.push(crate::editor::GLOBAL_MAP);
        names
    }

    /// The text in parentheses on the mode line: the major mode and then the
    /// minor modes.
    pub(crate) fn mode_line_modes(&self, buffer: BufferId) -> String {
        let buffer = &self.buffers[buffer];
        let pretty = |name: &str| {
            self.script
                .modes
                .get(name)
                .map_or(name, |def| def.pretty.as_str())
                .to_owned()
        };
        std::iter::once(&buffer.major_mode)
            .chain(&buffer.minor_modes)
            .chain(&self.script.global_minor_modes)
            .map(|name| pretty(name))
            .filter(|pretty| !pretty.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    // The message log

    /// Appends text to the `*Messages*` buffer, creating it if needed.
    pub fn log(&mut self, text: &str) {
        let id = match self.find_buffer(MESSAGES) {
            Some(id) => id,
            None => {
                let mut buffer = crate::buffer::Buffer::new(MESSAGES);
                buffer.read_only = true;
                // Appending to the list keeps it out of the way of
                // `switch-to-buffer`'s default.
                self.add_buffer(buffer)
            }
        };
        self.buffers[id].append_log(text);
    }
}
