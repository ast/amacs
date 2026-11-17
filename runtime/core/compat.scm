;;; Small things that the rest of the runtime is written with.

(define (value->string value)
  "Any value as `display` shows it, as a string."
  (->string value))

(define (load path)
  "Reads a file of Scheme and runs everything in it. Returns the last value."
  (eval-string (read-file path) path))

(define (call-with-error-handler handler thunk)
  "Calls thunk. If that raises an error, returns what handler returns for
the error's message, a string."
  (guard (raised (#t (handler (if (error-object? raised)
                                  (error-object->string raised)
                                  (->string raised)))))
    (thunk)))
