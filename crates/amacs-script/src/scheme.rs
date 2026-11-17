//! The backend built on amacs's own Scheme, `amacs-scheme`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use amacs_core::{Editor, ScriptCall, ScriptFn, ScriptOutcome, ScriptTarget, Value};
use amacs_scheme::{Core, Error, Object, Outcome, Report, Rooted, Vm};

use crate::api::API;
use crate::host::{CurrentEditor, Engine, EvalError, EvalResult, InterruptHandle, current_editor};
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
            // A procedure from some other engine cannot be called by this one.
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
    }
}

pub struct SchemeHost {
    vm: Vm,
    running: Arc<AtomicBool>,
    /// Code that waits for a promise, by the promise's number.
    waiting: Rc<RefCell<HashMap<i64, Rooted>>>,
    /// Whether the running code was called by the editor, as a command or
    /// a hook is, rather than evaluated as text. Only then can it wait.
    called: Rc<Cell<bool>>,
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
        let waiting: Rc<RefCell<HashMap<i64, Rooted>>> = Rc::default();
        let called: Rc<Cell<bool>> = Rc::default();
        let flag = called.clone();
        vm.core.define_native(
            "%can-wait?",
            0,
            Some(0),
            "Whether the running code can wait for a promise.",
            move |_, _| Ok(Scheme::Bool(flag.get())),
        );
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
            called,
        };
        for (name, source) in runtime::sources(Engine::Own) {
            host.eval_source(editor, &source, &format!("runtime/{name}"))
                .map_err(|report| EvalError {
                    summary: format!("runtime/{name}: {}", report.message),
                    report: report.to_string(),
                })?;
        }
        Ok(host)
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
    ) -> Result<Scheme, Report> {
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
        }
        match outcome {
            Ok(Outcome::Done(value)) => Ok(value),
            Ok(Outcome::Suspended { fiber, value }) => match value {
                Scheme::Int(promise) => {
                    self.waiting.borrow_mut().insert(promise, fiber);
                    Ok(Scheme::Void)
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
            Err(report) if report.escape.as_deref() == Some("quit") => Ok(Scheme::Void),
            Err(report) => Err(report),
        }
    }

    /// Evaluates source text. The value is that of the last form that has
    /// one.
    fn eval_source(
        &mut self,
        editor: &Rc<RefCell<Editor>>,
        source: &str,
        name: &str,
    ) -> Result<Scheme, Report> {
        // Evaluation inside a call, as by `eval-buffer` in a command, is
        // still part of that call.
        self.guarded(editor, |vm| {
            let mut last = Ok(Outcome::Done(Scheme::Void));
            vm.eval_each(source, name, |result| {
                let go_on = matches!(result, Ok(Outcome::Done(_)));
                match result {
                    // A form without a value leaves the last one standing.
                    Ok(Outcome::Done(Scheme::Void)) => {}
                    other => last = other,
                }
                go_on
            });
            last
        })
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
        from_scheme(&self.vm.core, value).map_err(|error| EvalError {
            summary: error.message.clone(),
            report: error.message,
        })
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
                    None => {
                        let summary = format!("unbound variable: {name}");
                        Err(EvalError {
                            report: summary.clone(),
                            summary,
                        })
                    }
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
        self.called.set(true);
        let result = self.guarded(editor, |vm| vm.call(procedure, &values));
        self.called.set(false);
        match result {
            Ok(value) => Ok(self.printed(value)),
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
        let Some(fiber) = self.waiting.borrow_mut().remove(promise) else {
            return Ok(String::new());
        };
        let value = if resolved {
            to_scheme(&mut self.vm.core, args.get(1).unwrap_or(&Value::Void))
        } else {
            let quit = self.vm.core.symbol("quit");
            self.vm.core.alloc(Object::Escape(quit, Scheme::Void))
        };
        self.called.set(true);
        let result = self.guarded(editor, |vm| vm.resume(&fiber, value));
        self.called.set(false);
        match result {
            Ok(value) => Ok(self.printed(value)),
            Err(report) => Err(eval_error(&report, false)),
        }
    }
}
