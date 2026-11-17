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
       (define (name) doc body ...)
       (%define-command 'name doc name))]))

;; Runs a command by name, whether it is built in or defined in Scheme.
(define (call-command name)
  (let ((f (%script-command name)))
    (if f (f) (%call-native-command name))))

;; Key sequences are strings in Emacs notation: "C-x C-f", "M-<left>", "H-c".
(define (global-set-key keys command) (%bind-key 'global keys command))
(define (global-unset-key keys) (%unbind-key 'global keys))

;; MAP is the name of a mode, or 'global or 'minibuffer.
(define (define-key map keys command) (%bind-key map keys command))
(define (undefine-key map keys) (%unbind-key map keys))
