//! The virtual machine: the interpreter's state and the loop that runs
//! compiled code.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::code::{Capture, Intrinsic, Op, Proto};
use crate::codegen::{Intrinsics, generate};
use crate::compile::{Compiler, Keywords, SPECIAL_FORMS, Special};
use crate::error::{Error, Report, TraceFrame};
use crate::expand::{Macro, Names};
use crate::fiber::{Cleanup, Fiber, FiberState, Frame, Handler, Restore};
use crate::heap::Heap;
use crate::reader::read_next;
use crate::source::{Sources, Span};
use crate::symbol::Interner;
use crate::syntax::{Datum, Ident, Syntax};
use crate::value::{
    Closure, Control, ErrorObject, Handle, Native, NativeKind, Object, Symbol, Value,
};

/// The file number of code that has no source text.
pub const NO_FILE: u32 = u32::MAX;

/// A variable of the top level.
pub struct Global {
    pub name: Symbol,
    pub value: Value,
    /// Where the latest `define` of it is.
    pub defined_at: Option<Span>,
}

#[derive(Default)]
struct Roots {
    values: Vec<Option<Value>>,
    free: Vec<usize>,
}

/// A value that the program embedding the interpreter holds on to. The
/// collector leaves it alone for as long as this exists.
pub struct Rooted {
    index: usize,
    roots: Rc<RefCell<Roots>>,
}

impl Rooted {
    pub fn value(&self) -> Value {
        self.roots.borrow().values[self.index].expect("a rooted value stays until dropped")
    }
}

impl Drop for Rooted {
    fn drop(&mut self) {
        let mut roots = self.roots.borrow_mut();
        roots.values[self.index] = None;
        roots.free.push(self.index);
    }
}

impl std::fmt::Debug for Rooted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Rooted({:?})", self.value())
    }
}

/// Everything about the interpreter except the code that is running: what
/// procedures written in Rust get to work with.
pub struct Core {
    pub heap: Heap,
    pub symbols: Interner,
    pub sources: Sources,
    pub globals: Vec<Global>,
    global_index: HashMap<Symbol, u32>,
    pub macros: HashMap<Symbol, Rc<Macro>>,
    pub specials: HashMap<Symbol, Special>,
    /// Documentation given to names that cannot carry their own: macros
    /// and variables.
    pub docs: HashMap<Symbol, String>,
    pub keywords: Keywords,
    marks: u32,
    functions: u32,
    roots: Rc<RefCell<Roots>>,
    /// What `display` and its like have written, until someone takes it.
    pub output: String,
    /// Calls a procedure of no arguments; used where a call needs a frame
    /// of its own to hang something on.
    pub(crate) call0: Handle,
    /// The same for one argument.
    pub(crate) call1: Handle,
    pub(crate) interrupt_tag: Symbol,
    /// For each entry of [`Intrinsic::ALL`]: the variable that holds the
    /// procedure, and the procedure. Code compiled to the instruction
    /// checks that the variable still holds it.
    intrinsics: Vec<(u32, Value)>,
    intrinsic_slots: Intrinsics,
}

impl Core {
    fn new() -> Self {
        let mut symbols = Interner::default();
        let mut heap = Heap::new();
        let keywords = Keywords {
            names: Names {
                ellipsis: symbols.intern("..."),
                underscore: symbols.intern("_"),
                syntax_rules: symbols.intern("syntax-rules"),
            },
            else_: symbols.intern("else"),
            arrow: symbols.intern("=>"),
            unquote: symbols.intern("unquote"),
            unquote_splicing: symbols.intern("unquote-splicing"),
            quasiquote: symbols.intern("quasiquote"),
        };
        let specials = SPECIAL_FORMS
            .iter()
            .map(|(name, special)| (symbols.intern(name), *special))
            .collect();
        let mut trampoline = |args: u16| {
            let mut code: Vec<Op> = (0..=args).map(Op::Local).collect();
            code.extend([Op::Call(args as u8), Op::Return]);
            let proto = Rc::new(Proto {
                code,
                constants: Vec::new(),
                protos: Vec::new(),
                captures: Vec::new(),
                required: args + 1,
                rest: false,
                locals: args + 1,
                name: None,
                doc: None,
                parameters: Vec::new(),
                span: Span {
                    file: NO_FILE,
                    start: 0,
                    end: 0,
                },
                spans: Vec::new(),
                epoch: std::cell::Cell::new(0),
            });
            heap.alloc(Object::Closure(Closure {
                proto,
                captured: Box::new([]),
            }))
        };
        let call0 = trampoline(0);
        let call1 = trampoline(1);
        let interrupt_tag = symbols.intern("interrupt");
        Self {
            heap,
            symbols,
            sources: Sources::default(),
            globals: Vec::new(),
            global_index: HashMap::new(),
            macros: HashMap::new(),
            specials,
            docs: HashMap::new(),
            keywords,
            marks: 0,
            functions: 0,
            roots: Rc::default(),
            output: String::new(),
            call0,
            call1,
            interrupt_tag,
            intrinsics: Vec::new(),
            intrinsic_slots: Intrinsics::new(),
        }
    }

    /// The number of the top-level variable with a name, which is made if
    /// it does not exist yet. Compiled code refers to variables by number,
    /// so a new definition of a name is seen by all code that uses it.
    pub fn global_slot(&mut self, name: Symbol) -> u32 {
        *self.global_index.entry(name).or_insert_with(|| {
            self.globals.push(Global {
                name,
                value: Value::Unbound,
                defined_at: None,
            });
            self.globals.len() as u32 - 1
        })
    }

    pub(crate) fn note_definition(&mut self, slot: u32, span: Span) {
        self.globals[slot as usize].defined_at = Some(span);
    }

    pub(crate) fn fresh_mark(&mut self) -> u32 {
        self.marks += 1;
        self.marks
    }

    pub(crate) fn fresh_function(&mut self) -> u32 {
        self.functions += 1;
        self.functions
    }

    /// Gives a top-level variable a value.
    pub fn define(&mut self, name: &str, value: Value) {
        let symbol = self.symbols.intern(name);
        let slot = self.global_slot(symbol);
        self.globals[slot as usize].value = value;
    }

