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
       (%define-command 'name doc name))]))

(set-documentation! 'define-command
  "(define-command (name) \"Documentation.\" body ...)
Defines a command, which M-x offers and keys can be bound to, and a
procedure of the same name.")

(define (call-command name)
  "Runs a command by name, whether it is built in or defined in Scheme."
  (let ((f (%script-command name)))
    (if f (f) (%call-native-command name))))

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
