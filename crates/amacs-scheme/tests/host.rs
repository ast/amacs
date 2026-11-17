//! The interpreter as the program that embeds it sees it.

mod common;
use common::run_in;

use std::sync::atomic::Ordering;

use amacs_scheme::{Error, Outcome, Value, Vm};

fn done(outcome: Result<Outcome, amacs_scheme::Report>) -> Value {
    match outcome {
        Ok(Outcome::Done(value)) => value,
        other => panic!("expected a value, got {other:?}"),
    }
}

#[test]
fn natives_are_defined_and_called() {
    let mut vm = Vm::new();
    vm.core.define_native(
        "host-add",
        2,
        Some(2),
        "(host-add a b) Adds.",
        |core, args| match (args[0], args[1]) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a + b)),
            _ => Err(Error::new(format!(
                "host-add: expected numbers, got {}",
                core.write(args[0])
            ))),
        },
    );
    assert_eq!(run_in(&mut vm, "(host-add 1 2)"), "3");
    assert_eq!(run_in(&mut vm, "(map host-add '(1 2) '(10 20))"), "(11 22)");
    assert_eq!(
        run_in(&mut vm, "(host-add 'a 2)"),
        "error: test.scm:1:1: host-add: expected numbers, got a"
    );
    assert_eq!(
        run_in(&mut vm, "(host-add 1)"),
        "error: test.scm:1:1: host-add: expected 2 arguments, got 1"
    );
    assert_eq!(
        run_in(
            &mut vm,
            "(guard (e (#t (error-object-message e))) (host-add 'a 2))"
        ),
        "\"host-add: expected numbers, got a\""
    );
    assert_eq!(
        run_in(&mut vm, "(describe 'host-add)"),
        "\"host-add is a procedure built in: (host-add arg1 arg2)\\n(host-add a b) Adds.\""
    );
}

#[test]
fn procedures_are_called_from_outside() {
    let mut vm = Vm::new();
    let double = done(vm.eval("(lambda (x) (* 2 x))", "host"));
    let double = vm.core.root(double);
    // The procedure stays alive though nothing in the program refers to it.
    vm.eval(
        "(define junk (map (lambda (i) (make-vector 100 i)) (iota 2000))) (set! junk #f)",
        "host",
    )
    .unwrap();
    vm.collect_garbage();
    assert_eq!(
        done(vm.call(double.value(), &[Value::Int(21)])),
        Value::Int(42)
    );
    let report = vm.call(double.value(), &[]).unwrap_err();
    assert_eq!(report.message, "a procedure: expected 1 argument, got 0");
    let plus = done(vm.eval("+", "host"));
    assert_eq!(
        done(vm.call(plus, &[Value::Int(1), Value::Int(2)])),
        Value::Int(3)
    );
    let report = vm.call(Value::Int(5), &[]).unwrap_err();
    assert_eq!(
        report.message,
        "this is not a procedure and cannot be called: 5"
    );
}

#[test]
fn code_suspends_and_is_resumed() {
    let mut vm = Vm::new();
    vm.eval(
        "(define (ask question) (%suspend (list 'ask question)))",
        "host",
    )
    .unwrap();
    let outcome = vm
        .eval("(define answers '()) (define (run) (let* ((a (ask \"first\")) (b (ask \"second\"))) (set! answers (list a b)) 'finished)) (run)", "host")
        .unwrap();
    let Outcome::Suspended {
        fiber: first,
        value,
    } = outcome
    else {
        panic!("expected the code to wait");
    };
    assert_eq!(vm.core.write(value), "(ask \"first\")");
    // Other code runs while the first waits, and may wait itself.
    assert_eq!(run_in(&mut vm, "(+ 1 2)"), "3");
    let Ok(Outcome::Suspended { fiber: other, .. }) =
        vm.eval("(list 'other (ask \"third\"))", "host")
    else {
        panic!("expected the other code to wait");
    };
    vm.collect_garbage();
    let Ok(Outcome::Suspended {
        fiber: second,
        value,
    }) = vm.resume(&first, Value::Int(1))
    else {
        panic!("expected the code to wait again");
    };
    assert_eq!(vm.core.write(value), "(ask \"second\")");
    let value = done(vm.resume(&other, Value::Int(3)));
    assert_eq!(vm.core.write(value), "(other 3)");
    let value = done(vm.resume(&second, Value::Int(2)));
    assert_eq!(vm.core.write(value), "finished");
    assert_eq!(run_in(&mut vm, "answers"), "(1 2)");
    // What has been resumed cannot be resumed again.
    let report = vm.resume(&first, Value::Int(9)).unwrap_err();
    assert_eq!(report.message, "this is not suspended code");
}