    /// The value of a top-level variable, if it has one.
    pub fn lookup(&self, name: Symbol) -> Option<Value> {
        let slot = *self.global_index.get(&name)?;
        Some(self.globals[slot as usize].value).filter(|value| *value != Value::Unbound)
    }

    pub fn global(&self, name: Symbol) -> Option<&Global> {
        let slot = *self.global_index.get(&name)?;
        Some(&self.globals[slot as usize]).filter(|global| global.value != Value::Unbound)
    }

    /// Defines a procedure written in Rust. `max_args` of `None` means any
    /// number.
    pub fn define_native(
        &mut self,
        name: &str,
        min_args: usize,
        max_args: Option<usize>,
        doc: &'static str,
        run: impl Fn(&mut Core, &[Value]) -> Result<Value, Error> + 'static,
    ) {
        self.define_native_kind(
            name,
            min_args,
            max_args,
            doc,
            NativeKind::Function(Box::new(run)),
        );
    }

    pub(crate) fn define_native_kind(
        &mut self,
        name: &str,
        min_args: usize,
        max_args: Option<usize>,
        doc: &'static str,
        run: NativeKind,
    ) {
        let symbol = self.symbols.intern(name);
        let native = Native {
            name: symbol,
            doc,
            min_args,
            max_args,
            run,
        };
        let value = self.alloc(Object::Native(Rc::new(native)));
        self.define(name, value);
    }

    /// Keeps a value alive for as long as the returned handle exists.
    pub fn root(&self, value: Value) -> Rooted {
        let mut roots = self.roots.borrow_mut();
        let index = match roots.free.pop() {
            Some(index) => {
                roots.values[index] = Some(value);
                index
            }
            None => {
                roots.values.push(Some(value));
                roots.values.len() - 1
            }
        };
        Rooted {
            index,
            roots: self.roots.clone(),
        }
    }

    // Making values

    pub fn alloc(&mut self, object: Object) -> Value {
        Value::Obj(self.heap.alloc(object))
    }

    pub fn string(&mut self, text: impl Into<String>) -> Value {
        self.alloc(Object::Str(text.into()))
    }

    pub fn symbol(&mut self, name: &str) -> Value {
        Value::Sym(self.symbols.intern(name))
    }

    pub fn cons(&mut self, car: Value, cdr: Value) -> Value {
        self.alloc(Object::Pair(car, cdr))
    }

    pub fn list(&mut self, items: &[Value]) -> Value {
        self.list_with_tail(items, Value::Nil)
    }

    pub fn list_with_tail(&mut self, items: &[Value], tail: Value) -> Value {
        let mut list = tail;
        for item in items.iter().rev() {
            list = self.cons(*item, list);
        }
        list
    }

    // Looking at values

    pub fn object(&self, value: Value) -> Option<&Object> {
        value.handle().map(|handle| self.heap.get(handle))
    }

    pub fn as_str(&self, value: Value) -> Option<&str> {
        match self.object(value)? {
            Object::Str(text) => Some(text),
            _ => None,
        }
    }

    pub fn as_pair(&self, value: Value) -> Option<(Value, Value)> {
        match self.object(value)? {
            Object::Pair(car, cdr) => Some((*car, *cdr)),
            _ => None,
        }
    }

    /// The elements of a list, or `None` if it does not end in the empty
    /// list (which includes going round in a circle).
    pub fn list_to_vec(&self, mut list: Value) -> Option<Vec<Value>> {
        let mut items = Vec::new();
        loop {
            match list {
                Value::Nil => return Some(items),
                _ => {
                    let (car, cdr) = self.as_pair(list)?;
                    items.push(car);
                    list = cdr;
                    if items.len() > self.heap.len() {
                        return None;
                    }
                }
            }
        }
    }

    pub fn is_procedure(&self, value: Value) -> bool {
        matches!(
            self.object(value),
            Some(Object::Closure(_) | Object::Native(_) | Object::Parameter { .. })
        )
    }

    /// An error with its message and irritants as one line, where it is,
    /// and the calls that led to it.
    pub fn report(&self, error: &Error) -> Report {
        let mut message = error.message.clone();
        for irritant in &error.irritants {
            message.push(' ');
            message.push_str(&self.write(*irritant));
        }
        let first = error.trace.iter().find_map(|frame| frame.span);
        Report {
            message,
            location: error
                .span
                .or(first)
                .and_then(|span| self.sources.location(span)),
            trace: error
                .trace
                .iter()
                .map(|frame| {
                    (
                        frame.name.clone(),
                        frame.span.and_then(|span| self.sources.location(span)),
                    )
                })
                .collect(),
            escape: match error.payload.and_then(|payload| self.object(payload)) {
                Some(Object::Escape(Value::Sym(tag), _)) => {
                    Some(self.symbols.name(*tag).to_owned())
                }
                _ => None,
            },
        }
    }
}

/// How running some code ended.
#[derive(Debug)]
pub enum Outcome {
    Done(Value),
    /// The code asked to wait for something, giving a value that says for
    /// what. [`Vm::resume`] continues it.
    Suspended {
        fiber: Rooted,
        value: Value,
    },
}

/// What a call did to the machine.
enum Flow {
    /// The value is on the stack; the running code goes on.
    Value,
    /// Another call is now the innermost.
    Frame,
    Suspended(Outcome),
}

