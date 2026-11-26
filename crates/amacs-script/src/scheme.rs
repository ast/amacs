//! The Scheme host: the interpreter with the editor's primitives in it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use amacs_core::{Editor, ScriptCall, ScriptFn, ScriptOutcome, ScriptTarget, Value};
use amacs_scheme::{Core, Error, Object, Outcome, Report, Rooted, Vm};

use crate::api::API;
use crate::host::{CurrentEditor, EvalError, EvalResult, InterruptHandle, current_editor};
use crate::runtime;

type Scheme = amacs_scheme::Value;

/// A Scheme value as the editor sees it.
fn from_scheme(core: &Core, value: Scheme) -> Result<Value, Error> {
    Ok(match value {
        Scheme::Void => Value::Void,
        Scheme::Nil => Value::List(Vec::new()),
        Scheme::Bool(b) => Value::Bool(b),
        Scheme::Int(n) => Value::Int(n),
        Scheme::Float(n) => Value::Float(n),
        Scheme::Char(c) => Value::Str(c.to_string()),
        Scheme::Sym(name) => Value::Sym(core.symbols.name(name).to_owned()),
        _ => match core.object(value) {
            Some(Object::Str(text)) => Value::Str(text.clone()),
            Some(Object::Pair(..)) => {
                let items = core.list_to_vec(value).ok_or_else(|| {
                    Error::new(format!(
                        "the editor cannot use a list that does not end: {}",
                        core.write(value)
                    ))
                })?;
                Value::List(
                    items
                        .into_iter()
                        .map(|item| from_scheme(core, item))
                        .collect::<Result<_, _>>()?,
                )
            }
            Some(Object::Vector(items)) => Value::List(
                items
                    .iter()
                    .map(|item| from_scheme(core, *item))
                    .collect::<Result<_, _>>()?,
            ),
            _ if core.is_procedure(value) => Value::Func(ScriptFn(Rc::new(core.root(value)))),
            _ => {
                return Err(Error::new(format!(
                    "the editor cannot use this value: {}",
                    core.write(value)
                )));
            }
        },
    })
}

fn to_scheme(core: &mut Core, value: &Value) -> Scheme {
    match value {
        Value::Void => Scheme::Void,
        Value::Bool(b) => Scheme::Bool(*b),
        Value::Int(n) => Scheme::Int(*n),
        Value::Float(n) => Scheme::Float(*n),
        Value::Str(s) => core.string(s.as_str()),
        Value::Sym(s) => core.symbol(s),
        Value::List(items) => {
            let items: Vec<Scheme> = items.iter().map(|item| to_scheme(core, item)).collect();
            core.list(&items)
        }
        Value::Func(ScriptFn(func)) => func
            .downcast_ref::<Rooted>()
            .map(Rooted::value)
            // Not a procedure of this interpreter.
            .unwrap_or(Scheme::Void),
    }
}

fn eval_error(report: &Report, located: bool) -> EvalError {
    EvalError {
        summary: if located {
            report.summary()
        } else {
            report.message.clone()
        },
        report: report.to_string(),
        message: report.message.clone(),
        location: report.location.as_ref().map(ToString::to_string),
        trace: report
            .trace
            .iter()
            .map(|(name, location)| (name.clone(), location.as_ref().map(ToString::to_string)))
            .collect(),
    }
}

/// Code that waits for a promise.
struct Waiting {
    fiber: Rooted,
    /// Source text to evaluate when the code is done, and its name: the
    /// forms that came after the one that waits.
    then: Option<(String, String)>,
}

/// How running some code went.
enum Ran {
    Done(Scheme),
    /// It waits for this promise.
    Waiting(i64),
    /// It was left because what it waited for will not come.
    Abandoned,
}

pub struct SchemeHost {
    vm: Vm,
    running: Arc<AtomicBool>,
    /// Code that waits for a promise, by the promise's number.
    waiting: Rc<RefCell<HashMap<i64, Waiting>>>,
    /// What code has written since it was last taken.
    output: String,
}

