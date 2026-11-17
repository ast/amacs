#![allow(dead_code)]

use amacs_scheme::{Outcome, Vm};

/// Runs a program and gives its last value as `write` shows it, or the
/// error's summary.
pub fn run(source: &str) -> String {
    let mut vm = Vm::new();
    run_in(&mut vm, source)
}

pub fn run_in(vm: &mut Vm, source: &str) -> String {
    match vm.eval(source, "test.scm") {
        Ok(Outcome::Done(value)) => vm.core.write(value),
        Ok(Outcome::Suspended { value, .. }) => format!("suspended: {}", vm.core.write(value)),
        Err(report) => format!("error: {}", report.summary()),
    }
}
