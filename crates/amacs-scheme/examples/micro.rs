//! Times single features of the machine. Run with `--release`.

use std::time::Instant;

use amacs_scheme::{Value, Vm};

fn main() {
    let mut vm = Vm::new();
    vm.core
        .define_native("host-id", 1, Some(1), "Returns its argument.", |_, args| {
            Ok(args[0])
        });
    vm.eval("(define (id x) x) (define (two a b) a)", "micro")
        .unwrap();
    let turns = 5_000_000;
    let cases = [
        ("an empty loop", "i"),
        ("a call of a host procedure", "(host-id i)"),
        ("a call of a Scheme procedure", "(id i)"),
        (
            "a call of a procedure built in, through a variable",
            "(f i)",
        ),
        ("making a pair", "(cons i i)"),
        ("making and calling a closure", "((lambda () i))"),
        ("a global variable", "turns"),
    ];
    let mut base = 0.0;
    for (name, body) in cases {
        let source = format!(
            "(define turns {turns}) (define f car) (let loop ((i 0)) (if (= i turns) i (begin {body} (loop (+ i 1)))))"
        );
        let source = source.replace("(f i)", "(f '(1))");
        let started = Instant::now();
        vm.eval(&source, "micro").unwrap();
        let each = started.elapsed().as_secs_f64() * 1e9 / f64::from(turns);
        if base == 0.0 {
            base = each;
            println!("{each:>7.1} ns  {name}");
        } else {
            println!("{:>7.1} ns  {name} (beyond the loop)", each - base);
        }
    }
    let _ = Value::Void;
}
