//! From the analysed tree to instructions.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::code::{Capture, Intrinsic, Op, Proto};
use crate::compile::{Expr, Lambda, Var};
use crate::error::Error;
use crate::source::Span;
use crate::value::Value;

/// Which top-level variables hold procedures that have an instruction of
/// their own: the variable's number, the instruction, the argument count.
pub type Intrinsics = HashMap<u32, (u8, usize)>;

struct Gen<'a> {
    lambda: &'a Lambda,
    intrinsics: &'a Intrinsics,
    code: Vec<Op>,
    constants: Vec<Value>,
    protos: Vec<Rc<Proto>>,
    spans: Vec<(u32, Span)>,
}

/// Compiles a function, and the functions inside it.
pub fn generate(lambda: &Lambda, intrinsics: &Intrinsics) -> Result<Rc<Proto>, Error> {
    crate::deep(|| generate_here(lambda, intrinsics))
}

fn generate_here(lambda: &Lambda, intrinsics: &Intrinsics) -> Result<Rc<Proto>, Error> {
    let mut generator = Gen {
        lambda,
        intrinsics,
        code: Vec::new(),
        constants: Vec::new(),
        protos: Vec::new(),
        spans: Vec::new(),
    };
    // A parameter that closures share and someone assigns to moves to a
    // cell before anything else.
    for param in &lambda.params {
        if param.in_cell() {
            generator.code.push(Op::MakeCell(param.slot));
        }
    }
    if lambda.toplevel {
        // Not a tail call, so that the form shows in the trace of an error.
        generator.expr(&lambda.body, false)?;
        generator.emit(Op::Return);
    } else {
        generator.expr(&lambda.body, true)?;
    }
    let captures = lambda
        .captures
        .iter()
        .map(|var| var.slot)
        .map(Capture::Local)
        .collect();
    Ok(Rc::new(Proto {
        code: generator.code,
        constants: generator.constants,
        protos: generator.protos,
        // Filled in by whoever makes the closure; see `Expr::Lambda`.
        captures,
        required: lambda.required as u16,
        rest: lambda.rest,
        locals: lambda.locals,
        name: lambda.name,
        doc: lambda.doc.clone(),
        parameters: lambda.params.iter().map(|param| param.name).collect(),
        span: lambda.span,
        spans: generator.spans,
        epoch: Cell::new(0),
    }))
}

