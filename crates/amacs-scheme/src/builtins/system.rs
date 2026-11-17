//! Control, errors, output, and looking at the program itself.

use std::fmt::Write;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use super::{index, symbol, text, wrong};
use crate::error::Error;
use crate::fiber::{Fiber, FiberState};
use crate::value::{Control, ErrorObject, NativeKind, Object, Symbol, Value};
use crate::vm::{Core, NO_FILE};

fn error_object<'a>(core: &'a Core, who: &str, value: Value) -> Result<&'a ErrorObject, Error> {
    match core.object(value) {
        Some(Object::Error(error)) => Ok(error),
        _ => Err(wrong(core, who, "an error", value)),
    }
}

/// What can be said about a procedure.
struct Signature {
    name: Option<Symbol>,
    doc: Option<String>,
    min: usize,
    /// `None` for any number.
    max: Option<usize>,
    /// The parameters' names, the one that takes the rest last if there is
    /// one. Procedures written in Rust have none.
    parameters: Option<Vec<Symbol>>,
    rest: bool,
    location: Option<String>,
    source: Option<String>,
}

fn signature(core: &Core, who: &str, value: Value) -> Result<Signature, Error> {
    match core.object(value) {
        Some(Object::Closure(closure)) => {
            let proto = &closure.proto;
            let known = proto.span.file != NO_FILE;
            Ok(Signature {
                name: proto.name,
                doc: proto.doc.as_ref().map(ToString::to_string),
                min: usize::from(proto.required),
                max: (!proto.rest).then_some(usize::from(proto.required)),
                parameters: Some(proto.parameters.clone()),
                rest: proto.rest,
                location: core
                    .sources
                    .location(proto.span)
                    .filter(|_| known)
                    .map(|location| location.to_string()),
                source: core
                    .sources
                    .text(proto.span)
                    .filter(|_| known)
                    .map(str::to_owned),
            })
        }
        Some(Object::Native(native)) => Ok(Signature {
            name: Some(native.name),
            doc: Some(native.doc.to_owned()).filter(|doc| !doc.is_empty()),
            min: native.min_args,
            max: native.max_args,
            parameters: None,
            rest: native.max_args.is_none(),
            location: None,
            source: None,
        }),
        Some(Object::Parameter { .. }) => Ok(Signature {
            name: None,
            doc: None,
            min: 0,
            max: Some(0),
            parameters: Some(Vec::new()),
            rest: false,
            location: None,
            source: None,
        }),
        _ => Err(wrong(core, who, "a procedure", value)),
    }
}

fn optional_string(core: &mut Core, text: Option<String>) -> Value {
    match text {
        Some(text) => core.string(text),
        None => Value::Bool(false),
    }
}

/// How a procedure is called, as it would be written: `(name a b . rest)`.
fn call_form(core: &Core, name: &str, signature: &Signature) -> String {
    let mut form = format!("({name}");
    match &signature.parameters {
        Some(parameters) => {
            for (index, parameter) in parameters.iter().enumerate() {
                let dot = if signature.rest && index + 1 == parameters.len() {
                    ". "
                } else {
                    ""
                };
                let _ = write!(form, " {dot}{}", core.symbols.name(*parameter));
            }
        }
        None => {
            for index in 0..signature.min {
                let _ = write!(form, " arg{}", index + 1);
            }
            match signature.max {
                None => form.push_str(" ..."),
                Some(max) if max > signature.min => {
                    let _ = write!(form, " [{} more]", max - signature.min);
                }
                Some(_) => {}
            }
        }
    }
    form.push(')');
    form
}

