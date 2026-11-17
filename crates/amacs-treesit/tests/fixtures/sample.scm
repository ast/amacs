;;; A sample.

(define limit 10)

(define (square x)
  "Return X squared."
  (* x x))

(define-command (insert-date)
  "Insert today's date."
  (let ((now (current-time))
        (fmt "%Y-%m-%d"))
    (when (> limit 0)
      (insert (format-time fmt now) #\space))
    (for-each (lambda (n)
                (display n))
              '(1 2 3))))

(cond
  [(null? xs) #f]
  [else 'done])
