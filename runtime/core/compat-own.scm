;;; Names that hide differences between Scheme implementations: the
;;; version for amacs's own Scheme.
;;;
;;; The rest of the runtime uses only standard Scheme plus the names defined
;;; here and in async-own.scm.

;; The display form of any value, as a string.
(define (value->string value) (->string value))

;; Tables keyed by symbols or numbers. Setting a key changes the table and
;; returns it.
(define (table) (make-hash-table))
(define (table-ref table key) (hash-table-ref table key))
(define (table-set table key value) (hash-table-set! table key value) table)
(define (table-remove table key) (hash-table-delete! table key) table)

;; Runs THUNK. If it raises an error, returns (HANDLER message) instead,
;; where message is a string.
(define (call-with-error-handler handler thunk)
  (guard (raised (#t (handler (if (error-object? raised)
                                  (error-object->string raised)
                                  (->string raised)))))
    (thunk)))
