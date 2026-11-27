//! Pairs and lists, and the tests for sameness.

use super::{Outcome, equal, eqv, index, list, pair, wrong};
use crate::error::Error;
use crate::value::{Object, Value};
use crate::vm::Core;

/// Follows a path of `a`s and `d`s into a structure of pairs, as in `cadr`.
fn path(core: &Core, who: &str, steps: &str, mut value: Value) -> Outcome {
    for step in steps.chars().rev() {
        let (car, cdr) = pair(core, who, value)?;
        value = if step == 'a' { car } else { cdr };
    }
    Ok(value)
}

fn member(
    core: &Core,
    who: &str,
    args: &[Value],
    same: fn(&Core, Value, Value) -> bool,
) -> Outcome {
    let mut rest = args[1];
    while let Some((car, cdr)) = core.as_pair(rest) {
        if same(core, args[0], car) {
            return Ok(rest);
        }
        rest = cdr;
    }
    match rest {
        Value::Nil => Ok(Value::Bool(false)),
        _ => Err(wrong(core, who, "a list", args[1])),
    }
}

fn assoc(core: &Core, who: &str, args: &[Value], same: fn(&Core, Value, Value) -> bool) -> Outcome {
    let mut rest = args[1];
    while let Some((entry, cdr)) = core.as_pair(rest) {
        let (key, _) = pair(core, who, entry)?;
        if same(core, args[0], key) {
            return Ok(entry);
        }
        rest = cdr;
    }
    match rest {
        Value::Nil => Ok(Value::Bool(false)),
        _ => Err(wrong(core, who, "a list of pairs", args[1])),
    }
}

