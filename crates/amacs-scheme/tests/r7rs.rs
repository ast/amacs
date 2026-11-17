//! Runs the R7RS test suite of chibi-scheme. This Scheme leaves parts of
//! the standard out on purpose, so some tests fail; the numbers below
//! record how many, and may only move in the right direction.

use amacs_scheme::{Outcome, Value, Vm};

/// Tests that pass. Raise it when more do.
const PASSING: i64 = 735;
/// Tests that fail, and forms that cannot be run at all (each may hold
/// several tests). Lower them when fewer do.
const FAILING: usize = 231;
const BROKEN_FORMS: usize = 14;

#[test]
fn r7rs_suite() {
    let mut vm = Vm::new();
    vm.eval(include_str!("r7rs/shim.scm"), "shim.scm")
        .expect("the shim loads");
    let mut broken = Vec::new();
    vm.eval_each(
        include_str!("r7rs/r7rs-tests.scm"),
        "r7rs-tests.scm",
        |result, _| {
            if let Err(report) = result {
                broken.push(report.summary());
            }
            true
        },
    );
    let value = |vm: &mut Vm, source: &str| match vm.eval(source, "query") {
        Ok(Outcome::Done(value)) => value,
        other => panic!("{source} gave {other:?}"),
    };
    let Value::Int(passed) = value(&mut vm, "%passed") else {
        panic!("the count of passed tests is not a number");
    };
    let failures = value(&mut vm, "(reverse %failures)");
    let failures = vm.core.list_to_vec(failures).expect("a list of failures");
    if std::env::var_os("R7RS_VERBOSE").is_some() {
        for failure in &failures {
            println!("FAIL {}", vm.core.write(*failure));
        }
        for report in &broken {
            println!("BROKEN {report}");
        }
    }
    println!(
        "r7rs: {passed} passed, {} failed, {} forms could not be run",
        failures.len(),
        broken.len()
    );
    assert!(passed >= PASSING, "fewer tests pass than before: {passed}");
    assert!(
        failures.len() <= FAILING,
        "more tests fail than before: {}",
        failures.len()
    );
    assert!(
        broken.len() <= BROKEN_FORMS,
        "more forms are broken than before: {}",
        broken.len()
    );
}
