//! Compiled code: the instructions of the virtual machine.

use std::cell::Cell;
use std::rc::Rc;

use crate::source::Span;
use crate::value::{Symbol, Value};

/// One instruction. The machine has a stack of values; each call has a
/// frame on it, whose first slots are the call's local variables.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op {
    /// Push a constant of the code.
    Const(u32),
    Nil,
    Void,
    True,
    False,
    Int(i32),
    /// Push a local variable.
    Local(u16),
    /// Push a local variable, failing if it has no value yet.
    LocalChecked(u16),
    /// Pop into a local variable.
    SetLocal(u16),
    /// Put a local variable's value into a cell of its own, for a variable
    /// that closures share and someone assigns to.
    MakeCell(u16),
    /// Push the content of the cell that a local variable holds.
    LocalCell(u16),
    /// Pop into the cell that a local variable holds.
    SetLocalCell(u16),
    /// Push a variable the running closure captured.
    Captured(u16),
    CapturedCell(u16),
    SetCapturedCell(u16),
    Global(u32),
    SetGlobal(u32),
    DefineGlobal(u32),
    Pop,
    Jump(u32),
    /// Pop, and jump if it was `#f`.
    JumpIfFalse(u32),
    /// Jump if the top is `#f`, leaving it; otherwise pop it.
    JumpIfFalseElsePop(u32),
    /// Jump if the top is not `#f`, leaving it; otherwise pop it.
    JumpIfTrueElsePop(u32),
    /// Call the procedure under the arguments with that many arguments.
    Call(u8),
    /// The same, in place of the running call.
    TailCall(u8),
    /// Call the running procedure again in place of the running call: its
    /// parameters take the values on top of the stack and it starts over.
    /// For a loop, where the compiler can tell that the call is of the
    /// procedure itself.
    TailSelf,
    Return,
    /// Make a closure of one of the code's inner pieces of code.
    Closure(u32),
    /// Do what a common built-in procedure does to the values on top of
    /// the stack, without the call. The number is its place in
    /// [`Intrinsic::ALL`].
    Intrinsic(u8),
}

/// The built-in procedures that compiled code does not call but carries
/// out directly, for as long as their names mean what they did at first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Intrinsic {
    Add,
    Sub,
    Mul,
    Less,
    Greater,
    LessEqual,
    GreaterEqual,
    NumberEqual,
    Eq,
    Cons,
    Car,
    Cdr,
    IsNull,
    IsPair,
    Not,
}

impl Intrinsic {
    /// Each with its name and the number of arguments it is used for.
    pub const ALL: [(Intrinsic, &'static str, usize); 16] = [
        (Intrinsic::Add, "+", 2),
        (Intrinsic::Sub, "-", 2),
        (Intrinsic::Mul, "*", 2),
        (Intrinsic::Less, "<", 2),
        (Intrinsic::Greater, ">", 2),
        (Intrinsic::LessEqual, "<=", 2),
        (Intrinsic::GreaterEqual, ">=", 2),
        (Intrinsic::NumberEqual, "=", 2),
        (Intrinsic::Eq, "eq?", 2),
        (Intrinsic::Eq, "eqv?", 2),
        (Intrinsic::Cons, "cons", 2),
        (Intrinsic::Car, "car", 1),
        (Intrinsic::Cdr, "cdr", 1),
        (Intrinsic::IsNull, "null?", 1),
        (Intrinsic::IsPair, "pair?", 1),
        (Intrinsic::Not, "not", 1),
    ];
}

/// Where a closure gets a variable it captures when it is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capture {
    /// A local variable of the code that makes the closure.
    Local(u16),
    /// A variable that code captured itself.
    Captured(u16),
}

/// The compiled form of a `lambda`, or of a form at the top level.
#[derive(Debug)]
pub struct Proto {
    pub code: Vec<Op>,
    pub constants: Vec<Value>,
    pub protos: Vec<Rc<Proto>>,
    pub captures: Vec<Capture>,
    /// Number of parameters that must be given.
    pub required: u16,
    /// Whether further arguments are gathered into a list.
    pub rest: bool,
    /// Number of local variable slots, parameters included.
    pub locals: u16,
    pub name: Option<Symbol>,
    /// The documentation string, if the body began with one.
    pub doc: Option<Rc<str>>,
    pub parameters: Vec<Symbol>,
    /// Where the whole `lambda` is in the source.
    pub span: Span,
    /// Where the source of each call instruction is: instruction index
    /// and span, in order.
    pub spans: Vec<(u32, Span)>,
    /// The last collection that marked the constants.
    pub epoch: Cell<u64>,
}

impl Proto {
    /// The source position of the instruction at `pc`, or of the nearest
    /// one before it that has a position.
    pub fn span_at(&self, pc: usize) -> Option<Span> {
        let after = self.spans.partition_point(|(at, _)| *at as usize <= pc);
        Some(self.spans[after.checked_sub(1)?].1)
    }
}