/// Everything known about what a name means at the top level, as text.
fn describe(core: &Core, name: Symbol) -> String {
    let text = core.symbols.name(name);
    let noted = core.docs.get(&name);
    let with_note = |mut out: String| {
        if let Some(doc) = noted {
            let _ = write!(out, "\n{doc}");
        }
        out
    };
    if core.macros.contains_key(&name) {
        return with_note(format!("{text} is a macro."));
    }
    if core.specials.contains_key(&name) {
        return with_note(format!("{text} is a special form."));
    }
    let Some(global) = core.global(name) else {
        return format!("{text} is not defined.");
    };
    let defined_at = global
        .defined_at
        .and_then(|span| core.sources.location(span))
        .map(|location| location.to_string());
    let Ok(signature) = signature(core, "describe", global.value) else {
        let mut out = with_note(format!(
            "{text} is a variable. Its value is {}",
            core.write(global.value)
        ));
        if let Some(location) = defined_at {
            let _ = write!(out, "\nDefined at {location}");
        }
        return out;
    };
    let kind = if signature.parameters.is_some() {
        "a procedure"
    } else {
        "a procedure built in"
    };
    let mut out = format!("{text} is {kind}: {}", call_form(core, text, &signature));
    if let Some(doc) = signature.doc.as_ref().or(noted) {
        let _ = write!(out, "\n{doc}");
    }
    if let Some(location) = signature.location.or(defined_at) {
        let _ = write!(out, "\nDefined at {location}");
    }
    out
}

fn sorted_symbols(core: &mut Core, mut names: Vec<Symbol>) -> Value {
    names.sort_by(|a, b| core.symbols.name(*a).cmp(core.symbols.name(*b)));
    names.dedup();
    let names: Vec<Value> = names.into_iter().map(Value::Sym).collect();
    core.list(&names)
}

