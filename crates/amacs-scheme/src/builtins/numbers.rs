//! Numbers: 64-bit integers and floats. Integer arithmetic that would
//! overflow gives a float instead.

use super::{index, int, text, wrong};
use crate::error::Error;
use crate::printer::float_string;
use crate::reader::parse_number;
use crate::syntax::Datum;
use crate::value::{Object, Value};
use crate::vm::Core;

#[derive(Clone, Copy)]
enum Num {
    Int(i64),
    Float(f64),
}

impl Num {
    fn float(self) -> f64 {
        match self {
            Num::Int(value) => value as f64,
            Num::Float(value) => value,
        }
    }

    fn value(self) -> Value {
        match self {
            Num::Int(value) => Value::Int(value),
            Num::Float(value) => Value::Float(value),
        }
    }
}

fn num(core: &Core, who: &str, value: Value) -> Result<Num, Error> {
    match value {
        Value::Int(value) => Ok(Num::Int(value)),
        Value::Float(value) => Ok(Num::Float(value)),
        _ => Err(wrong(core, who, "a number", value)),
    }
}

fn combine(
    a: Num,
    b: Num,
    exact: fn(i64, i64) -> Option<i64>,
    inexact: fn(f64, f64) -> f64,
) -> Num {
    if let (Num::Int(x), Num::Int(y)) = (a, b)
        && let Some(result) = exact(x, y)
    {
        return Num::Int(result);
    }
    Num::Float(inexact(a.float(), b.float()))
}

fn divide(a: Num, b: Num) -> Result<Num, Error> {
    match (a, b) {
        (Num::Int(_), Num::Int(0)) => Err(Error::new("/: division by zero")),
        (Num::Int(x), Num::Int(y)) if x % y == 0 => Ok(Num::Int(x / y)),
        _ => Ok(Num::Float(a.float() / b.float())),
    }
}

/// Compares an integer with a float without rounding the integer first.
fn compare_mixed(int: i64, float: f64) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering;
    if float.is_nan() {
        return None;
    }
    // Beyond these every float is outside the range of integers.
    if float >= 9.3e18 {
        return Some(Ordering::Less);
    }
    if float <= -9.3e18 {
        return Some(Ordering::Greater);
    }
    let whole = float.trunc();
    Some(int.cmp(&(whole as i64)).then_with(|| {
        0.0_f64
            .partial_cmp(&(float - whole))
            .unwrap_or(Ordering::Equal)
    }))
}

fn compare(a: Num, b: Num) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (Num::Int(x), Num::Int(y)) => Some(x.cmp(&y)),
        (Num::Int(x), Num::Float(y)) => compare_mixed(x, y),
        (Num::Float(x), Num::Int(y)) => compare_mixed(y, x).map(std::cmp::Ordering::reverse),
        (Num::Float(x), Num::Float(y)) => x.partial_cmp(&y),
    }
}

/// A number in a radix, as text.
pub(crate) fn number_string(value: Value, radix: u32) -> Option<String> {
    match value {
        Value::Int(value) if radix == 10 => Some(value.to_string()),
        Value::Int(value) => {
            let mut digits = Vec::new();
            let mut left = value.unsigned_abs();
            loop {
                digits.push(char::from_digit((left % u64::from(radix)) as u32, radix)?);
                left /= u64::from(radix);
                if left == 0 {
                    break;
                }
            }
            if value < 0 {
                digits.push('-');
            }
            Some(digits.iter().rev().collect())
        }
        Value::Float(value) => Some(float_string(value)),
        _ => None,
    }
}

fn round_even(value: f64) -> f64 {
    value.round_ties_even()
}