pub struct Vm {
    pub core: Core,
    fiber: Fiber,
    interrupt: Arc<AtomicBool>,
    /// How many calls can be in progress at once.
    pub max_depth: usize,
    /// How many more calls may be made before the code is interrupted, if
    /// there is a limit. For running code that may not end.
    pub fuel: Option<u64>,
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

fn arity_error(
    core: &Core,
    name: Option<Symbol>,
    min: usize,
    max: Option<usize>,
    got: usize,
) -> Error {
    let name = name.map_or("a procedure", |name| core.symbols.name(name));
    let expected = match max {
        Some(max) if max == min => format!("{min}"),
        Some(max) => format!("{min} to {max}"),
        None => format!("at least {min}"),
    };
    let plural = if expected == "1" { "" } else { "s" };
    Error::new(format!(
        "{name}: expected {expected} argument{plural}, got {got}"
    ))
}

fn too_deep() -> Error {
    Error::new("too many calls are in progress at once; is a recursion missing its end?")
}

impl Vm {
    /// An interpreter with the standard procedures defined.
    pub fn new() -> Self {
        let mut vm = Self {
            core: Core::new(),
            fiber: Fiber::new(),
            interrupt: Arc::default(),
            max_depth: 200_000,
            fuel: None,
        };
        crate::builtins::install(&mut vm.core);
        for (index, (_, name, count)) in Intrinsic::ALL.iter().enumerate() {
            let symbol = vm.core.symbols.intern(name);
            let slot = vm.core.global_slot(symbol);
            let original = vm.core.globals[slot as usize].value;
            vm.core.intrinsics.push((slot, original));
            vm.core.intrinsic_slots.insert(slot, (index as u8, *count));
        }
        if let Err(report) = vm.eval(crate::PRELUDE, "prelude.scm") {
            panic!("the prelude does not load: {report}");
        }
        vm
    }

    /// A flag that makes the running code stop when set, from any thread.
    pub fn interrupt_handle(&self) -> Arc<AtomicBool> {
        self.interrupt.clone()
    }

    /// Reads and runs every form in a text and gives the value of the last.
    pub fn eval(&mut self, source: &str, name: &str) -> Result<Outcome, Report> {
        let mut last = Ok(Outcome::Done(Value::Void));
        self.eval_each(source, name, |result, _| {
            let go_on = matches!(result, Ok(Outcome::Done(_)));
            last = result;
            go_on
        });
        last
    }

    /// Reads a text and runs its forms one by one, handing each result to
    /// `each` with where in the text the form ended; `each` says whether
    /// to go on with the next.
    pub fn eval_each(
        &mut self,
        source: &str,
        name: &str,
        mut each: impl FnMut(Result<Outcome, Report>, usize) -> bool,
    ) {
        let file = self.core.sources.add(name, source);
        let mut pos = 0;
        loop {
            // A form is read when the ones before it have run, so that a
            // mistake further down does not stop what comes before it.
            let form = match read_next(source, file, pos, &mut self.core.symbols) {
                Ok(Some((form, after))) => {
                    pos = after;
                    form
                }
                Ok(None) => return,
                Err(error) => {
                    each(Err(self.core.report(&error)), source.len());
                    return;
                }
            };
            let result = self
                .compile(form)
                .and_then(|closure| self.start(closure, &[]))
                .map_err(|error| self.core.report(&error));
            if !each(result, pos) {
                return;
            }
        }
    }

    fn compile(&mut self, form: Syntax) -> Result<Value, Error> {
        // Code is a tree that can be some thousand levels deep, and taking
        // such a tree apart when done with it needs stack like building it.
        let proto = stacker::maybe_grow(2 * 1024 * 1024, 16 * 1024 * 1024, || {
            let lambda = Compiler::new(&mut self.core).toplevel(&form)?;
            drop(form);
            generate(&lambda, &self.core.intrinsic_slots)
        })?;
        Ok(self.core.alloc(Object::Closure(Closure {
            proto,
            captured: Box::new([]),
        })))
    }

    /// Calls a procedure.
    pub fn call(&mut self, procedure: Value, args: &[Value]) -> Result<Outcome, Report> {
        self.start(procedure, args)
            .map_err(|error| self.core.report(&error))
    }

    /// Continues code that was suspended, with the value it waited for.
    pub fn resume(&mut self, fiber: &Rooted, value: Value) -> Result<Outcome, Report> {
        let waiting =
            fiber
                .value()
                .handle()
                .and_then(|handle| match self.core.heap.get_mut(handle) {
                    Object::Fiber(fiber) if fiber.state == FiberState::Suspended => {
                        Some(std::mem::take(&mut **fiber))
                    }
                    _ => None,
                });
        let Some(mut waiting) = waiting else {
            return Err(self.core.report(&Error::new("this is not suspended code")));
        };
        // The emptied fiber left behind says that this one is used up.
        if let Some(handle) = fiber.value().handle()
            && let Object::Fiber(left) = self.core.heap.get_mut(handle)
        {
            left.state = FiberState::Done;
        }
        waiting.state = FiberState::Running;
        waiting.stack.push(value);
        self.fiber = waiting;
        self.run().map_err(|error| self.core.report(&error))
    }

    fn start(&mut self, procedure: Value, args: &[Value]) -> Result<Outcome, Error> {
        self.fiber = Fiber::new();
        self.fiber.stack.push(procedure);
        self.fiber.stack.extend_from_slice(args);
        let flow = match self.call_value(args.len(), false) {
            Ok(flow) => flow,
            Err(error) => {
                self.throw(error)?;
                Flow::Frame
            }
        };
        match flow {
            Flow::Value => Ok(Outcome::Done(self.fiber.stack.pop().unwrap_or(Value::Void))),
            Flow::Suspended(outcome) => Ok(outcome),
            Flow::Frame => self.run(),
        }
    }

    fn run(&mut self) -> Result<Outcome, Error> {
        loop {
            match self.execute() {
                Ok(outcome) => return Ok(outcome),
                Err(error) => self.throw(error)?,
            }
        }
    }

    // Garbage collection

    fn collect(&mut self) {
        let Self { core, fiber, .. } = self;
        let Core {
            heap,
            globals,
            roots,
            call0,
            call1,
            intrinsics,
            ..
        } = core;
        heap.collect(|marker| {
            fiber.mark(marker);
            for global in globals.iter() {
                marker.value(global.value);
            }
            for value in roots.borrow().values.iter().flatten() {
                marker.value(*value);
            }
            marker.handle(*call0);
            marker.handle(*call1);
            for (_, original) in intrinsics.iter() {
                marker.value(*original);
            }
        });
    }

    /// Collects garbage now, whether or not it is time.
    pub fn collect_garbage(&mut self) {
        self.collect();
    }

    // Errors

