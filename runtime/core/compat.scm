;;; Names that hide differences between Scheme implementations.
;;;
;;; The rest of the runtime uses only standard Scheme plus the names defined
;;; here, so that changing the engine means rewriting this file, not the others.

(define (string-suffix? suffix s) (ends-with? s suffix))
(define (string-prefix? prefix s) (starts-with? s prefix))

;; The display form of any value, as a string.
(define (value->string value)
  (if (string? value) value (to-string value)))

;; The current local time formatted with strftime directives.
(define (format-current-time format) (local-time/now! format))

;; Immutable tables keyed by symbols or numbers.
(define (table) (hash))
(define (table-ref table key) (hash-try-get table key))
(define (table-set table key value) (hash-insert table key value))
(define (table-remove table key) (hash-remove table key))

;; Runs THUNK. If it raises an error, returns (HANDLER message) instead,
;; where message is a string.
(define (call-with-error-handler handler thunk)
  (with-handler (lambda (err) (handler (value->string err))) (thunk)))
