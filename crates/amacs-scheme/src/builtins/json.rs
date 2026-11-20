//! JSON, parsed and written in Rust for speed.
//!
//! `json-decode` and `json-encode` cross between JSON text and Scheme data,
//! keeping objects and arrays apart, which one empty list could not:
//!
//!   object  an association list of (symbol . value) pairs; {} is '()
//!   array   a vector; [] is #()
//!   true/false  #t/#f    null  the symbol `null`
//!
//! The parsing is serde_json's; the tree is built straight onto the heap.

use std::fmt::Write;

use super::text;
use crate::error::Error;
use crate::value::{Object, Value};
use crate::vm::Core;

pub(crate) fn install(core: &mut Core) {
    core.define_native(
        "json-decode",
        1,
        Some(1),
        "(json-decode string) The Scheme value a JSON string stands for: an \
         object as an association list with symbol keys, an array as a vector, \
         null as the symbol null. An error if the text is not valid JSON.",
        |core, args| {
            let source = text(core, "json-decode", args[0])?;
            let parsed: serde_json::Value = serde_json::from_str(source)
                .map_err(|error| Error::new(format!("json-decode: {error}")))?;
            Ok(from_json(core, &parsed))
        },
    );
    core.define_native(
        "json-encode",
        1,
        Some(1),
        "(json-encode value) The JSON text for a value: an association list \
         (symbol or string keys) as an object, a vector as an array, the \
         symbol null as null, #t and #f as true and false.",
        |core, args| {
            let mut out = String::new();
            to_json(core, args[0], &mut out)?;
            Ok(core.string(out))
        },
    );
}

fn from_json(core: &mut Core, value: &serde_json::Value) -> Value {
    match value {
        serde_json::Value::Null => core.symbol("null"),
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else {
                Value::Float(n.as_f64().unwrap_or(f64::NAN))
            }
        }
        serde_json::Value::String(s) => core.string(s.as_str()),
        serde_json::Value::Array(items) => {
            // Children are converted first; nothing collects during a native
            // call, so the unrooted intermediates are safe until the vector
            // that holds them is allocated.
            let converted: Vec<Value> = items.iter().map(|item| from_json(core, item)).collect();
            core.alloc(Object::Vector(converted))
        }
        serde_json::Value::Object(map) => {
            let mut entries = Vec::with_capacity(map.len());
            for (key, value) in map {
                let key = core.symbol(key);
                let value = from_json(core, value);
                entries.push(core.cons(key, value));
            }
            core.list(&entries)
        }
    }
}

fn to_json(core: &Core, value: Value, out: &mut String) -> Result<(), Error> {
    match value {
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Int(n) => {
            let _ = write!(out, "{n}");
        }
        Value::Float(f) => emit_number(f, out),
        // The empty list is an empty object; a non-empty list is an alist.
        Value::Nil => out.push_str("{}"),
        Value::Sym(symbol) => {
            let name = core.symbols.name(symbol);
            if name == "null" {
                out.push_str("null");
            } else {
                emit_string(name, out);
            }
        }
        Value::Obj(_) => match core.object(value) {
            Some(Object::Str(s)) => emit_string(s, out),
            Some(Object::Vector(items)) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    to_json(core, *item, out)?;
                }
                out.push(']');
            }
            Some(Object::Pair(_, _)) => emit_object(core, value, out)?,
            _ => return Err(cannot_encode(core, value)),
        },
        _ => return Err(cannot_encode(core, value)),
    }
    Ok(())
}

fn emit_object(core: &Core, value: Value, out: &mut String) -> Result<(), Error> {
    let entries = core
        .list_to_vec(value)
        .ok_or_else(|| Error::new("json-encode: an object must be a proper list of pairs"))?;
    out.push('{');
    for (i, entry) in entries.iter().enumerate() {
        let (key, field) = core
            .as_pair(*entry)
            .ok_or_else(|| Error::new("json-encode: an object entry must be a pair"))?;
        if i > 0 {
            out.push(',');
        }
        match key {
            Value::Sym(symbol) => emit_string(core.symbols.name(symbol), out),
            _ => {
                let name = core.as_str(key).ok_or_else(|| {
                    Error::new("json-encode: an object key must be a symbol or string")
                })?;
                emit_string(name, out);
            }
        }
        out.push(':');
        to_json(core, field, out)?;
    }
    out.push('}');
    Ok(())
}

fn cannot_encode(core: &Core, value: Value) -> Error {
    Error::new(format!("json-encode: cannot encode {}", core.write(value)))
}

/// JSON has one number type: a whole float is written without a fraction,
/// and anything not finite as null, since JSON has no infinity.
fn emit_number(f: f64, out: &mut String) {
    if !f.is_finite() {
        out.push_str("null");
    } else if f.fract() == 0.0 && f.abs() < 1e15 {
        let _ = write!(out, "{}", f as i64);
    } else {
        let _ = write!(out, "{f}");
    }
}

fn emit_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}
