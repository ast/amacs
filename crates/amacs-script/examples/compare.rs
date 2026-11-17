//! Times the same Scheme programs on Steel and on amacs's own Scheme.
//!
//! Run with `cargo run --release -p amacs-script --example compare`.

use std::time::{Duration, Instant};

use amacs_scheme::{Outcome, Value, Vm};
use steel::steel_vm::engine::Engine;
use steel::steel_vm::register_fn::RegisterFn;

/// Programs that both engines can run as they are.
const PROGRAMS: &[(&str, &str)] = &[
    (
        "fib 27",
        "(define (fib n) (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2))))) (fib 27)",
    ),
    (
        "loop of 1M turns calling a host procedure",
        "(let loop ((i 0) (sum 0)) (if (= i 1000000) sum (loop (+ i 1) (host-add sum i))))",
    ),
    (
        "map and filter over 300k items",
        "(define (count-up n) (let loop ((i (- n 1)) (acc '())) (if (< i 0) acc (loop (- i 1) (cons i acc)))))
         (length (filter even? (map (lambda (x) (* x 3)) (count-up 300000))))",
    ),
    (
        "closures and assignment: 1M counter calls",
        "(define (make) (let ((n 0)) (lambda () (set! n (+ n 1)) n)))
         (define c (make))
         (let loop ((i 0)) (if (= i 1000000) (c) (begin (c) (loop (+ i 1)))))",
    ),
    (
        "strings: append and compare 200k times",
        "(let loop ((i 0) (n 0)) (if (= i 200000) n (loop (+ i 1) (if (equal? (string-append \"ab\" \"cd\") \"abcd\") (+ n 1) n))))",
    ),
    (
        "association lists: 100k lookups among 50",
        "(define (count-up n) (let loop ((i (- n 1)) (acc '())) (if (< i 0) acc (loop (- i 1) (cons i acc)))))
         (define table (map (lambda (i) (cons i (* i i))) (count-up 50)))
         (let loop ((i 0) (sum 0)) (if (= i 100000) sum (loop (+ i 1) (+ sum (cdr (assoc 25 table))))))",
    ),
];

fn host_add(a: isize, b: isize) -> isize {
    a + b
}

fn steel() -> (Duration, Vec<(Duration, String)>) {
    let started = Instant::now();
    let mut engine = Engine::new();
    engine.register_fn("host-add", host_add);
    let startup = started.elapsed();
    let results = PROGRAMS
        .iter()
        .map(|(_, source)| {
            let started = Instant::now();
            let result = match engine.run(*source) {
                Ok(values) => values.last().map(ToString::to_string).unwrap_or_default(),
                Err(error) => format!("error: {error}"),
            };
            (started.elapsed(), result)
        })
        .collect();
    (startup, results)
}

fn own() -> (Duration, Vec<(Duration, String)>) {
    let started = Instant::now();
    let mut vm = Vm::new();
    vm.core
        .define_native("host-add", 2, Some(2), "Adds.", |_, args| {
            match (args[0], args[1]) {
                (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a + b)),
                _ => Ok(Value::Void),
            }
        });
    let startup = started.elapsed();
    let results = PROGRAMS
        .iter()
        .map(|(_, source)| {
            let started = Instant::now();
            let result = match vm.eval(source, "compare") {
                Ok(Outcome::Done(value)) => vm.core.write(value),
                Ok(other) => format!("{other:?}"),
                Err(report) => format!("error: {report}"),
            };
            (started.elapsed(), result)
        })
        .collect();
    (startup, results)
}

fn main() {
    let (steel_startup, steel_results) = steel();
    let (own_startup, own_results) = own();
    println!("{:>10} {:>10}", "Steel", "own");
    println!("{steel_startup:>10.2?} {own_startup:>10.2?}  start an engine");
    for (index, (name, _)) in PROGRAMS.iter().enumerate() {
        let (steel_time, steel_result) = &steel_results[index];
        let (own_time, own_result) = &own_results[index];
        let agree = if steel_result == own_result {
            String::new()
        } else {
            format!("  (results differ: {steel_result} and {own_result})")
        };
        println!("{steel_time:>10.2?} {own_time:>10.2?}  {name}{agree}");
    }
}
