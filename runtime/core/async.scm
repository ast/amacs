;;; Waiting for the editor.
;;;
;;; The editor never blocks inside Scheme. A procedure that needs something
;;; from the user, such as a line of input, gets a promise from the editor and
;;; calls `await` on it. That suspends the running code and returns control
;;; to the editor, which continues the code when the promise is resolved.

(define (await promise)
  "Suspends the running code until the promise is resolved and returns the
promise's value. If the promise is rejected, as when the user quits a
prompt, the code is left the way an error leaves it, except that handlers
for errors do not see it: what `dynamic-wind` and `unwind-protect` promise
to do on the way out is done."
  (let ((result (%suspend promise)))
    (if (%escape? result)
        (raise result)
        result)))

;; What `run-later` has been given and not yet run, oldest first.
(define %later '())

(define (%run-later!)
  (unless (null? %later)
    (let ((thunk (car %later)))
      (set! %later (cdr %later))
      (thunk))))

(define (run-after milliseconds thunk)
  "Runs a procedure of no arguments after a number of milliseconds, while
the editor goes on working. Returns a timer, which (cancel-timer! timer)
stops if the time has not come yet."
  (let ((timer (vector #t)))
    (run-later
     (lambda ()
       (await (%timer milliseconds))
       (when (vector-ref timer 0)
         (vector-set! timer 0 #f)
         (thunk))))
    timer))

(define (cancel-timer! timer)
  "Stops a timer that run-after made, so that its procedure is not run.
Does nothing if it has run already."
  (vector-set! timer 0 #f))

(define (run-later thunk)
  "Runs a procedure of no arguments once the running code has returned to
the editor: after the command, hook or evaluation that called this is
done, and outside any buffer that with-current-buffer made current for
it. A hook that must wait for a program uses this, so that the buffer it
runs in is not left on screen while it waits."
  (set! %later (append %later (list thunk)))
  (%queue-eval "(%run-later!)"))
