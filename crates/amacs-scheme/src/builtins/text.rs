//! Chars, strings and symbols. Positions in strings count chars.

use super::{character, index, int, list, range, symbol, text, wrong};
use crate::error::Error;
use crate::value::{Object, Value};
use crate::vm::Core;

/// The byte offset of char number `at`.
fn offset(text: &str, at: usize) -> usize {
    text.char_indices()
        .nth(at)
        .map_or(text.len(), |(offset, _)| offset)
}

/// The part of a string that optional start and end arguments select.
fn slice<'a>(core: &'a Core, who: &str, string: Value, bounds: &[Value]) -> Result<&'a str, Error> {
    let whole = text(core, who, string)?;
    if bounds.is_empty() {
        return Ok(whole);
    }
    let (start, end) = range(core, who, bounds, whole.chars().count())?;
    Ok(&whole[offset(whole, start)..offset(whole, end)])
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

#[allow(clippy::too_many_lines)]
pub(super) fn install(core: &mut Core) {
    // Symbols

    core.define_native(
        "symbol?",
        1,
        Some(1),
        "(symbol? x) Whether x is a symbol.",
        |_, args| Ok(Value::Bool(matches!(args[0], Value::Sym(_)))),
    );
    core.define_native(
        "symbol->string",
        1,
        Some(1),
        "(symbol->string symbol) The symbol's name.",
        |core, args| {
            let name = core
                .symbols
                .name(symbol(core, "symbol->string", args[0])?)
                .to_owned();
            Ok(core.string(name))
        },
    );
    core.define_native(
        "string->symbol",
        1,
        Some(1),
        "(string->symbol string) The symbol with that name.",
        |core, args| {
            let name = text(core, "string->symbol", args[0])?.to_owned();
            Ok(core.symbol(&name))
        },
    );
    core.define_native(
        "symbol=?",
        2,
        None,
        "(symbol=? a b ...) Whether the symbols are all the same.",
        |core, args| {
            for arg in args {
                symbol(core, "symbol=?", *arg)?;
            }
            Ok(Value::Bool(args.windows(2).all(|two| two[0] == two[1])))
        },
    );
    core.define_native(
        "gensym",
        0,
        Some(1),
        "(gensym [base]) A new symbol that is different from every other.",
        |core, args| {
            let base = match args.first() {
                Some(Value::Sym(symbol)) => core.symbols.name(*symbol).to_owned(),
                Some(other) => text(core, "gensym", *other)?.to_owned(),
                None => "g".to_owned(),
            };
            Ok(Value::Sym(core.symbols.fresh(&base)))
        },
    );
    core.define_native("keyword?", 1, Some(1), "(keyword? x) Whether x is a keyword such as #:name.", |core, args| {
        Ok(Value::Bool(
            matches!(args[0], Value::Sym(symbol) if core.symbols.name(symbol).starts_with("#:")),
        ))
    });

    // Chars

    core.define_native(
        "char?",
        1,
        Some(1),
        "(char? x) Whether x is a char.",
        |_, args| Ok(Value::Bool(matches!(args[0], Value::Char(_)))),
    );
    core.define_native(
        "char->integer",
        1,
        Some(1),
        "(char->integer char) The char's Unicode number.",
        |core, args| {
            Ok(Value::Int(i64::from(u32::from(character(
                core,
                "char->integer",
                args[0],
            )?))))
        },
    );
    core.define_native(
        "integer->char",
        1,
        Some(1),
        "(integer->char n) The char with that Unicode number.",
        |core, args| {
            u32::try_from(int(core, "integer->char", args[0])?)
                .ok()
                .and_then(char::from_u32)
                .map(Value::Char)
                .ok_or_else(|| wrong(core, "integer->char", "the number of a char", args[0]))
        },
    );
    type CharTest = fn(char) -> bool;
    let char_tests: [(&str, CharTest, &str); 5] = [
        (
            "char-alphabetic?",
            char::is_alphabetic,
            "(char-alphabetic? char) Whether it is a letter.",
        ),
        (
            "char-numeric?",
            char::is_numeric,
            "(char-numeric? char) Whether it is a digit.",
        ),
        (
            "char-whitespace?",
            char::is_whitespace,
            "(char-whitespace? char) Whether it is blank.",
        ),
        (
            "char-upper-case?",
            char::is_uppercase,
            "(char-upper-case? char) Whether it is a capital letter.",
        ),
        (
            "char-lower-case?",
            char::is_lowercase,
            "(char-lower-case? char) Whether it is a small letter.",
        ),
    ];
    for (name, test, doc) in char_tests {
        core.define_native(name, 1, Some(1), doc, move |core, args| {
            Ok(Value::Bool(test(character(core, name, args[0])?)))
        });
    }
    type CharMap = fn(char) -> char;
    let char_maps: [(&str, CharMap, &str); 3] = [
        (
            "char-upcase",
            |c| c.to_uppercase().next().unwrap_or(c),
            "(char-upcase char) The capital of a letter.",
        ),
        (
            "char-downcase",
            fold,
            "(char-downcase char) The small form of a letter.",
        ),
        (
            "char-foldcase",
            fold,
            "(char-foldcase char) The char as compared when case is ignored.",
        ),
    ];
    for (name, map, doc) in char_maps {
        core.define_native(name, 1, Some(1), doc, move |core, args| {
            Ok(Value::Char(map(character(core, name, args[0])?)))
        });
    }
    core.define_native(
        "digit-value",
        1,
        Some(1),
        "(digit-value char) The number a digit stands for, or #f.",
        |core, args| {
            Ok(character(core, "digit-value", args[0])?
                .to_digit(10)
                .map_or(Value::Bool(false), |digit| Value::Int(i64::from(digit))))
        },
    );

    type Order = fn(std::cmp::Ordering) -> bool;
    let orders: [(&str, Order); 5] = [
        ("=?", |o| o.is_eq()),
        ("<?", |o| o.is_lt()),
        (">?", |o| o.is_gt()),
        ("<=?", |o| o.is_le()),
        (">=?", |o| o.is_ge()),
    ];
    for (suffix, test) in orders {
        for ignore_case in [false, true] {
            let ci = if ignore_case { "-ci" } else { "" };
            let name: &'static str = format!("char{ci}{suffix}").leak();
            core.define_native(
                name,
                1,
                None,
                "(char<? a b ...) and its like compare chars; the -ci forms ignore case.",
                move |core, args| {
                    let mut chars = Vec::new();
                    for arg in args {
                        let c = character(core, name, *arg)?;
                        chars.push(if ignore_case { fold(c) } else { c });
                    }
                    Ok(Value::Bool(
                        chars.windows(2).all(|two| test(two[0].cmp(&two[1]))),
                    ))
                },
            );
            let name: &'static str = format!("string{ci}{suffix}").leak();
            core.define_native(
                name,
                1,
                None,
                "(string<? a b ...) and its like compare strings; the -ci forms ignore case.",
                move |core, args| {
                    let mut strings = Vec::new();
                    for arg in args {
                        let string = text(core, name, *arg)?;
                        strings.push(if ignore_case {
                            string.to_lowercase()
                        } else {
                            string.to_owned()
                        });
                    }
                    Ok(Value::Bool(
                        strings.windows(2).all(|two| test(two[0].cmp(&two[1]))),
                    ))
                },
            );
        }
    }

    // Strings

    core.define_native(
        "string?",
        1,
        Some(1),
        "(string? x) Whether x is a string.",
        |core, args| Ok(Value::Bool(core.as_str(args[0]).is_some())),
    );
    core.define_native(
        "make-string",
        1,
        Some(2),
        "(make-string n [char]) A string of n chars.",
        |core, args| {
            let count = index(core, "make-string", args[0])?;
            let fill = args
                .get(1)
                .map_or(Ok(' '), |arg| character(core, "make-string", *arg))?;
            Ok(core.string(fill.to_string().repeat(count)))
        },
    );
    core.define_native(
        "string",
        0,
        None,
        "(string x ...) A string of the chars; other things are added as display shows them.",
        |core, args| {
            let mut out = String::new();
            for arg in args {
                out.push_str(&core.display(*arg));
            }
            Ok(core.string(out))
        },
    );
    core.define_native(
        "string-length",
        1,
        Some(1),
        "(string-length string) The number of chars.",
        |core, args| {
            Ok(Value::from(
                text(core, "string-length", args[0])?.chars().count(),
            ))
        },
    );
    core.define_native(
        "string-ref",
        2,
        Some(2),
        "(string-ref string k) Char number k, counting from zero.",
        |core, args| {
            let at = index(core, "string-ref", args[1])?;
            text(core, "string-ref", args[0])?
                .chars()
                .nth(at)
                .map(Value::Char)
                .ok_or_else(|| Error::new(format!("string-ref: there is no char number {at}")))
        },
    );
    for name in ["substring", "string-copy"] {
        core.define_native(
            name,
            1,
            Some(3),
            "(substring string start [end]) (string-copy string [start end]) A new string of the chars from start up to end.",
            move |core, args| {
                let part = slice(core, name, args[0], &args[1..])?.to_owned();
                Ok(core.string(part))
            },
        );
    }
    core.define_native(
        "string-append",
        0,
        None,
        "(string-append string ...) The strings joined.",
        |core, args| {
            let mut out = String::new();
            for arg in args {
                out.push_str(text(core, "string-append", *arg)?);
            }
            Ok(core.string(out))
        },
    );
    core.define_native(
        "string->list",
        1,
        Some(3),
        "(string->list string [start end]) The chars as a list.",
        |core, args| {
            let chars: Vec<Value> = slice(core, "string->list", args[0], &args[1..])?
                .chars()
                .map(Value::Char)
                .collect();
            Ok(core.list(&chars))
        },
    );
    core.define_native(
        "list->string",
        1,
        Some(1),
        "(list->string list) A string of the chars in a list.",
        |core, args| {
            let mut out = String::new();
            for item in list(core, "list->string", args[0])? {
                out.push(character(core, "list->string", item)?);
            }
            Ok(core.string(out))
        },
    );
    core.define_native(
        "string->vector",
        1,
        Some(3),
        "(string->vector string [start end]) The chars as a vector.",
        |core, args| {
            let chars = slice(core, "string->vector", args[0], &args[1..])?
                .chars()
                .map(Value::Char)
                .collect();
            Ok(core.alloc(Object::Vector(chars)))
        },
    );
    core.define_native(
        "vector->string",
        1,
        Some(3),
        "(vector->string vector [start end]) A string of the chars in a vector.",
        |core, args| {
            let Some(Object::Vector(items)) = core.object(args[0]) else {
                return Err(wrong(core, "vector->string", "a vector", args[0]));
            };
            let (start, end) = range(core, "vector->string", &args[1..], items.len())?;
            let mut out = String::new();
            for item in &items[start..end] {
                out.push(character(core, "vector->string", *item)?);
            }
            Ok(core.string(out))
        },
    );
    type StringMap = fn(&str) -> String;
    let string_maps: [(&str, StringMap, &str); 6] = [
        (
            "string-upcase",
            str::to_uppercase,
            "(string-upcase string) The string in capitals.",
        ),
        (
            "string-downcase",
            str::to_lowercase,
            "(string-downcase string) The string in small letters.",
        ),
        (
            "string-foldcase",
            str::to_lowercase,
            "(string-foldcase string) The string as compared when case is ignored.",
        ),
        (
            "string-trim",
            |s| s.trim().to_owned(),
            "(string-trim string) The string without blanks at either end.",
        ),
        (
            "string-trim-left",
            |s| s.trim_start().to_owned(),
            "(string-trim-left string) The string without blanks at the start.",
        ),
        (
            "string-trim-right",
            |s| s.trim_end().to_owned(),
            "(string-trim-right string) The string without blanks at the end.",
        ),
    ];
    for (name, map, doc) in string_maps {
        core.define_native(name, 1, Some(1), doc, move |core, args| {
            let mapped = map(text(core, name, args[0])?);
            Ok(core.string(mapped))
        });
    }
    core.define_native(
        "string-null?",
        1,
        Some(1),
        "(string-null? string) Whether the string is empty.",
        |core, args| Ok(Value::Bool(text(core, "string-null?", args[0])?.is_empty())),
    );
    core.define_native(
        "string-prefix?",
        2,
        Some(2),
        "(string-prefix? prefix string) Whether the string starts with the prefix.",
        |core, args| {
            let prefix = text(core, "string-prefix?", args[0])?;
            Ok(Value::Bool(
                text(core, "string-prefix?", args[1])?.starts_with(prefix),
            ))
        },
    );
    core.define_native(
        "string-suffix?",
        2,
        Some(2),
        "(string-suffix? suffix string) Whether the string ends with the suffix.",
        |core, args| {
            let suffix = text(core, "string-suffix?", args[0])?;
            Ok(Value::Bool(
                text(core, "string-suffix?", args[1])?.ends_with(suffix),
            ))
        },
    );
    core.define_native(
        "string-search",
        2,
        Some(3),
        "(string-search string part [start]) Where part first occurs in string at or after start, or #f.",
        |core, args| {
            let whole = text(core, "string-search", args[0])?;
            let part = text(core, "string-search", args[1])?;
            let start = args.get(2).map_or(Ok(0), |arg| index(core, "string-search", *arg))?;
            let from = offset(whole, start);
            Ok(whole[from..]
                .find(part)
                .map_or(Value::Bool(false), |found| {
                    Value::from(whole[..from + found].chars().count())
                }))
        },
    );
    core.define_native(
        "string-contains?",
        2,
        Some(2),
        "(string-contains? string part) Whether part occurs in string.",
        |core, args| {
            let part = text(core, "string-contains?", args[1])?;
            Ok(Value::Bool(
                text(core, "string-contains?", args[0])?.contains(part),
            ))
        },
    );
    core.define_native(
        "string-join",
        1,
        Some(2),
        "(string-join strings [separator]) The strings of a list joined, with the separator (a space) between.",
        |core, args| {
            let separator = args.get(1).map_or(Ok(" "), |arg| text(core, "string-join", *arg))?;
            let mut parts = Vec::new();
            for item in list(core, "string-join", args[0])? {
                parts.push(text(core, "string-join", item)?);
            }
            let joined = parts.join(separator);
            Ok(core.string(joined))
        },
    );
    core.define_native(
        "string-split",
        1,
        Some(2),
        "(string-split string [separator]) The parts of the string between occurrences of the separator, a string or char. Without one, the words between blanks.",
        |core, args| {
            let whole = text(core, "string-split", args[0])?;
            let parts: Vec<String> = match args.get(1) {
                None => whole.split_whitespace().map(str::to_owned).collect(),
                Some(Value::Char(c)) => whole.split(*c).map(str::to_owned).collect(),
                Some(other) => {
                    let separator = text(core, "string-split", *other)?;
                    if separator.is_empty() {
                        return Err(Error::new("string-split: the separator is empty"));
                    }
                    whole.split(separator).map(str::to_owned).collect()
                }
            };
            let parts: Vec<Value> = parts.into_iter().map(|part| core.string(part)).collect();
            Ok(core.list(&parts))
        },
    );
    core.define_native(
        "string-replace",
        3,
        Some(3),
        "(string-replace string from to) The string with every occurrence of from replaced by to.",
        |core, args| {
            let from = text(core, "string-replace", args[1])?;
            if from.is_empty() {
                return Err(Error::new("string-replace: what to replace is empty"));
            }
            let replaced = text(core, "string-replace", args[0])?
                .replace(from, text(core, "string-replace", args[2])?);
            Ok(core.string(replaced))
        },
    );
    for (name, left) in [("string-pad-left", true), ("string-pad-right", false)] {
        core.define_native(
            name,
            2,
            Some(3),
            "(string-pad-left string n [char]) (string-pad-right string n [char]) The string made n chars long, by padding or cutting on that side.",
            move |core, args| {
                let chars: Vec<char> = text(core, name, args[0])?.chars().collect();
                let width = index(core, name, args[1])?;
                let fill = args.get(2).map_or(Ok(' '), |arg| character(core, name, *arg))?;
                let padding = std::iter::repeat_n(fill, width.saturating_sub(chars.len()));
                let padded: String = if left {
                    let kept = &chars[chars.len().saturating_sub(width)..];
                    padding.chain(kept.iter().copied()).collect()
                } else {
                    let kept = &chars[..chars.len().min(width)];
                    kept.iter().copied().chain(padding).collect()
                };
                Ok(core.string(padded))
            },
        );
    }
}