#[allow(clippy::too_many_lines)]
pub(super) fn install(core: &mut Core) {
    let controls = [
        (
            "apply",
            1,
            None,
            Control::Apply,
            "(apply procedure arg ... list) Calls the procedure with the args and the items of the list as arguments.",
        ),
        (
            "eval",
            1,
            Some(2),
            Control::Eval,
            "(eval datum) Runs a datum as code at the top level.",
        ),
        (
            "eval-string",
            1,
            Some(2),
            Control::EvalString,
            "(eval-string text [name]) Reads the text and runs everything in it, at the top level. Returns the last value. The name is what errors call the text; a file's path, when it comes from a file.",
        ),
        (
            "raise",
            1,
            Some(1),
            Control::Raise,
            "(raise object) Hands the object to the nearest handler, as an error that cannot be continued from.",
        ),
        (
            "raise-continuable",
            1,
            Some(1),
            Control::RaiseContinuable,
            "(raise-continuable object) Calls the nearest handler with the object; what it returns is returned here.",
        ),
        (
            "error",
            1,
            None,
            Control::Error,
            "(error message irritant ...) Raises an error with a message and the values it is about.",
        ),
        (
            "coroutine-resume",
            1,
            Some(2),
            Control::Resume,
            "(coroutine-resume coroutine [value]) Runs the coroutine until it yields or ends, and returns what it yielded or returned. The value is what its yield returns.",
        ),
        (
            "yield",
            0,
            Some(1),
            Control::Yield,
            "(yield [value]) Stops the running coroutine and hands the value to whoever resumed it. Returns what it is resumed with.",
        ),
        (
            "%suspend",
            1,
            Some(1),
            Control::Suspend,
            "Stops the running code and hands the value to the host program, which may continue it later.",
        ),
        (
            "%call-with-catch",
            2,
            Some(2),
            Control::CallWithCatch,
            "(%call-with-catch handler thunk) Calls thunk; if something is raised, leaves it and calls handler with what was raised.",
        ),
        (
            "with-exception-handler",
            2,
            Some(2),
            Control::WithExceptionHandler,
            "(with-exception-handler handler thunk) Calls thunk with handler in force: it is called, where the error is, with whatever is raised.",
        ),
    ];
    for (name, min, max, control, doc) in controls {
        core.define_native_kind(name, min, max, doc, NativeKind::Control(control));
    }

    core.define_native(
        "procedure?",
        1,
        Some(1),
        "(procedure? x) Whether x can be called.",
        |core, args| Ok(Value::Bool(core.is_procedure(args[0]))),
    );
    core.define_native(
        "void",
        0,
        None,
        "(void) The value that stands for no value.",
        |_, _| Ok(Value::Void),
    );
    core.define_native(
        "void?",
        1,
        Some(1),
        "(void? x) Whether x is the value that stands for no value.",
        |_, args| Ok(Value::Bool(args[0] == Value::Void)),
    );
    core.define_native(
        "eof-object",
        0,
        Some(0),
        "(eof-object) The value that marks the end of input.",
        |_, _| Ok(Value::Eof),
    );
    core.define_native(
        "eof-object?",
        1,
        Some(1),
        "(eof-object? x) Whether x marks the end of input.",
        |_, args| Ok(Value::Bool(args[0] == Value::Eof)),
    );
    core.define_native(
        "values",
        0,
        None,
        "(values x ...) Returns several values at once, for call-with-values and its like.",
        |core, args| {
            Ok(match args {
                [one] => *one,
                _ => core.alloc(Object::Values(args.to_vec())),
            })
        },
    );
    core.define_native(
        "%values->list",
        1,
        Some(1),
        "What a call returned, as a list of its values.",
        |core, args| match core.object(args[0]) {
            Some(Object::Values(values)) => {
                let values = values.clone();
                Ok(core.list(&values))
            }
            _ => Ok(core.list(&args[..1])),
        },
    );

    // Coroutines

    core.define_native(
        "make-coroutine",
        1,
        Some(1),
        "(make-coroutine thunk) A coroutine that will run thunk when first resumed. Inside it, (yield value) hands a value back and waits.",
        |core, args| {
            if !core.is_procedure(args[0]) {
                return Err(wrong(core, "make-coroutine", "a procedure", args[0]));
            }
            let mut fiber = Fiber::new();
            fiber.state = FiberState::New;
            fiber.stack = vec![Value::Obj(core.call0), args[0]];
            let coroutine = core.alloc(Object::Fiber(Box::new(fiber)));
            if let Some(handle) = coroutine.handle()
                && let Object::Fiber(fiber) = core.heap.get_mut(handle)
            {
                fiber.object = Some(handle);
            }
            Ok(coroutine)
        },
    );
    core.define_native(
        "coroutine?",
        1,
        Some(1),
        "(coroutine? x) Whether x is a coroutine.",
        |core, args| {
            Ok(Value::Bool(matches!(
                core.object(args[0]),
                Some(Object::Fiber(_))
            )))
        },
    );
    core.define_native(
        "coroutine-state",
        1,
        Some(1),
        "(coroutine-state coroutine) One of new, running, suspended and done.",
        |core, args| {
            let Some(Object::Fiber(fiber)) = core.object(args[0]) else {
                return Err(wrong(core, "coroutine-state", "a coroutine", args[0]));
            };
            let state = match fiber.state {
                FiberState::New => "new",
                FiberState::Running => "running",
                FiberState::Suspended => "suspended",
                FiberState::Done => "done",
            };
            Ok(core.symbol(state))
        },
    );

    // Errors and escapes

    core.define_native(
        "error-object?",
        1,
        Some(1),
        "(error-object? x) Whether x is an error, as made by error or by a failing procedure.",
        |core, args| {
            Ok(Value::Bool(matches!(
                core.object(args[0]),
                Some(Object::Error(_))
            )))
        },
    );
    core.define_native(
        "error-object-message",
        1,
        Some(1),
        "(error-object-message error) The error's message.",
        |core, args| {
            let message = error_object(core, "error-object-message", args[0])?
                .message
                .clone();
            Ok(core.string(message))
        },
    );
    core.define_native(
        "error-object-irritants",
        1,
        Some(1),
        "(error-object-irritants error) The values the error is about.",
        |core, args| {
            let irritants = error_object(core, "error-object-irritants", args[0])?
                .irritants
                .clone();
            Ok(core.list(&irritants))
        },
    );
    core.define_native(
        "error-object->string",
        1,
        Some(1),
        "(error-object->string error) The message and irritants as one line.",
        |core, args| {
            let error = error_object(core, "error-object->string", args[0])?;
            let mut line = error.message.clone();
            for irritant in &error.irritants {
                let _ = write!(line, " {}", core.write(*irritant));
            }
            Ok(core.string(line))
        },
    );
    core.define_native(
        "error-object-trace",
        1,
        Some(1),
        "(error-object-trace error) Where the error was raised: a list of (procedure-name . location) pairs, the innermost call first. Either part is #f if unknown.",
        |core, args| {
            let trace = error_object(core, "error-object-trace", args[0])?.trace.clone();
            let mut frames = Vec::new();
            for frame in trace {
                let name = frame.name.map_or(Value::Bool(false), |name| core.symbol(&name));
                let location = frame
                    .span
                    .and_then(|span| core.sources.location(span))
                    .map(|location| location.to_string());
                let location = optional_string(core, location);
                frames.push(core.cons(name, location));
            }
            Ok(core.list(&frames))
        },
    );
    core.define_native(
        "%make-escape",
        2,
        Some(2),
        "Makes what an escape procedure raises.",
        |core, args| Ok(core.alloc(Object::Escape(args[0], args[1]))),
    );
    core.define_native(
        "%escape?",
        1,
        Some(1),
        "Whether something raised is an escape rather than an error.",
        |core, args| {
            Ok(Value::Bool(matches!(
                core.object(args[0]),
                Some(Object::Escape(..))
            )))
        },
    );
    core.define_native(
        "%escape-tag",
        1,
        Some(1),
        "Which escape something raised is.",
        |core, args| match core.object(args[0]) {
            Some(Object::Escape(tag, _)) => Ok(*tag),
            _ => Err(wrong(core, "%escape-tag", "an escape", args[0])),
        },
    );
    core.define_native(
        "%escape-value",
        1,
        Some(1),
        "What an escape delivers.",
        |core, args| match core.object(args[0]) {
            Some(Object::Escape(_, value)) => Ok(*value),
            _ => Err(wrong(core, "%escape-value", "an escape", args[0])),
        },
    );

    // Output

    core.define_native(
        "display",
        1,
        Some(2),
        "(display x) Writes x for people to read: strings without quotes.",
        |core, args| {
            let shown = core.display(args[0]);
            core.output.push_str(&shown);
            Ok(Value::Void)
        },
    );
    for name in ["write", "write-shared", "write-simple"] {
        core.define_native(
            name,
            1,
            Some(2),
            "(write x) Writes x so that it can be read back: strings with quotes.",
            |core, args| {
                let shown = core.write(args[0]);
                core.output.push_str(&shown);
                Ok(Value::Void)
            },
        );
    }
    core.define_native(
        "newline",
        0,
        Some(1),
        "(newline) Ends the line of output.",
        |core, _| {
            core.output.push('\n');
            Ok(Value::Void)
        },
    );
    core.define_native(
        "write-string",
        1,
        Some(2),
        "(write-string string) Writes the string.",
        |core, args| {
            let string = text(core, "write-string", args[0])?.to_owned();
            core.output.push_str(&string);
            Ok(Value::Void)
        },
    );
    core.define_native(
        "write-char",
        1,
        Some(2),
        "(write-char char) Writes the char.",
        |core, args| {
            let c = super::character(core, "write-char", args[0])?;
            core.output.push(c);
            Ok(Value::Void)
        },
    );
    core.define_native(
        "%output-mark",
        0,
        Some(0),
        "How much output there is so far.",
        |core, _| Ok(Value::from(core.output.len())),
    );
    core.define_native(
        "%output-take",
        1,
        Some(1),
        "Takes back the output written since a mark.",
        |core, args| {
            let mark = index(core, "%output-take", args[0])?.min(core.output.len());
            let taken = core.output.split_off(mark);
            Ok(core.string(taken))
        },
    );
    core.define_native(
        "->string",
        1,
        Some(1),
        "(->string x) x as display shows it.",
        |core, args| {
            let shown = core.display(args[0]);
            Ok(core.string(shown))
        },
    );
    core.define_native(
        "write->string",
        1,
        Some(1),
        "(write->string x) x as write shows it.",
        |core, args| {
            let shown = core.write(args[0]);
            Ok(core.string(shown))
        },
    );

    core.define_native(
        "read-from-string",
        1,
        Some(1),
        "(read-from-string string) The first datum written in the string, or the end-of-input value if there is none. With eval, a way to run text as code.",
        |core, args| {
            let source = text(core, "read-from-string", args[0])?.to_owned();
            match crate::reader::read_next(&source, NO_FILE, 0, &mut core.symbols) {
                Ok(Some((datum, _))) => Ok(core.datum_value(&datum)),
                Ok(None) => Ok(Value::Eof),
                Err(error) => Err(Error::new(format!("read-from-string: {}", error.message))),
            }
        },
    );

    // Time

    let started = Instant::now();
    core.define_native(
        "current-jiffy",
        0,
        Some(0),
        "(current-jiffy) Microseconds since the interpreter started.",
        move |_, _| Ok(Value::Int(started.elapsed().as_micros() as i64)),
    );
    core.define_native(
        "jiffies-per-second",
        0,
        Some(0),
        "(jiffies-per-second) How many jiffies make a second.",
        |_, _| Ok(Value::Int(1_000_000)),
    );
    core.define_native(
        "current-second",
        0,
        Some(0),
        "(current-second) Seconds since 1970, as a float.",
        |_, _| {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default();
            Ok(Value::Float(now.as_secs_f64()))
        },
    );

    // The program looking at itself

    core.define_native(
        "procedure-name",
        1,
        Some(1),
        "(procedure-name procedure) The name the procedure was defined with, or #f.",
        |core, args| {
            Ok(signature(core, "procedure-name", args[0])?
                .name
                .map_or(Value::Bool(false), Value::Sym))
        },
    );
    core.define_native(
        "procedure-documentation",
        1,
        Some(1),
        "(procedure-documentation procedure) The string its body starts with, or #f.",
        |core, args| {
            let doc = signature(core, "procedure-documentation", args[0])?.doc;
            Ok(optional_string(core, doc))
        },
    );
    core.define_native(
        "procedure-arity",
        1,
        Some(1),
        "(procedure-arity procedure) A pair of the fewest and the most arguments it takes; the most is #f for any number.",
        |core, args| {
            let signature = signature(core, "procedure-arity", args[0])?;
            let max = signature.max.map_or(Value::Bool(false), Value::from);
            Ok(core.cons(Value::from(signature.min), max))
        },
    );
    core.define_native(
        "procedure-parameters",
        1,
        Some(1),
        "(procedure-parameters procedure) The parameters as written in its lambda, or #f for a procedure built in.",
        |core, args| {
            let signature = signature(core, "procedure-parameters", args[0])?;
            let Some(parameters) = signature.parameters else {
                return Ok(Value::Bool(false));
            };
            let mut names: Vec<Value> = parameters.into_iter().map(Value::Sym).collect();
            let tail = if signature.rest { names.pop().unwrap_or(Value::Nil) } else { Value::Nil };
            Ok(core.list_with_tail(&names, tail))
        },
    );
    core.define_native(
        "procedure-location",
        1,
        Some(1),
        "(procedure-location procedure) Where it is defined, as \"file:line:column\", or #f.",
        |core, args| {
            let location = signature(core, "procedure-location", args[0])?.location;
            Ok(optional_string(core, location))
        },
    );
    core.define_native(
        "procedure-source",
        1,
        Some(1),
        "(procedure-source procedure) The text of its lambda as it was read, or #f.",
        |core, args| {
            let source = signature(core, "procedure-source", args[0])?.source;
            Ok(optional_string(core, source))
        },
    );
    core.define_native(
        "describe",
        1,
        Some(1),
        "(describe 'name) What a name means at the top level, as text: how it is called, its documentation and where it is defined.",
        |core, args| {
            let described = describe(core, symbol(core, "describe", args[0])?);
            Ok(core.string(described))
        },
    );
    core.define_native(
        "set-documentation!",
        2,
        Some(2),
        "(set-documentation! 'name text) Gives documentation to a name that cannot carry its own: a macro or a variable. `describe` shows it.",
        |core, args| {
            let name = symbol(core, "set-documentation!", args[0])?;
            let doc = text(core, "set-documentation!", args[1])?.to_owned();
            core.docs.insert(name, doc);
            Ok(Value::Void)
        },
    );
    core.define_native(
        "documentation",
        1,
        Some(1),
        "(documentation 'name) The documentation of what a name means at the top level, or #f: a procedure's own, or what `set-documentation!` gave.",
        |core, args| {
            let name = symbol(core, "documentation", args[0])?;
            let own = core
                .global(name)
                .and_then(|global| signature(core, "documentation", global.value).ok())
                .and_then(|signature| signature.doc);
            let doc = own.or_else(|| core.docs.get(&name).cloned());
            Ok(optional_string(core, doc))
        },
    );
    core.define_native(
        "apropos",
        1,
        Some(1),
        "(apropos part) The names defined at the top level that contain part, a string or symbol: variables, macros and special forms.",
        |core, args| {
            let part = match args[0] {
                Value::Sym(symbol) => core.symbols.name(symbol).to_owned(),
                other => text(core, "apropos", other)?.to_owned(),
            };
            let names: Vec<Symbol> = core
                .symbols
                .all()
                .filter(|name| core.symbols.name(*name).contains(&part))
                .filter(|name| {
                    core.global(*name).is_some()
                        || core.macros.contains_key(name)
                        || core.specials.contains_key(name)
                })
                .collect();
            Ok(sorted_symbols(core, names))
        },
    );
    core.define_native(
        "global-names",
        0,
        Some(0),
        "(global-names) Every variable defined at the top level.",
        |core, _| {
            let names: Vec<Symbol> = core
                .globals
                .iter()
                .filter(|global| global.value != Value::Unbound)
                .map(|global| global.name)
                .collect();
            Ok(sorted_symbols(core, names))
        },
    );
    core.define_native(
        "bound?",
        1,
        Some(1),
        "(bound? 'name) Whether the name is a variable defined at the top level.",
        |core, args| {
            Ok(Value::Bool(
                core.global(symbol(core, "bound?", args[0])?).is_some(),
            ))
        },
    );
    core.define_native(
        "macro?",
        1,
        Some(1),
        "(macro? 'name) Whether the name is a macro or special form.",
        |core, args| {
            let name = symbol(core, "macro?", args[0])?;
            Ok(Value::Bool(
                core.macros.contains_key(&name) || core.specials.contains_key(&name),
            ))
        },
    );
    core.define_native(
        "global-ref",
        1,
        Some(2),
        "(global-ref 'name [default]) The value of a top-level variable.",
        |core, args| {
            let name = symbol(core, "global-ref", args[0])?;
            match (core.lookup(name), args.get(1)) {
                (Some(value), _) => Ok(value),
                (None, Some(default)) => Ok(*default),
                (None, None) => Err(Error::new(format!(
                    "unbound variable: {}",
                    core.symbols.name(name)
                ))),
            }
        },
    );
    core.define_native(
        "global-set!",
        2,
        Some(2),
        "(global-set! 'name value) Defines or redefines a top-level variable. Everything that uses the name sees the new value.",
        |core, args| {
            let name = symbol(core, "global-set!", args[0])?;
            let slot = core.global_slot(name);
            core.globals[slot as usize].value = args[1];
            Ok(Value::Void)
        },
    );
    core.define_native(
        "global-location",
        1,
        Some(1),
        "(global-location 'name) Where the name was last defined, as \"file:line:column\", or #f.",
        |core, args| {
            let name = symbol(core, "global-location", args[0])?;
            let location = core
                .global(name)
                .and_then(|global| global.defined_at)
                .and_then(|span| core.sources.location(span))
                .map(|location| location.to_string());
            Ok(optional_string(core, location))
        },
    );
}
