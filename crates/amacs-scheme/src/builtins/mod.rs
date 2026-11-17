//! The procedures that are written in Rust.

mod data;
mod lists;
mod numbers;
mod system;
mod text;

use crate::deep;
use crate::error::Error;
use crate::value::{Key, Object, Symbol, Value};
use crate::vm::Core;

pub(crate) type Outcome = Result<Value, Error>;

pub(crate) fn install(core: &mut Core) {
    numbers::install(core);
    lists::install(core);
    text::install(core);
    data::install(core);
    system::install(core);
}

/// The error for an argument of the wrong kind.
pub(crate) fn wrong(core: &Core, who: &str, expected: &str, got: Value) -> Error {
    Error::new(format!(
        "{who}: expected {expected}, got {}",
        core.write(got)
    ))
}

pub(crate) fn int(core: &Core, who: &str, value: Value) -> Result<i64, Error> {
    match value {
        Value::Int(value) => Ok(value),
        Value::Float(value) if value.fract() == 0.0 && value.abs() < 9.0e15 => Ok(value as i64),
        _ => Err(wrong(core, who, "an integer", value)),
    }
}

/// An argument that is a position or a count.
pub(crate) fn index(core: &Core, who: &str, value: Value) -> Result<usize, Error> {
    match value {
        Value::Int(value) if value >= 0 => Ok(value as usize),
        _ => Err(wrong(core, who, "a non-negative integer", value)),
    }
}

pub(crate) fn text<'a>(core: &'a Core, who: &str, value: Value) -> Result<&'a str, Error> {
    core.as_str(value)
        .ok_or_else(|| wrong(core, who, "a string", value))
}

pub(crate) fn symbol(core: &Core, who: &str, value: Value) -> Result<Symbol, Error> {
    match value {
        Value::Sym(symbol) => Ok(symbol),
        _ => Err(wrong(core, who, "a symbol", value)),
    }
}

pub(crate) fn character(core: &Core, who: &str, value: Value) -> Result<char, Error> {
    match value {
        Value::Char(c) => Ok(c),
        _ => Err(wrong(core, who, "a char", value)),
    }
}

pub(crate) fn list(core: &Core, who: &str, value: Value) -> Result<Vec<Value>, Error> {
    core.list_to_vec(value)
        .ok_or_else(|| wrong(core, who, "a list", value))
}

pub(crate) fn pair(core: &Core, who: &str, value: Value) -> Result<(Value, Value), Error> {
    core.as_pair(value)
        .ok_or_else(|| wrong(core, who, "a pair", value))
}

/// The start and end that optional arguments give for a sequence of
/// `len` items.
pub(crate) fn range(
    core: &Core,
    who: &str,
    args: &[Value],
    len: usize,
) -> Result<(usize, usize), Error> {
    let start = args.first().map_or(Ok(0), |arg| index(core, who, *arg))?;
    let end = args.get(1).map_or(Ok(len), |arg| index(core, who, *arg))?;
    if start > end || end > len {
        return Err(Error::new(format!(
            "{who}: the range {start} to {end} is not within the {len} items there are"
        )));
    }
    Ok((start, end))
}

/// `eqv?`: the same object, or the same number or char.
#[inline]
pub(crate) fn eqv(a: Value, b: Value) -> bool {
    match (a, b) {
        (Value::Float(a), Value::Float(b)) => a.to_bits() == b.to_bits(),
        _ => a == b,
    }
}

/// `equal?`: the same structure with the same contents.
pub(crate) fn equal(core: &Core, mut a: Value, mut b: Value) -> bool {
    loop {
        if eqv(a, b) {
            return true;
        }
        let (Some(left), Some(right)) = (core.object(a), core.object(b)) else {
            return false;
        };
        match (left, right) {
            (Object::Str(left), Object::Str(right)) => return left == right,
            (Object::Pair(car_a, cdr_a), Object::Pair(car_b, cdr_b)) => {
                if !deep(|| equal(core, *car_a, *car_b)) {
                    return false;
                }
                a = *cdr_a;
                b = *cdr_b;
            }
            (Object::Vector(left), Object::Vector(right)) => {
                return left.len() == right.len()
                    && left
                        .iter()
                        .zip(right)
                        .all(|(a, b)| deep(|| equal(core, *a, *b)));
            }
            _ => return false,
        }
    }
}

/// What a value is as the key of a hash table.
pub(crate) fn key(core: &Core, value: Value) -> Key {
    match value {
        Value::Nil | Value::Void | Value::Unbound | Value::Eof => Key::Nil,
        Value::Bool(value) => Key::Bool(value),
        Value::Int(value) => Key::Int(value),
        Value::Float(value) => Key::Float(value.to_bits()),
        Value::Char(c) => Key::Char(c),
        Value::Sym(symbol) => Key::Sym(symbol),
        Value::Obj(handle) => match core.heap.get(handle) {
            Object::Str(text) => Key::Str(text.clone()),
            _ => Key::Obj(handle),
        },
    }
}
