;;; JSON helpers.
;;;
;;; `json-decode` and `json-encode` are native (the Scheme crate's builtins).
;;; The data they use: an object is an association list of (symbol . value)
;;; pairs, so `{}` is '(); an array is a vector, so `[]` is #(); `null` is
;;; the symbol 'null. These helpers read and build that data.

(define (json-ref object key . default)
  "The value of a key in a decoded JSON object, or the default (#f unless
given) when the key, or the object itself, is absent. A missing object
reads as missing, so `(json-ref (json-ref m 'a) 'b)` is safe when `a` is
absent."
  (let ((pair (and (pair? object) (assq key object))))
    (cond [pair (cdr pair)]
          [(pair? default) (car default)]
          [else #f])))

(define (json-object . pairs)
  "A JSON object from alternating keys and values: (json-object 'a 1 'b 2)."
  (let loop ((rest pairs) (acc '()))
    (cond [(null? rest) (reverse acc)]
          [(null? (cdr rest)) (error "json-object: odd number of arguments")]
          [else (loop (cddr rest) (cons (cons (car rest) (cadr rest)) acc))])))

(define (json-array . values)
  "A JSON array from its values: (json-array 1 2 3)."
  (list->vector values))
