;;; Commands and key bindings.

;; Defines a command and a procedure of the same name:
;;
;;   (define-command (insert-date)
;;     "Insert today's date."
;;     (insert (format-current-time "%Y-%m-%d")))
;;
;; The documentation string is required.
(define-syntax define-command
  (syntax-rules ()
    [(_ (name) doc body ...)
     (begin
       (define (name) body ...)
       (set-documentation! 'name doc)
       (%install-command! 'name doc name))]))

(set-documentation! 'define-command
  "(define-command (name) \"Documentation.\" body ...)
Defines a command, which M-x offers and keys can be bound to, and a
procedure of the same name. A command of the same name that is built in
is replaced; (call-built-in-command 'name) still runs it.")

(define (call-command name)
  "Runs a command by name, whether it is built in or defined in Scheme,
with the advice it has been given."
  (let ((f (%script-command name)))
    (if f (f) (%call-native-command name))))

(define (call-built-in-command name)
  "Runs the built-in command of a name, also when a command defined in
Scheme has taken its place."
  (%call-native-command name))

;; The editor runs commands through here when the hook has functions.
(define (%run-command name)
  (run-hooks 'pre-command-hook)
  (call-command name))

;;; Advice: procedures that run before, after or around a command, whoever
;;; defined it and however it is run.
;;;
;;;   (add-advice! 'save-buffer 'before 'tidy
;;;     (lambda () (delete-trailing-whitespace)))
;;;   (add-advice! 'kill-buffer 'around 'ask
;;;     (lambda (next) (when (y-or-n? "Really? ") (next))))
;;;   (remove-advice! 'save-buffer 'tidy)

;; What commands defined in Scheme do, before advice, by name.
(define %command-bodies (make-hash-table))

;; The advice of commands, by name: lists of (id kind procedure), the
;; innermost first.
(define %command-advice (make-hash-table))

(define (%install-command! name doc procedure)
  (hash-table-set! %command-bodies name procedure)
  (%rebuild-command! name doc))

(define (%advise kind advice inner)
  (case kind
    [(before) (lambda () (advice) (inner))]
    [(after) (lambda () (let ((result (inner))) (advice) result))]
    [(around) (lambda () (advice inner))]
    [else (error "advice is one of 'before, 'after and 'around, not" kind)]))

;; Makes the command what its body and its advice come to.
(define (%rebuild-command! name doc)
  (let* ((body (hash-table-ref %command-bodies name))
         (advice (hash-table-ref %command-advice name '()))
         (base (or body (lambda () (%call-native-command name)))))
    (if (and (null? advice) (not body))
        ;; Nothing of ours is left on a built-in command.
        (%restore-native-command name)
        (%define-command
         name doc
         (fold (lambda (entry inner) (%advise (cadr entry) (caddr entry) inner))
               base advice)))))

(define (add-advice! name kind id procedure)
  "Makes a procedure run with a command: 'before or 'after it, with no
arguments, or 'around it, with a procedure that runs the command as its
argument. The id, a symbol, names the advice: adding advice under an id
that is in use replaces it. Later advice goes outside earlier."
  (unless (command? name)
    (error "there is no command named" name))
  (%advise kind procedure (lambda () #f))
  (hash-table-set!
   %command-advice name
   (append (remove (lambda (entry) (eq? (car entry) id))
                   (hash-table-ref %command-advice name '()))
           (list (list id kind procedure))))
  (%rebuild-command! name (or (command-documentation name) "")))

(define (remove-advice! name id)
  "Takes away the advice that was added to a command under an id."
  (hash-table-set!
   %command-advice name
   (remove (lambda (entry) (eq? (car entry) id))
           (hash-table-ref %command-advice name '())))
  (%rebuild-command! name (or (command-documentation name) "")))

(define (command-advice name)
  "The advice of a command: a list of (id kind procedure) lists, the
innermost first."
  (hash-table-ref %command-advice name '()))

(define (global-set-key keys command)
  "Binds a key sequence to a command, a symbol, in every buffer. Key
sequences are strings in Emacs notation: \"C-x C-f\", \"M-<left>\", \"H-c\"."
  (%bind-key 'global keys command))

(define (global-unset-key keys)
  "Removes the global binding of a key sequence."
  (%unbind-key 'global keys))

(define (define-key map keys command)
  "Binds a key sequence to a command in a keymap: the name of a mode, or
'global or 'minibuffer."
  (%bind-key map keys command))

(define (undefine-key map keys)
  "Removes the binding of a key sequence in a keymap."
  (%unbind-key map keys))
