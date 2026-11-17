;;; Waiting for the editor: the version for amacs's own Scheme.
;;;
;;; The editor never blocks inside Scheme. A procedure that needs something
;;; from the user, such as a line of input, gets a promise from the editor and
;;; calls `await` on it. That suspends the running code and returns control
;;; to the editor, which continues the code when the promise is resolved.

;; Suspends the running code until PROMISE is resolved and returns the
;; promise's value. If the promise is rejected, as when the user quits a
;; prompt, the code is left the way an error leaves it, except that
;; handlers for errors do not see it: what `dynamic-wind` and
;; `unwind-protect` promise to do on the way out is done.
(define (await promise)
  (unless (%can-wait?)
    (error "await: nothing can wait here; use it inside a command or a hook"))
  (let ((result (%suspend promise)))
    (if (%escape? result)
        (raise result)
        result)))