#[allow(clippy::too_many_lines)]
pub(super) fn install(core: &mut Core) {
    core.define_native(
        "eq?",
        2,
        Some(2),
        "(eq? a b) Whether a and b are the same object.",
        |_, args| Ok(Value::Bool(eqv(args[0], args[1]))),
    );
    core.define_native(
        "eqv?",
        2,
        Some(2),
        "(eqv? a b) Whether a and b are the same object, number or char.",
        |_, args| Ok(Value::Bool(eqv(args[0], args[1]))),
    );
    core.define_native(
        "equal?",
        2,
        Some(2),
        "(equal? a b) Whether a and b have the same structure and contents.",
        |core, args| Ok(Value::Bool(equal(core, args[0], args[1]))),
    );
    core.define_native(
        "not",
        1,
        Some(1),
        "(not x) #t if x is #f, otherwise #f.",
        |_, args| Ok(Value::Bool(args[0] == Value::Bool(false))),
    );
    core.define_native(
        "boolean?",
        1,
        Some(1),
        "(boolean? x) Whether x is #t or #f.",
        |_, args| Ok(Value::Bool(matches!(args[0], Value::Bool(_)))),
    );
    core.define_native(
        "boolean=?",
        2,
        None,
        "(boolean=? a b ...) Whether the booleans are all the same.",
        |core, args| {
            for arg in args {
                if !matches!(arg, Value::Bool(_)) {
                    return Err(wrong(core, "boolean=?", "a boolean", *arg));
                }
            }
            Ok(Value::Bool(args.windows(2).all(|two| two[0] == two[1])))
        },
    );

    core.define_native(
        "cons",
        2,
        Some(2),
        "(cons a d) A new pair of a and d.",
        |core, args| Ok(core.cons(args[0], args[1])),
    );
    for name in [
        "car", "cdr", "caar", "cadr", "cdar", "cddr", "caddr", "cdddr", "cadddr", "caadr", "cddar",
        "cdadr", "cadar",
    ] {
        let steps = &name[1..name.len() - 1];
        core.define_native(
            name,
            1,
            Some(1),
            "(car pair) (cdr pair) The first and second part of a pair. Names such as cadr combine them: the car of the cdr.",
            move |core, args| path(core, name, steps, args[0]),
        );
    }
    for (name, steps) in [
        ("first", "a"),
        ("second", "ad"),
        ("third", "add"),
        ("rest", "d"),
    ] {
        core.define_native(
            name,
            1,
            Some(1),
            "(first list) (second list) (third list) (rest list) Parts of a list.",
            move |core, args| path(core, name, steps, args[0]),
        );
    }
    core.define_native(
        "set-car!",
        2,
        Some(2),
        "(set-car! pair value) Changes the first part of a pair.",
        |core, args| {
            match args[0].handle().map(|handle| core.heap.get_mut(handle)) {
                Some(Object::Pair(car, _)) => *car = args[1],
                _ => return Err(wrong(core, "set-car!", "a pair", args[0])),
            }
            Ok(Value::Void)
        },
    );
    core.define_native(
        "set-cdr!",
        2,
        Some(2),
        "(set-cdr! pair value) Changes the second part of a pair.",
        |core, args| {
            match args[0].handle().map(|handle| core.heap.get_mut(handle)) {
                Some(Object::Pair(_, cdr)) => *cdr = args[1],
                _ => return Err(wrong(core, "set-cdr!", "a pair", args[0])),
            }
            Ok(Value::Void)
        },
    );
    core.define_native(
        "pair?",
        1,
        Some(1),
        "(pair? x) Whether x is a pair.",
        |core, args| Ok(Value::Bool(core.as_pair(args[0]).is_some())),
    );
    core.define_native(
        "null?",
        1,
        Some(1),
        "(null? x) Whether x is the empty list.",
        |_, args| Ok(Value::Bool(args[0] == Value::Nil)),
    );
    core.define_native(
        "list?",
        1,
        Some(1),
        "(list? x) Whether x is a list that ends.",
        |core, args| Ok(Value::Bool(core.list_length(args[0]).is_some())),
    );
    core.define_native(
        "list",
        0,
        None,
        "(list item ...) A new list of the items.",
        |core, args| Ok(core.list(args)),
    );
    core.define_native(
        "cons*",
        1,
        None,
        "(cons* item ... tail) A list of the items in front of tail.",
        |core, args| {
            let (tail, items) = args.split_last().expect("at least one argument");
            Ok(core.list_with_tail(items, *tail))
        },
    );
    core.define_native(
        "make-list",
        1,
        Some(2),
        "(make-list n [fill]) A list of n items, each fill.",
        |core, args| {
            let count = index(core, "make-list", args[0])?;
            let fill = args.get(1).copied().unwrap_or(Value::Void);
            Ok(core.list(&vec![fill; count]))
        },
    );
    core.define_native(
        "length",
        1,
        Some(1),
        "(length list) The number of items.",
        |core, args| match core.list_length(args[0]) {
            Some(count) => Ok(Value::from(count)),
            None => Err(wrong(core, "length", "a list", args[0])),
        },
    );
    core.define_native(
        "append",
        0,
        None,
        "(append list ... tail) The lists joined. The last argument is shared, not copied.",
        |core, args| {
            let Some((tail, lists)) = args.split_last() else {
                return Ok(Value::Nil);
            };
            let mut items = Vec::new();
            for part in lists {
                items.extend(list(core, "append", *part)?);
            }
            Ok(core.list_with_tail(&items, *tail))
        },
    );
    core.define_native(
        "reverse",
        1,
        Some(1),
        "(reverse list) A list of the items in the opposite order.",
        |core, args| {
            // Straight from one list to the other, once it is known that
            // the list ends.
            if core.list_length(args[0]).is_none() {
                return Err(wrong(core, "reverse", "a list", args[0]));
            }
            let mut reversed = Value::Nil;
            let mut rest = args[0];
            while let Some((car, cdr)) = core.as_pair(rest) {
                reversed = core.cons(car, reversed);
                rest = cdr;
            }
            Ok(reversed)
        },
    );
    core.define_native(
        "list-copy",
        1,
        Some(1),
        "(list-copy list) A new list of the same items. What is not a pair at the end is shared.",
        |core, args| {
            let mut items = Vec::new();
            let mut rest = args[0];
            while let Some((car, cdr)) = core.as_pair(rest) {
                items.push(car);
                rest = cdr;
                if items.len() > core.heap.len() {
                    return Err(wrong(core, "list-copy", "a list that ends", args[0]));
                }
            }
            Ok(core.list_with_tail(&items, rest))
        },
    );
    core.define_native(
        "list-tail",
        2,
        Some(2),
        "(list-tail list k) The list without its first k items.",
        |core, args| {
            let mut rest = args[0];
            for _ in 0..index(core, "list-tail", args[1])? {
                rest = pair(core, "list-tail", rest)
                    .map_err(|_| Error::new("list-tail: the list is too short"))?
                    .1;
            }
            Ok(rest)
        },
    );
    core.define_native(
        "list-ref",
        2,
        Some(2),
        "(list-ref list k) Item number k, counting from zero.",
        |core, args| {
            let mut rest = args[0];
            for _ in 0..index(core, "list-ref", args[1])? {
                rest = core
                    .as_pair(rest)
                    .ok_or_else(|| Error::new("list-ref: the list is too short"))?
                    .1;
            }
            Ok(core
                .as_pair(rest)
                .ok_or_else(|| Error::new("list-ref: the list is too short"))?
                .0)
        },
    );
    core.define_native(
        "last-pair",
        1,
        Some(1),
        "(last-pair list) The last pair of a list.",
        |core, args| {
            let mut rest = args[0];
            pair(core, "last-pair", rest)?;
            while let Some((_, cdr)) = core.as_pair(rest)
                && core.as_pair(cdr).is_some()
            {
                rest = cdr;
            }
            Ok(rest)
        },
    );
    core.define_native(
        "last",
        1,
        Some(1),
        "(last list) The last item of a list.",
        |core, args| {
            list(core, "last", args[0])?
                .last()
                .copied()
                .ok_or_else(|| Error::new("last: the list is empty"))
        },
    );
    core.define_native(
        "iota",
        1,
        Some(3),
        "(iota count [start step]) A list of count numbers, from start (0) in steps of step (1).",
        |core, args| {
            let count = index(core, "iota", args[0])?;
            let start = args.get(1).copied().unwrap_or(Value::Int(0));
            let step = args.get(2).copied().unwrap_or(Value::Int(1));
            let items: Vec<Value> = match (start, step) {
                (Value::Int(start), Value::Int(step)) => (0..count as i64)
                    .map(|index| Value::Int(start + index * step))
                    .collect(),
                (Value::Int(_) | Value::Float(_), Value::Int(_) | Value::Float(_)) => {
                    let float = |value| match value {
                        Value::Int(value) => value as f64,
                        Value::Float(value) => value,
                        _ => 0.0,
                    };
                    (0..count)
                        .map(|index| Value::Float(float(start) + index as f64 * float(step)))
                        .collect()
                }
                _ => return Err(wrong(core, "iota", "numbers", start)),
            };
            Ok(core.list(&items))
        },
    );

    type Same = fn(&Core, Value, Value) -> bool;
    let by_identity: Same = |_, a, b| eqv(a, b);
    let searches: [(&str, &str, Same); 3] = [
        ("memq", "assq", by_identity),
        ("memv", "assv", by_identity),
        ("%member", "%assoc", equal),
    ];
    for (member_name, assoc_name, same) in searches {
        core.define_native(
            member_name,
            2,
            Some(2),
            "(memq x list) (memv x list) (member x list [same?]) The part of the list that starts with x, or #f.",
            move |core, args| member(core, member_name, args, same),
        );
        core.define_native(
            assoc_name,
            2,
            Some(2),
            "(assq key alist) (assv key alist) (assoc key alist [same?]) The first pair in the list whose car is the key, or #f.",
            move |core, args| assoc(core, assoc_name, args, same),
        );
    }
}
