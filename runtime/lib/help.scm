;;; Help that is made from what the editor knows: the keys of a mode come
;;; from its keymap, and what each key does from the documentation of its
;;; command, so the help cannot say something else than the code. The first
;;; sentence of a command's documentation is what is shown here and by M-x.

(set-face! 'help-heading 'bold #t)
(set-face! 'help-key 'fg 'blue 'bold #t)
(set-face! 'help-command 'fg 'fg-dim)

;; The buffer that the window showed before it showed help.
(define %help-previous-buffer #f)

(define-major-mode help-mode special-mode "Help"
  "Major mode of the buffer that shows help. The text is made anew each
time help is asked for."
  (set-buffer-read-only! #t)
  (enable-mode! 'visual-line-mode))

(define-command (help-quit)
  "Leave the help and show what the window showed before."
  (if (and %help-previous-buffer (member %help-previous-buffer (buffer-list)))
      (switch-to-buffer %help-previous-buffer)
      (kill-buffer)))

(define-key 'help-mode "q" 'help-quit)

;; Shows lines in the help buffer, in the selected window: help is read
;; and left, and a window of its own would be half as wide. A line is a
;; list of pieces; a piece is a string, or a list of a string
;; and the face to show it in.
(define (%help-show lines)
  (let ((parts '())
        (faces '())
        (at 0))
    (for-each
     (lambda (line)
       (for-each
        (lambda (piece)
          (let* ((text (if (pair? piece) (car piece) piece))
                 (end (+ at (string-length text))))
            (when (pair? piece)
              (set! faces (cons (list at end (cadr piece)) faces)))
            (set! parts (cons text parts))
            (set! at end)))
        line)
       (set! parts (cons "\n" parts))
       (set! at (+ at 1)))
     lines)
    (unless (equal? (buffer-name) "*Help*")
      (set! %help-previous-buffer (buffer-name))
      (switch-to-buffer "*Help*"))
    (unless (eq? (major-mode) 'help-mode)
      (help-mode))
    (%replace-buffer-text! (apply string-append (reverse parts)))
    (clear-faces!)
    (for-each (lambda (face) (apply add-face! face)) faces)
    (goto-char 0)))

;; Whether a key is one of the pointer.
(define (%help-pointer? keys)
  (and (string-search keys "mouse") #t))

;; The order of keys in help: one char before several, a letter next to
;; its capital, and the keys of the pointer last.
(define (%help-key<? a b)
  (define (rank keys)
    (cond [(%help-pointer? keys) 2]
          [(= (string-length keys) 1) 0]
          [else 1]))
  (cond
   [(not (= (rank a) (rank b))) (< (rank a) (rank b))]
   [(string-ci=? a b) (string>? a b)]
   [else (string-ci<? a b)]))

;; The bindings of a keymap that hold in the current buffer, as lists of
;; a command and its keys: one list for each command, in the order of each
;; command's first key. A command that has keys of the keyboard is listed
;; without its keys of the pointer. `taken` is a hash table of the keys
;; that an earlier keymap binds, which this adds its own to.
(define (%help-rows keymap taken)
  (let ((rows '()))
    (for-each
     (lambda (binding)
       (let ((keys (car binding))
             (command (cadr binding)))
         (unless (hash-table-contains? taken keys)
           (hash-table-set! taken keys #t)
           (let ((row (assoc command rows)))
             (cond
              [(not row) (set! rows (append rows (list (list command keys))))]
              [(not (%help-pointer? keys))
               (set-cdr! row (append (cdr row) (list keys)))])))))
     (sort (keymap-bindings keymap)
           (lambda (a b) (%help-key<? (car a) (car b)))))
    rows))

;; The lines that list the keys of a keymap, or none if it binds nothing
;; that holds: the keys of each command and what the command does, and
;; with `names?` the name of the command between them.
(define (%help-keymap-lines keymap taken names?)
  (let* ((rows (%help-rows keymap taken))
         (shown (map (lambda (row) (string-join (cdr row) ", ")) rows))
         (width (min 12 (apply max 0 (map string-length shown))))
         (name-width (min 28 (apply max 0 (map (lambda (row) (string-length (car row)))
                                               rows)))))
    (if (null? rows)
        '()
        (append
         (list (list (list (if (equal? keymap "global")
                               "Keys that hold everywhere"
                               (string-append "Keys of " keymap))
                           'help-heading)))
         (map (lambda (row keys)
                (append
                 (list "  "
                       (list (string-pad-right keys (max width (string-length keys)))
                             'help-key)
                       "  ")
                 (if names?
                     (list (list (string-pad-right
                                  (car row)
                                  (max name-width (string-length (car row))))
                                 'help-command)
                           "  ")
                     '())
                 (list (or (command-summary (car row)) ""))))
              rows shown)
         (list '())))))

;; The lines of help for the current buffer: its major mode and what that
;; is for, then the keys of each keymap that holds here.
(define (%help-mode-lines global? names?)
  (let* ((mode (symbol->string (major-mode)))
         (taken (make-hash-table))
         (keymaps (filter (lambda (keymap) (or global? (not (equal? keymap "global"))))
                          (active-keymaps))))
    (append
     (list (list (list mode 'help-heading)) '())
     (map list (string-split (or (command-documentation mode) "") #\newline))
     (list '())
     (append-map (lambda (keymap) (%help-keymap-lines keymap taken names?)) keymaps))))

(define-command (describe-mode)
  "Show what the mode of this buffer is for and what its keys do."
  (%help-show (%help-mode-lines #f #f)))

(define-command (describe-bindings)
  "Show what all keys do in this buffer, with the names of their commands.
The keys that hold everywhere are listed too."
  (%help-show (%help-mode-lines #t #t)))

;;; Help for names: what a procedure, a command or an option is. The
;;; text is what `describe` and `documentation` give to code, so that a
;;; person and a program are told the same.

;; Whether a name is one that only the runtime itself uses.
(define (%help-internal? name)
  (string-prefix? "%" name))

;; The lines of a text, each as a line of help.
(define (%help-text-lines text)
  (map list (string-split text #\newline)))

;; The first sentence of a documentation, on one line, as M-x shows that
;; of a command; all of it if it has no sentence. "" for none.
(define (%help-summary doc)
  (if doc
      (let* ((flat (string-replace doc "\n" " "))
             (end (string-search flat ". ")))
        (if end (substring flat 0 (+ end 1)) flat))
      ""))

;; What the keys of a command are, in words.
(define (%help-keys-of command)
  (let ((keys (command-keys command)))
    (if (null? keys)
        "It is not on a key here; M-x runs it."
        (string-append "Keys: " (string-join keys ", ")))))

(define (help-function-names)
  "The names that `describe-function` offers, in order: the commands, and
the procedures and macros of the top level that are meant to be used."
  (let ((names (make-hash-table)))
    (for-each (lambda (name) (hash-table-set! names name #t)) (command-names))
    (for-each
     (lambda (symbol)
       (let ((name (symbol->string symbol)))
         (when (and (not (%help-internal? name))
                    (or (documentation symbol)
                        (and (bound? symbol) (procedure? (global-ref symbol)))))
           (hash-table-set! names name #t))))
     ;; Every name there is, the macros among them.
     (apropos ""))
    (sort (hash-table-keys names) string<?)))

(define (help-function-lines name)
  "The lines that `describe-function` shows for a name, a string: what the
command of that name does and its keys, and how the procedure or macro of
that name is called, its documentation and where it is defined. An error
if nothing has the name."
  (let* ((symbol (string->symbol name))
         (command (and (command? name) (command-documentation name)))
         (defined? (or (bound? symbol) (documentation symbol)))
         (described (and defined? (describe symbol))))
    (unless (or command defined?)
      (error (string-append "Nothing is named " name)))
    (append
     (list (list (list name 'help-heading)) '())
     (if command
         (append (list (list name " is a command."))
                 (%help-text-lines command)
                 (list (list (list (%help-keys-of name) 'help-command)) '()))
         '())
     (if described (%help-text-lines described) '()))))

(define-command (describe-function)
  "Show what a procedure, a macro or a command is: how it is called, its documentation, where it is defined and the keys that run it."
  (%help-show
   (help-function-lines
    (completing-read "Describe function: " (help-function-names) 'function))))

(define (help-variable-lines name)
  "The lines that `describe-variable` shows for a name, a string: the value
of the option of that name here, its value where it has not been set, and
what it is for; or what the variable of that name at the top level holds.
An error if nothing has the name."
  (let ((doc (option-documentation name))
        (symbol (string->symbol name)))
    (cond
     [doc
      (let ((value (option name))
            (default (option-default name)))
        (append
         (list (list (list name 'help-heading)) '()
               (list name " is an option. Its value here is "
                     (list (write->string value) 'help-key)))
         (if (equal? value default)
             '()
             (list (list "Where it has not been set it is "
                         (list (write->string default) 'help-key))))
         (list '())
         (%help-text-lines doc)))]
     [(bound? symbol)
      (append (list (list (list name 'help-heading)) '())
              (%help-text-lines (describe symbol)))]
     [else (error (string-append "Nothing is named " name))])))

(define-command (describe-variable)
  "Show the value of an option and what the option is for."
  (%help-show
   (help-variable-lines
    (completing-read "Describe variable: "
                     (map (lambda (name)
                            (list name (%help-summary (option-documentation name))))
                          (option-names))
                     'option))))

(define-command (where-is)
  "Say which keys run a command in this buffer."
  (let* ((name (completing-read "Where is command: " (command-names) 'command))
         (keys (if (command? name)
                   (command-keys name)
                   (error (string-append "No command named " name)))))
    (message (if (null? keys)
                 (string-append name " is not on a key here; M-x runs it")
                 (string-append name " is on " (string-join keys ", "))))))

;; A section of what apropos found: a heading and a row for each name,
;; with what `beside` gives for the name next to it and what `about`
;; gives after that. None if no name was found.
(define (%help-found-lines heading names beside about)
  (if (null? names)
      '()
      (let ((width (min 32 (apply max 0 (map string-length names)))))
        (append
         (list (list (list heading 'help-heading)))
         (map (lambda (name)
                (let ((keys (beside name)))
                  (append
                   (list "  "
                         (list (string-pad-right name (max width (string-length name)))
                               'help-command)
                         "  ")
                   (if (string-null? keys) '() (list (list keys 'help-key) "  "))
                   (list (about name)))))
              names)
         (list '())))))

(define (help-apropos-lines text)
  "The lines that `help-apropos` shows for a text: the commands, the
procedures and macros, and the options whose names contain it, each with
the first of what its documentation says."
  (let* ((has? (lambda (name) (and (string-search name text) #t)))
         (commands (filter has? (command-names)))
         (procedures
          (sort (filter (lambda (name)
                          (not (or (%help-internal? name) (command? name))))
                        (map symbol->string (apropos text)))
                string<?))
         (options (filter has? (option-names))))
    (if (and (null? commands) (null? procedures) (null? options))
        (list (list "Nothing has a name with " text " in it."))
        (append
         (%help-found-lines "Commands" commands
                            (lambda (name) (string-join (command-keys name) ", "))
                            (lambda (name) (or (command-summary name) "")))
         (%help-found-lines "Procedures and macros" procedures
                            (lambda (name) "")
                            (lambda (name)
                              (%help-summary (documentation (string->symbol name)))))
         (%help-found-lines "Options" options
                            (lambda (name) "")
                            (lambda (name)
                              (%help-summary (option-documentation name))))))))

(define-command (help-apropos)
  "Show the commands, procedures and options whose names contain a text."
  (%help-show (help-apropos-lines (read-string "Apropos: "))))

;; Help is on C-h, as in Emacs, and on <f1> for those who have C-h for
;; something else.
(for-each
 (lambda (prefix)
   (for-each
    (lambda (binding)
      (global-set-key (string-append prefix " " (car binding)) (cadr binding)))
    '(("m" describe-mode) ("b" describe-bindings)
      ("k" describe-key) ("f" describe-function) ("x" describe-command)
      ("v" describe-variable) ("w" where-is) ("a" help-apropos)
      ("e" view-echo-area-messages))))
 '("C-h" "<f1>"))