impl SchemeHost {
    /// Creates an interpreter with the editor primitives and the Scheme
    /// runtime loaded. Loading the runtime runs Scheme code that calls
    /// primitives, so it needs the editor.
    pub fn new(editor: &Rc<RefCell<Editor>>) -> Result<Self, EvalError> {
        let mut vm = Vm::new();
        for api in API {
            vm.core.define_native(
                api.name,
                api.min_args,
                api.max_args,
                api.doc,
                move |core, args| {
                    let args: Vec<Value> = args
                        .iter()
                        .map(|arg| from_scheme(core, *arg))
                        .collect::<Result<_, _>>()?;
                    let editor = current_editor().ok_or_else(|| {
                        Error::new(format!("{}: no editor is attached", api.name))
                    })?;
                    let mut editor = editor
                        .try_borrow_mut()
                        .map_err(|_| Error::new(format!("{}: the editor is busy", api.name)))?;
                    // A program does nothing in a buffer that is secret
                    // but leave it or ask what it is.
                    if !PROGRAM_MAY_IN_SECRET.contains(&api.name) {
                        let current = editor.window().buffer;
                        editor
                            .keep_from_program(current)
                            .map_err(|error| Error::new(format!("{}: {error}", api.name)))?;
                    }
                    let result = (api.run)(&mut editor, &args)
                        .map_err(|error| Error::new(format!("{}: {error}", api.name)))?;
                    Ok(to_scheme(core, &result))
                },
            );
        }
        vm.core.define_native(
            "format-current-time",
            1,
            Some(1),
            "(format-current-time format) The local time now, written as the strftime directives in format say.",
            |core, args| {
                let Some(format) = core.as_str(args[0]) else {
                    return Err(Error::new("format-current-time: expected a string"));
                };
                let formatted = jiff::fmt::strtime::format(format, &jiff::Zoned::now())
                    .map_err(|error| Error::new(format!("format-current-time: {error}")))?;
                Ok(core.string(formatted))
            },
        );
        vm.core.define_native(
            "format-time",
            2,
            Some(2),
            "(format-time seconds format) A time, given in seconds since 1970, written in local time as the strftime directives in format say.",
            |core, args| {
                let (Scheme::Int(seconds), Some(format)) = (args[0], core.as_str(args[1])) else {
                    return Err(Error::new("format-time: expected seconds and a format string"));
                };
                let time = jiff::Timestamp::from_second(seconds)
                    .map_err(|error| Error::new(format!("format-time: {error}")))?
                    .to_zoned(jiff::tz::TimeZone::system());
                let formatted = jiff::fmt::strtime::format(format, &time)
                    .map_err(|error| Error::new(format!("format-time: {error}")))?;
                Ok(core.string(formatted))
            },
        );
        let waiting: Rc<RefCell<HashMap<i64, Waiting>>> = Rc::default();
        let count = waiting.clone();
        vm.core.define_native(
            "%waiting-count",
            0,
            Some(0),
            "How many pieces of code are waiting for a promise.",
            move |_, _| Ok(Scheme::Int(count.borrow().len() as i64)),
        );
        let mut host = Self {
            vm,
            running: Arc::default(),
            waiting,
            output: String::new(),
        };
        for (name, source) in runtime::sources() {
            host.eval_source(editor, &source, &format!("runtime/{name}"))
                .map_err(|report| EvalError {
                    summary: format!("runtime/{name}: {}", report.message),
                    ..eval_error(&report, true)
                })?;
        }
        Ok(host)
    }

    pub fn take_output(&mut self) -> String {
        std::mem::take(&mut self.output)
    }

    pub fn interrupt_handle(&self) -> InterruptHandle {
        let flag = self.vm.interrupt_handle();
        InterruptHandle::new(self.running.clone(), move || {
            flag.store(true, Ordering::SeqCst);
        })
    }

