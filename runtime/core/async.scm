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

;;; Prompts

(define (message . parts)
  (%message (apply string-append (map value->string parts))))

;; Reads a line in the minibuffer. The optional argument is the initial input.
(define (read-string prompt . initial)
  (await (%read-string prompt (if (null? initial) "" (car initial)) 'none)))

;; Reads a file name, with completion, starting in the working directory.
(define (read-file-name prompt)
  (await (%read-string prompt (string-append (working-directory) "/") 'file)))

;; Reads a buffer name, with completion.
(define (read-buffer-name prompt)
  (await (%read-string prompt "" 'buffer)))

;; Lets the user choose one of CANDIDATES, narrowing them by typing, and
;; returns the choice as a string. A candidate is a string, or a list of a
;; string and an annotation shown beside it. If nothing matches, the text
;; typed is returned. The optional category names the kind of thing, which
;; keeps a separate input history for it.
(define (completing-read prompt candidates . category)
  (await (%completing-read prompt candidates (if (null? category) 'choice (car category)))))

;; Asks a yes-or-no question and returns a boolean.
(define (y-or-n? prompt)
  (await (%y-or-n prompt)))
