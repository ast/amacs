;;; The smaller commands of Emacs.
;;;
;;; Commands that every Emacs has and that are little more than a few
;;; steps on the text: inserting a file, transposing, sorting lines,
;;; filling a paragraph, counting, registers, evaluating the expression
;;; before point. They are written on the primitives of `docs/extending.md`
;;; and need nothing of their own from the core, so they are also examples
;;; of how such commands are written.

(define-option 'fill-column 70
  "The column that `fill-paragraph` (M-q) keeps lines within.")

;;; What a command works on

(define (%simple-region)
  ;; The region if it is active, or else the whole buffer, as a pair of
  ;; positions.
  (if (region-active?)
      (cons (min (mark) (point)) (max (mark) (point)))
      (cons (point-min) (point-max))))

(define (%simple-lines-region)
  ;; The whole lines that the region touches, or all of the buffer, as a
  ;; pair of positions: from the start of a line to the end of one. A
  ;; region that ends at the start of a line does not take that line.
  (let* ((region (%simple-region))
         (start (line-beginning-position (car region)))
         (end (cdr region)))
    (cons start
          (if (and (> end start) (= end (line-beginning-position end)) (region-active?))
              (- end 1)
              (line-end-position end)))))

(define (%simple-replace! start end text)
  ;; Puts a text in place of what is between two positions, and leaves
  ;; point after it.
  (delete-region start end)
  (goto-char start)
  (insert text))