    /// The calls in progress, innermost first.
    fn trace(&self) -> Vec<TraceFrame> {
        self.fiber
            .frames
            .iter()
            .rev()
            .filter(|frame| frame.proto.span.file != NO_FILE)
            .take(64)
            .map(|frame| TraceFrame {
                name: frame
                    .proto
                    .name
                    .map(|name| self.core.symbols.name(name).to_owned()),
                span: frame.proto.span_at(frame.pc.saturating_sub(1)),
            })
            .collect()
    }

    /// Hands an error to whoever in the program handles errors. Fails with
    /// the error if nobody does.
    fn throw(&mut self, error: Error) -> Result<(), Error> {
        let payload = match error.payload {
            Some(payload) => payload,
            None => {
                let trace = self.trace();
                self.core.alloc(Object::Error(Box::new(ErrorObject {
                    message: error.message,
                    irritants: error.irritants,
                    trace,
                    span: error.span,
                })))
            }
        };
        self.raise(payload, false)
    }

    /// What an error that nobody handled is, for outside the machine.
    fn unhandled(&self, payload: Value) -> Error {
        let mut error = match self.core.object(payload) {
            Some(Object::Error(object)) => {
                let mut error = Error::new(object.message.clone());
                error.irritants.clone_from(&object.irritants);
                error.trace.clone_from(&object.trace);
                error.span = object.span;
                error
            }
            Some(Object::Escape(tag, _)) if *tag == Value::Sym(self.core.interrupt_tag) => {
                Error::new("interrupted")
            }
            Some(Object::Escape(..)) => Error::new(
                "an escape procedure was called after the call that made it had returned",
            ),
            _ => {
                let mut error =
                    Error::new(format!("uncaught exception: {}", self.core.write(payload)));
                error.trace = self.trace();
                error
            }
        };
        error.payload = Some(payload);
        error
    }

    /// Finds the handler for something raised and arranges for it to be
    /// called.
    fn raise(&mut self, payload: Value, continuable: bool) -> Result<(), Error> {
        let is_escape = matches!(self.core.object(payload), Some(Object::Escape(..)));
        loop {
            // An escape passes by the handlers that are only told of errors.
            let found = self
                .fiber
                .handlers
                .iter()
                .rposition(|handler| match handler {
                    Handler::Catch { .. } => true,
                    Handler::Procedure(_) => !is_escape,
                });
            let Some(index) = found else {
                // Nobody in this fiber: on to the one that resumed it.
                let Some(parent) = self.fiber.parent.take() else {
                    return Err(self.unhandled(payload));
                };
                let done = std::mem::replace(&mut self.fiber, *parent);
                self.retire(done, FiberState::Done);
                continue;
            };
            let call1 = Value::Obj(self.core.call1);
            match self.fiber.handlers[index].clone() {
                Handler::Catch {
                    frames,
                    slot,
                    handler,
                } => {
                    self.fiber.handlers.truncate(index);
                    self.fiber.frames.truncate(frames);
                    self.fiber.stack.truncate(slot);
                    self.fiber.stack.extend([call1, handler, payload]);
                    let base = self.fiber.stack.len() - 2;
                    self.push_frame(self.core.call1, base, Cleanup::None)?;
                }
                Handler::Procedure(handler) => {
                    // It runs with the handlers outside it in force.
                    let handlers = self.fiber.handlers.split_off(index);
                    self.fiber.stack.extend([call1, handler, payload]);
                    let base = self.fiber.stack.len() - 2;
                    let cleanup = Cleanup::RestoreHandlers(Box::new(Restore {
                        handlers,
                        reraise: (!continuable).then_some(payload),
                    }));
                    self.push_frame(self.core.call1, base, cleanup)?;
                }
            }
            return Ok(());
        }
    }

    /// Puts a fiber that has stopped running back into its coroutine.
    fn retire(&mut self, mut fiber: Fiber, state: FiberState) {
        fiber.state = state;
        if state == FiberState::Done {
            fiber.stack = Vec::new();
            fiber.frames = Vec::new();
            fiber.handlers = Vec::new();
        }
        if let Some(object) = fiber.object {
            *self.core.heap.get_mut(object) = Object::Fiber(Box::new(fiber));
        }
    }

    // Calls

    /// Starts a call of a closure whose arguments are on the stack from
    /// `base`.
    fn push_frame(&mut self, closure: Handle, base: usize, cleanup: Cleanup) -> Result<(), Error> {
        let Object::Closure(Closure { proto, .. }) = self.core.heap.get(closure) else {
            unreachable!("only closures get frames");
        };
        let proto = proto.clone();
        if self.fiber.frames.len() >= self.max_depth {
            return Err(too_deep());
        }
        self.fiber
            .stack
            .resize(base + usize::from(proto.locals), Value::Unbound);
        self.fiber.frames.push(Frame {
            closure,
            proto,
            pc: 0,
            base,
            cleanup,
        });
        Ok(())
    }

    /// Whether nothing waits to be seen to before the next call: garbage
    /// to collect, a limit on the number of calls, a request to stop.
    #[inline]
    fn calm(&self) -> bool {
        !self.core.heap.wants_collection()
            && self.fuel.is_none()
            && !self.interrupt.load(Ordering::Relaxed)
    }

    /// Sees to what [`Vm::calm`] found. Every call comes by here, and every
    /// turn of a loop, so that code that does not end can be stopped.
    #[cold]
    fn attend(&mut self) -> Result<(), Error> {
        if self.core.heap.wants_collection() {
            self.collect();
        }
        if let Some(fuel) = &mut self.fuel {
            if *fuel == 0 {
                self.interrupt.store(true, Ordering::Relaxed);
            }
            *fuel = fuel.saturating_sub(1);
        }
        if self.interrupt.load(Ordering::Relaxed) {
            self.interrupt.store(false, Ordering::Relaxed);
            let tag = Value::Sym(self.core.interrupt_tag);
            let mut error = Error::new("interrupted");
            error.payload = Some(self.core.alloc(Object::Escape(tag, Value::Void)));
            return Err(error);
        }
        Ok(())
    }