#[allow(clippy::too_many_lines)]
pub(super) fn install(core: &mut Core) {
    type Exact = fn(i64, i64) -> Option<i64>;
    type Inexact = fn(f64, f64) -> f64;
    let folds: [(&str, i64, Exact, Inexact, &str); 2] = [
        (
            "+",
            0,
            i64::checked_add,
            |a, b| a + b,
            "(+ number ...) The sum.",
        ),
        (
            "*",
            1,
            i64::checked_mul,
            |a, b| a * b,
            "(* number ...) The product.",
        ),
    ];
    for (name, unit, exact, inexact, doc) in folds {
        core.define_native(name, 0, None, doc, move |core, args| {
            let mut total = Num::Int(unit);
            for arg in args {
                total = combine(total, num(core, name, *arg)?, exact, inexact);
            }
            Ok(total.value())
        });
    }
    core.define_native(
        "-",
        1,
        None,
        "(- number ...) The first less the others; the negation of one.",
        |core, args| {
            let first = num(core, "-", args[0])?;
            if args.len() == 1 {
                return Ok(combine(Num::Int(0), first, i64::checked_sub, |a, b| a - b).value());
            }
            let mut total = first;
            for arg in &args[1..] {
                total = combine(total, num(core, "-", *arg)?, i64::checked_sub, |a, b| a - b);
            }
            Ok(total.value())
        },
    );
    core.define_native(
        "/",
        1,
        None,
        "(/ number ...) The first divided by the others. Integers that do not divide evenly give a float.",
        |core, args| {
            let first = num(core, "/", args[0])?;
            if args.len() == 1 {
                return Ok(divide(Num::Int(1), first)?.value());
            }
            let mut total = first;
            for arg in &args[1..] {
                total = divide(total, num(core, "/", *arg)?)?;
            }
            Ok(total.value())
        },
    );

    type Test = fn(std::cmp::Ordering) -> bool;
    let comparisons: [(&str, Test, &str); 5] = [
        (
            "=",
            |o| o.is_eq(),
            "(= number number ...) Whether all are equal.",
        ),
        (
            "<",
            |o| o.is_lt(),
            "(< number number ...) Whether each is less than the next.",
        ),
        (
            ">",
            |o| o.is_gt(),
            "(> number number ...) Whether each is greater than the next.",
        ),
        (
            "<=",
            |o| o.is_le(),
            "(<= number number ...) Whether none is greater than the next.",
        ),
        (
            ">=",
            |o| o.is_ge(),
            "(>= number number ...) Whether none is less than the next.",
        ),
    ];
    for (name, test, doc) in comparisons {
        core.define_native(name, 1, None, doc, move |core, args| {
            let mut holds = true;
            let mut previous = num(core, name, args[0])?;
            for arg in &args[1..] {
                let next = num(core, name, *arg)?;
                holds &= compare(previous, next).is_some_and(test);
                previous = next;
            }
            Ok(Value::Bool(holds))
        });
    }

    let divisions: [(&str, Exact, Inexact, &str); 7] = [
        (
            "quotient",
            i64::checked_div,
            |a, b| (a / b).trunc(),
            "(quotient n d) n divided by d, rounded towards zero.",
        ),
        (
            "truncate-quotient",
            i64::checked_div,
            |a, b| (a / b).trunc(),
            "(truncate-quotient n d) n divided by d, rounded towards zero.",
        ),
        (
            "remainder",
            i64::checked_rem,
            |a, b| a % b,
            "(remainder n d) What is left of n after dividing by d; has the sign of n.",
        ),
        (
            "truncate-remainder",
            i64::checked_rem,
            |a, b| a % b,
            "(truncate-remainder n d) What is left of n after dividing by d; has the sign of n.",
        ),
        (
            "modulo",
            floor_remainder,
            |a, b| a - b * (a / b).floor(),
            "(modulo n d) What is left of n after dividing by d; has the sign of d.",
        ),
        (
            "floor-remainder",
            floor_remainder,
            |a, b| a - b * (a / b).floor(),
            "(floor-remainder n d) What is left of n after dividing by d; has the sign of d.",
        ),
        (
            "floor-quotient",
            floor_quotient,
            |a, b| (a / b).floor(),
            "(floor-quotient n d) n divided by d, rounded down.",
        ),
    ];
    for (name, exact, inexact, doc) in divisions {
        core.define_native(name, 2, Some(2), doc, move |core, args| {
            let (a, b) = (num(core, name, args[0])?, num(core, name, args[1])?);
            match (a, b) {
                (_, Num::Int(0)) => Err(Error::new(format!("{name}: division by zero"))),
                (Num::Int(x), Num::Int(y)) => Ok(exact(x, y)
                    .map_or_else(|| Value::Float(inexact(x as f64, y as f64)), Value::Int)),
                _ => Ok(Value::Float(inexact(a.float(), b.float()))),
            }
        });
    }

    type Predicate = fn(Value) -> bool;
    let predicates: [(&str, Predicate, &str); 9] = [
        (
            "number?",
            |v| matches!(v, Value::Int(_) | Value::Float(_)),
            "(number? x) Whether x is a number.",
        ),
        (
            "real?",
            |v| matches!(v, Value::Int(_) | Value::Float(_)),
            "(real? x) Whether x is a number.",
        ),
        (
            "complex?",
            |v| matches!(v, Value::Int(_) | Value::Float(_)),
            "(complex? x) Whether x is a number.",
        ),
        (
            "rational?",
            |v| matches!(v, Value::Int(_)) || matches!(v, Value::Float(f) if f.is_finite()),
            "(rational? x) Whether x is a finite number.",
        ),
        (
            "integer?",
            |v| {
                matches!(v, Value::Int(_))
                    || matches!(v, Value::Float(f) if f.is_finite() && f.fract() == 0.0)
            },
            "(integer? x) Whether x is a whole number.",
        ),
        (
            "exact?",
            |v| matches!(v, Value::Int(_)),
            "(exact? x) Whether x is an integer rather than a float.",
        ),
        (
            "inexact?",
            |v| matches!(v, Value::Float(_)),
            "(inexact? x) Whether x is a float.",
        ),
        (
            "exact-integer?",
            |v| matches!(v, Value::Int(_)),
            "(exact-integer? x) Whether x is an integer rather than a float.",
        ),
        (
            "nan?",
            |v| matches!(v, Value::Float(f) if f.is_nan()),
            "(nan? x) Whether x is the float that is not a number.",
        ),
    ];
    for (name, test, doc) in predicates {
        core.define_native(name, 1, Some(1), doc, move |_, args| {
            Ok(Value::Bool(test(args[0])))
        });
    }

    type NumTest = fn(f64) -> bool;
    let tests: [(&str, NumTest, &str); 5] = [
        ("zero?", |v| v == 0.0, "(zero? number) Whether it is zero."),
        (
            "positive?",
            |v| v > 0.0,
            "(positive? number) Whether it is greater than zero.",
        ),
        (
            "negative?",
            |v| v < 0.0,
            "(negative? number) Whether it is less than zero.",
        ),
        (
            "infinite?",
            f64::is_infinite,
            "(infinite? number) Whether it is an infinity.",
        ),
        (
            "finite?",
            f64::is_finite,
            "(finite? number) Whether it is neither an infinity nor not a number.",
        ),
    ];
    for (name, test, doc) in tests {
        core.define_native(name, 1, Some(1), doc, move |core, args| {
            Ok(Value::Bool(test(num(core, name, args[0])?.float())))
        });
    }
    core.define_native(
        "odd?",
        1,
        Some(1),
        "(odd? integer) Whether it is odd.",
        |core, args| Ok(Value::Bool(int(core, "odd?", args[0])? % 2 != 0)),
    );
    core.define_native(
        "even?",
        1,
        Some(1),
        "(even? integer) Whether it is even.",
        |core, args| Ok(Value::Bool(int(core, "even?", args[0])? % 2 == 0)),
    );

    core.define_native(
        "abs",
        1,
        Some(1),
        "(abs number) The number without its sign.",
        |core, args| {
            Ok(match num(core, "abs", args[0])? {
                Num::Int(value) => value
                    .checked_abs()
                    .map_or(Value::Float((value as f64).abs()), Value::Int),
                Num::Float(value) => Value::Float(value.abs()),
            })
        },
    );
    for (name, wanted, doc) in [
        (
            "min",
            std::cmp::Ordering::Less,
            "(min number ...) The smallest.",
        ),
        (
            "max",
            std::cmp::Ordering::Greater,
            "(max number ...) The largest.",
        ),
    ] {
        core.define_native(name, 1, None, doc, move |core, args| {
            let mut best = num(core, name, args[0])?;
            let mut inexact = matches!(best, Num::Float(_));
            for arg in &args[1..] {
                let next = num(core, name, *arg)?;
                inexact |= matches!(next, Num::Float(_));
                if compare(next, best) == Some(wanted) {
                    best = next;
                }
            }
            Ok(if inexact {
                Value::Float(best.float())
            } else {
                best.value()
            })
        });
    }
    core.define_native(
        "gcd",
        0,
        None,
        "(gcd integer ...) The greatest common divisor.",
        |core, args| {
            let mut result: i64 = 0;
            for arg in args {
                let mut other = int(core, "gcd", *arg)?;
                while other != 0 {
                    (result, other) = (other, result % other);
                }
            }
            Ok(Value::Int(result.abs()))
        },
    );
    core.define_native(
        "lcm",
        0,
        None,
        "(lcm integer ...) The least common multiple.",
        |core, args| {
            let mut result: i64 = 1;
            for arg in args {
                let other = int(core, "lcm", *arg)?;
                if other == 0 {
                    return Ok(Value::Int(0));
                }
                let (mut a, mut b) = (result, other);
                while b != 0 {
                    (a, b) = (b, a % b);
                }
                result = (result / a).saturating_mul(other).abs();
            }
            Ok(Value::Int(result))
        },
    );

    type Rounding = fn(f64) -> f64;
    let roundings: [(&str, Rounding, &str); 4] = [
        (
            "floor",
            f64::floor,
            "(floor number) The nearest whole number that is not greater.",
        ),
        (
            "ceiling",
            f64::ceil,
            "(ceiling number) The nearest whole number that is not less.",
        ),
        (
            "truncate",
            f64::trunc,
            "(truncate number) The number without its fraction.",
        ),
        (
            "round",
            round_even,
            "(round number) The nearest whole number; halfway goes to the even one.",
        ),
    ];
    for (name, round, doc) in roundings {
        core.define_native(name, 1, Some(1), doc, move |core, args| {
            Ok(match num(core, name, args[0])? {
                Num::Int(value) => Value::Int(value),
                Num::Float(value) => Value::Float(round(value)),
            })
        });
    }
    for name in ["exact", "inexact->exact"] {
        core.define_native(
            name,
            1,
            Some(1),
            "(exact number) The number as an integer. A float must be whole.",
            move |core, args| match args[0] {
                Value::Int(_) => Ok(args[0]),
                Value::Float(value) if value.fract() == 0.0 && value.abs() < 9.2e18 => {
                    Ok(Value::Int(value as i64))
                }
                other => Err(wrong(core, name, "a whole number", other)),
            },
        );
    }
    for name in ["inexact", "exact->inexact"] {
        core.define_native(
            name,
            1,
            Some(1),
            "(inexact number) The number as a float.",
            move |core, args| Ok(Value::Float(num(core, name, args[0])?.float())),
        );
    }

    type Function = fn(f64) -> f64;
    let functions: [(&str, Function, &str); 6] = [
        ("exp", f64::exp, "(exp x) e to the power of x."),
        ("sin", f64::sin, "(sin x) The sine of x radians."),
        ("cos", f64::cos, "(cos x) The cosine of x radians."),
        ("tan", f64::tan, "(tan x) The tangent of x radians."),
        ("asin", f64::asin, "(asin x) The angle whose sine is x."),
        ("acos", f64::acos, "(acos x) The angle whose cosine is x."),
    ];
    for (name, function, doc) in functions {
        core.define_native(name, 1, Some(1), doc, move |core, args| {
            Ok(Value::Float(function(num(core, name, args[0])?.float())))
        });
    }
    core.define_native(
        "log",
        1,
        Some(2),
        "(log x [base]) The logarithm of x, natural unless a base is given.",
        |core, args| {
            let value = num(core, "log", args[0])?.float();
            Ok(Value::Float(match args.get(1) {
                Some(base) => value.log(num(core, "log", *base)?.float()),
                None => value.ln(),
            }))
        },
    );
    core.define_native(
        "atan",
        1,
        Some(2),
        "(atan y [x]) The angle whose tangent is y, or y/x.",
        |core, args| {
            let y = num(core, "atan", args[0])?.float();
            Ok(Value::Float(match args.get(1) {
                Some(x) => y.atan2(num(core, "atan", *x)?.float()),
                None => y.atan(),
            }))
        },
    );
    core.define_native(
        "sqrt",
        1,
        Some(1),
        "(sqrt number) The square root; an integer if it is exact.",
        |core, args| {
            Ok(match num(core, "sqrt", args[0])? {
                Num::Int(value) if value >= 0 && value.isqrt().pow(2) == value => {
                    Value::Int(value.isqrt())
                }
                other => Value::Float(other.float().sqrt()),
            })
        },
    );
    core.define_native(
        "exact-integer-sqrt",
        1,
        Some(1),
        "(exact-integer-sqrt n) Two values: the largest integer whose square is at most n, and what is left.",
        |core, args| match args[0] {
            Value::Int(value) if value >= 0 => {
                let root = value.isqrt();
                Ok(core.alloc(Object::Values(vec![Value::Int(root), Value::Int(value - root * root)])))
            }
            other => Err(wrong(core, "exact-integer-sqrt", "a non-negative integer", other)),
        },
    );
    core.define_native(
        "expt",
        2,
        Some(2),
        "(expt base power) The base raised to the power.",
        |core, args| {
            let (base, power) = (num(core, "expt", args[0])?, num(core, "expt", args[1])?);
            if let (Num::Int(base), Num::Int(power)) = (base, power)
                && let Ok(power) = u32::try_from(power)
                && let Some(result) = base.checked_pow(power)
            {
                return Ok(Value::Int(result));
            }
            Ok(Value::Float(base.float().powf(power.float())))
        },
    );
    core.define_native(
        "square",
        1,
        Some(1),
        "(square number) The number times itself.",
        |core, args| {
            let value = num(core, "square", args[0])?;
            Ok(combine(value, value, i64::checked_mul, |a, b| a * b).value())
        },
    );
    core.define_native(
        "number->string",
        1,
        Some(2),
        "(number->string number [radix]) The number as text, in base ten unless a radix is given.",
        |core, args| {
            let radix = args
                .get(1)
                .map_or(Ok(10), |arg| index(core, "number->string", *arg))?;
            if !(2..=36).contains(&radix) {
                return Err(Error::new("number->string: the radix must be from 2 to 36"));
            }
            let text = number_string(args[0], radix as u32)
                .ok_or_else(|| wrong(core, "number->string", "a number", args[0]))?;
            Ok(core.string(text))
        },
    );
    core.define_native(
        "string->number",
        1,
        Some(2),
        "(string->number string [radix]) The number the text spells, or #f if it spells none.",
        |core, args| {
            let radix = args
                .get(1)
                .map_or(Ok(10), |arg| index(core, "string->number", *arg))?;
            if !(2..=36).contains(&radix) {
                return Err(Error::new("string->number: the radix must be from 2 to 36"));
            }
            Ok(
                match parse_number(text(core, "string->number", args[0])?, radix as u32) {
                    Some(Datum::Int(value)) => Value::Int(value),
                    Some(Datum::Float(value)) => Value::Float(value),
                    _ => Value::Bool(false),
                },
            )
        },
    );
    let state = std::cell::Cell::new(0x9E37_79B9_7F4A_7C15_u64);
    core.define_native(
        "random",
        1,
        Some(1),
        "(random n) A random integer from 0 up to but not including n.",
        move |core, args| {
            let bound = int(core, "random", args[0])?;
            if bound <= 0 {
                return Err(wrong(core, "random", "a positive integer", args[0]));
            }
            // xorshift64*: plenty for an editor, useless for secrets.
            let mut x = state.get();
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            state.set(x);
            let random = x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 1;
            Ok(Value::Int((random % bound as u64) as i64))
        },
    );
}

/// Division that rounds down.
fn floor_quotient(a: i64, b: i64) -> Option<i64> {
    let quotient = a.checked_div(b)?;
    let remainder = a.checked_rem(b)?;
    Some(if remainder != 0 && (remainder < 0) != (b < 0) {
        quotient - 1
    } else {
        quotient
    })
}

/// The remainder that goes with division that rounds down.
fn floor_remainder(a: i64, b: i64) -> Option<i64> {
    let remainder = a.checked_rem(b)?;
    Some(if remainder != 0 && (remainder < 0) != (b < 0) {
        remainder + b
    } else {
        remainder
    })
}