    /// Runs `body` with the editor as the target of primitives and the
    /// interpreter marked as running, and deals with what it leaves.
    fn guarded(
        &mut self,
        editor: &Rc<RefCell<Editor>>,
        body: impl FnOnce(&mut Vm) -> Result<Outcome, Report>,
    ) -> Result<Ran, Report> {
        let outcome = {
            let _current = CurrentEditor::enter(editor);
            // An interrupt that arrived just after the previous run ended
            // must not stop this one.
            self.vm.interrupt_handle().store(false, Ordering::SeqCst);
            self.running.store(true, Ordering::SeqCst);
            let outcome = body(&mut self.vm);
            self.running.store(false, Ordering::SeqCst);
            outcome
        };
        // What the code wrote with `display` goes to the message log.
        let written = std::mem::take(&mut self.vm.core.output);
        if !written.is_empty() {
            editor.borrow_mut().log(written.trim_end_matches('\n'));
            self.output.push_str(&written);
            // Nobody may ever ask for it.
            if self.output.len() > 1 << 20 {
                let keep = self.output.len() - (1 << 19);
                let cut = (keep..self.output.len())
                    .find(|at| self.output.is_char_boundary(*at))
                    .unwrap_or(self.output.len());
                self.output.drain(..cut);
            }
        }
        match outcome {
            Ok(Outcome::Done(value)) => Ok(Ran::Done(value)),
            Ok(Outcome::Suspended { fiber, value }) => match value {
                Scheme::Int(promise) => {
                    self.waiting
                        .borrow_mut()
                        .insert(promise, Waiting { fiber, then: None });
                    Ok(Ran::Waiting(promise))
                }
                other => Err(Report {
                    message: format!(
                        "await: expected a promise, got {}",
                        self.vm.core.write(other)
                    ),
                    location: None,
                    trace: Vec::new(),
                    escape: None,
                }),
            },
            // The code was waiting for something that will not come, as
            // when the user quits a prompt: it has been left, in order.
            Err(report) if report.escape.as_deref() == Some("quit") => Ok(Ran::Abandoned),
            Err(report) => Err(report),
        }
    }

    /// Evaluates source text. The value is that of the last form that has
    /// one. If a form waits for something, the forms after it run when
    /// that has come, and the value is none.
    fn eval_source(
        &mut self,
        editor: &Rc<RefCell<Editor>>,
        source: &str,
        name: &str,
    ) -> Result<Scheme, Report> {
        let mut stopped = source.len();
        let ran = self.guarded(editor, |vm| {
            let mut last = Ok(Outcome::Done(Scheme::Void));
            vm.eval_each(source, name, |result, after| {
                let go_on = matches!(result, Ok(Outcome::Done(_)));
                stopped = after;
                match result {
                    // A form without a value leaves the last one standing.
                    Ok(Outcome::Done(Scheme::Void)) => {}
                    other => last = other,
                }
                go_on
            });
            last
        })?;
        match ran {
            Ran::Done(value) => Ok(value),
            Ran::Abandoned => Ok(Scheme::Void),
            Ran::Waiting(promise) => {
                let rest = &source[stopped.min(source.len())..];
                if !rest.trim().is_empty()
                    && let Some(waiting) = self.waiting.borrow_mut().get_mut(&promise)
                {
                    // What has run is blanked out rather than cut off, so
                    // that errors in the rest are placed where they are.
                    let done: String = source[..stopped]
                        .chars()
                        .map(|c| if c == '\n' { '\n' } else { ' ' })
                        .collect();
                    waiting.then = Some((format!("{done}{rest}"), name.to_owned()));
                }
                Ok(Scheme::Void)
            }
        }
    }

    fn printed(&self, value: Scheme) -> String {
        match value {
            Scheme::Void => String::new(),
            value => self.vm.core.write(value),
        }
    }

    /// Evaluates source text at top level. Returns the printed form of the
    /// last value, or an empty string if there is none.
    pub fn eval(
        &mut self,
        editor: &Rc<RefCell<Editor>>,
        source: &str,
        path: Option<&Path>,
    ) -> EvalResult {
        let name = path.map_or_else(|| "eval".to_owned(), |path| path.display().to_string());
        match self.eval_source(editor, source, &name) {
            Ok(value) => Ok(self.printed(value)),
            Err(report) => Err(eval_error(&report, path.is_some())),
        }
    }

    /// Evaluates source text at top level and converts the last value.
    pub fn eval_value(
        &mut self,
        editor: &Rc<RefCell<Editor>>,
        source: &str,
    ) -> Result<Value, EvalError> {
        let value = self
            .eval_source(editor, source, "eval")
            .map_err(|report| eval_error(&report, false))?;
        from_scheme(&self.vm.core, value).map_err(|error| EvalError::plain(error.message))
    }