(define (%simple-change-lines! change)
  ;; Gives the lines of the region, or of the buffer, to a procedure, and
  ;; puts the lines it returns in their place. Returns how many lines there
  ;; were and how many there are.
  (let* ((region (%simple-lines-region))
         (lines (string-split (buffer-substring (car region) (cdr region)) #\newline))
         (changed (change lines)))
    (unless (equal? lines changed)
      (%simple-replace! (car region) (cdr region) (string-join changed "\n")))
    (deactivate-mark!)
    (cons (length lines) (length changed))))

(define (%simple-blank? text)
  (equal? (string-trim text) ""))

;;; Files and buffers

(define-command (insert-file)
  "Insert the text of a file at point. Point stays before what was
inserted, and the mark is set after it."
  (let ((path (read-file-name "Insert file: ")))
    (cond [(not (file-exists? path)) (error (string-append "No such file: " path))]
          [(file-directory? path) (error (string-append path " is a directory"))]
          [(not (file-text? path)) (error (string-append path " is not text"))])
    (let ((start (point)))
      (insert (read-file path))
      (set-mark! (point))
      (goto-char start))))

(define-command (insert-buffer)
  "Insert the text of another buffer at point. Point stays before what was
inserted, and the mark is set after it."
  (let* ((name (read-buffer-name "Insert buffer: "))
         (text (buffer-string name))
         (start (point)))
    (insert text)
    (set-mark! (point))
    (goto-char start)))

(define-command (load-file)
  "Read a file of Scheme and run everything in it."
  (let ((path (read-file-name "Load file: ")))
    (load path)
    (message "Loaded " path)))

(define-command (pwd)
  "Show the directory of the current buffer."
  (message "Directory " (working-directory)))

(define-command (cd)
  "Give the current buffer another directory: where its commands look for
files and run programs."
  (let ((directory (read-directory-name "Change directory: ")))
    (unless (file-directory? directory)
      (error (string-append "No such directory: " directory)))
    (set-buffer-directory! directory)
    (message "Directory " (working-directory))))

(define (%simple-other-buffer step)
  ;; The buffer a number of steps from the current one, in the order of
  ;; their names, around the ends.
  (let* ((names (sort (buffer-list) string<?))
         (count (length names))
         (index (let loop ((rest names) (n 0))
                  (cond [(null? rest) 0]
                        [(equal? (car rest) (buffer-name)) n]
                        [else (loop (cdr rest) (+ n 1))]))))
    (list-ref names (modulo (+ index step) count))))

(define-command (next-buffer)
  "Show the next buffer in the selected window: the one after this in the
order of their names, and the first after the last."
  (switch-to-buffer (%simple-other-buffer (prefix-arg))))

(define-command (previous-buffer)
  "Show the previous buffer in the selected window: the one before this in
the order of their names, and the last before the first."
  (switch-to-buffer (%simple-other-buffer (- (prefix-arg)))))

(define-command (view-echo-area-messages)
  "Show the buffer of messages: what the echo area has shown, with the
reports of errors in full."
  (switch-to-buffer "*Messages*")
  (goto-char (point-max)))

;;; Transposing

(define-command (transpose-chars)
  "Exchange the chars on either side of point and move past them. At the
end of a line, exchange the two before point."
  (let* ((at (if (and (= (point) (line-end-position)) (> (point) (line-beginning-position)))
                 (- (point) 1)
                 (point)))
         (before (- at 1)))
    (when (or (< before (point-min)) (>= at (point-max)))
      (error "Nothing to transpose here"))
    (let ((pair (buffer-substring before (+ at 1))))
      (%simple-replace! before (+ at 1)
                        (string-append (substring pair 1 2) (substring pair 0 1))))))

(define (%simple-word-after)
  ;; The word after point, as a pair of positions, with point left after
  ;; it.
  (call-command 'forward-word)
  (let ((end (point)))
    (call-command 'backward-word)
    (let ((start (point)))
      (goto-char end)
      (cons start end))))

(define-command (transpose-words)
  "Exchange the word before point with the word after it, and move past
both."
  (let ((origin (point)))
    (let* ((second (%simple-word-after))
           (first (begin (goto-char (car second))
                         (call-command 'backward-word)
                         (let ((start (point)))
                           (call-command 'forward-word)
                           (cons start (point))))))
      (when (or (>= (car first) (car second)) (> (cdr first) (car second)))
        (goto-char origin)
        (error "No two words to transpose here"))
      (let ((one (buffer-substring (car first) (cdr first)))
            (between (buffer-substring (cdr first) (car second)))
            (two (buffer-substring (car second) (cdr second))))
        (%simple-replace! (car first) (cdr second) (string-append two between one))))))

(define-command (transpose-lines)
  "Exchange the line of point with the line before it, and move to the
line after. On the first line, exchange it with the second."
  (when (= (line-number-at-pos) 1)
    (goto-char (line-end-position))
    (when (= (point) (point-max)) (error "No two lines to transpose here"))
    (goto-char (+ (point) 1)))
  (let* ((start (line-beginning-position (- (line-beginning-position) 1)))
         (middle (line-beginning-position))
         (end (line-end-position))
         (before (buffer-substring start (- middle 1)))
         (this (buffer-substring middle end)))
    (%simple-replace! start end (string-append this "\n" before))
    (when (< (point) (point-max)) (goto-char (+ (point) 1)))))

;;; Lines

(define-command (kill-whole-line)
  "Kill the line of point, all of it and its line break, wherever in it
point is."
  (let ((start (line-beginning-position))
        (end (line-end-position)))
    (cond [(< end (point-max)) (kill-region start (+ end 1))]
          [(> start (point-min)) (kill-region (- start 1) end)]
          [else (kill-region start end)])))

(define-command (duplicate-line)
  "Put a copy of the line of point under it. Point stays where it is."
  (let ((origin (point))
        (line (buffer-substring (line-beginning-position) (line-end-position))))
    (goto-char (line-end-position))
    (insert "\n" line)
    (goto-char origin)))

(define-command (delete-blank-lines)
  "On a blank line among others, delete all of them but one; on a blank
line alone, delete it; on a line with text, delete the blank lines after
it."
  (define (blank-at? position)
    (%simple-blank? (buffer-substring (line-beginning-position position)
                                      (line-end-position position))))
  (define (first-blank position)
    ;; The start of the run of blank lines that the line at a position is
    ;; in.
    (let ((start (line-beginning-position position)))
      (if (and (> start (point-min)) (blank-at? (- start 1)))
          (first-blank (- start 1))
          start)))
  (define (last-blank position)
    (let ((end (line-end-position position)))
      (if (and (< end (point-max)) (blank-at? (+ end 1)))
          (last-blank (+ end 1))
          end)))
  (cond
    [(blank-at? (point))
     (let ((start (first-blank (point)))
           (end (last-blank (point))))
       (if (= (line-number-at-pos start) (line-number-at-pos end))
           ;; Alone: the line goes, with a line break of its.
           (cond [(< end (point-max)) (delete-region start (+ end 1))]
                 [(> start (point-min)) (delete-region (- start 1) end)]
                 [else (delete-region start end)])
           (%simple-replace! start end "")))]
    [(and (< (line-end-position) (point-max)) (blank-at? (+ (line-end-position) 1)))
     (let ((origin (point))
           (start (+ (line-end-position) 1)))
       (delete-region start (min (point-max) (+ (last-blank start) 1)))
       (goto-char origin))]
    [else (message "No blank lines here")]))

(define (%simple-trimmed-end line)
  ;; A line without the spaces and tabs at its end.
  (let loop ((end (string-length line)))
    (if (and (> end 0) (memv (string-ref line (- end 1)) '(#\space #\tab)))
        (loop (- end 1))
        (substring line 0 end))))

(define-command (delete-trailing-whitespace)
  "Delete the spaces and tabs at the ends of the lines of the region, or
of the buffer."
  (let ((line (line-number-at-pos))
        (column (current-column)))
    (%simple-change-lines! (lambda (lines) (map %simple-trimmed-end lines)))
    ;; Point stays on its line, at its column or at the end that is left.
    (goto-line line)
    (goto-char (min (+ (point) column) (line-end-position)))))

(define-command (sort-lines)
  "Put the lines of the region, or of the buffer, in order. With a prefix
argument, in the opposite order."
  (let ((reverse? (prefix-arg-given?)))
    (%simple-change-lines!
     (lambda (lines)
       (let ((sorted (sort lines string<?)))
         (if reverse? (reverse sorted) sorted))))))

(define-command (reverse-region)
  "Put the lines of the region, or of the buffer, in the opposite order."
  (%simple-change-lines! reverse))

(define-command (delete-duplicate-lines)
  "Delete each line of the region, or of the buffer, that is the same as
one before it."
  (let ((counts (%simple-change-lines!
                 (lambda (lines)
                   (let loop ((rest lines) (seen '()) (kept '()))
                     (cond [(null? rest) (reverse kept)]
                           [(member (car rest) seen) (loop (cdr rest) seen kept)]
                           [else (loop (cdr rest) (cons (car rest) seen)
                                       (cons (car rest) kept))]))))))
    (message "Deleted " (number->string (- (car counts) (cdr counts))) " lines")))

(define (%simple-filter-lines prompt keep?)
  ;; Keeps the lines of the region, or of the buffer, that a procedure of
  ;; the line and the text read says to keep.
  (let* ((text (read-string prompt))
         (counts (%simple-change-lines!
                  (lambda (lines)
                    (filter (lambda (line) (keep? line text)) lines)))))
    (message "Deleted " (number->string (- (car counts) (cdr counts))) " lines")))

(define-command (flush-lines)
  "Delete the lines of the region, or of the buffer, that a regular
expression matches some of."
  (%simple-filter-lines "Flush lines matching regexp: "
                        (lambda (line regexp) (not (regexp-search regexp line)))))

(define-command (keep-lines)
  "Delete the lines of the region, or of the buffer, that a regular
expression matches nothing of."
  (%simple-filter-lines "Keep lines matching regexp: "
                        (lambda (line regexp) (regexp-search regexp line))))

;;; Replacing

(define (%simple-replace-all text from to)
  ;; A text with every occurrence of one string replaced by another, and
  ;; how many there were.
  (let loop ((at 0) (parts '()) (count 0))
    (let ((found (string-search text from at)))
      (if found
          (loop (+ found (string-length from))
                (cons to (cons (substring text at found) parts))
                (+ count 1))
          (cons (apply string-append
                       (reverse (cons (substring text at (string-length text)) parts)))
                count)))))

(define (%simple-replace-in-reach what replace)
  ;; Asks what to replace and with what, and gives the text of the region,
  ;; or from point to the end of the buffer, to a procedure with them. It
  ;; returns a pair of the new text and how much was replaced.
  (let* ((from (read-string (string-append "Replace " what ": ")))
         (to (read-string (string-append "Replace " what " " from " with: ")))
         (start (if (region-active?) (min (mark) (point)) (point)))
         (end (if (region-active?) (max (mark) (point)) (point-max))))
    (when (equal? from "") (error "Nothing to replace"))
    (let ((result (replace (buffer-substring start end) from to)))
      (when (> (cdr result) 0)
        (%simple-replace! start end (car result)))
      (deactivate-mark!)
      (message "Replaced " (number->string (cdr result))
               (if (= (cdr result) 1) " occurrence" " occurrences")))))

(define-command (replace-string)
  "Replace every occurrence of a text with another, in the region or from
point to the end of the buffer, without asking for each as M-% does."
  (%simple-replace-in-reach "string" %simple-replace-all))

(define-command (replace-regexp)
  "Replace every match of a regular expression, in the region or from
point to the end of the buffer, without asking for each as C-M-% does. In
the replacement, \\& is what matched and \\1 to \\9 what a group matched."
  (%simple-replace-in-reach
   "regexp"
   (lambda (text regexp to)
     (let ((result (regexp-replace regexp text to)))
       (cons (car result) (cadr result))))))

(define-command (count-matches)
  "Say how many matches of a regular expression there are in the region,
or from point to the end of the buffer."
  (let* ((regexp (read-string "Count matches of regexp: "))
         (start (if (region-active?) (min (mark) (point)) (point)))
         (end (if (region-active?) (max (mark) (point)) (point-max)))
         (count (cadr (regexp-replace regexp (buffer-substring start end) ""))))
    (message (number->string count) (if (= count 1) " match" " matches"))))

;;; Marking

(define-command (mark-paragraph)
  "Put point at the start of the paragraph and the mark at its end."
  (call-command 'forward-paragraph)
  (set-mark! (point))
  (call-command 'backward-paragraph)
  (activate-mark!))

(define-command (mark-word)
  "Set the mark at the end of the word after point, and leave point where
it is."
  (let ((origin (point)))
    (call-command 'forward-word)
    (set-mark! (point))
    (goto-char origin)
    (activate-mark!)))

;;; Counting and asking

(define-command (count-words)
  "Say how many lines, words and chars the region has, or the buffer."
  (let* ((region (%simple-region))
         (text (buffer-substring (car region) (cdr region)))
         ;; A line that the text only reaches the start of is not one of
         ;; its lines.
         (lines (if (equal? text "")
                    0
                    (- (+ (line-number-at-pos (cdr region))
                          (if (= (cdr region) (line-beginning-position (cdr region))) 0 1))
                       (line-number-at-pos (car region))))))
    (message (if (region-active?) "Region" "Buffer") " has "
             (number->string lines) (if (= lines 1) " line, " " lines, ")
             (number->string (length (filter (lambda (word) (not (equal? word "")))
                                             (string-split text))))
             " words and "
             (number->string (string-length text)) " chars")))

(define-command (what-line)
  "Say which line point is on."
  (message "Line " (number->string (line-number-at-pos))))

(define-command (what-cursor-position)
  "Say what char is at point, where point is among the chars of the
buffer, and its column."
  (let ((char (char-after)))
    (message (if char
                 (string-append "Char: " (if (equal? char "\n") "line break" char)
                                " (" (number->string (char->integer (string-ref char 0)))
                                ")  ")
                 "At the end  ")
             "point " (number->string (point)) " of " (number->string (point-max))
             "  column " (number->string (current-column)))))

(define-command (goto-char-position)
  "Move point to a position, counted in chars from 0, that is asked for."
  (let ((position (string->number (string-trim (read-string "Go to char: ")))))
    (unless (and position (exact? position)) (error "A position is a whole number"))
    (goto-char (max (point-min) (min position (point-max))))))

;;; Filling

(define (%simple-fill-prefix line)
  ;; What a line starts with that is not its text: blanks, and the marks
  ;; of a comment or a quotation.
  (let loop ((n 0))
    (if (and (< n (string-length line))
             (memv (string-ref line n) '(#\space #\tab #\; #\/ #\# #\>)))
        (loop (+ n 1))
        (substring line 0 n))))

(define (%simple-fill words first rest width)
  ;; Lines of words no wider than a width where they can be, the first
  ;; after one prefix and the others after another.
  (let loop ((words words) (line first) (empty? #t) (lines '()))
    (cond
      [(null? words) (reverse (cons line lines))]
      [empty? (loop (cdr words) (string-append line (car words)) #f lines)]
      [(> (+ (string-width line) 1 (string-width (car words))) width)
       (loop words rest #t (cons line lines))]
      [else (loop (cdr words) (string-append line " " (car words)) #f lines)])))

(define-command (fill-paragraph)
  "Break the lines of the paragraph of point anew, between words, so that
none is wider than the `fill-column` option where that can be. What the
lines start with, the marks of a comment or their indentation, is kept."
  (define (blank-at? position)
    (%simple-blank? (buffer-substring (line-beginning-position position)
                                      (line-end-position position))))
  (when (blank-at? (point)) (error "No paragraph here"))
  (let* ((start (let loop ((start (line-beginning-position)))
                  (if (and (> start (point-min)) (not (blank-at? (- start 1))))
                      (loop (line-beginning-position (- start 1)))
                      start)))
         (end (let loop ((end (line-end-position)))
                (if (and (< end (point-max)) (not (blank-at? (+ end 1))))
                    (loop (line-end-position (+ end 1)))
                    end)))
         (lines (string-split (buffer-substring start end) #\newline))
         (first (%simple-fill-prefix (car lines)))
         (rest (if (pair? (cdr lines)) (%simple-fill-prefix (cadr lines)) first))
         (words (apply append
                       (map (lambda (line)
                              (let ((prefix (%simple-fill-prefix line)))
                                (filter (lambda (word) (not (equal? word "")))
                                        (string-split
                                         (substring line (string-length prefix)
                                                    (string-length line))))))
                            lines)))
         (filled (string-join (%simple-fill words first rest (option 'fill-column)) "\n")))
    (if (equal? filled (buffer-substring start end))
        (goto-char end)
        (%simple-replace! start end filled))))

;;; Evaluating

(define (%simple-eval-and-show text)
  (message (value->string (eval-string text))))

(define-command (eval-last-sexp)
  "Evaluate the Scheme expression before point and show its value."
  (let ((end (point)))
    (call-command 'backward-sexp)
    (let ((start (point)))
      (goto-char end)
      (when (= start end) (error "No expression before point"))
      (%simple-eval-and-show (buffer-substring start end)))))

(define-command (eval-region)
  "Evaluate the region as Scheme and show the value of its last
expression."
  (unless (region-active?) (error "The region is not active"))
  (let ((text (buffer-substring (min (mark) (point)) (max (mark) (point)))))
    (deactivate-mark!)
    (%simple-eval-and-show text)))

;;; Keyboard macros
;;;
;;; F3 starts the definition of one and F4 ends it, and types its keys
;;; again; that is in the core, which sees every key. A macro is its keys,
;;; so one that is to be kept is a command that types them.

(define-command (name-last-kbd-macro)
  "Make a command of the keyboard macro last defined, by a name that is
asked for: M-x and the name types its keys again, as many times as the
prefix argument says, when another macro has taken its place. It lasts
until the editor is left; (execute-kbd-macro \"keys\") in init.scm does
the same for good."
  (let ((keys (last-kbd-macro)))
    (unless keys (error "No keyboard macro is defined"))
    (let ((name (string-trim (read-string "Name for the last keyboard macro: "))))
      (when (equal? name "") (error "A command needs a name"))
      (when (command? name) (error (string-append "There is a command " name " already")))
      (%install-command! (string->symbol name)
                         (string-append "Type the keys of a keyboard macro: " keys)
                         (lambda () (execute-kbd-macro keys (prefix-arg))))
      (message "M-x " name " types " keys))))

;;; Registers
;;;
;;; A register is a char under which a place or a text is kept, to come
;;; back to or to insert.

;; The registers: pairs of a char and what is kept under it, which is a
;; text, or a list of the name of a buffer and a position in it.
(define %simple-registers '())

(define (%simple-set-register! char value)
  (set! %simple-registers
    (cons (cons char value)
          (filter (lambda (register) (not (eqv? (car register) char)))
                  %simple-registers))))

(define (%simple-register char)
  (let ((register (assv char %simple-registers)))
    (unless register
      (error (string-append "Register " (string char) " is empty")))
    (cdr register)))

(define-command (point-to-register)
  "Keep where point is under a char, for C-x r j to come back to."
  (let ((char (read-char "Point to register: ")))
    (%simple-set-register! char (list (buffer-name) (point)))
    (message "Kept this place in register " (string char))))

(define-command (jump-to-register)
  "Go to the place that is kept under a char."
  (let* ((char (read-char "Jump to register: "))
         (place (%simple-register char)))
    (unless (pair? place)
      (error (string-append "Register " (string char) " holds text, not a place")))
    (unless (member (car place) (buffer-list))
      (error (string-append "The buffer " (car place) " is gone")))
    (switch-to-buffer (car place))
    (goto-char (min (cadr place) (point-max)))))

(define-command (copy-to-register)
  "Keep the text of the region under a char, for C-x r i to insert."
  (unless (region-active?) (error "The region is not active"))
  (let ((char (read-char "Copy to register: ")))
    (%simple-set-register! char (buffer-substring (min (mark) (point)) (max (mark) (point))))
    (deactivate-mark!)
    (message "Kept the region in register " (string char))))

(define-command (insert-register)
  "Insert the text that is kept under a char."
  (let* ((char (read-char "Insert register: "))
         (text (%simple-register char)))
    (unless (string? text)
      (error (string-append "Register " (string char) " holds a place, not text")))
    (insert text)))

;;; Inserting what a key would otherwise do

(define-command (quoted-insert)
  "Insert the char of the next key, whatever the key is bound to: C-q TAB
inserts a tab, C-q C-j a line break, C-q C-l a form feed."
  (let* ((key (read-key "C-q-"))
         (text (cond [(= (string-length key) 1) key]
                     [(equal? key "TAB") "\t"]
                     [(equal? key "SPC") " "]
                     [(or (equal? key "RET") (equal? key "C-j")) "\n"]
                     [(and (= (string-length key) 3) (string-prefix? "C-" key)
                           (char-alphabetic? (string-ref key 2)))
                      (string (integer->char
                               (- (char->integer (char-upcase (string-ref key 2))) 64)))]
                     [else #f])))
    (unless text (error (string-append key " is not a key for a char")))
    (insert text)))

;;; The other window

(define (%simple-in-other-window command)
  (let ((windows (window-list)))
    (when (null? (cdr windows)) (error "There is no other window"))
    (with-selected-window (cadr windows) (call-command command))))

(define-command (scroll-other-window)
  "Scroll the text of the next window upward by nearly a full window."
  (%simple-in-other-window 'scroll-up-command))

(define-command (scroll-other-window-down)
  "Scroll the text of the next window downward by nearly a full window."
  (%simple-in-other-window 'scroll-down-command))

(for-each
 (lambda (binding) (global-set-key (car binding) (cadr binding)))
 '(("C-x i" insert-file)
   ("C-t" transpose-chars) ("M-t" transpose-words) ("C-x C-t" transpose-lines)
   ("C-S-DEL" kill-whole-line)
   ("C-x C-o" delete-blank-lines)
   ("M-h" mark-paragraph) ("M-@" mark-word)
   ("M-=" count-words) ("C-x =" what-cursor-position)
   ("M-q" fill-paragraph)
   ("C-x C-e" eval-last-sexp)
   ("C-x <right>" next-buffer) ("C-x <left>" previous-buffer)
   ("C-h e" view-echo-area-messages)
   ("C-x r SPC" point-to-register) ("C-x r j" jump-to-register)
   ("C-x r s" copy-to-register) ("C-x r i" insert-register)
   ("C-q" quoted-insert)
   ("C-M-v" scroll-other-window) ("C-M-S-v" scroll-other-window-down)))