#[test]
fn suspended_code_keeps_its_handlers_and_coroutines() {
    let mut vm = Vm::new();
    let outcome = vm
        .eval(
            "(define trail '())
             (define (note x) (set! trail (cons x trail)))
             (define worker (make-coroutine (lambda () (note (%suspend 'inside)) (yield 'yielded) 'worker-done)))
             (guard (e (#t (note (list 'caught e)) 'recovered))
               (dynamic-wind
                 (lambda () (note 'in))
                 (lambda () (note (coroutine-resume worker)) (error (%suspend 'outside)))
                 (lambda () (note 'out))))",
            "host",
        )
        .unwrap();
    let Outcome::Suspended { fiber, value } = outcome else {
        panic!("expected the code to wait");
    };
    assert_eq!(vm.core.write(value), "inside");
    let Ok(Outcome::Suspended { fiber, value }) = vm.resume(&fiber, Value::Int(1)) else {
        panic!("expected the code to wait again");
    };
    assert_eq!(vm.core.write(value), "outside");
    let message = vm.core.string("failed");
    let value = done(vm.resume(&fiber, message));
    assert_eq!(vm.core.write(value), "recovered");
    assert_eq!(
        run_in(
            &mut vm,
            "(list (reverse (map (lambda (x) (if (pair? x) (list (car x) (error-object-message (cadr x))) x)) trail)) (coroutine-state worker))"
        ),
        "((in 1 yielded out (caught \"failed\")) suspended)"
    );
}

#[test]
fn an_error_in_resumed_code_is_reported() {
    let mut vm = Vm::new();
    let Ok(Outcome::Suspended { fiber, .. }) =
        vm.eval("(define (f) (car (%suspend 'wait)))\n(f)", "host")
    else {
        panic!("expected the code to wait");
    };
    let report = vm.resume(&fiber, Value::Int(5)).unwrap_err();
    assert_eq!(report.summary(), "host:1:13: car: expected a pair, got 5");
}

#[test]
fn running_code_can_be_interrupted() {
    let mut vm = Vm::new();
    let interrupt = vm.interrupt_handle();
    let setter = interrupt.clone();
    let thread = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        setter.store(true, Ordering::Relaxed);
    });
    // Catching errors does not catch the interruption, but what must be
    // done on the way out is done.
    let report = vm
        .eval(
            "(define cleaned #f)
             (define (spin) (guard (e (#t (spin))) (let loop () (loop))))
             (with-exception-handler (lambda (e) 'ignored)
               (lambda () (dynamic-wind (lambda () #f) spin (lambda () (set! cleaned #t)))))",
            "host",
        )
        .unwrap_err();
    thread.join().unwrap();
    assert_eq!(report.message, "interrupted");
    assert_eq!(run_in(&mut vm, "cleaned"), "#t");
    assert!(!interrupt.load(Ordering::Relaxed));
    assert_eq!(run_in(&mut vm, "(+ 1 2)"), "3");
}

#[test]
fn fuel_bounds_how_long_code_runs() {
    let mut vm = Vm::new();
    vm.fuel = Some(10_000);
    assert_eq!(
        run_in(&mut vm, "(let loop () (loop))"),
        "error: interrupted"
    );
    vm.fuel = Some(10_000);
    assert_eq!(
        run_in(
            &mut vm,
            "(let loop ((i 0)) (if (< i 100) (loop (+ i 1)) i))"
        ),
        "100"
    );
    vm.fuel = None;
}

#[test]
fn garbage_is_collected_and_what_is_used_is_kept() {
    let mut vm = Vm::new();
    // Much more is allocated than is alive at any time.
    assert_eq!(
        run_in(
            &mut vm,
            "(define keep (make-hash-table))
             (define (churn n)
               (let loop ((i 0) (acc '()))
                 (if (= i n)
                     (length acc)
                     (begin
                       (when (= 0 (modulo i 1000))
                         (hash-table-set! keep i (list i (number->string i) (vector i))))
                       (loop (+ i 1) (if (> (length acc) 50) '() (cons (make-string 10 #\\x) acc)))))))
             (churn 400000)"
        ),
        "16"
    );
    assert!(
        vm.core.heap.collections > 3,
        "the collector ran: {}",
        vm.core.heap.collections
    );
    assert!(
        vm.core.heap.len() < 200_000,
        "garbage was freed: {}",
        vm.core.heap.len()
    );
    assert_eq!(
        run_in(
            &mut vm,
            "(list (hash-table-count keep) (hash-table-ref keep 399000))"
        ),
        "(400 (399000 \"399000\" #(399000)))"
    );
    // Closures, cells, records, parameters and coroutines survive too.
    assert_eq!(
        run_in(
            &mut vm,
            "(define-record-type box (make-box v) box? (v unbox set-box!))
             (define b (make-box (list 1 2)))
             (define p (make-parameter (vector 'p)))
             (define counter (let ((n 0)) (lambda () (set! n (+ n 1)) n)))
             (define co (make-coroutine (lambda () (let loop ((i 0)) (yield (list 'turn i)) (loop (+ i 1))))))
             (coroutine-resume co) (counter)
             (churn 300000)
             (list (unbox b) (p) (counter) (coroutine-resume co))"
        ),
        "((1 2) #(p) 2 (turn 1))"
    );
}

#[test]
fn the_program_can_look_at_itself() {
    let mut vm = Vm::new();
    vm.eval(
        ";; A file of definitions.\n(define (area width height)\n  \"The area of a rectangle.\"\n  (* width height))\n(define (sum . numbers) (apply + numbers))\n(define answer 42)\n",
        "shapes.scm",
    )
    .unwrap();
    assert_eq!(
        run_in(
            &mut vm,
            "(list (procedure-name area) (procedure-documentation area) (procedure-arity area) (procedure-parameters area) (procedure-location area))"
        ),
        "(area \"The area of a rectangle.\" (2 . 2) (width height) \"shapes.scm:2:1\")"
    );
    assert_eq!(
        run_in(&mut vm, "(procedure-source area)"),
        "\"(define (area width height)\\n  \\\"The area of a rectangle.\\\"\\n  (* width height))\""
    );
    assert_eq!(
        run_in(
            &mut vm,
            "(list (procedure-arity sum) (procedure-parameters sum) (procedure-arity car) (procedure-parameters car) (procedure-name car) (procedure-name (lambda (x) x)))"
        ),
        "((0 . #f) numbers (1 . 1) #f car #f)"
    );
    assert_eq!(
        run_in(&mut vm, "(describe 'sum)"),
        "\"sum is a procedure: (sum . numbers)\\nDefined at shapes.scm:5:1\""
    );
    assert_eq!(
        run_in(&mut vm, "(describe 'answer)"),
        "\"answer is a variable. Its value is 42\\nDefined at shapes.scm:6:1\""
    );
    assert_eq!(
        run_in(
            &mut vm,
            "(string-search (describe 'map) \"(map f first . rest)\\n(map f list ...) A list of what f returns\")"
        ),
        "20"
    );
    assert_eq!(
        run_in(
            &mut vm,
            "(string-prefix? \"prelude.scm:\" (procedure-location map))"
        ),
        "#t"
    );
    assert_eq!(
        run_in(
            &mut vm,
            "(list (describe 'when) (describe 'while) (describe 'nothing-here))"
        ),
        "(\"when is a special form.\" \"while is a macro.\" \"nothing-here is not defined.\")"
    );
    assert_eq!(
        run_in(&mut vm, "(apropos \"hash-table-c\")"),
        "(hash-table-clear! hash-table-contains? hash-table-copy hash-table-count)"
    );
    assert_eq!(
        run_in(
            &mut vm,
            "(list (bound? 'area) (bound? 'nope) (macro? 'while) (macro? 'if) (macro? 'car) (global-ref 'answer) (global-ref 'nope 'default) (global-location 'answer))"
        ),
        "(#t #f #t #t #f 42 default \"shapes.scm:6:1\")"
    );
    assert_eq!(
        run_in(
            &mut vm,
            "(and (memq 'area (global-names)) (memq 'car (global-names)) #t)"
        ),
        "#t"
    );
    // Every procedure that is built in says what it does.
    assert_eq!(
        run_in(
            &mut vm,
            "(filter (lambda (name) (let ((value (global-ref name))) (and (procedure? value) (not (procedure-documentation value)) (not (char=? #\\% (string-ref (symbol->string name) 0)))))) (global-names))"
        ),
        "(promise? sum)"
    );
}

#[test]
fn a_new_definition_is_seen_by_everything_that_uses_the_name() {
    let mut vm = Vm::new();
    vm.eval("(define (greeting) \"hello\") (define (greet name) (string-append (greeting) \", \" name))", "one.scm").unwrap();
    assert_eq!(run_in(&mut vm, "(greet \"you\")"), "\"hello, you\"");
    vm.eval("(define (greeting) \"good day\")", "two.scm")
        .unwrap();
    assert_eq!(
        run_in(
            &mut vm,
            "(list (greet \"you\") (procedure-location greeting))"
        ),
        "(\"good day, you\" \"two.scm:1:1\")"
    );
    // What is built in can be wrapped or replaced the same way.
    vm.eval("(define original-length length) (define calls 0) (define (length items) (set! calls (+ calls 1)) (original-length items))", "three.scm").unwrap();
    assert_eq!(run_in(&mut vm, "(list (length '(1 2 3)) calls)"), "(3 1)");
    assert_eq!(
        run_in(
            &mut vm,
            "(global-set! 'greeting (lambda () \"hej\")) (greet \"du\")"
        ),
        "\"hej, du\""
    );
    // A macro defined later is used by code compiled after it, not before.
    vm.eval("(define (twice-old x) (twice x)) (define-syntax twice (syntax-rules () ((_ e) (* 2 e)))) (define (twice-new x) (twice x))", "four.scm").unwrap();
    assert_eq!(run_in(&mut vm, "(twice-new 4)"), "8");
    assert_eq!(
        run_in(&mut vm, "(twice-old 4)"),
        "error: four.scm:1:24: unbound variable: twice"
    );
}

#[test]
fn a_mistake_in_a_file_does_not_stop_the_forms_before_it() {
    let mut vm = Vm::new();
    let mut results = Vec::new();
    vm.eval_each(
        "(define a 1)\n(car a)\n(define b 2)\n(define c",
        "file.scm",
        |result| {
            results.push(result.map(|_| ()).map_err(|report| report.summary()));
            true
        },
    );
    assert_eq!(
        results,
        vec![
            Ok(()),
            Err("file.scm:2:1: car: expected a pair, got 1".to_owned()),
            Ok(()),
            Err("file.scm:4:1: this bracket is never closed".to_owned()),
        ]
    );
    assert_eq!(run_in(&mut vm, "(list a b)"), "(1 2)");
    // `eval` stops at the first error.
    assert_eq!(
        run_in(&mut vm, "(define d 1) (car d) (define e 2)"),
        "error: test.scm:1:14: car: expected a pair, got 1"
    );
    assert_eq!(run_in(&mut vm, "(list (bound? 'd) (bound? 'e))"), "(#t #f)");
}
