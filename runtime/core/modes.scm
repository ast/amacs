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
(set-documentation! 'define-major-mode
  "(define-major-mode name parent \"Pretty name\" \"Documentation.\" body ...)
Defines a major mode and the command that selects it. The parent is another
mode, or #f. The body runs each time the mode is selected, before the hooks
of the mode and its ancestors.")

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

(define (enable-mode! mode)
  "Turns a minor mode on without saying so, for hooks and init files. A
mode defined as global is turned on everywhere."
  (%switch-mode! mode #t))

(define (disable-mode! mode)
  "Turns a minor mode off without saying so."
  (%switch-mode! mode #f))

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
(set-documentation! 'define-minor-mode
  "(define-minor-mode name \"Pretty name\" \"Documentation.\")
Defines a minor mode and the command that toggles it in the current buffer.
The mode's hook runs on every toggle; use `minor-mode-enabled?` in it.")

(set-documentation! 'define-global-minor-mode
  "(define-global-minor-mode name \"Pretty name\" \"Documentation.\")
Like `define-minor-mode`, for a mode that is on in every buffer at once.")

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

;;; What a library knows of each mode
;;;
;;; A library that serves several modes, such as the one for links, keeps a
;;; hash table from the name of a mode to what that mode told it, often a
;;; list of keys and values.

(define (mode-lookup table)
  "The entry of a hash table for the major mode of the current buffer, or
for the nearest of the modes that it is derived from that has one; #f if
none has. The keys of the table are names of modes."
  (let loop ((mode (major-mode)))
    (and mode
         (or (hash-table-ref table mode)
             (loop (%mode-parent mode))))))

(define (property-ref properties key . default)
  "The value after a key in a list of keys and values, such as
'(marker \"#\" max-level 6); the default, or else #f, if the key is not
there."
  (cond [(or (null? properties) (null? (cdr properties)))
         (and (pair? default) (car default))]
        [(eq? (car properties) key) (cadr properties)]
        [else (apply property-ref (cddr properties) key default)]))

;;; Choosing a major mode from the file name

;; Pairs of (suffix . mode), newest first.
(define %auto-modes '())

(define (add-auto-mode! suffix mode)
  "Files whose names end in the suffix open in the mode. A suffix can be an
extension such as \".rs\" or a whole name such as \"/Makefile\"."
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

(define-major-mode special-mode #f "Special"
  "Parent of the major modes of buffers that are not typed into, such as
lists that the editor makes, where a key alone runs a command. ? and h
show what the keys of the mode do.")

(define-key 'special-mode "?" 'describe-mode)
(define-key 'special-mode "h" 'describe-mode)

(add-auto-mode! ".txt" 'text-mode)

;;; Built-in minor modes. Their behaviour is part of the editor; these
;;; define the commands that switch them.

(define-minor-mode paredit-mode "Paredit"
  "Keep brackets balanced while editing Lisp code.")

(define-global-minor-mode auto-pair-mode ""
  "Insert brackets and quotes in pairs.")

(define-minor-mode hl-line-mode ""
  "Mark the line that point is on.")

(define-minor-mode symbol-highlight-mode ""
  "Mark the other occurrences of the symbol at point.")

(define-minor-mode hl-todo-mode ""
  "Make words such as TODO and FIXME stand out in comments.")

(define-minor-mode whitespace-mode ""
  "Mark trailing whitespace and text beyond `whitespace-line-column`.")

(define-minor-mode hide-markup-mode ""
  "Leave markup out of what is shown, so that text reads as it is meant:
the marks around emphasis, as the stars of *strong*, and of a link all but
its text. The line that point is on is shown as it is written, markup and
all, so that what is edited is seen. Only how the text is shown changes,
not the text. For Org: turn it on in a hook, as in
(add-hook! 'org-mode-hook (lambda () (enable-mode! 'hide-markup-mode))).")

(define-minor-mode visual-line-mode ""
  "Break lines that are too long for the window between words, where they
are otherwise broken at the edge of the window with a mark. Only how the
text is shown changes, not the text. For prose: turn it on in a hook, as
in (add-hook! 'markdown-mode-hook (lambda () (enable-mode! 'visual-line-mode))).")
