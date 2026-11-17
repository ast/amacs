//! Times a few small programs. Run with `--release`.

use std::time::Instant;

use amacs_scheme::{Outcome, Value, Vm};

const PROGRAMS: &[(&str, &str)] = &[
    (
        "fib 30 (1.6M calls, not in tail position)",
        "(define (fib n) (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2))))) (fib 30)",
    ),
    (
        "loop of 3M turns calling a host procedure",
        "(let loop ((i 0) (sum 0)) (if (= i 3000000) sum (loop (+ i 1) (host-add sum i))))",
    ),
    (
        "map, filter and fold over 1M items",
        "(fold + 0 (filter even? (map (lambda (x) (* x 3)) (iota 1000000))))",
    ),
    (
        "sort 200k numbers",
        "(let loop ((i 0) (items '())) (if (= i 200000) (length (sort items <)) (loop (+ i 1) (cons (random 1000000) items))))",
    ),
    (
        "closures and assignment: 2M counter calls",
        "(define (make) (let ((n 0)) (lambda () (set! n (+ n 1)) n))) (define c (make)) (let loop ((i 0)) (if (= i 2000000) (c) (begin (c) (loop (+ i 1)))))",
    ),
    (
        "strings: build and split 100k words",
        "(length (string-split (string-join (map number->string (iota 100000)) \" \")))",
    ),
    (
        "guard: 300k errors raised and caught",
        "(let loop ((i 0) (n 0)) (if (= i 300000) n (loop (+ i 1) (+ n (guard (e (#t 1)) (raise 'x))))))",
    ),
    (
        "hash table: 500k inserts and lookups",
        "(define t (make-hash-table)) (let loop ((i 0)) (when (< i 500000) (hash-table-set! t i (* i 2)) (loop (+ i 1)))) (let loop ((i 0) (sum 0)) (if (= i 500000) sum (loop (+ i 1) (+ sum (hash-table-ref t i)))))",
    ),
];

fn main() {
    let started = Instant::now();
    let mut vm = Vm::new();
    println!(
        "{:>9.2?}  start an interpreter, with its prelude",
        started.elapsed()
    );
    vm.core
        .define_native("host-add", 2, Some(2), "Adds.", |_, args| {
            match (args[0], args[1]) {
                (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a + b)),
                _ => Ok(Value::Void),
            }
        });
    for (name, source) in PROGRAMS {
        let started = Instant::now();
        let result = match vm.eval(source, "bench") {
            Ok(Outcome::Done(value)) => vm.core.write(value),
            other => format!("{other:?}"),
        };
        println!("{:>9.2?}  {name} => {result}", started.elapsed());
    }
    println!(
        "{} collections, {} objects alive",
        vm.core.heap.collections,
        vm.core.heap.len()
    );
}