    /// Runs a call that the editor queued.
    pub fn run(&mut self, editor: &Rc<RefCell<Editor>>, call: &ScriptCall) -> ScriptOutcome {
        let result = match &call.target {
            ScriptTarget::Eval(source) => self.eval(editor, source, None),
            ScriptTarget::Func(func) => {
                let procedure = to_scheme(&mut self.vm.core, &Value::Func(func.clone()));
                self.call(editor, procedure, &call.args)
            }
            // The editor settles promises by these two names.
            ScriptTarget::Named(name) if *name == "%promise-resolve" => {
                self.settle(editor, &call.args, true)
            }
            ScriptTarget::Named(name) if *name == "%promise-reject" => {
                self.settle(editor, &call.args, false)
            }
            ScriptTarget::Named(name) => {
                let symbol = self.vm.core.symbols.intern(name);
                match self.vm.core.lookup(symbol) {
                    Some(procedure) => self.call(editor, procedure, &call.args),
                    None => Err(EvalError::plain(format!("unbound variable: {name}"))),
                }
            }
        };
        match result {
            Ok(printed) => ScriptOutcome::Returned(printed),
            Err(error) => ScriptOutcome::Failed {
                summary: error.summary,
                report: error.report,
            },
        }
    }

    fn call(
        &mut self,
        editor: &Rc<RefCell<Editor>>,
        procedure: Scheme,
        args: &[Value],
    ) -> EvalResult {
        // The arguments are kept from the collector by being rooted until
        // the call has them.
        let args: Vec<Rooted> = args
            .iter()
            .map(|arg| {
                let value = to_scheme(&mut self.vm.core, arg);
                self.vm.core.root(value)
            })
            .collect();
        let values: Vec<Scheme> = args.iter().map(Rooted::value).collect();
        match self.guarded(editor, |vm| vm.call(procedure, &values)) {
            Ok(Ran::Done(value)) => Ok(self.printed(value)),
            Ok(Ran::Waiting(_) | Ran::Abandoned) => Ok(String::new()),
            Err(report) => Err(eval_error(&report, false)),
        }
    }

    /// Continues the code that waits for a promise: with the promise's
    /// value, or, if the promise was rejected, by making it leave.
    fn settle(
        &mut self,
        editor: &Rc<RefCell<Editor>>,
        args: &[Value],
        resolved: bool,
    ) -> EvalResult {
        let Some(Value::Int(promise)) = args.first() else {
            return Ok(String::new());
        };
        let Some(Waiting { fiber, then }) = self.waiting.borrow_mut().remove(promise) else {
            return Ok(String::new());
        };
        let value = if resolved {
            to_scheme(&mut self.vm.core, args.get(1).unwrap_or(&Value::Void))
        } else {
            let quit = self.vm.core.symbol("quit");
            self.vm.core.alloc(Object::Escape(quit, Scheme::Void))
        };
        let ran = self
            .guarded(editor, |vm| vm.resume(&fiber, value))
            .map_err(|report| eval_error(&report, false))?;
        match (ran, then) {
            // It waits again, and what is to follow waits with it.
            (Ran::Waiting(next), then) => {
                if let Some(waiting) = self.waiting.borrow_mut().get_mut(&next) {
                    waiting.then = then;
                }
                Ok(String::new())
            }
            (Ran::Done(_), Some((source, name))) => self
                .eval_source(editor, &source, &name)
                .map(|value| self.printed(value))
                .map_err(|report| eval_error(&report, false)),
            (Ran::Done(value), None) => Ok(self.printed(value)),
            (Ran::Abandoned, _) => Ok(String::new()),
        }
    }
}

/// The primitives that a program may call while the current buffer is a
/// secret one: those that leave the buffer, and those that say what the
/// buffer is and nothing of what is in it. A primitive that is not here
/// is refused there, so that one that is added later is safe without
/// being thought of.
const PROGRAM_MAY_IN_SECRET: [&str; 22] = [
    "%set-buffer",
    "switch-to-buffer",
    "kill-buffer",
    "buffer-name",
    "buffer-list",
    "buffer-file-name",
    "buffer-modified?",
    "buffer-read-only?",
    "buffer-tick",
    "major-mode",
    "option",
    "%message",
    "selected-window",
    "window-list",
    "window-buffer",
    "select-window",
    "working-directory",
    "expand-file-name",
    "abbreviate-file-name",
    "file-exists?",
    "file-remote?",
    "command-names",
];