impl Gen<'_> {
    fn emit(&mut self, op: Op) -> usize {
        self.code.push(op);
        self.code.len() - 1
    }

    fn note(&mut self, span: Span) {
        self.spans.push((self.code.len() as u32, span));
    }

    /// Makes the jump at `at` go to the next instruction.
    fn land(&mut self, at: usize) {
        let target = self.code.len() as u32;
        self.code[at] = self.code[at].with_target(target);
    }

    /// The procedure with an instruction of its own that a call is of, if
    /// it is of one, by the name and the number of arguments.
    fn intrinsic(&self, callee: &Expr, args: usize) -> Option<Intrinsic> {
        let Expr::Global(slot, _) = callee else {
            return None;
        };
        let (index, count) = self.intrinsics.get(slot)?;
        (*count == args).then(|| Intrinsic::ALL[usize::from(*index)].0)
    }

    /// Compiles the test of a conditional. Gives the jumps that are taken
    /// when it is false, to be made to land where the alternative is.
    fn test(&mut self, test: &Expr) -> Result<Vec<usize>, Error> {
        if let Expr::Call(callee, args, span) = test
            && let Some(jump) = self
                .intrinsic(callee, args.len())
                .and_then(Intrinsic::jump_unless)
        {
            for arg in args {
                self.expr(arg, false)?;
            }
            self.note(*span);
            // The second jump is for when the first has to call what the
            // name has come to mean.
            return Ok(vec![self.emit(jump), self.emit(Op::JumpIfFalse(0))]);
        }
        self.expr(test, false)?;
        Ok(vec![self.emit(Op::JumpIfFalse(0))])
    }

    fn constant(&mut self, value: Value) {
        let op = match value {
            Value::Nil => Op::Nil,
            Value::Void => Op::Void,
            Value::Bool(true) => Op::True,
            Value::Bool(false) => Op::False,
            Value::Int(small) if i32::try_from(small).is_ok() => Op::Int(small as i32),
            _ => {
                self.constants.push(value);
                Op::Const(self.constants.len() as u32 - 1)
            }
        };
        self.emit(op);
    }

    /// Where this function has a variable of an enclosing one.
    fn captured(&self, var: &Rc<Var>) -> u16 {
        self.lambda
            .captures
            .iter()
            .position(|seen| Rc::ptr_eq(seen, var))
            .expect("a variable from outside was noted as captured") as u16
    }

    fn is_local(&self, var: &Var) -> bool {
        var.func == self.lambda.func
    }

    /// Whether a call of `callee` with that many arguments is a call of
    /// the function being compiled, by the variable that holds it.
    fn calls_itself(&self, callee: &Expr, args: usize) -> bool {
        let (Expr::Local(var) | Expr::LocalAt(var, _)) = callee else {
            return false;
        };
        self.lambda.self_var.as_ref().is_some_and(|own| {
            Rc::ptr_eq(own, var)
                && !own.mutated.get()
                && !self.lambda.rest
                && self.lambda.required == args
        })
    }

    fn load(&mut self, var: &Rc<Var>) {
        let op = match (self.is_local(var), var.in_cell()) {
            (true, true) => Op::LocalCell(var.slot),
            (true, false) if var.recursive => Op::LocalChecked(var.slot),
            (true, false) => Op::Local(var.slot),
            (false, true) => Op::CapturedCell(self.captured(var)),
            (false, false) => Op::Captured(self.captured(var)),
        };
        self.emit(op);
    }

    fn store(&mut self, var: &Rc<Var>) {
        let op = match (self.is_local(var), var.in_cell()) {
            (true, true) => Op::SetLocalCell(var.slot),
            (true, false) => Op::SetLocal(var.slot),
            // A variable that is assigned to from an inner function is
            // always in a cell.
            (false, _) => Op::SetCapturedCell(self.captured(var)),
        };
        self.emit(op);
    }

    /// Compiles an expression. In tail position its value is returned.
    fn expr(&mut self, expr: &Expr, tail: bool) -> Result<(), Error> {
        crate::deep(|| self.expr_here(expr, tail))
    }

    fn expr_here(&mut self, expr: &Expr, tail: bool) -> Result<(), Error> {
        match expr {
            Expr::Const(value) => self.constant(*value),
            Expr::Local(var) => self.load(var),
            Expr::LocalAt(var, span) => {
                self.note(*span);
                self.load(var);
            }
            Expr::SetLocal(var, value) | Expr::InitLocal(var, value) => {
                self.expr(value, false)?;
                self.store(var);
                self.emit(Op::Void);
            }
            Expr::Global(slot, span) => {
                self.note(*span);
                self.emit(Op::Global(*slot));
            }
            Expr::SetGlobal(slot, value, span) => {
                self.expr(value, false)?;
                self.note(*span);
                self.emit(Op::SetGlobal(*slot));
                self.emit(Op::Void);
            }
            Expr::DefineGlobal(slot, value) => {
                self.expr(value, false)?;
                self.emit(Op::DefineGlobal(*slot));
                self.emit(Op::Void);
            }
            Expr::If(test, then, otherwise) => {
                let to_else = self.test(test)?;
                self.expr(then, tail)?;
                if tail {
                    for jump in to_else {
                        self.land(jump);
                    }
                    return self.expr(otherwise, true);
                }
                let to_end = self.emit(Op::Jump(0));
                for jump in to_else {
                    self.land(jump);
                }
                self.expr(otherwise, false)?;
                self.land(to_end);
            }
            Expr::Begin(exprs) => {
                let Some((last, before)) = exprs.split_last() else {
                    self.emit(Op::Void);
                    return self.finish(tail);
                };
                for expr in before {
                    self.expr(expr, false)?;
                    self.emit(Op::Pop);
                }
                return self.expr(last, tail);
            }
            Expr::And(exprs) | Expr::Or(exprs) => {
                let (last, before) = exprs.split_last().expect("and/or with operands");
                let mut exits = Vec::new();
                for operand in before {
                    self.expr(operand, false)?;
                    exits.push(self.emit(match expr {
                        Expr::And(_) => Op::JumpIfFalseElsePop(0),
                        _ => Op::JumpIfTrueElsePop(0),
                    }));
                }
                self.expr(last, tail)?;
                // The last operand has returned by itself in tail
                // position; the early exits still have to.
                for exit in &exits {
                    self.land(*exit);
                }
                if tail && !exits.is_empty() {
                    self.emit(Op::Return);
                }
                return Ok(());
            }
            Expr::Lambda(inner) => {
                let mut proto = generate(inner, self.intrinsics)?;
                let captures = inner
                    .captures
                    .iter()
                    .map(|var| {
                        if self.is_local(var) {
                            Capture::Local(var.slot)
                        } else {
                            Capture::Captured(self.captured(var))
                        }
                    })
                    .collect();
                Rc::get_mut(&mut proto)
                    .expect("new code has one owner")
                    .captures = captures;
                self.protos.push(proto);
                self.emit(Op::Closure(self.protos.len() as u32 - 1));
            }
            Expr::Call(callee, args, span)
                if let Some(kind) = self.intrinsic(callee, args.len()) =>
            {
                // A small integer to add or to take away is part of the
                // instruction.
                let fused = match (kind, &args[..]) {
                    (Intrinsic::Add, [_, Expr::Const(Value::Int(small))]) => {
                        i32::try_from(*small).ok().map(Op::AddInt)
                    }
                    (Intrinsic::Sub, [_, Expr::Const(Value::Int(small))]) => {
                        i32::try_from(*small).ok().map(Op::SubInt)
                    }
                    _ => None,
                };
                let operands = if fused.is_some() {
                    &args[..1]
                } else {
                    &args[..]
                };
                for arg in operands {
                    self.expr(arg, false)?;
                }
                self.note(*span);
                self.emit(fused.unwrap_or(kind.op()));
            }
            Expr::Call(callee, args, span) if tail && self.calls_itself(callee, args.len()) => {
                for arg in args {
                    self.expr(arg, false)?;
                }
                self.note(*span);
                self.emit(Op::TailSelf);
                return Ok(());
            }
            Expr::Call(callee, args, span) => {
                self.expr(callee, false)?;
                for arg in args {
                    self.expr(arg, false)?;
                }
                self.note(*span);
                if tail {
                    // A procedure written in Rust cannot take the frame
                    // over, so a return follows for that case.
                    self.emit(Op::TailCall(args.len() as u8));
                    self.emit(Op::Return);
                    return Ok(());
                }
                self.emit(Op::Call(args.len() as u8));
            }
            Expr::Let(bindings, body) => {
                for (var, value) in bindings {
                    self.expr(value, false)?;
                    self.emit(Op::SetLocal(var.slot));
                    if var.in_cell() {
                        self.emit(Op::MakeCell(var.slot));
                    }
                }
                return self.expr(body, tail);
            }
            Expr::Letrec(vars, body) => {
                for var in vars {
                    if var.in_cell() {
                        self.emit(Op::MakeCell(var.slot));
                    }
                }
                return self.expr(body, tail);
            }
        }
        self.finish(tail)
    }

    fn finish(&mut self, tail: bool) -> Result<(), Error> {
        if tail {
            self.emit(Op::Return);
        }
        if self.code.len() > u32::MAX as usize / 2 {
            return Err(Error::new("this function is too large to compile"));
        }
        Ok(())
    }
}
