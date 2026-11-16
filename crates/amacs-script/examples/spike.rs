//! M0 feasibility spike: does Steel support what the amacs design relies on?
//!
//! 1. Can a tight tail-recursive loop be interrupted from another thread (C-g)?
//! 2. Can a continuation captured in one `Engine::run` be resumed from a later
//!    one (the basis for `await`, the minibuffer and recursive minibuffers)?
//! 3. How expensive is a call from Scheme into a registered Rust primitive?
//! 4. What does an error report look like?
//!
//! Run with `cargo run --release -p amacs-script --example spike`.

use std::time::{Duration, Instant};

use steel::steel_vm::engine::Engine;
use steel::steel_vm::register_fn::RegisterFn;

fn main() {
    let results = [
        ("interrupt tight loop", interrupt_tight_loop()),
        ("engine usable after interrupt", usable_after_interrupt()),
        ("resume continuation in later run", resume_continuation()),
        ("await-style suspend and resume", await_style()),
        ("primitive call overhead", primitive_overhead()),
        ("error report", error_report()),
    ];

    println!("\n== Steel spike summary ==");
    for (name, result) in &results {
        match result {
            Ok(detail) => println!("PASS  {name}: {detail}"),
            Err(detail) => println!("FAIL  {name}: {detail}"),
        }
    }
    if results.iter().any(|(_, r)| r.is_err()) {
        std::process::exit(1);
    }
}

type Outcome = Result<String, String>;

/// Interrupts `src` after 200 ms. Aborts the whole process if the interrupt has
/// not taken effect after five seconds, since there is no other way out.
fn run_interrupted(engine: &mut Engine, src: &'static str) -> Outcome {
    let controller = engine.get_thread_state_controller();
    let watchdog = std::thread::spawn({
        let controller = controller.clone();
        move || {
            std::thread::sleep(Duration::from_millis(200));
            controller.interrupt();
            std::thread::sleep(Duration::from_secs(5));
            println!("FAIL  interrupt did not stop `{src}` within 5 s");
            std::process::exit(2);
        }
    });
    let start = Instant::now();
    let result = engine.run(src);
    let elapsed = start.elapsed();
    controller.resume();
    drop(watchdog); // detached; it exits with the process
    match result {
        Err(e) => Ok(format!("stopped after {elapsed:?} with `{e}`")),
        Ok(v) => Err(format!("loop returned normally: {v:?}")),
    }
}

fn interrupt_tight_loop() -> Outcome {
    let mut engine = Engine::new();
    run_interrupted(&mut engine, "(define (spin) (spin)) (spin)")
}

fn usable_after_interrupt() -> Outcome {
    let mut engine = Engine::new();
    run_interrupted(&mut engine, "(define (spin2 n) (spin2 (+ n 1))) (spin2 0)")?;
    match engine.run("(+ 1 2)") {
        Ok(v) => Ok(format!("(+ 1 2) => {v:?}")),
        Err(e) => Err(format!("engine broken after interrupt: {e}")),
    }
}

fn resume_continuation() -> Outcome {
    let mut engine = Engine::new();
    engine
        .run(
            "(define saved #f)
             (define seen '())
             (set! seen (cons (+ 100 (call/cc (lambda (k) (set! saved k) 1))) seen))",
        )
        .map_err(|e| format!("capture failed: {e}"))?;
    engine
        .run("(saved 5)")
        .map_err(|e| format!("resume failed: {e}"))?;
    let log = engine
        .run("seen")
        .map_err(|e| format!("read failed: {e}"))?;
    let shown = format!("{log:?}");
    if shown.contains("105") {
        Ok(format!("log = {shown}"))
    } else {
        Err(format!(
            "continuation did not re-run its frame, log = {shown}"
        ))
    }
}

/// The shape `await` would take: a command captures its continuation, escapes
/// to the top of the run, and is resumed later with a value.
fn await_style() -> Outcome {
    let mut engine = Engine::new();
    engine
        .run(
            "(define pending #f)
             (define top #f)
             (define trace '())
             (define (note x) (set! trace (cons x trace)))
             (define (await-value)
               (call/cc (lambda (k) (set! pending k) (top 'suspended))))
             (define (command)
               (note 'before)
               (let ((v (await-value)))
                 (note v)
                 (let ((w (await-value)))
                   (note w)
                   'finished)))
             (define (start thunk)
               (call/cc (lambda (k) (set! top k) (thunk))))
             (define (resume v)
               (call/cc (lambda (k) (set! top k) (pending v))))",
        )
        .map_err(|e| format!("setup failed: {e}"))?;

    let first = engine
        .run("(start command)")
        .map_err(|e| format!("start failed: {e}"))?;
    let second = engine
        .run("(resume 'first-value)")
        .map_err(|e| format!("first resume failed: {e}"))?;
    let third = engine
        .run("(resume 'second-value)")
        .map_err(|e| format!("second resume failed: {e}"))?;
    let trace = engine.run("(reverse trace)").map_err(|e| e.to_string())?;

    let shown = format!("{first:?} {second:?} {third:?} trace={trace:?}");
    let ok = shown.contains("before")
        && shown.contains("first-value")
        && shown.contains("second-value")
        && shown.contains("finished");
    if ok { Ok(shown) } else { Err(shown) }
}

fn add1(x: isize) -> isize {
    x + 1
}

fn primitive_overhead() -> Outcome {
    const CALLS: u32 = 1_000_000;
    let mut engine = Engine::new();
    engine.register_fn("prim-add1", add1);
    engine
        .run(
            "(define (bench n acc)
               (if (= n 0) acc (bench (- n 1) (prim-add1 acc))))",
        )
        .map_err(|e| e.to_string())?;
    let start = Instant::now();
    let out = engine
        .run(format!("(bench {CALLS} 0)"))
        .map_err(|e| e.to_string())?;
    let elapsed = start.elapsed();
    let per_call = elapsed / CALLS;
    let detail =
        format!("{CALLS} calls in {elapsed:?} ({per_call:?} per iteration), result {out:?}");
    if elapsed < Duration::from_secs(2) {
        Ok(detail)
    } else {
        Err(detail)
    }
}

fn error_report() -> Outcome {
    let mut engine = Engine::new();
    let err = match engine.run(
        "(define (inner x) (car x))
         (define (outer x) (+ 1 (inner x)))
         (outer 42)",
    ) {
        Ok(v) => return Err(format!("expected an error, got {v:?}")),
        Err(e) => e,
    };
    let short = err.to_string();
    match engine.raise_error_to_string(err) {
        Some(report) => Ok(format!("`{short}`\n{report}")),
        None => Ok(format!("`{short}` (no rendered report available)")),
    }
}
