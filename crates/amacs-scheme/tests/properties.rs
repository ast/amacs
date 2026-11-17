//! Properties that hold for any input.

mod common;
use common::run_in;

use amacs_scheme::Vm;
use proptest::prelude::*;

/// Pieces that programs are made of, to be put together at random.
const PIECES: &[&str] = &[
    "(",
    "(",
    "(",
    ")",
    ")",
    ")",
    "[",
    "]",
    "'",
    "`",
    ",",
    ",@",
    ".",
    "#(",
    "#;",
    "\"s\"",
    "#\\a",
    "1",
    "-2",
    "3.5",
    "x",
    "y",
    "f",
    "loop",
    "#t",
    "#f",
    "...",
    "_",
    "=>",
    "else",
    "define",
    "lambda",
    "let",
    "let*",
    "letrec",
    "if",
    "cond",
    "case",
    "and",
    "or",
    "when",
    "set!",
    "begin",
    "do",
    "quote",
    "quasiquote",
    "unquote",
    "define-syntax",
    "syntax-rules",
    "let-syntax",
    "define-record-type",
    "parameterize",
    "guard",
    "let-values",
    "define-values",
    "delay",
    "force",
    "case-lambda",
    "while",
    "+",
    "-",
    "*",
    "<",
    "=",
    "car",
    "cdr",
    "cons",
    "list",
    "map",
    "for-each",
    "apply",
    "vector",
    "raise",
    "error",
    "call/cc",
    "dynamic-wind",
    "values",
    "call-with-values",
    "make-coroutine",
    "coroutine-resume",
    "yield",
    "%suspend",
    "eval",
    "with-exception-handler",
    "raise-continuable",
    "make-parameter",
    "string-append",
    "sort",
    "make-hash-table",
    "hash-table-set!",
    "describe",
    "procedure-source",
    "global-set!",
];

fn program() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(PIECES), 0..40).prop_map(|pieces| pieces.join(" "))
}

/// A program with its brackets in order, which gets further than the reader.
fn balanced() -> impl Strategy<Value = String> {
    let leaf = prop::sample::select(PIECES)
        .prop_filter("not a bracket", |piece| !"()[]#(#;'`,,@.".contains(piece))
        .prop_map(str::to_owned);
    leaf.prop_recursive(5, 60, 6, |inner| {
        prop::collection::vec(inner, 0..6).prop_map(|items| format!("({})", items.join(" ")))
    })
}

fn datum() -> impl Strategy<Value = String> {
    let leaf = prop_oneof![
        any::<i64>().prop_map(|n| n.to_string()),
        any::<f64>().prop_map(|x| format!("{x:?}")
            .replace("inf", "+inf.0")
            .replace("NaN", "+nan.0")
            .replace("-+", "-")),
        "[a-z!?*<>=/+-]{1,6}".prop_filter("not a number or dot", |s| {
            s.parse::<f64>().is_err()
                && s != "."
                && s != "+"
                && s != "-"
                && !s.starts_with("+i")
                && !s.starts_with("-i")
                && !s.starts_with("+n")
                && !s.starts_with("-n")
        }),
        any::<String>().prop_map(|s| format!("{s:?}").replace("\\u{", "\\x").replace('}', ";")),
        any::<char>()
            .prop_filter("printable", |c| !c.is_control() && !c.is_whitespace())
            .prop_map(|c| format!("#\\{c}")),
        Just("#t".to_owned()),
        Just("()".to_owned()),
    ];
    leaf.prop_recursive(4, 40, 5, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..5)
                .prop_map(|items| format!("({})", items.join(" "))),
            prop::collection::vec(inner.clone(), 0..5)
                .prop_map(|items| format!("#({})", items.join(" "))),
            (prop::collection::vec(inner.clone(), 1..4), inner)
                .prop_map(|(items, tail)| format!("({} . {tail})", items.join(" "))),
        ]
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    /// Whatever it is given, the interpreter answers or reports an error,
    /// and works afterwards.
    #[test]
    fn no_program_breaks_the_interpreter(programs in prop::collection::vec(prop_oneof![program(), balanced()], 1..6)) {
        let mut vm = Vm::new();
        vm.max_depth = 2_000;
        for source in &programs {
            vm.fuel = Some(20_000);
            let _ = vm.eval(source, "random.scm");
        }
        vm.fuel = None;
        vm.collect_garbage();
        // The names the programs may have redefined are not used here.
        prop_assert_eq!(run_in(&mut vm, "(vector-length (make-vector 3 0))"), "3");
    }

    /// What is written reads back as the same.
    #[test]
    fn written_data_reads_back(source in datum()) {
        let mut vm = Vm::new();
        let program = format!(
            "(define original '{source})
             (define text (write->string original))
             (define again (read-from-string text))
             (list (equal? text (write->string again)))"
        );
        prop_assert_eq!(run_in(&mut vm, &program), "(#t)");
    }

    /// Sorting agrees with the sort of the standard library, and is stable.
    #[test]
    fn sort_sorts(items in prop::collection::vec((0..20i64, 0..1000i64), 0..200)) {
        let mut vm = Vm::new();
        let list: Vec<String> = items.iter().map(|(key, tag)| format!("({key} . {tag})")).collect();
        let mut expected = items.clone();
        expected.sort_by_key(|(key, _)| *key);
        let expected: Vec<String> = expected.iter().map(|(key, tag)| format!("({key} . {tag})")).collect();
        let program = format!("(sort '({}) (lambda (a b) (< (car a) (car b))))", list.join(" "));
        prop_assert_eq!(run_in(&mut vm, &program), format!("({})", expected.join(" ")));
    }

    /// Integer arithmetic agrees with Rust's where that does not overflow.
    #[test]
    fn arithmetic_agrees(a in -1_000_000i64..1_000_000, b in -1_000_000i64..1_000_000) {
        prop_assume!(b != 0);
        let mut vm = Vm::new();
        let program = format!("(list (+ {a} {b}) (- {a} {b}) (* {a} {b}) (quotient {a} {b}) (remainder {a} {b}) (modulo {a} {b}) (< {a} {b}) (max {a} {b}))");
        let expected = format!(
            "({} {} {} {} {} {} {} {})",
            a + b, a - b, a * b, a / b, a % b, a.rem_euclid(b) + if b < 0 && a.rem_euclid(b) != 0 { b } else { 0 },
            if a < b { "#t" } else { "#f" }, a.max(b)
        );
        prop_assert_eq!(run_in(&mut vm, &program), expected);
    }
}