    /// Calls the procedure that is on the stack under `nargs` arguments.
    /// With `tail`, a closure takes the place of the running call.
    fn call_value(&mut self, mut nargs: usize, tail: bool) -> Result<Flow, Error> {
        if !self.calm() {
            self.attend()?;
        }
        loop {
            let slot = self.fiber.stack.len() - nargs - 1;
            let callee = self.fiber.stack[slot];
            let object = callee.handle().map(|handle| self.core.heap.get(handle));
            match object {
                Some(Object::Closure(closure)) => {
                    let required = usize::from(closure.proto.required);
                    let rest = closure.proto.rest;
                    if nargs < required || (nargs > required && !rest) {
                        let max = (!rest).then_some(required);
                        return Err(arity_error(
                            &self.core,
                            closure.proto.name,
                            required,
                            max,
                            nargs,
                        ));
                    }
                    let handle = callee.handle().expect("a closure is an object");
                    if rest {
                        let extra = self.fiber.stack.split_off(slot + 1 + required);
                        let list = self.core.list(&extra);
                        self.fiber.stack.push(list);
                    }
                    let Object::Closure(closure) = self.core.heap.get(handle) else {
                        unreachable!("it was a closure a moment ago");
                    };
                    let proto = &closure.proto;
                    let stack = &mut self.fiber.stack;
                    if tail && let Some(frame) = self.fiber.frames.last_mut() {
                        // The new call's values move down over the old
                        // one's, and the frame is used again.
                        let moved = stack.len() - slot;
                        stack.copy_within(slot.., frame.base - 1);
                        stack.truncate(frame.base - 1 + moved);
                        stack.resize(frame.base + usize::from(proto.locals), Value::Unbound);
                        frame.closure = handle;
                        if !Rc::ptr_eq(&frame.proto, proto) {
                            frame.proto = proto.clone();
                        }
                        frame.pc = 0;
                    } else {
                        if self.fiber.frames.len() >= self.max_depth {
                            return Err(too_deep());
                        }
                        stack.resize(slot + 1 + usize::from(proto.locals), Value::Unbound);
                        self.fiber.frames.push(Frame {
                            closure: handle,
                            proto: proto.clone(),
                            pc: 0,
                            base: slot + 1,
                            cleanup: Cleanup::None,
                        });
                    }
                    return Ok(Flow::Frame);
                }
                Some(Object::Native(native)) => {
                    let native = native.clone();
                    if nargs < native.min_args || native.max_args.is_some_and(|max| nargs > max) {
                        return Err(arity_error(
                            &self.core,
                            Some(native.name),
                            native.min_args,
                            native.max_args,
                            nargs,
                        ));
                    }
                    match &native.run {
                        NativeKind::Function(run) => {
                            let result = run(&mut self.core, &self.fiber.stack[slot + 1..])?;
                            self.fiber.stack.truncate(slot);
                            self.fiber.stack.push(result);
                            return Ok(Flow::Value);
                        }
                        NativeKind::Control(Control::Apply) => {
                            let last = self.fiber.stack.pop().expect("apply has arguments");
                            let Some(spread) = self.core.list_to_vec(last) else {
                                return Err(Error::new("apply: the last argument must be a list")
                                    .with_irritants(vec![last]));
                            };
                            // The procedure and the arguments move down over
                            // `apply` itself, and the call starts over.
                            self.fiber.stack.remove(slot);
                            self.fiber.stack.extend(spread);
                            nargs = self.fiber.stack.len() - slot - 1;
                        }
                        NativeKind::Control(control) => return self.control(*control, slot, nargs),
                    }
                }
                Some(Object::Parameter { value, .. }) if nargs == 0 => {
                    let value = *value;
                    self.fiber.stack.truncate(slot);
                    self.fiber.stack.push(value);
                    return Ok(Flow::Value);
                }
                _ => {
                    return Err(Error::new(format!(
                        "this is not a procedure and cannot be called: {}",
                        self.core.write(callee)
                    )));
                }
            }
        }
    }

    /// Calls `procedure` through the helper of no arguments, so that the
    /// call has a frame with `cleanup` on it. The result goes to `slot`.
    fn call_guarded(
        &mut self,
        slot: usize,
        procedure: Value,
        cleanup: Cleanup,
    ) -> Result<Flow, Error> {
        self.fiber.stack.truncate(slot);
        self.fiber
            .stack
            .extend([Value::Obj(self.core.call0), procedure]);
        self.push_frame(self.core.call0, slot + 1, cleanup)?;
        Ok(Flow::Frame)
    }

