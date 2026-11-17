;;; Entering Scheme from the editor, and waiting for the editor.
;;;
;;; The editor never blocks inside Scheme. A procedure that needs something
;;; from the user, such as a line of input, gets a promise from the editor and
;;; calls `await` on it. `await` saves the rest of the computation and returns
;;; control to the editor. When the promise is resolved, the editor calls back
;;; and the computation continues from where it left off.

;; The escape back to the editor for the call that is running, or #f when
;; Scheme was not entered through `%enter` (as when a file is being loaded).
(define %top #f)

;; Computations waiting for a promise, by promise id.
(define %waiters (table))

;; The editor calls every queued procedure through here.
(define (%enter f args)
  (let ((result (call/cc (lambda (k) (set! %top k) (apply f args)))))
    (set! %top #f)
    result))

;; Suspends the running command until PROMISE is resolved and returns the
;; promise's value. If the promise is rejected, as when the user quits a
;; prompt, the command is abandoned.
(define (await promise)
  (if %top
      (call/cc
       (lambda (k)
         (set! %waiters (table-set %waiters promise k))
         (%top '%suspended)))
      (error "await: nothing can wait here; use it inside a command or a hook")))

(define (%promise-resolve id value)
  (let ((k (table-ref %waiters id)))
    (when k
      (set! %waiters (table-remove %waiters id))
      (k value))))

(define (%promise-reject id)
  (set! %waiters (table-remove %waiters id)))

;; How many computations are waiting for a promise.
(define (%waiting-count) (hash-length %waiters))
