//! The language as programs see it.

mod common;
use common::{run, run_in};

use amacs_scheme::Vm;

#[test]
fn definitions_and_calls() {
    assert_eq!(run("(define x 2) (define (f a b) (+ a b x)) (f 1 2)"), "5");
    assert_eq!(run("(define (f . all) all) (f 1 2 3)"), "(1 2 3)");
    assert_eq!(
        run("(define (f a . more) (list a more)) (list (f 1) (f 1 2 3))"),
        "((1 ()) (1 (2 3)))"
    );
    assert_eq!(run("((lambda (a b) (- a b)) 5 3)"), "2");
    assert_eq!(run("(define ((adder n) m) (+ n m)) ((adder 2) 3)"), "5");
    assert_eq!(run("(define x 1) (set! x (+ x 1)) x"), "2");
    assert_eq!(
        run("(define (f) (define a 1) (define (g) (* a 2)) (g)) (f)"),
        "2"
    );
    assert_eq!(run("(apply + 1 2 '(3 4))"), "10");
    assert_eq!(run("(define x 5) (define (get) x) (define x 6) (get)"), "6");
}

#[test]
fn closures_share_what_they_capture() {
    assert_eq!(
        run("(define (counter)
               (let ((n 0))
                 (cons (lambda () (set! n (+ n 1)) n) (lambda () n))))
             (define c (counter))
             ((car c)) ((car c))
             (list ((cdr c)) ((cdr (counter))))"),
        "(2 0)"
    );
    // Each turn of a loop has its own variable.
    assert_eq!(
        run("(define procs (map (lambda (i) (lambda () i)) '(1 2 3)))
             (map (lambda (p) (p)) procs)"),
        "(1 2 3)"
    );
    assert_eq!(
        run("(define (f x) (lambda () (set! x (+ x 1)) x)) (define g (f 10)) (g) (g)"),
        "12"
    );
    assert_eq!(
        run("(define (even? n) (if (= n 0) #t (odd? (- n 1))))
             (define (odd? n) (if (= n 0) #f (even? (- n 1))))
             (define (f) (define (a n) (if (= n 0) 'a (b (- n 1)))) (define (b n) (if (= n 0) 'b (a (- n 1)))) (a 7))
             (list (even? 100) (f))"),
        "(#t b)"
    );
}

#[test]
fn tail_calls_do_not_grow_the_stack() {
    let looping = |body: &str| run(&format!("(define (loop n) {body}) (loop 300000)"));
    assert_eq!(looping("(if (= n 0) 'done (loop (- n 1)))"), "done");
    assert_eq!(
        looping("(cond ((= n 0) 'done) (else (loop (- n 1))))"),
        "done"
    );
    assert_eq!(
        looping("(and #t (or #f (if (= n 0) 'done (loop (- n 1)))))"),
        "done"
    );
    assert_eq!(
        looping("(when (> n -1) (if (= n 0) 'done (begin 1 (loop (- n 1)))))"),
        "done"
    );
    assert_eq!(
        looping("(let ((m (- n 1))) (if (< m 0) 'done (loop m)))"),
        "done"
    );
    assert_eq!(
        looping("(if (= n 0) 'done (apply loop (list (- n 1))))"),
        "done"
    );
    assert_eq!(
        looping("(case n ((0) 'done) (else (loop (- n 1))))"),
        "done"
    );
    assert_eq!(
        run("(do ((i 0 (+ i 1)) (sum 0 (+ sum i))) ((= i 300000) sum))"),
        "44999850000"
    );
    // Recursion that is not in tail position goes deep, and comes back.
    assert_eq!(
        run("(define (count n) (if (= n 0) 0 (+ 1 (count (- n 1))))) (count 100000)"),
        "100000"
    );
    assert_eq!(
        run("(define (forever n) (+ 1 (forever n))) (forever 1)"),
        "error: test.scm:1:26: too many calls are in progress at once; is a recursion missing its end?"
    );
}

#[test]
fn a_loop_that_calls_itself_starts_afresh() {
    // Each turn has variables of its own, for the closures made in it.
    assert_eq!(
        run("(let loop ((i 0) (made '()))
               (if (= i 3)
                   (map (lambda (f) (f)) made)
                   (loop (+ i 1) (cons (lambda () i) made))))"),
        "(2 1 0)"
    );
    assert_eq!(
        run("(let loop ((i 0) (made '()))
               (if (= i 3)
                   (map (lambda (f) (f)) made)
                   (let ((f (lambda () (set! i (+ i 10)) i)))
                     (loop (+ i 1) (cons f made)))))"),
        "(12 11 10)"
    );
    // A definition in the body has no value left over from the turn before.
    assert_eq!(
        run("(let loop ((i 0))
               (define a (if (= i 1) b 1))
               (define b 2)
               (if (= i 1) a (loop (+ i 1))))"),
        "error: test.scm:2:38: a variable is used before its definition has been reached"
    );
    // A definition inside a body is a loop too.
    assert_eq!(
        run("(define (sum n)
               (define (go i acc) (if (= i n) acc (go (+ i 1) (+ acc i))))
               (go 0 0))
             (sum 300000)"),
        "44999850000"
    );
    // The name may come to mean something else, by assignment or by a
    // binding nearer to the call.
    assert_eq!(
        run("(let loop ((i 0))
               (if (= i 0)
                   (begin (set! loop (lambda (i) 'other)) (loop 1))
                   'itself))"),
        "other"
    );
    assert_eq!(
        run("(let loop ((i 0)) (let ((loop (lambda (x) 'nearer))) (loop i)))"),
        "nearer"
    );
    // A call from a function inside the loop is an ordinary call.
    assert_eq!(
        run("(let loop ((i 0)) (if (= i 3) i ((lambda () (loop (+ i 1))))))"),
        "3"
    );
    assert_eq!(
        run("(let loop ((i 0)) (if (= i 0) (loop 1 2) i))"),
        "error: test.scm:1:31: loop: expected 1 argument, got 2"
    );
}

#[test]
fn loops_inside_expressions_and_each_other() {
    // A loop whose value something still waits for.
    assert_eq!(
        run("(define (f n) (+ 1000 (let loop ((i 0) (sum 0)) (if (= i n) sum (loop (+ i 1) (+ sum i))))))
             (list (f 4) (list 'a (do ((i 0 (+ i 1))) ((= i 3) i)) 'b))"),
        "(1006 (a 3 b))"
    );
    // One inside the other, the inner one using and leaving for the outer.
    assert_eq!(
        run("(define (pairs n)
               (let outer ((i 0) (found '()))
                 (if (= i n)
                     (reverse found)
                     (let inner ((j 0) (found found))
                       (cond ((= j n) (outer (+ i 1) found))
                             ((< i j) (inner (+ j 1) (cons (cons i j) found)))
                             (else (inner (+ j 1) found)))))))
             (pairs 3)"),
        "((0 . 1) (0 . 2) (1 . 2))"
    );
    assert_eq!(
        run("(define (table n)
               (do ((i 0 (+ i 1)) (rows '() (cons (do ((j 0 (+ j 1)) (row '() (cons (* i j) row)))
                                                      ((= j n) (reverse row)))
                                                  rows)))
                   ((= i n) (reverse rows))))
             (table 3)"),
        "((0 0 0) (0 1 2) (0 2 4))"
    );
    // The name used in other ways makes an ordinary procedure of the loop.
    assert_eq!(
        run("(let loop ((i 0)) (if (= i 3) '() (cons i (loop (+ i 1)))))"),
        "(0 1 2)"
    );
    assert_eq!(
        run("(let loop ((x '(1 (2 3) 4))) (if (pair? x) (apply + (map loop x)) x))"),
        "10"
    );
    assert_eq!(
        run("(let loop ((i 0)) (if (procedure? loop) (if (< i 2) (loop (+ i 1)) i) 'no))"),
        "2"
    );
    // The variables around a loop, assigned to in it and after it.
    assert_eq!(
        run("(define (f)
               (let ((count 0) (made '()))
                 (let loop ((i 0))
                   (when (< i 3)
                     (set! count (+ count i))
                     (set! made (cons (lambda () (* i count)) made))
                     (loop (+ i 1))))
                 (set! count 10)
                 (map (lambda (g) (g)) made)))
             (f)"),
        "(20 10 0)"
    );
    // An error in a loop says where.
    let report = run(
        "(define (f items)\n  (let loop ((rest items))\n    (if (null? rest) 'done (loop (cdr (car rest))))))\n(f '(1))",
    );
    assert!(report.starts_with("error: test.scm:3:"), "{report}");
    assert!(report.ends_with("cdr: expected a pair, got 1"), "{report}");
}

#[test]
fn built_in_procedures_with_instructions_of_their_own() {
    // The tests that are part of a conditional.
    assert_eq!(
        run("(map (lambda (x)
                    (list (if (null? x) 'null 'no)
                          (if (pair? x) 'pair 'no)
                          (if (not x) 'false 'no)
                          (if (eq? x 'a) 'a 'no)
                          (if (eqv? x 2) 'two 'no)))
                  (list '() '(1) #f 'a 2))"),
        "((null no no no no) (no pair no no no) (no no false no no) \
         (no no no a no) (no no no no two))"
    );
    assert_eq!(
        run("(define (order a b)
               (list (if (< a b) '< '-) (if (> a b) '> '-) (if (<= a b) '<= '-)
                     (if (>= a b) '>= '-) (if (= a b) '= '-)))
             (list (order 1 2) (order 2 2) (order 2.5 1.5) (order 1 1.5))"),
        "((< - <= - -) (- - <= >= =) (- > - >= -) (< - <= - -))"
    );
    // A number that is part of the instruction, with numbers of every kind.
    assert_eq!(
        run(
            "(define (both x) (list (+ x 1) (- x 1) (+ x -2) (- x 100000)))
             (list (both 5) (both 1.5))"
        ),
        "((6 4 3 -99995) (2.5 0.5 -0.5 -99998.5))"
    );
    assert_eq!(
        run(
            "(let ((most 9223372036854775807) (least -9223372036854775808))
               (list (equal? (+ most 1) (apply + (list most 1)))
                     (equal? (- least 1) (apply - (list least 1)))))"
        ),
        "(#t #t)"
    );
    assert_eq!(
        run("(define (f x) (if (< x 1) 'less 'more)) (f 'a)"),
        run("(define (f x) (if (apply < (list x 1)) 'less 'more)) (f 'a)")
    );
    assert_eq!(
        run("(define (f x) (+ x 1)) (f \"a\")"),
        run("(define (f x) (apply + (list x 1))) (f \"a\")")
    );

    // A name that is given another meaning has it in code compiled before
    // and after, and has its first meaning again when given it back.
    assert_eq!(
        run("(define (uses x)
               (list (+ x 1) (- x 1) (+ x x) (car (list x)) (if (< x 0) 'below 'not)
                     (if (null? x) 'null 'not) (if (not x) 'false 'true)))
             (define before (uses 5))
             (define first-meanings (list + - car < null? not))
             (define (+ a b) (list 'plus a b))
             (define (- a b) (list 'minus a b))
             (set! car (lambda (pair) 'car))
             (global-set! '< (lambda (a b) #t))
             (define (null? x) 'yes)
             (define (not x) #f)
             (define changed (uses 5))
             (define (uses-anew x) (list (+ x 1) (if (< x 0) 'below 'not)))
             (define anew (uses-anew 5))
             (set! + (list-ref first-meanings 0))
             (set! - (list-ref first-meanings 1))
             (set! car (list-ref first-meanings 2))
             (set! < (list-ref first-meanings 3))
             (set! null? (list-ref first-meanings 4))
             (set! not (list-ref first-meanings 5))
             (list before changed anew (uses 5))"),
        "((6 4 10 5 not not true) \
         ((plus 5 1) (minus 5 1) (plus 5 5) car below null true) \
         ((plus 5 1) below) \
         (6 4 10 5 not not true))"
    );
}

#[test]
fn special_forms() {
    assert_eq!(
        run("(list (if #f #f) (if '() 1 2) (when #f 1) (unless #f 1 2))"),
        "(#<void> 1 #<void> 2)"
    );
    assert_eq!(
        run("(list (and) (and 1 2) (and 1 #f 3) (or) (or #f 2) (or #f #f))"),
        "(#t 2 #f #f 2 #f)"
    );
    assert_eq!(
        run("(let* ((a 1) (b (+ a 1)) (a (* b 10))) (list a b))"),
        "(20 2)"
    );
    assert_eq!(run("(let ((a 1)) (let ((a 2) (b a)) (list a b)))"), "(2 1)");
    assert_eq!(
        run("(letrec ((f (lambda (n) (if (= n 0) 1 (* n (f (- n 1))))))) (f 10))"),
        "3628800"
    );
    assert_eq!(
        run("(define (kind x) (cond ((assv x '((1 . one))) => cdr) ((symbol? x) 'symbol) ((string? x)) (else 'other)))
             (map kind '(1 a \"s\" 2.5))"),
        "(one symbol #t other)"
    );
    assert_eq!(
        run("(define (kind x) (case x ((1 2 3) 'small) ((a b) 'letter) ((#\\x) => char-upcase) (else 'other)))
             (map kind '(2 b #\\x 99))"),
        "(small letter #\\X other)"
    );
    assert_eq!(
        run("(do ((i 0 (+ i 1)) (acc '() (cons i acc))) ((= i 3) acc))"),
        "(2 1 0)"
    );
    assert_eq!(
        run("(define-values (a b . c) (values 1 2 3 4)) (list a b c)"),
        "(1 2 (3 4))"
    );
    assert_eq!(
        run("(define (f) (define-values (a b) (values 1 2)) (+ a b)) (f)"),
        "3"
    );
    assert_eq!(
        run("(let*-values (((a b) (values 1 2)) ((c) (values (+ a b)))) c)"),
        "3"
    );
    assert_eq!(
        run("(receive-like)"),
        "error: test.scm:1:2: unbound variable: receive-like"
    );
    assert_eq!(run("(let loop ((i 0)) (if (< i 3) (loop (+ i 1)) i))"), "3");
    assert_eq!(run("(define i 0) (while (< i 5) (set! i (+ i 1))) i"), "5");
}

#[test]
fn quasiquote() {
    assert_eq!(run("(let ((x 1) (y '(2 3))) `(a ,x ,@y b))"), "(a 1 2 3 b)");
    assert_eq!(run("`#(1 ,(+ 1 1))"), "#(1 2)");
    assert_eq!(run("`(1 `(2 ,(3 ,(+ 1 3))))"), "(1 `(2 ,(3 4)))");
    assert_eq!(run("(let ((f (lambda (x) (* x 2)))) ((car `(,f)) 4))"), "8");
    assert_eq!(run("`(1 . ,(+ 1 1))"), "(1 . 2)");
    assert_eq!(run("'(a . (b . (c)))"), "(a b c)");
    assert_eq!(run("(quote #:key)"), "#:key");
}

#[test]
fn macros_keep_their_names_apart() {
    // The macro's `tmp` is not the user's `tmp`.
    assert_eq!(
        run("(define-syntax swap! (syntax-rules () ((_ a b) (let ((tmp a)) (set! a b) (set! b tmp)))))
             (let ((tmp 1) (other 2)) (swap! tmp other) (list tmp other))"),
        "(2 1)"
    );
    // The macro's `if` is the real one though the user has a variable `if`.
    assert_eq!(
        run("(define-syntax my-or (syntax-rules () ((_) #f) ((_ e) e) ((_ e r ...) (let ((t e)) (if t t (my-or r ...))))))
             (let ((t 5) (if list)) (my-or #f t))"),
        "5"
    );
    // A name the macro refers to means what it meant where the macro is.
    assert_eq!(
        run("(define (helper x) (* x 2))
             (define-syntax twice (syntax-rules () ((_ e) (helper e))))
             (let ((helper (lambda (x) 'wrong))) (twice 4))"),
        "8"
    );
    assert_eq!(
        run("(define-syntax for (syntax-rules (in) ((_ x in items body ...) (for-each (lambda (x) body ...) items))))
             (define total 0) (for x in '(1 2 3) (set! total (+ total x))) total"),
        "6"
    );
    assert_eq!(
        run("(define-syntax my-let (syntax-rules () ((_ ((name value) ...) body ...) ((lambda (name ...) body ...) value ...))))
             (my-let ((a 1) (b 2)) (+ a b))"),
        "3"
    );
    assert_eq!(
        run(
            "(define-syntax flat (syntax-rules () ((_ (a ...) ...) '(a ... ...))))
             (flat (1 2) (3) ())"
        ),
        "(1 2 3)"
    );
    assert_eq!(
        run("(define (f) (define-syntax ten (syntax-rules () ((_) 10))) (+ (ten) 1)) (f)"),
        "11"
    );
    assert_eq!(
        run(
            "(define-syntax-rule (unless2 test body ...) (if test #f (begin body ...))) (unless2 #f 1 2)"
        ),
        "2"
    );
    assert_eq!(
        run("(define-syntax m (syntax-rules () ((_ a) a))) (m 1 2)"),
        "error: test.scm:1:47: no rule of the macro matches this use"
    );
    assert_eq!(
        run("(define-syntax forever (syntax-rules () ((_) (forever)))) (forever)"),
        "error: test.scm:1:59: this macro's expansion does not come to an end"
    );
}

#[test]
fn errors_say_what_and_where() {
    assert_eq!(run("(foo)"), "error: test.scm:1:2: unbound variable: foo");
    assert_eq!(
        run("\n  (car '())"),
        "error: test.scm:2:3: car: expected a pair, got ()"
    );
    assert_eq!(
        run("(define (f x) x) (f 1 2)"),
        "error: test.scm:1:18: f: expected 1 argument, got 2"
    );
    assert_eq!(
        run("((lambda (a b . c) a) 1)"),
        "error: test.scm:1:1: a procedure: expected at least 2 arguments, got 1"
    );
    assert_eq!(
        run("(5 5)"),
        "error: test.scm:1:1: this is not a procedure and cannot be called: 5"
    );
    assert_eq!(
        run("(error \"no such thing:\" 'x \"y\" 3)"),
        "error: test.scm:1:1: no such thing: x \"y\" 3"
    );
    assert_eq!(
        run("(raise 'oops)"),
        "error: test.scm:1:1: uncaught exception: oops"
    );
    assert_eq!(
        run("(if)"),
        "error: test.scm:1:1: if takes a test, a consequence and at most one alternative"
    );
    assert_eq!(
        run("(let ((x)) x)"),
        "error: test.scm:1:7: a binding is a name and a value"
    );
    assert_eq!(
        run("(lambda (1) 1)"),
        "error: test.scm:1:10: a parameter must be an identifier"
    );
    assert_eq!(
        run("(define (f) (define x 1))"),
        "error: test.scm:1:1: a body must end with an expression, not a definition"
    );
    assert_eq!(
        run("(+ 1 (if #t (define x 1)))"),
        "error: test.scm:1:13: a definition is only allowed at the top level or at the start of a body"
    );
    assert_eq!(
        run("(1 2"),
        "error: test.scm:1:1: this bracket is never closed"
    );
    assert_eq!(
        run("(set! nowhere 1)"),
        "error: test.scm:1:1: set!: there is no variable nowhere"
    );
    assert_eq!(
        run("(define (f) (g)) (define (g) y) (define y 1) (f) (letrec ((a b) (b 1)) a)"),
        "error: test.scm:1:62: a variable is used before its definition has been reached"
    );
    assert_eq!(run("else"), "error: test.scm:1:1: unbound variable: else");
    assert_eq!(
        run("(vector-ref (vector 1 2) 2)"),
        "error: test.scm:1:1: vector-ref: there is no item number 2"
    );
    assert_eq!(
        run("(+ 'a 1)"),
        "error: test.scm:1:1: +: expected a number, got a"
    );
    assert_eq!(
        run("(quotient 1 0)"),
        "error: test.scm:1:1: quotient: division by zero"
    );
}

#[test]
fn errors_carry_the_calls_that_led_to_them() {
    let mut vm = Vm::new();
    let report = vm
        .eval(
            "(define (inner x)\n  (+ 1 (car x)))\n(define (middle x)\n  (list (inner x)))\n(define (outer)\n  (vector (middle 5)))\n(outer)",
            "trace.scm",
        )
        .unwrap_err();
    assert_eq!(
        report.to_string(),
        "error: car: expected a pair, got 5\n  at trace.scm:2:8\n  in inner, at trace.scm:2:8\n  in middle, at trace.scm:4:9\n  in outer, at trace.scm:6:11\n  in top level, at trace.scm:7:1"
    );
    // The same is there for the program to look at.
    assert_eq!(
        run_in(&mut vm, "(guard (e (#t (error-object-trace e))) (outer))"),
        "((inner . \"trace.scm:2:8\") (middle . \"trace.scm:4:9\") (outer . \"trace.scm:6:11\") (#f . \"test.scm:1:1\"))"
    );
}

#[test]
fn guard_and_handlers() {
    assert_eq!(
        run("(guard (e ((symbol? e) (list 'sym e)) ((string? e) 'str)) (raise 'boom))"),
        "(sym boom)"
    );
    assert_eq!(
        run("(guard (e ((string? e) 'str)) (guard (e2 ((number? e2) 'num)) (raise \"s\")))"),
        "str"
    );
    assert_eq!(
        run("(guard (e (#f 'no)) (raise 'unhandled))"),
        "error: test.scm:1:1: uncaught exception: unhandled"
    );
    assert_eq!(
        run(
            "(guard (e ((error-object? e) (list (error-object-message e) (error-object-irritants e)))) (error \"bad\" 1 '(2)))"
        ),
        "(\"bad\" (1 (2)))"
    );
    assert_eq!(
        run("(guard (e (#t (error-object->string e))) (vector-ref (vector) 0))"),
        "\"vector-ref: there is no item number 0\""
    );
    assert_eq!(
        run("(guard (e ((assq 'a e) => cdr) ((assq 'b e))) (raise (list (cons 'a 42))))"),
        "42"
    );
    assert_eq!(run("(+ 1 (guard (e (#t 10)) (+ 100 (raise 'x))))"), "11");
    assert_eq!(
        run("(guard (e (#t (list 'outer e))) (guard (e (#t (raise (list 'again e)))) (raise 1)))"),
        "(outer (again 1))"
    );
    // A handler is called where the error is, and can give a value back.
    assert_eq!(
        run("(with-exception-handler (lambda (e) 42) (lambda () (+ 1 (raise-continuable 'c))))"),
        "43"
    );
    // Returning from one for an error that cannot be continued passes it on.
    assert_eq!(
        run("(define log '())
             (guard (e (#t (list e log)))
               (with-exception-handler (lambda (e) (set! log (cons 'seen log)) 'ignored)
                 (lambda () (+ 1 (raise 'fatal)))))"),
        "(fatal (seen))"
    );
    // While a handler runs, the handlers outside it are in force.
    assert_eq!(
        run("(with-exception-handler (lambda (e) (list 'outer e))
               (lambda () (with-exception-handler (lambda (e) (raise-continuable (list 'inner e)))
                            (lambda () (raise-continuable 1)))))"),
        "(outer (inner 1))"
    );
    assert_eq!(
        run("(define trail '())
             (define (note x) (set! trail (cons x trail)))
             (guard (e (#t (note 'caught)))
               (dynamic-wind (lambda () (note 'in)) (lambda () (error \"x\")) (lambda () (note 'out))))
             (reverse trail)"),
        "(in out caught)"
    );
    assert_eq!(run("(unwind-protect 1 2 3)"), "1");
}

#[test]
fn escapes() {
    assert_eq!(run("(+ 1 (call/cc (lambda (k) (+ 10 (k 1)))))"), "2");
    assert_eq!(
        run(
            "(call-with-current-continuation (lambda (k) (for-each (lambda (x) (when (negative? x) (k x))) '(1 -2 3)) 'none))"
        ),
        "-2"
    );
    // An escape passes through guards and handlers that are for errors.
    assert_eq!(
        run(
            "(call/cc (lambda (k) (guard (e (#t 'guarded)) (with-exception-handler (lambda (e) 'handled) (lambda () (k 'escaped))))))"
        ),
        "escaped"
    );
    assert_eq!(
        run("(define trail '())
             (call/cc (lambda (k) (dynamic-wind (lambda () #f) (lambda () (k 1)) (lambda () (set! trail 'unwound)))))
             trail"),
        "unwound"
    );
    assert_eq!(
        run("(call-with-values (lambda () (call/cc (lambda (k) (k 1 2)))) list)"),
        "(1 2)"
    );
    assert_eq!(
        run("(define saved #f) (call/cc (lambda (k) (set! saved k))) (saved 1)"),
        "error: an escape procedure was called after the call that made it had returned"
    );
}

#[test]
fn coroutines() {
    assert_eq!(
        run("(define (generator items)
               (make-coroutine (lambda () (for-each yield items) 'end)))
             (define g (generator '(a b)))
             (list (coroutine-state g) (coroutine-resume g) (coroutine-state g)
                   (coroutine-resume g) (coroutine-resume g) (coroutine-state g))"),
        "(new a suspended b end done)"
    );
    // What a coroutine is resumed with is what its yield returns.
    assert_eq!(
        run("(define c (make-coroutine (lambda () (let loop ((sum 0)) (loop (+ sum (yield sum)))))))
             (coroutine-resume c)
             (list (coroutine-resume c 5) (coroutine-resume c 10))"),
        "(5 15)"
    );
    // An error in a coroutine comes out where it was resumed.
    assert_eq!(
        run("(define c (make-coroutine (lambda () (car 1))))
             (list (guard (e (#t 'caught)) (coroutine-resume c)) (coroutine-state c))"),
        "(caught done)"
    );
    assert_eq!(
        run("(define c (make-coroutine (lambda () 1))) (coroutine-resume c) (coroutine-resume c)"),
        "error: test.scm:1:64: coroutine-resume: expected a coroutine that is waiting #<coroutine>"
    );
    assert_eq!(
        run("(yield 1)"),
        "error: test.scm:1:1: yield: no coroutine is running"
    );
    // Coroutines in coroutines.
    assert_eq!(
        run("(define inner (make-coroutine (lambda () (yield 1) (yield 2) 'inner-done)))
             (define outer (make-coroutine (lambda () (yield (coroutine-resume inner)) (yield (coroutine-resume inner)) 'outer-done)))
             (list (coroutine-resume outer) (coroutine-resume outer) (coroutine-resume outer))"),
        "(1 2 outer-done)"
    );
}

#[test]
fn numbers() {
    assert_eq!(
        run("(list (+) (*) (+ 1 2.5) (- 5) (/ 6 3) (/ 1 2) (/ 2.0) (* 1.5 2))"),
        "(0 1 3.5 -5 2 0.5 0.5 3.0)"
    );
    assert_eq!(
        run(
            "(list (quotient 7 2) (remainder -7 2) (modulo -7 2) (modulo 7 -2) (expt 2 10) (expt 2 0.5) (expt 2 -1))"
        ),
        "(3 -1 1 -1 1024 1.4142135623730951 0.5)"
    );
    assert_eq!(
        run(
            "(list (max 1 2.0) (min 1 2) (abs -5) (gcd 12 18) (lcm 4 6) (sqrt 16) (sqrt 2) (exact 2.0) (inexact 1))"
        ),
        "(2.0 1 5 6 12 4 1.4142135623730951 2 1.0)"
    );
    assert_eq!(
        run("(list (floor 2.5) (ceiling 2.5) (round 2.5) (round 3.5) (truncate -2.5) (round 7))"),
        "(2.0 3.0 2.0 4.0 -2.0 7)"
    );
    assert_eq!(
        run("(list (= 1 1.0) (< 1 2 3) (< 1 3 2) (>= 2 2 1) (eqv? 1 1.0) (equal? 2 2))"),
        "(#t #t #f #t #f #t)"
    );
    // Integers that overflow become floats rather than wrap around.
    assert_eq!(
        run(
            "(list (* 4611686018427387904 2) (+ 9223372036854775807 1) (- -9223372036854775807 2))"
        ),
        "(9.223372036854776e18 9.223372036854776e18 -9.223372036854776e18)"
    );
    assert_eq!(
        run(
            "(list (number->string 255 16) (number->string -5 2) (string->number \"ff\" 16) (string->number \"1e3\") (string->number \"x\"))"
        ),
        "(\"ff\" \"-101\" 255 1000.0 #f)"
    );
    assert_eq!(
        run("(list (exact->inexact 1/4) #xff #b101 1e2 -0.5 +inf.0 (- +inf.0) (nan? +nan.0))"),
        "(0.25 255 5 100.0 -0.5 +inf.0 -inf.0 #t)"
    );
    assert_eq!(
        run(
            "(list (integer? 2.0) (integer? 2.5) (exact? 2.0) (number? 'a) (zero? 0.0) (even? 10) (odd? 10))"
        ),
        "(#t #f #f #f #t #t #f)"
    );
    assert_eq!(
        run("(call-with-values (lambda () (floor/ -7 2)) list)"),
        "(-4 1)"
    );
    assert_eq!(run("(let ((r (random 10))) (and (>= r 0) (< r 10)))"), "#t");
}

#[test]
fn lists() {
    assert_eq!(
        run(
            "(list (length '(1 2 3)) (append '(1) '(2) '() '(3 4)) (append) (append '(1) 2) (reverse '(1 2 3)))"
        ),
        "(3 (1 2 3 4) () (1 . 2) (3 2 1))"
    );
    assert_eq!(
        run(
            "(list (list-tail '(1 2 3) 1) (list-ref '(1 2 3) 2) (memq 'c '(a b c d)) (member \"b\" '(\"a\" \"b\")) (assq 'b '((a 1) (b 2))) (assoc 2.0 '((1 a) (2 b)) =))"
        ),
        "((2 3) 3 (c d) (\"b\") (b 2) (2 b))"
    );
    assert_eq!(
        run(
            "(list (cadr '(1 2 3)) (cddr '(1 2 3)) (caar '((1) 2)) (first '(1 2)) (last '(1 2 3)) (list? '(1 . 2)) (pair? '()) (null? '()))"
        ),
        "(2 (3) 1 1 3 #f #f #t)"
    );
    assert_eq!(
        run(
            "(list (filter odd? '(1 2 3 4 5)) (remove odd? '(1 2 3)) (fold + 0 '(1 2 3)) (fold cons '() '(1 2 3)) (fold-right cons '() '(1 2 3)) (reduce max 0 '(3 9 2)))"
        ),
        "((1 3 5) (2) 6 (3 2 1) (1 2 3) 9)"
    );
    assert_eq!(
        run(
            "(list (find even? '(1 2 3)) (find even? '(1 3)) (any even? '(1 2)) (every even? '(2 4)) (every even? '()) (count even? '(1 2 4)) (list-index even? '(1 3 4)))"
        ),
        "(2 #f #t #t #t 2 2)"
    );
    assert_eq!(
        run(
            "(list (iota 3) (iota 3 1) (make-list 2 'x) (delete 2 '(1 2 3 2)) (delete-duplicates '(1 2 1 3 2)) (append-map list '(1 2)) (filter-map (lambda (x) (and (odd? x) (* x x))) '(1 2 3)))"
        ),
        "((0 1 2) (1 2 3) (x x) (1 3) (1 2 3) (1 2) (1 9))"
    );
    assert_eq!(
        run(
            "(list (sort '(3 1 2) <) (sort '(\"b\" \"a\") string<?) (sort (vector 2 1) <) (list-sort > '(1 3 2)))"
        ),
        "((1 2 3) (\"a\" \"b\") #(1 2) (3 2 1))"
    );
    // Sorting keeps the order of items that compare the same.
    assert_eq!(
        run("(sort '((1 . a) (0 . b) (1 . c) (0 . d)) (lambda (x y) (< (car x) (car y))))"),
        "((0 . b) (0 . d) (1 . a) (1 . c))"
    );
    assert_eq!(
        run("(let ((l (list 1 2 3))) (set-car! (cdr l) 'x) (list-set! l 2 'y) l)"),
        "(1 x y)"
    );
    assert_eq!(
        run("(map (lambda (x y z) (+ x y z)) '(1 2) '(10 20) '(100 200 300))"),
        "(111 222)"
    );
    assert_eq!(
        run("(let ((n 0)) (for-each (lambda (a b) (set! n (+ n (* a b)))) '(1 2) '(3 4)) n)"),
        "11"
    );
    assert_eq!(run("(length (map (lambda (x) x) (iota 200000)))"), "200000");
}

#[test]
fn lists_that_do_not_end() {
    assert_eq!(
        run("(list (length '()) (length '(1 2 3)) (reverse '()) (reverse '(1 2 3)))"),
        "(0 3 () (3 2 1))"
    );
    assert_eq!(
        run("(list (list? '(1 2)) (list? '(1 . 2)) (list? 5))"),
        "(#t #f #f)"
    );
    assert_eq!(
        run("(length '(1 2 . 3))"),
        "error: test.scm:1:1: length: expected a list, got (1 2 . 3)"
    );
    assert_eq!(
        run("(reverse '(1 2 . 3))"),
        "error: test.scm:1:1: reverse: expected a list, got (1 2 . 3)"
    );
    let circle = "(define circle (list 1 2 3)) (set-cdr! (cddr circle) circle)";
    assert_eq!(run(&format!("{circle} (list? circle)")), "#f");
    assert!(run(&format!("{circle} (length circle)")).contains("length: expected a list"));
    assert!(run(&format!("{circle} (reverse circle)")).contains("reverse: expected a list"));
}

#[test]
fn strings_and_chars() {
    assert_eq!(
        run(
            "(list (string-length \"åäö\") (string-ref \"åäö\" 1) (substring \"hello\" 1 3) (string-append \"a\" \"b\" \"c\") (string-upcase \"abc\"))"
        ),
        "(3 #\\ä \"el\" \"abc\" \"ABC\")"
    );
    assert_eq!(
        run(
            "(list (string=? \"a\" \"a\" \"a\") (string<? \"a\" \"b\") (string-ci=? \"Ab\" \"aB\") (string-prefix? \"he\" \"hello\") (string-suffix? \"lo\" \"hello\") (string-contains? \"hello\" \"ell\") (string-search \"hello\" \"l\") (string-search \"hello\" \"z\"))"
        ),
        "(#t #t #t #t #t #t 2 #f)"
    );
    assert_eq!(
        run(
            "(list (string-join '(\"a\" \"b\") \", \") (string-split \"a,b,,c\" #\\,) (string-split \"  two  words \") (string-trim \"  x \") (string-replace \"a-b-c\" \"-\" \"+\"))"
        ),
        "(\"a, b\" (\"a\" \"b\" \"\" \"c\") (\"two\" \"words\") \"x\" \"a+b+c\")"
    );
    assert_eq!(
        run(
            "(list (string->list \"abc\") (list->string '(#\\a #\\b)) (string->symbol \"x y\") (symbol->string 'abc) (string #\\a \"b\" 1) (string-pad-left \"5\" 3 #\\0) (string-pad-right \"abcd\" 2))"
        ),
        "((#\\a #\\b #\\c) \"ab\" |x y| \"abc\" \"ab1\" \"005\" \"ab\")"
    );
    assert_eq!(
        run(
            "(list (char-upcase #\\a) (char->integer #\\A) (integer->char 955) (char<? #\\a #\\b #\\c) (char-numeric? #\\5) (char-whitespace? #\\space) (digit-value #\\7))"
        ),
        "(#\\A 65 #\\λ #t #t #t 7)"
    );
    assert_eq!(
        run(
            "(list (string-map char-upcase \"abc\") (string-copy \"hello\" 2) (string-null? \"\") (write->string \"a\\nb\") (->string '(1 \"two\" #\\3)))"
        ),
        "(\"ABC\" \"llo\" #t \"\\\"a\\\\nb\\\"\" \"(1 two 3)\")"
    );
    assert_eq!(
        run("(string-ref \"abc\" 3)"),
        "error: test.scm:1:1: string-ref: there is no char number 3"
    );
    assert_eq!(
        run("(substring \"abc\" 2 1)"),
        "error: test.scm:1:1: substring: the range 2 to 1 is not within the 3 items there are"
    );
}

#[test]
fn vectors_tables_records() {
    assert_eq!(
        run(
            "(let ((v (make-vector 3 0))) (vector-set! v 0 'a) (list v (vector-length v) (vector->list v 1) (vector-map + #(1 2) #(10 20)) (vector-append #(1) #(2 3)) (vector-copy #(1 2 3) 1)))"
        ),
        "(#(a 0 0) 3 (0 0) #(11 22) #(1 2 3) #(2 3))"
    );
    assert_eq!(
        run("(define t (make-hash-table))
             (hash-table-set! t 'a 1) (hash-table-set! t \"key\" 2) (hash-table-set! t 3 'three) (hash-table-set! t 'a 10)
             (hash-table-update!/default t 'n (lambda (n) (+ n 1)) 0)
             (hash-table-delete! t 3)
             (list (hash-table-ref t 'a) (hash-table-ref t \"key\") (hash-table-ref t 'none) (hash-table-ref t 'none 'default)
                   (hash-table-count t) (hash-table-keys t) (hash-table->alist t) (hash-table-contains? t 3))"),
        "(10 2 #f default 3 (a \"key\" n) ((a . 10) (\"key\" . 2) (n . 1)) #f)"
    );
    assert_eq!(
        run("(define-record-type <point> (make-point x y) point? (x point-x set-point-x!) (y point-y))
             (define p (make-point 1 2))
             (set-point-x! p 10)
             (list (point? p) (point? 5) (point-x p) (point-y p) p (record->list p) (record? p))"),
        "(#t #f 10 2 #<point 10 2> (<point> (x . 10) (y . 2)) #t)"
    );
    assert_eq!(
        run(
            "(define-record-type a (make-a v) a? (v a-v)) (define-record-type b (make-b v) b? (v b-v)) (a-v (make-b 1))"
        ),
        "error: test.scm:1:1: a-v: expected a a, got #<b 1>"
    );
    assert_eq!(
        run(
            "(list (equal? '(1 #(2 \"x\")) '(1 #(2 \"x\"))) (eq? '() '()) (equal? \"a\" \"b\") (eqv? \"a\" \"a\") (eq? 'a 'a))"
        ),
        "(#t #t #f #f #t)"
    );
}

#[test]
fn parameters_and_promises() {
    assert_eq!(
        run("(define p (make-parameter 10 (lambda (x) (* x 2))))
             (define (get) (p))
             (list (get) (parameterize ((p 3)) (get)) (guard (e (#t (get))) (parameterize ((p 4)) (error \"x\"))) (get))"),
        "(20 6 20 20)"
    );
    assert_eq!(
        run(
            "(define n 0) (define p (delay (begin (set! n (+ n 1)) n))) (list (force p) (force p) (force 5) (force (make-promise 6)))"
        ),
        "(1 1 5 6)"
    );
    assert_eq!(
        run(
            "(define (stream-from n) (cons n (delay (stream-from (+ n 1)))))
             (define (take s k) (if (= k 0) '() (cons (car s) (take (force (cdr s)) (- k 1)))))
             (take (stream-from 5) 3)"
        ),
        "(5 6 7)"
    );
    // A long chain of delay-force does not grow the stack.
    assert_eq!(
        run(
            "(define (loop n) (if (= n 0) (delay 'done) (delay-force (loop (- n 1))))) (force (loop 100000))"
        ),
        "done"
    );
}

#[test]
fn output_is_collected() {
    let mut vm = Vm::new();
    run_in(
        &mut vm,
        "(display \"a\") (write \"b\") (newline) (display '(1 #\\c \"s\")) (write-string \"!\")",
    );
    assert_eq!(vm.core.output, "a\"b\"\n(1 c s)!");
    assert_eq!(
        run_in(
            &mut vm,
            "(with-output-to-string (lambda () (display 'inner) (write 1.5)))"
        ),
        "\"inner1.5\""
    );
    assert_eq!(vm.core.output, "a\"b\"\n(1 c s)!");
}

#[test]
fn eval_runs_data_as_code() {
    assert_eq!(run("(eval '(+ 1 2))"), "3");
    assert_eq!(run("(eval (list 'define 'made-up 5)) made-up"), "5");
    assert_eq!(
        run("(eval (read-from-string \"(map (lambda (x) (* x x)) '(1 2 3))\"))"),
        "(1 4 9)"
    );
    assert_eq!(run("(read-from-string \"\")"), "#<eof>");
    assert_eq!(
        run(
            "(eval-string \"(define-syntax twice (syntax-rules () ((_ e) (* 2 e)))) (define (f x) (twice x)) (f 4)\")"
        ),
        "8"
    );
    assert_eq!(run("(eval-string \"\")"), "#<void>");
    assert_eq!(
        run("(eval-string \"(define ok 1)\\n(car ok)\" \"lib.scm\")"),
        "error: lib.scm:2:1: car: expected a pair, got 1"
    );
    assert_eq!(
        run("(eval-string \"(define (g) 1\" \"lib.scm\")"),
        "error: lib.scm:1:1: this bracket is never closed"
    );
    assert_eq!(
        run("(guard (e (#t (error-object-message e))) (read-from-string \"(1\"))"),
        "\"read-from-string: this bracket is never closed\""
    );
}

#[test]
fn deep_nesting_is_handled() {
    // Code nested as deep as anyone writes it, and deeper.
    let depth = 900;
    let nested = format!("{}1{}", "(list ".repeat(depth), ")".repeat(depth));
    let source =
        format!("(define (depth x) (if (pair? x) (+ 1 (depth (car x))) 0)) (depth {nested})");
    assert_eq!(run(&source), depth.to_string());
    let too_deep = format!("{}1{}", "(".repeat(50_000), ")".repeat(50_000));
    assert_eq!(
        run(&too_deep),
        "error: test.scm:1:1002: brackets are nested too deeply"
    );
    let by_macro =
        "(define-syntax nest (syntax-rules () ((_) 0) ((_ x more ...) (+ 1 (nest more ...)))))";
    let many = "x ".repeat(5000);
    assert_eq!(
        run(&format!("{by_macro} (nest {many})")),
        "error: test.scm:1:87: this code is nested too deeply"
    );
    // Data has no limit: it is built, compared and written without one.
    let build =
        "(define (nest n) (let loop ((n n) (x '())) (if (= n 0) x (loop (- n 1) (list x)))))";
    assert_eq!(
        run(&format!("{build} (equal? (nest 200000) (nest 200000))")),
        "#t"
    );
    assert_eq!(
        run(&format!(
            "{build} (string-length (write->string (nest 30000)))"
        )),
        "60002"
    );
    assert_eq!(
        run(&format!("{build} (eval (list 'quote (nest 200000)))")),
        "error: test.scm:1:85: eval: this code is nested too deeply"
    );
}

#[test]
fn lists_turned_around_in_place() {
    assert_eq!(
        run("(list (reverse! '()) (reverse! (list 1)) (reverse! (list 1 2 3)))"),
        "(() (1) (3 2 1))"
    );
    assert_eq!(
        run(
            "(list (append-reverse! (list 3 2 1) '(4 5)) (append-reverse! '() '(1)) (append-reverse! (list 1) 2))"
        ),
        "((1 2 3 4 5) (1) (1 . 2))"
    );
    // The result is made of the pairs of the list.
    assert_eq!(
        run(
            "(define items (list 1 2 3)) (define turned (reverse! items)) (list (eq? items (cddr turned)) items)"
        ),
        "(#t (1))"
    );
    // What sort and its like are given is left as it was.
    assert_eq!(
        run("(define items (list 3 1 2 5 4))
             (list (sort items <) (map - items) (filter odd? items) (delete-duplicates items) items)"),
        "((1 2 3 4 5) (-3 -1 -2 -5 -4) (3 1 5) (3 1 2 5 4) (3 1 2 5 4))"
    );
    assert_eq!(
        run("(call-with-values (lambda () (partition even? '(1 2 3 4 5))) list)"),
        "((2 4) (1 3 5))"
    );
}

#[test]
fn strings_as_keys_of_a_hash_table() {
    assert_eq!(
        run("(define t (make-hash-table))
             (define name (string-append \"a\" \"b\"))
             (hash-table-set! t name 1)
             (hash-table-set! t \"ab\" 2)
             (hash-table-set! t 'ab 3)
             (hash-table-set! t \"c\" 4)
             (define before (list (hash-table-count t) (hash-table-ref t (string #\\a #\\b))
                                  (hash-table-ref t 'ab) (hash-table-contains? t \"c\")))
             (hash-table-delete! t (string-append \"a\" \"b\"))
             (hash-table-delete! t \"not there\")
             (list before (hash-table-keys t) (hash-table-ref t \"ab\" 'none))"),
        "((3 2 3 #t) (ab \"c\") none)"
    );
}
