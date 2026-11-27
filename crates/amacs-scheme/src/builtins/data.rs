//! Vectors, hash tables, records and parameters.

use super::{index, key, list, range, symbol, wrong};
use crate::error::Error;
use crate::value::{Object, Table, Value};
use crate::vm::Core;

fn vector<'a>(core: &'a Core, who: &str, value: Value) -> Result<&'a Vec<Value>, Error> {
    match core.object(value) {
        Some(Object::Vector(items)) => Ok(items),
        _ => Err(wrong(core, who, "a vector", value)),
    }
}

fn vector_mut<'a>(
    core: &'a mut Core,
    who: &str,
    value: Value,
) -> Result<&'a mut Vec<Value>, Error> {
    if vector(core, who, value).is_err() {
        return Err(wrong(core, who, "a vector", value));
    }
    match value.handle().map(|handle| core.heap.get_mut(handle)) {
        Some(Object::Vector(items)) => Ok(items),
        _ => unreachable!("checked to be a vector"),
    }
}

fn table<'a>(core: &'a Core, who: &str, value: Value) -> Result<&'a Table, Error> {
    match core.object(value) {
        Some(Object::Table(table)) => Ok(table),
        _ => Err(wrong(core, who, "a hash table", value)),
    }
}

fn table_mut<'a>(core: &'a mut Core, who: &str, value: Value) -> Result<&'a mut Table, Error> {
    if table(core, who, value).is_err() {
        return Err(wrong(core, who, "a hash table", value));
    }
    match value.handle().map(|handle| core.heap.get_mut(handle)) {
        Some(Object::Table(table)) => Ok(table),
        _ => unreachable!("checked to be a hash table"),
    }
}

/// The fields of a record of the given type.
fn record(core: &Core, who: Value, kind: Value, value: Value) -> Result<&[Value], Error> {
    match core.object(value) {
        Some(Object::Record {
            kind: actual,
            fields,
        }) if Value::Obj(*actual) == kind => Ok(fields),
        _ => {
            let name = match core.object(kind) {
                Some(Object::RecordType { name, .. }) => core.symbols.name(*name),
                _ => "record",
            };
            Err(wrong(core, &core.display(who), &format!("a {name}"), value))
        }
    }
}