    /// The procedures that change what runs next.
    fn control(&mut self, control: Control, slot: usize, nargs: usize) -> Result<Flow, Error> {
        let arg = |vm: &Self, index: usize| vm.fiber.stack[slot + 1 + index];
        match control {
            Control::Apply => unreachable!("handled by the caller"),
            Control::Eval => {
                let syntax = self.core.value_syntax(arg(self, 0))?;
                let closure = self.compile(syntax)?;
                self.fiber.stack.truncate(slot);
                self.fiber.stack.push(closure);
                self.call_value(0, false)
            }
            Control::EvalString => {
                let Some(source) = self.core.as_str(arg(self, 0)).map(str::to_owned) else {
                    return Err(Error::new("eval-string: expected a string")
                        .with_irritants(vec![arg(self, 0)]));
                };
                let name = match (nargs > 1).then(|| self.core.as_str(arg(self, 1))) {
                    Some(Some(name)) => name.to_owned(),
                    Some(None) => {
                        return Err(Error::new("eval-string: expected a string as the name")
                            .with_irritants(vec![arg(self, 1)]));
                    }
                    None => "eval-string".to_owned(),
                };
                // The forms become one `begin`, which at the top level is
                // the same as the forms one after the other.
                let file = self.core.sources.add(&name, &source);
                let begin = self.core.symbols.intern("begin");
                let whole = Span {
                    file,
                    start: 0,
                    end: source.len() as u32,
                };
                let mut forms = vec![Syntax::new(Datum::Ident(Ident::plain(begin)), whole)];
                let mut pos = 0;
                while let Some((form, after)) =
                    read_next(&source, file, pos, &mut self.core.symbols)?
                {
                    forms.push(form);
                    pos = after;
                }
                let closure = self.compile(Syntax::new(Datum::List(forms), whole))?;
                self.fiber.stack.truncate(slot);
                self.fiber.stack.push(closure);
                self.call_value(0, false)
            }
            Control::Raise | Control::RaiseContinuable => {
                let payload = arg(self, 0);
                self.fiber.stack.truncate(slot);
                self.raise(payload, control == Control::RaiseContinuable)?;
                Ok(Flow::Frame)
            }
            Control::Error => {
                let first = arg(self, 0);
                let message = match self.core.as_str(first) {
                    Some(text) => text.to_owned(),
                    None => self.core.display(first),
                };
                let irritants = self.fiber.stack[slot + 2..].to_vec();
                let trace = self.trace();
                let payload = self.core.alloc(Object::Error(Box::new(ErrorObject {
                    message,
                    irritants,
                    trace,
                    span: None,
                })));
                self.raise(payload, false)?;
                Ok(Flow::Frame)
            }
            Control::CallWithCatch | Control::WithExceptionHandler => {
                let (handler, thunk) = (arg(self, 0), arg(self, 1));
                self.fiber
                    .handlers
                    .push(if control == Control::CallWithCatch {
                        Handler::Catch {
                            frames: self.fiber.frames.len(),
                            slot,
                            handler,
                        }
                    } else {
                        Handler::Procedure(handler)
                    });
                self.call_guarded(slot, thunk, Cleanup::PopHandler)
            }
            Control::Suspend => {
                let value = arg(self, 0);
                self.fiber.stack.truncate(slot);
                self.fiber.state = FiberState::Suspended;
                let fiber = std::mem::take(&mut self.fiber);
                let object = self.core.alloc(Object::Fiber(Box::new(fiber)));
                Ok(Flow::Suspended(Outcome::Suspended {
                    fiber: self.core.root(object),
                    value,
                }))
            }
            Control::Yield => {
                let value = if nargs > 0 { arg(self, 0) } else { Value::Void };
                let Some(parent) = self.fiber.parent.take() else {
                    return Err(Error::new("yield: no coroutine is running"));
                };
                self.fiber.stack.truncate(slot);
                let yielding = std::mem::replace(&mut self.fiber, *parent);
                self.retire(yielding, FiberState::Suspended);
                self.fiber.stack.push(value);
                Ok(Flow::Frame)
            }
            Control::Resume => {
                let coroutine = arg(self, 0);
                let value = if nargs > 1 { arg(self, 1) } else { Value::Void };
                let taken =
                    coroutine
                        .handle()
                        .and_then(|handle| match self.core.heap.get_mut(handle) {
                            Object::Fiber(fiber)
                                if matches!(
                                    fiber.state,
                                    FiberState::New | FiberState::Suspended
                                ) && fiber.object == Some(handle) =>
                            {
                                let taken = std::mem::take(&mut **fiber);
                                fiber.state = FiberState::Running;
                                Some(taken)
                            }
                            _ => None,
                        });
                let Some(mut child) = taken else {
                    return Err(Error::new(
                        "coroutine-resume: expected a coroutine that is waiting",
                    )
                    .with_irritants(vec![coroutine]));
                };
                self.fiber.stack.truncate(slot);
                let started = child.state == FiberState::Suspended;
                child.state = FiberState::Running;
                let parent = std::mem::replace(&mut self.fiber, child);
                self.fiber.parent = Some(Box::new(parent));
                if started {
                    self.fiber.stack.push(value);
                } else {
                    // Its stack holds the call that starts it.
                    self.push_frame(self.core.call0, 1, Cleanup::None)?;
                }
                Ok(Flow::Frame)
            }
        }
    }

    /// Returns from the innermost call. Gives the outcome if that was the
    /// last one.
    fn ret(&mut self) -> Result<Option<Outcome>, Error> {
        let result = self.fiber.stack.pop().expect("a call leaves a value");
        let frame = self.fiber.frames.pop().expect("a call is in progress");
        self.fiber.stack.truncate(frame.base - 1);
        match frame.cleanup {
            Cleanup::None => {}
            Cleanup::PopHandler => {
                self.fiber.handlers.pop();
            }
            Cleanup::RestoreHandlers(restore) => match restore.reraise {
                None => self.fiber.handlers.extend(restore.handlers),
                // The handler did not deal with the error by leaving, so
                // the error goes on to the handlers outside it.
                Some(payload) => {
                    self.raise(payload, false)?;
                    return Ok(None);
                }
            },
        }
        if !self.fiber.frames.is_empty() {
            self.fiber.stack.push(result);
            return Ok(None);
        }
        match self.fiber.parent.take() {
            Some(parent) => {
                let done = std::mem::replace(&mut self.fiber, *parent);
                self.retire(done, FiberState::Done);
                self.fiber.stack.push(result);
                Ok(None)
            }
            None => Ok(Some(Outcome::Done(result))),
        }
    }

    fn save_pc(&mut self, pc: usize) {
        self.fiber
            .frames
            .last_mut()
            .expect("a call is in progress")
            .pc = pc;
    }

    fn unbound(&mut self, pc: usize, slot: u32, what: &str) -> Error {
        self.save_pc(pc);
        let name = self
            .core
            .symbols
            .name(self.core.globals[slot as usize].name);
        Error::new(format!("{what}{name}"))
    }

    fn no_value_yet(&mut self, pc: usize) -> Error {
        self.save_pc(pc);
        Error::new("a variable is used before its definition has been reached")
    }

