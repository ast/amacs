;;; A small test framework for Scheme code that runs inside the editor.
;;;
;;;   (deftest insert-moves-point
;;;     (insert "abc")
;;;     (check-equal? (point) 3))
;;;
;;; Each test starts in a new, empty buffer. Run the tests
;;; with `amacs --batch --test FILE...`.

;; Pairs of (name . thunk), newest first.
(define %tests '())

(define-syntax deftest
  (syntax-rules ()
    [(_ name body ...)
     (set! %tests (cons (cons 'name (lambda () body ...)) %tests))]))

(set-documentation! 'deftest
  "(deftest name body ...)
Defines a test, which `run-tests` runs in a new, empty buffer.")

(define (check-equal? actual expected)
  "Fails the running test unless the two values are equal?."
  (unless (equal? actual expected)
    (error (string-append "check-equal? failed: got " (write->string actual)
                          " but expected " (write->string expected)))))

(define (check-true value)
  "Fails the running test if the value is #f."
  (unless value (error "check-true failed")))

(define (check-false value)
  "Fails the running test unless the value is #f."
  (when value (error "check-false failed: got" value)))

(define (check-error thunk)
  "Fails the running test unless calling thunk raises an error."
  (unless (call-with-error-handler (lambda (message) #t) (lambda () (thunk) #f))
    (error "check-error failed: no error was raised")))

(define (%kill-test-buffer)
  (when (member "*test*" (buffer-list))
    (kill-buffer "*test*")))

;; Runs one test. Returns #f if it passed, else the failure message.
(define (%run-test test)
  (%kill-test-buffer)
  (switch-to-buffer "*test*")
  (call-with-error-handler
   (lambda (message) message)
   (lambda () ((cdr test)) #f)))

(define (run-tests)
  "Runs every test defined so far, in definition order. Returns a list
whose first element is the number of tests that passed and whose other
elements are (name message) for each failure."
  (let loop ((tests (reverse %tests)) (passed 0) (failures '()))
    (if (null? tests)
        (begin
          (set! %tests '())
          (%kill-test-buffer)
          (cons passed (reverse failures)))
        (let ((failure (%run-test (car tests))))
          (if failure
              (loop (cdr tests) passed (cons (list (car (car tests)) failure) failures))
              (loop (cdr tests) (+ passed 1) failures))))))
