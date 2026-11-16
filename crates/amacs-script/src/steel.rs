//! The Steel backend. This is the only file that names the Scheme engine.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use amacs_core::{Editor, ScriptCall, ScriptFn, ScriptOutcome, ScriptTarget, Value};
use steel::rerrs::ErrorKind;
use steel::steel_vm::ThreadStateController;
use steel::steel_vm::engine::Engine;
use steel::{SteelErr, SteelVal};

use crate::api::{API, ApiFn};
use crate::runtime;

thread_local! {
    /// The editor that primitives act on while Scheme code runs on this thread.
    static CURRENT: RefCell<Option<Rc<RefCell<Editor>>>> = const { RefCell::new(None) };
}

/// Makes `editor` the target of primitives until the guard is dropped.
struct CurrentEditor {
    previous: Option<Rc<RefCell<Editor>>>,
}

impl CurrentEditor {
    fn enter(editor: &Rc<RefCell<Editor>>) -> Self {
        let previous = CURRENT.with(|current| current.replace(Some(editor.clone())));
        Self { previous }
    }
}

impl Drop for CurrentEditor {
    fn drop(&mut self) {
        CURRENT.with(|current| current.replace(self.previous.take()));
    }
}

fn error(message: impl Into<String>) -> SteelErr {
    SteelErr::new(ErrorKind::Generic, message.into())
}

fn is_procedure(value: &SteelVal) -> bool {
    matches!(
        value,
        SteelVal::Closure(_)
            | SteelVal::FuncV(_)
            | SteelVal::MutFunc(_)
            | SteelVal::BuiltIn(_)
            | SteelVal::BoxedFunction(_)
            | SteelVal::ContinuationFunction(_)
    )
}

fn from_steel(value: &SteelVal) -> Result<Value, SteelErr> {
    Ok(match value {
        SteelVal::Void => Value::Void,
        SteelVal::BoolV(b) => Value::Bool(*b),
        SteelVal::IntV(n) => Value::Int(*n as i64),
        SteelVal::NumV(n) => Value::Float(*n),
        SteelVal::StringV(s) => Value::Str(s.to_string()),
        SteelVal::CharV(c) => Value::Str(c.to_string()),
        SteelVal::SymbolV(s) => Value::Sym(s.to_string()),
        SteelVal::ListV(items) => {
            Value::List(items.iter().map(from_steel).collect::<Result<_, _>>()?)
        }
        procedure if is_procedure(procedure) => Value::Func(ScriptFn(Rc::new(procedure.clone()))),
        other => return Err(error(format!("the editor cannot use this value: {other}"))),
    })
}

fn to_steel(value: &Value) -> SteelVal {
    match value {
        Value::Void => SteelVal::Void,
        Value::Bool(b) => SteelVal::BoolV(*b),
        Value::Int(n) => SteelVal::IntV(*n as isize),
        Value::Float(n) => SteelVal::NumV(*n),
        Value::Str(s) => SteelVal::StringV(s.as_str().into()),
        Value::Sym(s) => SteelVal::SymbolV(s.as_str().into()),
        Value::List(items) => SteelVal::ListV(items.iter().map(to_steel).collect()),
        Value::Func(ScriptFn(func)) => func
            .downcast_ref::<SteelVal>()
            .cloned()
            // A procedure from some other engine cannot be called by this one.
            .unwrap_or(SteelVal::Void),
    }
}

/// Wraps a primitive as a Steel procedure that checks its argument count,
/// converts values and borrows the current editor for the call.
fn primitive(api: &'static ApiFn) -> SteelVal {
    SteelVal::anonymous_boxed_function(Arc::new(move |args: &[SteelVal]| {
        let too_many = api.max_args.is_some_and(|max| args.len() > max);
        if args.len() < api.min_args || too_many {
            let expected = match api.max_args {
                Some(max) if max == api.min_args => format!("{max}"),
                Some(max) => format!("{} to {max}", api.min_args),
                None => format!("at least {}", api.min_args),
            };
            return Err(SteelErr::new(
                ErrorKind::ArityMismatch,
                format!(
                    "{} takes {expected} arguments, got {}",
                    api.name,
                    args.len()
                ),
            ));
        }
        let args: Vec<Value> = args.iter().map(from_steel).collect::<Result<_, _>>()?;
        let editor = CURRENT
            .with(|current| current.borrow().clone())
            .ok_or_else(|| error(format!("{}: no editor is attached", api.name)))?;
        let mut editor = editor
            .try_borrow_mut()
            .map_err(|_| error(format!("{}: the editor is busy", api.name)))?;
        (api.run)(&mut editor, &args)
            .map(|value| to_steel(&value))
            .map_err(|e| error(format!("{}: {e}", api.name)))
    }))
}

/// Lets another thread stop Scheme code that is running, for `C-g`.
#[derive(Clone)]
pub struct InterruptHandle {
    running: Arc<AtomicBool>,
    controller: ThreadStateController,
}

impl InterruptHandle {
    /// Interrupts the running Scheme code. Does nothing, and returns false,
    /// if none is running.
    pub fn interrupt(&self) -> bool {
        let running = self.running.load(Ordering::SeqCst);
        if running {
            self.controller.interrupt();
        }
        running
    }
}