    /// Runs instructions until the code is done, suspends itself or fails.
    #[allow(clippy::too_many_lines)]
    fn execute(&mut self) -> Result<Outcome, Error> {
        'frame: loop {
            let frame = self.fiber.frames.last().expect("a call is in progress");
            let proto = frame.proto.clone();
            let base = frame.base;
            let mut closure = frame.closure;
            let mut pc = frame.pc;
            let code = &proto.code[..];
            loop {
                let op = code[pc];
                pc += 1;
                match op {
                    Op::Const(index) => self.fiber.stack.push(proto.constants[index as usize]),
                    Op::Nil => self.fiber.stack.push(Value::Nil),
                    Op::Void => self.fiber.stack.push(Value::Void),
                    Op::True => self.fiber.stack.push(Value::Bool(true)),
                    Op::False => self.fiber.stack.push(Value::Bool(false)),
                    Op::Int(value) => self.fiber.stack.push(Value::Int(i64::from(value))),
                    Op::Local(slot) => {
                        let value = self.fiber.stack[base + usize::from(slot)];
                        self.fiber.stack.push(value);
                    }
                    Op::LocalChecked(slot) => {
                        let value = self.fiber.stack[base + usize::from(slot)];
                        if value == Value::Unbound {
                            return Err(self.no_value_yet(pc));
                        }
                        self.fiber.stack.push(value);
                    }
                    Op::SetLocal(slot) => {
                        let value = self.fiber.stack.pop().expect("a value to store");
                        self.fiber.stack[base + usize::from(slot)] = value;
                    }
                    Op::MakeCell(slot) => {
                        let value = self.fiber.stack[base + usize::from(slot)];
                        self.fiber.stack[base + usize::from(slot)] =
                            self.core.alloc(Object::Cell(value));
                    }
                    Op::LocalCell(slot) => {
                        let cell = self.fiber.stack[base + usize::from(slot)];
                        let value = self.cell(cell);
                        if value == Value::Unbound {
                            return Err(self.no_value_yet(pc));
                        }
                        self.fiber.stack.push(value);
                    }
                    Op::SetLocalCell(slot) => {
                        let value = self.fiber.stack.pop().expect("a value to store");
                        let cell = self.fiber.stack[base + usize::from(slot)];
                        self.set_cell(cell, value);
                    }
                    Op::Captured(index) => {
                        let value = self.captured(closure, index);
                        self.fiber.stack.push(value);
                    }
                    Op::CapturedCell(index) => {
                        let cell = self.captured(closure, index);
                        let value = self.cell(cell);
                        if value == Value::Unbound {
                            return Err(self.no_value_yet(pc));
                        }
                        self.fiber.stack.push(value);
                    }
                    Op::SetCapturedCell(index) => {
                        let value = self.fiber.stack.pop().expect("a value to store");
                        let cell = self.captured(closure, index);
                        self.set_cell(cell, value);
                    }
                    Op::Global(slot) => {
                        let value = self.core.globals[slot as usize].value;
                        if value == Value::Unbound {
                            return Err(self.unbound(pc, slot, "unbound variable: "));
                        }
                        self.fiber.stack.push(value);
                    }
                    Op::SetGlobal(slot) => {
                        if self.core.globals[slot as usize].value == Value::Unbound {
                            return Err(self.unbound(pc, slot, "set!: there is no variable "));
                        }
                        let value = self.fiber.stack.pop().expect("a value to store");
                        self.core.globals[slot as usize].value = value;
                    }
                    Op::DefineGlobal(slot) => {
                        let value = self.fiber.stack.pop().expect("a value to store");
                        self.core.globals[slot as usize].value = value;
                    }
                    Op::Pop => {
                        self.fiber.stack.pop();
                    }
                    Op::Jump(target) => pc = target as usize,
                    Op::JumpIfFalse(target) => {
                        if self.fiber.stack.pop() == Some(Value::Bool(false)) {
                            pc = target as usize;
                        }
                    }
                    Op::JumpIfFalseElsePop(target) => {
                        if self.fiber.stack.last() == Some(&Value::Bool(false)) {
                            pc = target as usize;
                        } else {
                            self.fiber.stack.pop();
                        }
                    }
                    Op::JumpIfTrueElsePop(target) => {
                        if self.fiber.stack.last() == Some(&Value::Bool(false)) {
                            self.fiber.stack.pop();
                        } else {
                            pc = target as usize;
                        }
                    }
                    Op::Call(nargs) | Op::TailCall(nargs) => {
                        self.save_pc(pc);
                        let tail = matches!(op, Op::TailCall(_));
                        let nargs = usize::from(nargs);
                        // The usual case first: a closure that takes just
                        // these arguments, and nothing else to see to.
                        let slot = self.fiber.stack.len() - nargs - 1;
                        if let Value::Obj(handle) = self.fiber.stack[slot]
                            && let Object::Closure(callee) = self.core.heap.get(handle)
                            && usize::from(callee.proto.required) == nargs
                            && !callee.proto.rest
                            && self.calm()
                        {
                            let locals = usize::from(callee.proto.locals);
                            let stack = &mut self.fiber.stack;
                            if tail {
                                // The new call's values move down over the
                                // old one's, and the frame is used again.
                                stack.copy_within(slot.., base - 1);
                                stack.truncate(base + nargs);
                                stack.resize(base + locals, Value::Unbound);
                                let frame =
                                    self.fiber.frames.last_mut().expect("a call is in progress");
                                frame.closure = handle;
                                frame.pc = 0;
                                if Rc::ptr_eq(&proto, &callee.proto) {
                                    // The same code, as when a procedure
                                    // calls itself: only start over.
                                    closure = handle;
                                    pc = 0;
                                    continue;
                                }
                                frame.proto = callee.proto.clone();
                                continue 'frame;
                            }
                            if self.fiber.frames.len() < self.max_depth {
                                stack.resize(slot + 1 + locals, Value::Unbound);
                                self.fiber.frames.push(Frame {
                                    closure: handle,
                                    proto: callee.proto.clone(),
                                    pc: 0,
                                    base: slot + 1,
                                    cleanup: Cleanup::None,
                                });
                                continue 'frame;
                            }
                        }
                        match self.call_value(nargs, tail)? {
                            Flow::Value => {}
                            Flow::Frame => continue 'frame,
                            Flow::Suspended(outcome) => return Ok(outcome),
                        }
                    }
                    Op::TailSelf => {
                        let required = usize::from(proto.required);
                        let stack = &mut self.fiber.stack;
                        let args = stack.len() - required;
                        stack.copy_within(args.., base);
                        // The other variables are without values again, as
                        // in a call that has just begun.
                        stack.truncate(base + required);
                        stack.resize(base + usize::from(proto.locals), Value::Unbound);
                        pc = 0;
                        if !self.calm() {
                            self.save_pc(pc);
                            self.attend()?;
                        }
                    }
                    Op::Return => {
                        if let [.., _, frame] = &self.fiber.frames[..]
                            && matches!(frame.cleanup, Cleanup::None)
                        {
                            let stack = &mut self.fiber.stack;
                            let result = stack.pop().expect("a call leaves a value");
                            stack.truncate(base - 1);
                            stack.push(result);
                            self.fiber.frames.pop();
                            continue 'frame;
                        }
                        self.save_pc(pc);
                        match self.ret()? {
                            Some(outcome) => return Ok(outcome),
                            None => continue 'frame,
                        }
                    }
                    Op::Intrinsic(index) => {
                        if !self.intrinsic(index) {
                            // Not the simple case: an ordinary call of
                            // whatever the name means now.
                            let (_, _, count) = Intrinsic::ALL[usize::from(index)];
                            let (slot, _) = self.core.intrinsics[usize::from(index)];
                            let callee = self.core.globals[slot as usize].value;
                            let at = self.fiber.stack.len() - count;
                            self.fiber.stack.insert(at, callee);
                            self.save_pc(pc);
                            match self.call_value(count, false)? {
                                Flow::Value => {}
                                Flow::Frame => continue 'frame,
                                Flow::Suspended(outcome) => return Ok(outcome),
                            }
                        }
                    }
                    Op::Closure(index) => {
                        let inner = proto.protos[index as usize].clone();
                        let captured = inner
                            .captures
                            .iter()
                            .map(|capture| match capture {
                                Capture::Local(slot) => self.fiber.stack[base + usize::from(*slot)],
                                Capture::Captured(index) => self.captured(closure, *index),
                            })
                            .collect();
                        let value = self.core.alloc(Object::Closure(Closure {
                            proto: inner,
                            captured,
                        }));
                        self.fiber.stack.push(value);
                    }
                }
            }
        }
    }

