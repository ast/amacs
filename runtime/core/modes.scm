;;; Major and minor modes.
;;;
;;; A mode has a keymap of the same name, so `(define-key 'rust-mode ...)`
;;; binds a key in buffers that are in rust-mode or a mode derived from it.

(define (%mode-hook-name mode)
  (string->symbol (string-append (symbol->string mode) "-hook")))

;; Runs the hooks of MODE's ancestors, oldest first, and then its own.
(define (%run-mode-hooks mode)
  (let ((parent (%mode-parent mode)))
    (when parent (%run-mode-hooks parent))
    (run-hooks (%mode-hook-name mode))))

;; Defines a major mode and the command that selects it:
;;
;;   (define-major-mode rust-mode prog-mode "Rust"
;;     "Major mode for Rust source."
;;     (set-local! 'tab-width 4))
;;
;; The parent is another mode, or #f. The body runs each time the mode is
;; selected, before the mode hooks.
(define-syntax define-major-mode
  (syntax-rules ()
    [(_ name parent pretty doc body ...)
     (begin
       (%define-mode 'name 'parent pretty)
       (define-command (name)
         doc
         (%set-major-mode! 'name)
         body ...
         (%run-mode-hooks 'name)))]))

;; Turns a minor mode on or off without saying so, for use in hooks and
;; init files. A mode defined as global is switched everywhere.
(define %global-modes '())

(define (%switch-mode! mode on)
  (unless (equal? on (minor-mode-enabled? mode))
    (%set-minor-mode! mode (if (member mode %global-modes) #t #f) on)
    (run-hooks (%mode-hook-name mode))))

(define (enable-mode! mode) (%switch-mode! mode #t))
(define (disable-mode! mode) (%switch-mode! mode #f))

(define (%toggle-minor-mode name pretty global?)
  (let ((on (not (minor-mode-enabled? name))))
    (%set-minor-mode! name global? on)
    (run-hooks (%mode-hook-name name))
    (message pretty (if on " mode enabled" " mode disabled"))))

;; Defines a minor mode and the command that toggles it in the current buffer:
;;
;;   (define-minor-mode ruler-mode "Ruler" "Show a ruler.")
;;
;; The mode's hook runs on every toggle; use `minor-mode-enabled?` in it.
(define-syntax define-minor-mode
  (syntax-rules ()
    [(_ name pretty doc)
     (begin
       (%define-mode 'name #f pretty)
       (define-command (name) doc (%toggle-minor-mode 'name pretty #f)))]))

;; Like `define-minor-mode`, for a mode that is on in every buffer at once.
(define-syntax define-global-minor-mode
  (syntax-rules ()
    [(_ name pretty doc)
     (begin
       (%define-mode 'name #f pretty)
       (set! %global-modes (cons 'name %global-modes))
       (define-command (name) doc (%toggle-minor-mode 'name pretty #t)))]))

;;; Choosing a major mode from the file name

;; Pairs of (suffix . mode), newest first.
(define %auto-modes '())

;; Files whose names end in SUFFIX open in MODE. A suffix can be an extension
;; such as ".rs" or a whole name such as "/Makefile".
(define (add-auto-mode! suffix mode)
  (set! %auto-modes (cons (cons suffix mode) %auto-modes)))

(define (%auto-mode-for file)
  (let loop ((entries %auto-modes))
    (cond
      [(null? entries) #f]
      [(string-suffix? (car (car entries)) file) (cdr (car entries))]
      [else (loop (cdr entries))])))

(add-hook! 'find-file-hook
           (lambda ()
             (let* ((file (buffer-file-name))
                    (mode (and file (%auto-mode-for file))))
               (when mode (call-command mode)))))

;;; Built-in modes

(define-command (fundamental-mode)
  "Major mode with no special behaviour."
  (%set-major-mode! 'fundamental-mode)
  (%run-mode-hooks 'fundamental-mode))

(define-major-mode text-mode #f "Text"
  "Major mode for text written for people to read.")

(define-major-mode prog-mode #f "Prog"
  "Parent of the major modes for programming languages.")

(define-major-mode conf-mode #f "Conf"
  "Parent of the major modes for configuration files.")

(add-auto-mode! ".txt" 'text-mode)

;;; Built-in minor modes. Their behaviour is part of the editor; these
;;; define the commands that switch them.

(define-minor-mode paredit-mode "Paredit"
  "Keep brackets balanced while editing Lisp code.")

(define-global-minor-mode auto-pair-mode ""
  "Insert brackets and quotes in pairs.")
