;;; Hooks: named lists of procedures that run when something happens.
;;;
;;; The editor runs these hooks itself:
;;;   find-file-hook     after a file is first visited, in its buffer
;;;   after-save-hook    after a buffer is written to its file
;;;   post-command-hook  after every command
;;; Every mode also has a hook named after it, such as `rust-mode-hook`.

(define %hooks (make-hash-table))

(define (hook-functions name)
  "The procedures on a hook, in the order they run."
  (hash-table-ref %hooks name '()))

(define (%set-hook-functions! name functions)
  (hash-table-set! %hooks name functions)
  (%hook-active! name (not (null? functions))))

(define (add-hook! name f)
  "Adds a procedure to the end of a hook, unless it is already there."
  (unless (member f (hook-functions name))
    (%set-hook-functions! name (append (hook-functions name) (list f)))))

(define (remove-hook! name f)
  "Takes a procedure off a hook."
  (%set-hook-functions! name (filter (lambda (g) (not (equal? f g))) (hook-functions name))))

(define (run-hooks name . args)
  "Calls every procedure on a hook with the arguments."
  (for-each (lambda (f) (apply f args)) (hook-functions name)))

;; The editor's entry point.
(define (%run-hook name . args)
  (apply run-hooks name args))