    /// Does what a built-in procedure does to the values on top of the
    /// stack, if its name still means that procedure and the values are of
    /// the usual kind. Otherwise leaves everything as it is and says no.
    #[inline]
    fn intrinsic(&mut self, index: u8) -> bool {
        let (slot, original) = self.core.intrinsics[usize::from(index)];
        if self.core.globals[slot as usize].value != original {
            return false;
        }
        let stack = &mut self.fiber.stack;
        let top = stack.len() - 1;
        let (kind, _, count) = Intrinsic::ALL[usize::from(index)];
        if count == 1 {
            let value = stack[top];
            let result = match kind {
                Intrinsic::Car | Intrinsic::Cdr => {
                    let Value::Obj(handle) = value else {
                        return false;
                    };
                    match self.core.heap.get(handle) {
                        Object::Pair(car, _) if kind == Intrinsic::Car => *car,
                        Object::Pair(_, cdr) => *cdr,
                        _ => return false,
                    }
                }
                Intrinsic::IsNull => Value::Bool(value == Value::Nil),
                Intrinsic::IsPair => Value::Bool(matches!(
                    value.handle().map(|handle| self.core.heap.get(handle)),
                    Some(Object::Pair(..))
                )),
                Intrinsic::Not => Value::Bool(value == Value::Bool(false)),
                _ => return false,
            };
            stack[top] = result;
            return true;
        }
        let (a, b) = (stack[top - 1], stack[top]);
        let result = match (kind, a, b) {
            (Intrinsic::Cons, _, _) => self.core.alloc(Object::Pair(a, b)),
            (Intrinsic::Eq, _, _) => Value::Bool(crate::builtins::eqv(a, b)),
            (_, Value::Int(x), Value::Int(y)) => match kind {
                Intrinsic::Add => match x.checked_add(y) {
                    Some(sum) => Value::Int(sum),
                    None => return false,
                },
                Intrinsic::Sub => match x.checked_sub(y) {
                    Some(difference) => Value::Int(difference),
                    None => return false,
                },
                Intrinsic::Mul => match x.checked_mul(y) {
                    Some(product) => Value::Int(product),
                    None => return false,
                },
                Intrinsic::Less => Value::Bool(x < y),
                Intrinsic::Greater => Value::Bool(x > y),
                Intrinsic::LessEqual => Value::Bool(x <= y),
                Intrinsic::GreaterEqual => Value::Bool(x >= y),
                Intrinsic::NumberEqual => Value::Bool(x == y),
                _ => return false,
            },
            (_, Value::Float(x), Value::Float(y)) => match kind {
                Intrinsic::Add => Value::Float(x + y),
                Intrinsic::Sub => Value::Float(x - y),
                Intrinsic::Mul => Value::Float(x * y),
                Intrinsic::Less => Value::Bool(x < y),
                Intrinsic::Greater => Value::Bool(x > y),
                Intrinsic::LessEqual => Value::Bool(x <= y),
                Intrinsic::GreaterEqual => Value::Bool(x >= y),
                Intrinsic::NumberEqual => Value::Bool(x == y),
                _ => return false,
            },
            _ => return false,
        };
        let stack = &mut self.fiber.stack;
        stack[top - 1] = result;
        stack.pop();
        true
    }

    fn captured(&self, closure: Handle, index: u16) -> Value {
        match self.core.heap.get(closure) {
            Object::Closure(closure) => closure.captured[usize::from(index)],
            _ => unreachable!("a frame's closure is a closure"),
        }
    }

    fn cell(&self, cell: Value) -> Value {
        match self.core.object(cell) {
            Some(Object::Cell(value)) => *value,
            _ => unreachable!("the compiler put a cell here"),
        }
    }

    fn set_cell(&mut self, cell: Value, value: Value) {
        match cell.handle().map(|handle| self.core.heap.get_mut(handle)) {
            Some(Object::Cell(content)) => *content = value,
            _ => unreachable!("the compiler put a cell here"),
        }
    }
}
