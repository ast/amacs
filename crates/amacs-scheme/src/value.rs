//! Values and the objects on the heap.

use std::any::Any;
use std::rc::Rc;

use indexmap::IndexMap;

use crate::code::Proto;
use crate::error::Error;
use crate::fiber::Fiber;
use crate::vm::Core;

/// An interned symbol. Two symbols are the same if their numbers are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Symbol(pub(crate) u32);

/// A reference to an object on the heap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Handle(pub(crate) u32);

/// A Scheme value. Small things are held directly; everything else is a
/// handle to an object that the garbage collector manages.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    /// The empty list.
    Nil,
    /// What procedures called for effect return.
    Void,
    /// A variable that has no value yet. Never seen by programs.
    Unbound,
    Eof,
    Bool(bool),
    Int(i64),
    Float(f64),
    Char(char),
    Sym(Symbol),
    Obj(Handle),
}

impl Value {
    /// Everything but `#f` counts as true.
    pub fn is_true(self) -> bool {
        self != Value::Bool(false)
    }

    pub fn handle(self) -> Option<Handle> {
        match self {
            Value::Obj(handle) => Some(handle),
            _ => None,
        }
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Value::Bool(value)
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Value::Int(value)
    }
}

impl From<usize> for Value {
    fn from(value: usize) -> Self {
        Value::Int(value as i64)
    }
}

/// A procedure written in Rust. It gets the interpreter's state and its
/// arguments; it cannot call back into Scheme, which keeps every call
/// stack in one place where it can be shown and suspended.
pub type NativeFn = dyn Fn(&mut Core, &[Value]) -> Result<Value, Error>;

/// What a native procedure is, besides its code.
pub struct Native {
    pub name: Symbol,
    pub doc: &'static str,
    pub min_args: usize,
    /// `None` for any number.
    pub max_args: Option<usize>,
    pub run: NativeKind,
}

/// Natives the machine carries out itself, because they change what runs
/// next rather than compute a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Apply,
    Eval,
    /// `eval-string`: read a text and run everything in it.
    EvalString,
    Raise,
    RaiseContinuable,
    /// `error`: make an error of a message and raise it.
    Error,
    Resume,
    Yield,
    /// Hand control to the program that embeds the interpreter.
    Suspend,
    CallWithCatch,
    WithExceptionHandler,
}

pub enum NativeKind {
    Function(Box<NativeFn>),
    Control(Control),
}

/// A procedure made by `lambda`: its code and the variables it closed over.
pub struct Closure {
    pub proto: Rc<Proto>,
    pub captured: Box<[Value]>,
}

/// The key of a hash table entry. Numbers, symbols, chars, booleans and
/// strings are keys by value; every other object by identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Nil,
    Bool(bool),
    Int(i64),
    /// The bits of a float.
    Float(u64),
    Char(char),
    Sym(Symbol),
    Str(String),
    Obj(Handle),
}

/// A [`Key`] that borrows its string: for looking a value up without
/// copying it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyRef<'a> {
    Nil,
    Bool(bool),
    Int(i64),
    Float(u64),
    Char(char),
    Sym(Symbol),
    Str(&'a str),
    Obj(Handle),
}

impl Key {
    pub fn borrowed(&self) -> KeyRef<'_> {
        match self {
            Key::Nil => KeyRef::Nil,
            Key::Bool(value) => KeyRef::Bool(*value),
            Key::Int(value) => KeyRef::Int(*value),
            Key::Float(bits) => KeyRef::Float(*bits),
            Key::Char(c) => KeyRef::Char(*c),
            Key::Sym(symbol) => KeyRef::Sym(*symbol),
            Key::Str(text) => KeyRef::Str(text),
            Key::Obj(handle) => KeyRef::Obj(*handle),
        }
    }
}

impl KeyRef<'_> {
    pub fn owned(self) -> Key {
        match self {
            KeyRef::Nil => Key::Nil,
            KeyRef::Bool(value) => Key::Bool(value),
            KeyRef::Int(value) => Key::Int(value),
            KeyRef::Float(bits) => Key::Float(bits),
            KeyRef::Char(c) => Key::Char(c),
            KeyRef::Sym(symbol) => Key::Sym(symbol),
            KeyRef::Str(text) => Key::Str(text.to_owned()),
            KeyRef::Obj(handle) => Key::Obj(handle),
        }
    }
}

// A key and what borrows it must hash alike for the lookup to find it.
impl std::hash::Hash for Key {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.borrowed().hash(state);
    }
}

impl indexmap::Equivalent<Key> for KeyRef<'_> {
    fn equivalent(&self, key: &Key) -> bool {
        *self == key.borrowed()
    }
}

/// A mutable hash table, which remembers the order things were put in.
/// Entries keep the key as it was given.
#[derive(Default)]
pub struct Table {
    pub entries: IndexMap<Key, (Value, Value)>,
}

/// An error as a Scheme value.
pub struct ErrorObject {
    pub message: String,
    pub irritants: Vec<Value>,
    /// Where it was raised: the innermost call first.
    pub trace: Vec<crate::error::TraceFrame>,
    /// Where in the source the mistake is, when that is somewhere other
    /// than where the innermost call had got to: in code being read or
    /// compiled.
    pub span: Option<crate::source::Span>,
}

pub enum Object {
    Pair(Value, Value),
    Str(String),
    Vector(Vec<Value>),
    Closure(Closure),
    Native(Rc<Native>),
    /// A variable that is both captured by a closure and assigned to.
    Cell(Value),
    Table(Box<Table>),
    RecordType {
        name: Symbol,
        fields: Box<[Symbol]>,
    },
    Record {
        kind: Handle,
        fields: Box<[Value]>,
    },
    /// Several values returned at once.
    Values(Vec<Value>),
    Parameter {
        value: Value,
        converter: Value,
    },
    Fiber(Box<Fiber>),
    Error(Box<ErrorObject>),
    /// What an escape procedure raises to get to where it was made: which
    /// escape it is, and the values to deliver.
    Escape(Value, Value),
    /// A value of the program that embeds the interpreter.
    Foreign(Rc<dyn Any>),
}

impl Object {
    /// What kind of thing this is, for error messages.
    pub fn type_name(&self) -> &'static str {
        match self {
            Object::Pair(..) => "pair",
            Object::Str(_) => "string",
            Object::Vector(_) => "vector",
            Object::Closure(_) | Object::Native(_) => "procedure",
            Object::Cell(_) => "cell",
            Object::Table(_) => "hash table",
            Object::RecordType { .. } => "record type",
            Object::Record { .. } => "record",
            Object::Values(_) => "multiple values",
            Object::Parameter { .. } => "parameter",
            Object::Fiber(_) => "coroutine",
            Object::Error(_) => "error",
            Object::Escape(..) => "escape",
            Object::Foreign(_) => "foreign object",
        }
    }
}
