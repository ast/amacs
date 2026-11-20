;;; A terminal in a buffer.
;;;
;;; `term` (C-c v) starts a shell on a pseudo-terminal and shows its screen
;;; in a buffer, as eat and vterm do in Emacs. The buffer is an ordinary
;;; one: the lines that scroll off the top stay as text to search and copy,
;;; the live screen is its last rows, and colours are spans. The keys that
;;; the shell and the programs in it see are decided here; the screen
;;; itself is kept by the editor's core (`crates/amacs-core/src/terminal.rs`
;;; on `crates/amacs-term`).
;;;
;;; Keys go to the program, `C-c` among them, except those in
;;; `term-emacs-keys`, which keep their meaning in the editor, so that
;;; `C-x`, `M-x` and `M-o` work as everywhere: eat calls this semi-char
;;; mode. The escape key, `term-escape-key` (C-q, as the prefix of tmux),
;;; leads to the terminal's own commands and sends a reserved key: `C-q C-x`
;;; sends `C-x`, `C-q C-q` sends `C-q`. Point follows the program's cursor
;;; until it is moved away, and follows again with the next key sent or
;;; `C-q C-l`.

(define-option 'term-shell #f
  "The program that `term` runs: a list of the program and its arguments,
or #f for the SHELL of the environment, or sh.")

(define-option 'term-escape-key "C-q"
  "The key that leads to the terminal's own commands in a terminal buffer,
as the prefix key of tmux: C-q C-k toggles char mode, C-q C-l brings point
back to the cursor. After it, a key that the editor keeps for itself goes
to the program instead: C-q C-x sends C-x, and C-q C-q sends the escape
key itself. Set it in init.scm; terminals started afterwards use it.")

(define-option 'term-emacs-keys
  '("C-x" "M-x" "M-:" "M-o" "C-u" "M-<" "M->" "C-v" "M-v"
    "S-<left>" "S-<right>" "S-<up>" "S-<down>")
  "Keys that keep their meaning in the editor in a terminal buffer, where
every other key goes to the program, `C-c` included. Each is sent to the
program by pressing it after the `term-escape-key`. Set it in init.scm;
terminals started afterwards use it.")

(define-option 'term-scrollback-lines 10000
  "How many lines that scrolled off the top a terminal buffer keeps.")

(set-face! 'term 'fg #f 'bg #f)
(set-documentation! 'term
  "The default colours of a terminal buffer: the background and foreground
of its empty cells and of text the program did not colour. Set its `fg`
and `bg` to give terminals a colour scheme of their own, apart from the
editor's theme. The sixteen colours the programs use are the `fg-term-*`
palette entries.")

(define-major-mode term-mode #f "Term"
  "The screen of a program on a pseudo-terminal. Keys go to the program,
`C-c` among them, except those of the `term-emacs-keys` option and the
`term-escape-key` (C-q), which leads to the terminal's commands and sends
a reserved key: `C-q C-x` sends `C-x`. `C-q C-l` brings point back to the
cursor, `C-q C-k` toggles char mode, `C-q C-y` sends the last kill."
  (%term-bind-keys!)
  (set-buffer-read-only! #t)
  (set-buffer-default-face! 'term))

(define-minor-mode term-char-mode "Char"
  "Char mode: every key goes to the program, including C-x, M-x and the
other keys that semi-char mode keeps for the editor; only the
`term-escape-key` (C-q) is the editor's, and C-q C-k returns to semi-char
mode. For a program that needs the editor's keys, such as a shell with its
own C-x, a full-screen editor, or tmux.")

;;; Which keys go to the program

(define (%term-all-keys)
  "The keys that term-mode binds to `term-send-key` when nothing keeps them
for the editor: those that the editor binds somewhere, so that the default
binding would not catch them."
  (let ((keys '()))
    (define (add! key)
      (set! keys (cons key keys)))
    (define (add-with-modifiers! name)
      (for-each (lambda (prefix) (add! (string-append prefix name)))
                '("" "S-" "C-" "M-" "C-S-" "C-M-" "M-S-")))
    (string-for-each
     (lambda (c)
       (let ((name (string c)))
         (add! (string-append "C-" name))
         (add! (string-append "M-" name))
         (add! (string-append "C-M-" name))))
     "abcdefghijklmnopqrstuvwxyz0123456789-=[]\\;',./`")
    (add! "C-SPC")
    (add! "M-SPC")
    (add! "C-@")
    (add! "C-_")
    (add! "C-?")
    (string-for-each
     (lambda (c) (add! (string-append "M-" (string c))))
     "!\"#$%&()*+:<>?@^_{|}~")
    (for-each add-with-modifiers!
              '("RET" "TAB" "DEL" "ESC" "<up>" "<down>" "<left>" "<right>"
                "<home>" "<end>" "<prior>" "<next>" "<delete>" "<insert>"
                "<f1>" "<f2>" "<f3>" "<f4>" "<f5>" "<f6>" "<f7>" "<f8>"
                "<f9>" "<f10>" "<f11>" "<f12>"))
    (reverse keys)))

(define (%term-reserved-keys)
  ;; The keys that are the editor's in semi-char mode: the escape key and
  ;; the keys of `term-emacs-keys`, with the escape key first.
  (let ((escape (option 'term-escape-key)))
    (cons escape (filter (lambda (key) (not (equal? key escape)))
                         (option 'term-emacs-keys)))))

;; The commands after the escape key, as (key . command), the last defined
;; first. `term-define-key` adds to it.
(define %term-escape-commands '())

;; The reserved keys that the keymaps were last built for, so that a change
;; of the options through init.scm or `set-option!` is seen by the next
;; terminal.
(define %term-bound-keys #f)

(define (%term-escape-sequence key)
  ;; The key after the escape key, as a key sequence.
  (string-append (car %term-bound-keys) " " key))

(define (%term-unbind-keys!)
  ;; Takes back what was bound for the reserved keys of the last build: the
  ;; escape key's map, and the editor's keys that char mode sent on.
  (undefine-key 'term-mode (car %term-bound-keys))
  (for-each (lambda (key) (undefine-key 'term-char-mode key))
            (cdr %term-bound-keys)))

(define (%term-bind-semi-char-keys!)
  ;; Every key the editor binds goes to the program, except the reserved
  ;; ones, which keep their meaning.
  (define-key 'term-mode #t 'term-send-key)
  (for-each (lambda (key)
              (if (member key %term-bound-keys)
                  (undefine-key 'term-mode key)
                  (define-key 'term-mode key 'term-send-key)))
            (%term-all-keys)))

(define (%term-bind-escape-keys!)
  ;; After the escape key, a reserved key goes to the program, the escape
  ;; key itself among them; the terminal's commands come after it too.
  (for-each (lambda (key)
              (define-key 'term-mode (%term-escape-sequence key) 'term-send-key))
            %term-bound-keys)
  (for-each (lambda (binding)
              (define-key 'term-mode (%term-escape-sequence (car binding))
                (cdr binding)))
            %term-escape-commands))

(define (%term-bind-char-keys!)
  ;; In char mode the keys that semi-char mode keeps for the editor go to
  ;; the program too, so they are bound here to override the global
  ;; prefixes; only the escape key stays, as the way out.
  (define-key 'term-char-mode #t 'term-send-key)
  (for-each (lambda (key) (define-key 'term-char-mode key 'term-send-key))
            (cdr %term-bound-keys)))

(define (%term-bind-keys!)
  "Binds the keys of term-mode and term-char-mode as the options
`term-escape-key` and `term-emacs-keys` say now. Each terminal started
does this, so a change of the options needs no more; nothing is done when
they have not changed."
  (let ((reserved (%term-reserved-keys)))
    (unless (equal? reserved %term-bound-keys)
      (when %term-bound-keys (%term-unbind-keys!))
      (set! %term-bound-keys reserved)
      (%term-bind-semi-char-keys!)
      (%term-bind-escape-keys!)
      (%term-bind-char-keys!))))

(define (term-define-key key command)
  "Binds a command to a key pressed after the `term-escape-key` in terminal
buffers, in semi-char mode and char mode alike: (term-define-key \"C-d\"
'term-dired) makes C-q C-d show the listing. The binding follows the
escape key when the option is changed."
  (set! %term-escape-commands
        (cons (cons key command)
              (filter (lambda (binding) (not (equal? (car binding) key)))
                      %term-escape-commands)))
  (when %term-bound-keys
    (define-key 'term-mode (%term-escape-sequence key) command)))

;;; The buffers

(define (%term-shell)
  (or (option 'term-shell)
      (let ((shell (getenv "SHELL")))
        (list (if (and shell (not (equal? shell ""))) shell "sh")))))

(define (%term-environment)
  (append '(("TERM" "xterm-256color")
            ("COLORTERM" "truecolor")
            ("AMACS" "1"))
          (let ((socket (getenv "AMACS_SOCKET")))
            (if socket (list (list "AMACS_SOCKET" socket)) '()))))

(define (%term-window-size)
  ;; One column short of the window's text width: the editor keeps the last
  ;; column for a line-continuation mark, so a program told the full width
  ;; would draw lines that the editor then wraps. The spare column is filled
  ;; with the terminal's own background, so there is no visible gap.
  (list (max 1 (- (window-width) 1)) (max 1 (window-height))))

(define (%term-handle-event buffer event argument)
  (case event
    [(title)
     (when (member buffer (buffer-list))
       (with-current-buffer buffer
         (set-local! 'term-title (or argument ""))))]
    [(bell)
     (message "Bell in " buffer)]
    [(clipboard)
     (kill-new argument)]
    [(directory)
     (run-hooks 'term-directory-hook buffer argument)]
    [(exit)
     (when (member buffer (buffer-list))
       (with-current-buffer buffer
         (set-buffer-read-only! #f)
         (goto-char (point-max))
         (insert "\n[Process exited"
                 (if argument (string-append " with status " (number->string argument)) "")
                 "]\n")
         (set-buffer-read-only! #t)))]))

(define-option 'term-title ""
  "The title that the program in a terminal buffer last set.")

(define (term-start name command . directory)
  "Starts a command, a list of the program and its arguments, on a
pseudo-terminal the size of the selected window, with its screen in a
buffer of the name, which is shown in the selected window. The optional
argument is the directory to run in; the working directory is used
otherwise. Returns the buffer's name."
  (let ((dir (if (null? directory) (working-directory) (car directory))))
    (switch-to-buffer name)
    (term-mode)
    (let* ((size (%term-window-size))
           (process (make-process name command
                                  'pty size
                                  'buffer name
                                  'environment (%term-environment)
                                  'directory dir)))
      (%make-terminal name process (car size) (cadr size)
                      (option 'term-scrollback-lines)
                      %term-handle-event)
      name)))

(define (%term-buffers)
  (filter (lambda (name) (terminal-buffer? name)) (buffer-list)))

(define (%term-buffer-name)
  (let loop ((n 1))
    (let ((name (if (= n 1) "*term*" (string-append "*term*<" (number->string n) ">"))))
      (if (member name (buffer-list))
          (loop (+ n 1))
          name))))

(define (%term-for-project)
  "A live terminal buffer that was started in the current project, if any."
  (let ((root (project-root)))
    (let loop ((buffers (%term-buffers)))
      (cond [(null? buffers) #f]
            [(equal? (with-current-buffer (car buffers) (project-root)) root)
             (car buffers)]
            [else (loop (cdr buffers))]))))

(define-command (term)
  "Switch to a terminal of the current project, starting a shell in one
if there is none. With a prefix argument, start another."
  (let ((existing (and (= (prefix-arg) 1) (%term-for-project))))
    (if existing
        (switch-to-buffer existing)
        (term-start (%term-buffer-name) (%term-shell)))))

(define-command (term-other-window)
  "As `term`, in another window."
  (select-other-window)
  (term))

;;; Keys to the program

(define-command (term-send-key)
  "Send the key that ran this command to the program in the terminal."
  (let ((key (last-key)))
    (cond [(not (terminal-buffer?))
           (error "The program of this terminal has ended")]
          [(not key) #f]
          [else (terminal-send-key key)])))

(define-command (term-paste)
  "Send the pasted text to the program in the terminal."
  (if (terminal-buffer?)
      (terminal-paste (or (last-paste) ""))
      (error "The program of this terminal has ended")))

(define-command (term-yank)
  "Send the last kill to the program in the terminal, as a paste."
  (if (terminal-buffer?)
      (terminal-paste (or (current-kill) ""))
      (error "The program of this terminal has ended")))

(define (term-send-string text)
  "Sends text to the program in the current terminal buffer, as if typed."
  (terminal-paste text))

(define-command (term-recenter)
  "Bring point back to the program's cursor and keep it there."
  (set-terminal-follow! #t))

(define (%term-note-point-moved)
  ;; Moving point away from the cursor stops it following, until a key is
  ;; sent to the program again.
  (when (and (eq? (major-mode) 'term-mode)
             (terminal-buffer?)
             (terminal-follows?)
             (not (memq (this-command) '(term-send-key term-paste term-yank
                                         term-recenter)))
             (let ((cursor (terminal-cursor)))
               (and cursor (not (= cursor (point))))))
    (set-terminal-follow! #f)))

(add-hook! 'post-command-hook %term-note-point-moved)

(define (%term-fit-windows)
  ;; A terminal takes the size of the window that shows it.
  (for-each
   (lambda (window)
     (let ((buffer (window-buffer window)))
       (when (terminal-buffer? buffer)
         (with-selected-window window
           (let ((size (%term-window-size)))
             (unless (equal? size (terminal-size buffer))
               (terminal-resize (car size) (cadr size) buffer)))))))
   (window-list)))

(add-hook! 'window-size-change-hook %term-fit-windows)

(term-define-key "C-l" 'term-recenter)
(term-define-key "C-y" 'term-yank)
(term-define-key "C-k" 'term-char-mode)
(define-key 'term-mode "<paste>" 'term-paste)
(define-key 'term-char-mode "<paste>" 'term-paste)
(%term-bind-keys!)
(global-set-key "C-c v" 'term)
(global-set-key "C-c V" 'term-other-window)
