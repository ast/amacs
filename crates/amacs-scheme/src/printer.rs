//! Values as text.

use std::fmt::Write;

use crate::deep;
use crate::value::{Object, Value};
use crate::vm::Core;

/// How many items are written before the rest is left out. Data can be
/// circular.
const LIMIT: usize = 100_000;

/// A float the way Scheme writes it.
pub fn float_string(value: f64) -> String {
    if value.is_nan() {
        "+nan.0".to_owned()
    } else if value.is_infinite() {
        if value > 0.0 { "+inf.0" } else { "-inf.0" }.to_owned()
    } else {
        format!("{value:?}")
    }
}

fn write_string(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => {
                let _ = write!(out, "\\x{:x};", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn write_char(out: &mut String, c: char) {
    out.push_str("#\\");
    match c {
        ' ' => out.push_str("space"),
        '\n' => out.push_str("newline"),
        '\t' => out.push_str("tab"),
        '\r' => out.push_str("return"),
        '\0' => out.push_str("null"),
        '\u{7f}' => out.push_str("delete"),
        '\u{1b}' => out.push_str("escape"),
        '\u{7}' => out.push_str("alarm"),
        '\u{8}' => out.push_str("backspace"),
        c if c.is_control() => {
            let _ = write!(out, "x{:x}", c as u32);
        }
        c => out.push(c),
    }
}

struct Printer<'a> {
    core: &'a Core,
    out: String,
    /// Whether strings and chars are written as they are read.
    readable: bool,
    left: usize,
}

impl Printer<'_> {
    fn items(&mut self, items: &[Value]) {
        for (index, item) in items.iter().enumerate() {
            if index > 0 {
                self.out.push(' ');
            }
            self.value(*item);
        }
    }

    fn value(&mut self, value: Value) {
        if self.left == 0 {
            self.out.push_str("...");
            return;
        }
        self.left -= 1;
        let core = self.core;
        match value {
            Value::Nil => self.out.push_str("()"),
            Value::Void => self.out.push_str("#<void>"),
            Value::Unbound => self.out.push_str("#<unbound>"),
            Value::Eof => self.out.push_str("#<eof>"),
            Value::Bool(true) => self.out.push_str("#t"),
            Value::Bool(false) => self.out.push_str("#f"),
            Value::Int(value) => {
                let _ = write!(self.out, "{value}");
            }
            Value::Float(value) => self.out.push_str(&float_string(value)),
            Value::Char(c) if self.readable => write_char(&mut self.out, c),
            Value::Char(c) => self.out.push(c),
            Value::Sym(symbol) => {
                let name = core.symbols.name(symbol);
                let odd = name.is_empty()
                    || name.contains(|c: char| c.is_whitespace() || "()[]\"';`,|".contains(c));
                if self.readable && odd {
                    let _ = write!(self.out, "|{name}|");
                } else {
                    self.out.push_str(name);
                }
            }
            Value::Obj(handle) => match core.heap.get(handle) {
                Object::Str(text) if self.readable => write_string(&mut self.out, text),
                Object::Str(text) => self.out.push_str(text),
                Object::Pair(..) => deep(|| self.list(value)),
                Object::Vector(items) => {
                    self.out.push_str("#(");
                    deep(|| self.items(items));
                    self.out.push(')');
                }
                Object::Closure(closure) => match closure.proto.name {
                    Some(name) => {
                        let _ = write!(self.out, "#<procedure {}>", core.symbols.name(name));
                    }
                    None => self.out.push_str("#<procedure>"),
                },
                Object::Native(native) => {
                    let _ = write!(self.out, "#<procedure {}>", core.symbols.name(native.name));
                }
                Object::Cell(_) => self.out.push_str("#<cell>"),
                Object::Table(table) => {
                    let _ = write!(self.out, "#<hash-table {}>", table.entries.len());
                }
                Object::RecordType { name, .. } => {
                    let _ = write!(self.out, "#<record-type {}>", core.symbols.name(*name));
                }
                Object::Record { kind, fields } => {
                    let name = match core.heap.get(*kind) {
                        Object::RecordType { name, .. } => core.symbols.name(*name),
                        _ => "record",
                    };
                    let _ = write!(self.out, "#<{}", name.trim_matches(['<', '>']));
                    for field in fields {
                        self.out.push(' ');
                        deep(|| self.value(*field));
                    }
                    self.out.push('>');
                }
                Object::Values(items) => deep(|| self.items(items)),
                Object::Parameter { .. } => self.out.push_str("#<parameter>"),
                Object::Fiber(_) => self.out.push_str("#<coroutine>"),
                Object::Error(error) => {
                    self.out.push_str("#<error ");
                    write_string(&mut self.out, &error.message);
                    for irritant in &error.irritants {
                        self.out.push(' ');
                        deep(|| self.value(*irritant));
                    }
                    self.out.push('>');
                }
                Object::Escape(..) => self.out.push_str("#<escape>"),
                Object::Foreign(_) => self.out.push_str("#<foreign>"),
            },
        }
    }

    fn list(&mut self, mut list: Value) {
        // 'x and its relatives are written the short way.
        if let Some((Value::Sym(head), rest)) = self.core.as_pair(list)
            && let Some((quoted, Value::Nil)) = self.core.as_pair(rest)
        {
            let prefix = match self.core.symbols.name(head) {
                "quote" => Some("'"),
                "quasiquote" => Some("`"),
                "unquote" => Some(","),
                "unquote-splicing" => Some(",@"),
                _ => None,
            };
            if let Some(prefix) = prefix {
                self.out.push_str(prefix);
                self.value(quoted);
                return;
            }
        }
        self.out.push('(');
        let mut first = true;
        loop {
            match self.core.as_pair(list) {
                Some((car, cdr)) => {
                    if !first {
                        self.out.push(' ');
                    }
                    first = false;
                    self.value(car);
                    list = cdr;
                    if self.left == 0 {
                        self.out.push_str(" ...");
                        break;
                    }
                }
                None => {
                    if list != Value::Nil {
                        self.out.push_str(" . ");
                        self.value(list);
                    }
                    break;
                }
            }
        }
        self.out.push(')');
    }
}

impl Core {
    fn print(&self, value: Value, readable: bool) -> String {
        let mut printer = Printer {
            core: self,
            out: String::new(),
            readable,
            left: LIMIT,
        };
        printer.value(value);
        printer.out
    }

    /// The value as `display` shows it: strings and chars as themselves.
    pub fn display(&self, value: Value) -> String {
        self.print(value, false)
    }

    /// The value as `write` shows it: so that it reads back as the same.
    pub fn write(&self, value: Value) -> String {
        self.print(value, true)
    }
}