/// What evaluating Scheme code produced.
pub type EvalResult = Result<String, EvalError>;

#[derive(Debug, Clone)]
pub struct EvalError {
    /// One line.
    pub summary: String,
    /// The full report with the source location.
    pub report: String,
}

pub struct SteelHost {
    engine: Engine,
    interrupt: InterruptHandle,
}

impl SteelHost {
    /// Creates an engine with the editor primitives and the Scheme runtime
    /// loaded. Loading the runtime runs Scheme code that calls primitives,
    /// so it needs the editor.
    pub fn new(editor: &Rc<RefCell<Editor>>) -> Result<Self, EvalError> {
        let mut engine = Engine::new();
        for api in API {
            engine.register_value(api.name, primitive(api));
        }
        let interrupt = InterruptHandle {
            running: Arc::new(AtomicBool::new(false)),
            controller: engine.get_thread_state_controller(),
        };
        let mut host = Self { engine, interrupt };
        for (name, source) in runtime::sources() {
            // The engine wants a path that exists, and embedded files have
            // none, so the file name goes into the error instead.
            host.eval(editor, &source, None)
                .map_err(|error| EvalError {
                    summary: format!("runtime/{name}: {}", error.summary),
                    report: format!("In runtime/{name}:\n{}", error.report),
                })?;
        }
        Ok(host)
    }

    pub fn interrupt_handle(&self) -> InterruptHandle {
        self.interrupt.clone()
    }

    /// Runs `body` with the engine marked as running and interruptible.
    fn guarded<T>(
        &mut self,
        editor: &Rc<RefCell<Editor>>,
        body: impl FnOnce(&mut Engine) -> Result<T, SteelErr>,
    ) -> Result<T, EvalError> {
        let _current = CurrentEditor::enter(editor);
        // An interrupt that arrived just after the previous run ended must
        // not stop this one.
        self.interrupt.controller.resume();
        self.interrupt.running.store(true, Ordering::SeqCst);
        let result = body(&mut self.engine);
        self.interrupt.running.store(false, Ordering::SeqCst);
        self.interrupt.controller.resume();
        result.map_err(|error| {
            let summary = error.to_string();
            let report = self
                .engine
                .raise_error_to_string(error)
                .unwrap_or_else(|| summary.clone());
            EvalError { summary, report }
        })
    }

    /// Evaluates source text at top level. Returns the printed form of the
    /// last value, or an empty string if there is none.
    pub fn eval(
        &mut self,
        editor: &Rc<RefCell<Editor>>,
        source: &str,
        path: Option<&Path>,
    ) -> EvalResult {
        let value = self.eval_last(editor, source, path)?;
        Ok(value.map(|value| value.to_string()).unwrap_or_default())
    }

    /// Evaluates source text at top level and converts the last value.
    pub fn eval_value(
        &mut self,
        editor: &Rc<RefCell<Editor>>,
        source: &str,
    ) -> Result<Value, EvalError> {
        let value = self.eval_last(editor, source, None)?;
        from_steel(&value.unwrap_or(SteelVal::Void)).map_err(|error| EvalError {
            summary: error.to_string(),
            report: error.to_string(),
        })
    }

    /// The value of the last top-level form that has one.
    fn eval_last(
        &mut self,
        editor: &Rc<RefCell<Editor>>,
        source: &str,
        path: Option<&Path>,
    ) -> Result<Option<SteelVal>, EvalError> {
        let source = source.to_owned();
        let values = self.guarded(editor, |engine| match path {
            Some(path) => engine.compile_and_run_raw_program_with_path(source, path.to_owned()),
            None => engine.run(source),
        })?;
        Ok(values
            .into_iter()
            .rev()
            .find(|value| !matches!(value, SteelVal::Void)))
    }

    /// Runs a call that the editor queued.
    pub fn run(&mut self, editor: &Rc<RefCell<Editor>>, call: &ScriptCall) -> ScriptOutcome {
        let result = match &call.target {
            ScriptTarget::Eval(source) => self.eval(editor, source, None),
            ScriptTarget::Func(func) => {
                self.enter(editor, to_steel(&Value::Func(func.clone())), &call.args)
            }
            ScriptTarget::Named(name) => match self.engine.extract_value(name) {
                Ok(func) => self.enter(editor, func, &call.args),
                Err(error) => Err(EvalError {
                    summary: error.to_string(),
                    report: error.to_string(),
                }),
            },
        };
        match result {
            Ok(printed) => ScriptOutcome::Returned(printed),
            Err(error) => ScriptOutcome::Failed {
                summary: error.summary,
                report: error.report,
            },
        }
    }

    /// Calls a procedure through the runtime's `%enter`, which is what lets
    /// the code it runs use `await`.
    fn enter(
        &mut self,
        editor: &Rc<RefCell<Editor>>,
        func: SteelVal,
        args: &[Value],
    ) -> EvalResult {
        let args = to_steel(&Value::List(args.to_vec()));
        let value = self.guarded(editor, |engine| {
            engine.call_function_by_name_with_args("%enter", vec![func, args])
        })?;
        Ok(value.to_string())
    }
}