#[allow(clippy::too_many_lines)]
pub(super) fn install(core: &mut Core) {
    // Vectors

    core.define_native(
        "vector?",
        1,
        Some(1),
        "(vector? x) Whether x is a vector.",
        |core, args| {
            Ok(Value::Bool(matches!(
                core.object(args[0]),
                Some(Object::Vector(_))
            )))
        },
    );
    core.define_native(
        "make-vector",
        1,
        Some(2),
        "(make-vector n [fill]) A vector of n items.",
        |core, args| {
            let count = index(core, "make-vector", args[0])?;
            let fill = args.get(1).copied().unwrap_or(Value::Int(0));
            Ok(core.alloc(Object::Vector(vec![fill; count])))
        },
    );
    core.define_native(
        "vector",
        0,
        None,
        "(vector item ...) A new vector of the items.",
        |core, args| Ok(core.alloc(Object::Vector(args.to_vec()))),
    );
    core.define_native(
        "vector-length",
        1,
        Some(1),
        "(vector-length vector) The number of items.",
        |core, args| Ok(Value::from(vector(core, "vector-length", args[0])?.len())),
    );
    core.define_native(
        "vector-ref",
        2,
        Some(2),
        "(vector-ref vector k) Item number k, counting from zero.",
        |core, args| {
            let at = index(core, "vector-ref", args[1])?;
            vector(core, "vector-ref", args[0])?
                .get(at)
                .copied()
                .ok_or_else(|| Error::new(format!("vector-ref: there is no item number {at}")))
        },
    );
    core.define_native(
        "vector-set!",
        3,
        Some(3),
        "(vector-set! vector k value) Changes item number k.",
        |core, args| {
            let at = index(core, "vector-set!", args[1])?;
            match vector_mut(core, "vector-set!", args[0])?.get_mut(at) {
                Some(slot) => *slot = args[2],
                None => {
                    return Err(Error::new(format!(
                        "vector-set!: there is no item number {at}"
                    )));
                }
            }
            Ok(Value::Void)
        },
    );
    core.define_native(
        "vector->list",
        1,
        Some(3),
        "(vector->list vector [start end]) The items as a list.",
        |core, args| {
            let items = vector(core, "vector->list", args[0])?;
            let (start, end) = range(core, "vector->list", &args[1..], items.len())?;
            let items = items[start..end].to_vec();
            Ok(core.list(&items))
        },
    );
    core.define_native(
        "list->vector",
        1,
        Some(1),
        "(list->vector list) The items as a vector.",
        |core, args| {
            let items = list(core, "list->vector", args[0])?;
            Ok(core.alloc(Object::Vector(items)))
        },
    );
    core.define_native(
        "vector-fill!",
        2,
        Some(4),
        "(vector-fill! vector value [start end]) Makes every item the value.",
        |core, args| {
            let len = vector(core, "vector-fill!", args[0])?.len();
            let (start, end) = range(core, "vector-fill!", &args[2..], len)?;
            vector_mut(core, "vector-fill!", args[0])?[start..end].fill(args[1]);
            Ok(Value::Void)
        },
    );
    core.define_native(
        "vector-copy",
        1,
        Some(3),
        "(vector-copy vector [start end]) A new vector of the same items.",
        |core, args| {
            let items = vector(core, "vector-copy", args[0])?;
            let (start, end) = range(core, "vector-copy", &args[1..], items.len())?;
            let items = items[start..end].to_vec();
            Ok(core.alloc(Object::Vector(items)))
        },
    );
    core.define_native(
        "vector-copy!",
        3,
        Some(5),
        "(vector-copy! to at from [start end]) Copies items of one vector into another, starting at a position.",
        |core, args| {
            let at = index(core, "vector-copy!", args[1])?;
            let source = vector(core, "vector-copy!", args[2])?;
            let (start, end) = range(core, "vector-copy!", &args[3..], source.len())?;
            let items = source[start..end].to_vec();
            let target = vector_mut(core, "vector-copy!", args[0])?;
            if at + items.len() > target.len() {
                return Err(Error::new("vector-copy!: the items do not fit in the vector"));
            }
            target[at..at + items.len()].copy_from_slice(&items);
            Ok(Value::Void)
        },
    );
    core.define_native(
        "vector-append",
        0,
        None,
        "(vector-append vector ...) The vectors joined.",
        |core, args| {
            let mut items = Vec::new();
            for arg in args {
                items.extend_from_slice(vector(core, "vector-append", *arg)?);
            }
            Ok(core.alloc(Object::Vector(items)))
        },
    );

    // Hash tables

    core.define_native(
        "make-hash-table",
        0,
        None,
        "(make-hash-table) A new, empty hash table. Numbers, symbols, chars and strings are keys by their value; other things by identity.",
        |core, _| Ok(core.alloc(Object::Table(Box::default()))),
    );
    core.define_native(
        "hash-table?",
        1,
        Some(1),
        "(hash-table? x) Whether x is a hash table.",
        |core, args| {
            Ok(Value::Bool(matches!(
                core.object(args[0]),
                Some(Object::Table(_))
            )))
        },
    );
    core.define_native(
        "hash-table-ref",
        2,
        Some(3),
        "(hash-table-ref table key [default]) The value for the key, or the default (#f) if there is none.",
        |core, args| {
            let wanted = key(core, args[1]);
            Ok(table(core, "hash-table-ref", args[0])?
                .entries
                .get(&wanted)
                .map_or(args.get(2).copied().unwrap_or(Value::Bool(false)), |entry| entry.1))
        },
    );
    core.define_native(
        "hash-table-set!",
        3,
        Some(3),
        "(hash-table-set! table key value) Sets the value for a key.",
        |core, args| {
            // A key that is there is not copied to be found.
            let wanted = key(core, args[1]);
            let found = table(core, "hash-table-set!", args[0])?
                .entries
                .get_index_of(&wanted)
                .ok_or_else(|| wanted.owned());
            let entries = &mut table_mut(core, "hash-table-set!", args[0])?.entries;
            match found {
                Ok(at) => entries[at] = (args[1], args[2]),
                Err(new) => {
                    entries.insert(new, (args[1], args[2]));
                }
            }
            Ok(Value::Void)
        },
    );
    core.define_native(
        "hash-table-delete!",
        2,
        Some(2),
        "(hash-table-delete! table key) Removes a key and its value.",
        |core, args| {
            let wanted = key(core, args[1]);
            let at = table(core, "hash-table-delete!", args[0])?
                .entries
                .get_index_of(&wanted);
            let entries = &mut table_mut(core, "hash-table-delete!", args[0])?.entries;
            if let Some(at) = at {
                entries.shift_remove_index(at);
            }
            Ok(Value::Void)
        },
    );
    core.define_native(
        "hash-table-contains?",
        2,
        Some(2),
        "(hash-table-contains? table key) Whether the key has a value.",
        |core, args| {
            let wanted = key(core, args[1]);
            Ok(Value::Bool(
                table(core, "hash-table-contains?", args[0])?
                    .entries
                    .contains_key(&wanted),
            ))
        },
    );
    core.define_native(
        "hash-table-count",
        1,
        Some(1),
        "(hash-table-count table) The number of keys.",
        |core, args| {
            Ok(Value::from(
                table(core, "hash-table-count", args[0])?.entries.len(),
            ))
        },
    );
    core.define_native(
        "hash-table-clear!",
        1,
        Some(1),
        "(hash-table-clear! table) Removes every key.",
        |core, args| {
            table_mut(core, "hash-table-clear!", args[0])?
                .entries
                .clear();
            Ok(Value::Void)
        },
    );
    core.define_native(
        "hash-table-keys",
        1,
        Some(1),
        "(hash-table-keys table) The keys, in the order they were first set.",
        |core, args| {
            let keys: Vec<Value> = table(core, "hash-table-keys", args[0])?
                .entries
                .values()
                .map(|entry| entry.0)
                .collect();
            Ok(core.list(&keys))
        },
    );
    core.define_native(
        "hash-table-values",
        1,
        Some(1),
        "(hash-table-values table) The values, in the order their keys were first set.",
        |core, args| {
            let values: Vec<Value> = table(core, "hash-table-values", args[0])?
                .entries
                .values()
                .map(|entry| entry.1)
                .collect();
            Ok(core.list(&values))
        },
    );
    core.define_native(
        "hash-table->alist",
        1,
        Some(1),
        "(hash-table->alist table) A list of (key . value) pairs.",
        |core, args| {
            let entries: Vec<(Value, Value)> = table(core, "hash-table->alist", args[0])?
                .entries
                .values()
                .copied()
                .collect();
            let pairs: Vec<Value> = entries
                .into_iter()
                .map(|(key, value)| core.cons(key, value))
                .collect();
            Ok(core.list(&pairs))
        },
    );
    core.define_native(
        "hash-table-copy",
        1,
        Some(1),
        "(hash-table-copy table) A new table with the same keys and values.",
        |core, args| {
            let entries = table(core, "hash-table-copy", args[0])?.entries.clone();
            Ok(core.alloc(Object::Table(Box::new(Table { entries }))))
        },
    );

    // Records

    core.define_native(
        "%make-record-type",
        2,
        Some(2),
        "Makes a record type of a name and field names.",
        |core, args| {
            let name = symbol(core, "define-record-type", args[0])?;
            let mut fields = Vec::new();
            for field in list(core, "define-record-type", args[1])? {
                fields.push(symbol(core, "define-record-type", field)?);
            }
            Ok(core.alloc(Object::RecordType {
                name,
                fields: fields.into(),
            }))
        },
    );
    core.define_native(
        "%record",
        1,
        None,
        "Makes a record of a type and its field values.",
        |core, args| {
            let Some(kind) = args[0].handle() else {
                return Err(wrong(core, "record", "a record type", args[0]));
            };
            Ok(core.alloc(Object::Record {
                kind,
                fields: args[1..].into(),
            }))
        },
    );
    core.define_native("%record?", 2, Some(2), "Whether something is a record of a type.", |core, args| {
        Ok(Value::Bool(
            matches!(core.object(args[1]), Some(Object::Record { kind, .. }) if Value::Obj(*kind) == args[0]),
        ))
    });
    core.define_native(
        "%record-ref",
        4,
        Some(4),
        "A field of a record of a type.",
        |core, args| {
            let at = index(core, "record", args[2])?;
            Ok(record(core, args[3], args[0], args[1])?[at])
        },
    );
    core.define_native(
        "%record-set!",
        5,
        Some(5),
        "Changes a field of a record of a type.",
        |core, args| {
            let at = index(core, "record", args[2])?;
            record(core, args[4], args[0], args[1])?;
            if let Some(Object::Record { fields, .. }) =
                args[1].handle().map(|handle| core.heap.get_mut(handle))
            {
                fields[at] = args[3];
            }
            Ok(Value::Void)
        },
    );
    core.define_native(
        "record?",
        1,
        Some(1),
        "(record? x) Whether x is a record of any type.",
        |core, args| {
            Ok(Value::Bool(matches!(
                core.object(args[0]),
                Some(Object::Record { .. })
            )))
        },
    );
    core.define_native(
        "record->list",
        1,
        Some(1),
        "(record->list record) The record's type name followed by (field . value) pairs: a way to look inside any record.",
        |core, args| {
            let Some(Object::Record { kind, fields }) = core.object(args[0]) else {
                return Err(wrong(core, "record->list", "a record", args[0]));
            };
            let Object::RecordType { name, fields: names } = core.heap.get(*kind) else {
                unreachable!("a record's type is a record type");
            };
            let (name, names, fields) = (*name, names.to_vec(), fields.to_vec());
            let mut items = vec![Value::Sym(name)];
            for (name, value) in names.into_iter().zip(fields) {
                items.push(core.cons(Value::Sym(name), value));
            }
            Ok(core.list(&items))
        },
    );

    // Parameters

    core.define_native(
        "%make-parameter",
        2,
        Some(2),
        "Makes a parameter of a value and a converter.",
        |core, args| {
            Ok(core.alloc(Object::Parameter {
                value: args[0],
                converter: args[1],
            }))
        },
    );
    core.define_native(
        "%parameter-converter",
        1,
        Some(1),
        "The converter of a parameter.",
        |core, args| match core.object(args[0]) {
            Some(Object::Parameter { converter, .. }) => Ok(*converter),
            _ => Err(wrong(core, "parameterize", "a parameter", args[0])),
        },
    );
    core.define_native(
        "%parameter-set!",
        2,
        Some(2),
        "Changes the value of a parameter.",
        |core, args| {
            match args[0].handle().map(|handle| core.heap.get_mut(handle)) {
                Some(Object::Parameter { value, .. }) => *value = args[1],
                _ => return Err(wrong(core, "parameterize", "a parameter", args[0])),
            }
            Ok(Value::Void)
        },
    );
}
