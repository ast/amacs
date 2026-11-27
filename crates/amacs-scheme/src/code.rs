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
    /// Go round a loop that is part of the running procedure: the `count`
    /// values on top of the stack go to the local variables from `first`
    /// on, and the code goes on at `target`.
    Loop {
        target: u32,
        first: u16,
        count: u8,
    },
    /// Take a local variable's value away, for a variable that comes into
    /// being anew in each turn of a loop.
    ClearLocal(u16),
    Return,
    /// Make a closure of one of the code's inner pieces of code.
    Closure(u32),
    // What follows does what a common built-in procedure does to the
    // values on top of the stack, without the call, for as long as the
    // procedure's name means what it did at first and the values are of
    // the usual kind. Otherwise it calls what the name means now. See
    // [`Intrinsic`].
    Add,
    Sub,
    Mul,
    Less,
    Greater,
    LessEqual,
    GreaterEqual,
    NumberEqual,
    Eq,
    Eqv,
    Cons,
    Car,
    Cdr,
    IsNull,
    IsPair,
    Not,
    /// `+` with a small integer as its second argument, which is not on
    /// the stack.
    AddInt(i32),
    /// `-` in the same way.
    SubInt(i32),
    // A test and the jump that depends on it, in one. Where the test is of
    // the usual kind, the instruction pops its operands and either jumps or
    // steps over the next instruction. Otherwise it calls what the name
    // means now, and the next instruction, a `JumpIfFalse` to the same
    // place, sees to the value.
    JumpUnlessLess(u32),
    JumpUnlessGreater(u32),
    JumpUnlessLessEqual(u32),
    JumpUnlessGreaterEqual(u32),
    JumpUnlessNumberEqual(u32),
    JumpUnlessEq(u32),
    JumpUnlessEqv(u32),
    JumpUnlessNull(u32),
    JumpUnlessPair(u32),
    JumpUnlessNot(u32),
}

impl Op {
    /// The same jump to another place, if this is a jump.
    pub fn with_target(self, target: u32) -> Self {
        match self {
            Op::Jump(_) => Op::Jump(target),
            Op::JumpIfFalse(_) => Op::JumpIfFalse(target),
            Op::JumpIfFalseElsePop(_) => Op::JumpIfFalseElsePop(target),
            Op::JumpIfTrueElsePop(_) => Op::JumpIfTrueElsePop(target),
            Op::JumpUnlessLess(_) => Op::JumpUnlessLess(target),
            Op::JumpUnlessGreater(_) => Op::JumpUnlessGreater(target),
            Op::JumpUnlessLessEqual(_) => Op::JumpUnlessLessEqual(target),
            Op::JumpUnlessGreaterEqual(_) => Op::JumpUnlessGreaterEqual(target),
            Op::JumpUnlessNumberEqual(_) => Op::JumpUnlessNumberEqual(target),
            Op::JumpUnlessEq(_) => Op::JumpUnlessEq(target),
            Op::JumpUnlessEqv(_) => Op::JumpUnlessEqv(target),
            Op::JumpUnlessNull(_) => Op::JumpUnlessNull(target),
            Op::JumpUnlessPair(_) => Op::JumpUnlessPair(target),
            Op::JumpUnlessNot(_) => Op::JumpUnlessNot(target),
            other => other,
        }
    }
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
    Eqv,
    Cons,
    Car,
    Cdr,
    IsNull,
    IsPair,
    Not,
}

impl Intrinsic {
    /// Each with its name and the number of arguments it is used for, in
    /// the order of the enumeration.
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
        (Intrinsic::Eqv, "eqv?", 2),
        (Intrinsic::Cons, "cons", 2),
        (Intrinsic::Car, "car", 1),
        (Intrinsic::Cdr, "cdr", 1),
        (Intrinsic::IsNull, "null?", 1),
        (Intrinsic::IsPair, "pair?", 1),
        (Intrinsic::Not, "not", 1),
    ];

    /// The instruction that does what the procedure does.
    pub fn op(self) -> Op {
        match self {
            Intrinsic::Add => Op::Add,
            Intrinsic::Sub => Op::Sub,
            Intrinsic::Mul => Op::Mul,
            Intrinsic::Less => Op::Less,
            Intrinsic::Greater => Op::Greater,
            Intrinsic::LessEqual => Op::LessEqual,
            Intrinsic::GreaterEqual => Op::GreaterEqual,
            Intrinsic::NumberEqual => Op::NumberEqual,
            Intrinsic::Eq => Op::Eq,
            Intrinsic::Eqv => Op::Eqv,
            Intrinsic::Cons => Op::Cons,
            Intrinsic::Car => Op::Car,
            Intrinsic::Cdr => Op::Cdr,
            Intrinsic::IsNull => Op::IsNull,
            Intrinsic::IsPair => Op::IsPair,
            Intrinsic::Not => Op::Not,
        }
    }

    /// The instruction that jumps unless the procedure's value is true,
    /// for the procedures that are tests.
    pub fn jump_unless(self) -> Option<Op> {
        Some(match self {
            Intrinsic::Less => Op::JumpUnlessLess(0),
            Intrinsic::Greater => Op::JumpUnlessGreater(0),
            Intrinsic::LessEqual => Op::JumpUnlessLessEqual(0),
            Intrinsic::GreaterEqual => Op::JumpUnlessGreaterEqual(0),
            Intrinsic::NumberEqual => Op::JumpUnlessNumberEqual(0),
            Intrinsic::Eq => Op::JumpUnlessEq(0),
            Intrinsic::Eqv => Op::JumpUnlessEqv(0),
            Intrinsic::IsNull => Op::JumpUnlessNull(0),
            Intrinsic::IsPair => Op::JumpUnlessPair(0),
            Intrinsic::Not => Op::JumpUnlessNot(0),
            _ => return None,
        })
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_intrinsics_are_listed_in_their_order() {
        for (index, (kind, _, _)) in Intrinsic::ALL.iter().enumerate() {
            assert_eq!(*kind as usize, index);
        }
        assert!(std::mem::size_of::<Op>() <= 8);
    }
}
