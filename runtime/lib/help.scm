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

;; Help is on C-h, as in Emacs, and on <f1> for those who have C-h for
;; something else.
(for-each
 (lambda (prefix)
   (for-each
    (lambda (binding)
      (global-set-key (string-append prefix " " (car binding)) (cadr binding)))
    '(("m" describe-mode) ("b" describe-bindings)
      ("k" describe-key) ("f" describe-command) ("x" describe-command)
      ("e" view-echo-area-messages))))
 '("C-h" "<f1>"))
