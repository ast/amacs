;;; Editing Markdown: tables that keep their columns in line, lists that
;;; go on, boxes that are ticked, moving by headings, and links that are
;;; followed. How Markdown is coloured is the business of its grammar, in
;;; crates/amacs-treesit.

;;; Lines

(define (%markdown-line pos)
  (buffer-substring (line-beginning-position pos) (line-end-position pos)))

;; The start of the line before or after the one at a position, or #f.
(define (%markdown-line-before pos)
  (let ((start (line-beginning-position pos)))
    (and (> start 0) (line-beginning-position (- start 1)))))

(define (%markdown-line-after pos)
  (let ((end (line-end-position pos)))
    (and (< end (point-max)) (+ end 1))))

(define (%markdown-indentation line)
  (let loop ((index 0))
    (if (and (< index (string-length line))
             (memv (string-ref line index) '(#\space #\tab)))
        (loop (+ index 1))
        index)))

;;; Tables

(define (markdown-table-line? line)
  "Whether a line is a row of a table: it starts with a bar."
  (string-prefix? "|" (string-trim-left line)))

(define (markdown-table-cells line)
  "The cells of a row of a table, without the blanks around them. A bar
after a backslash is part of its cell."
  (let loop ((chars (string->list (string-trim line))) (cell '()) (cells '()))
    (define (done) (string-trim (list->string (reverse cell))))
    (cond
     [(null? chars)
      (let* ((all (reverse (cons (done) cells)))
             ;; The bars at either end of the row enclose no cell.
             (all (if (and (pair? all) (string-null? (car all))) (cdr all) all))
             (all (reverse all))
             (all (if (and (pair? all) (string-null? (car all))) (cdr all) all)))
        (reverse all))]
     [(and (char=? (car chars) #\\) (pair? (cdr chars)))
      (loop (cddr chars) (cons (cadr chars) (cons #\\ cell)) cells)]
     [(char=? (car chars) #\|)
      (loop (cdr chars) '() (cons (done) cells))]
     [else (loop (cdr chars) (cons (car chars) cell) cells)])))

;; How the column under a cell of the row of dashes is lined up: left,
;; right, center or none; #f if the cell is not of that row.
(define (%markdown-alignment cell)
  (let* ((length (string-length cell))
         (left? (and (> length 0) (char=? (string-ref cell 0) #\:)))
         (right? (and (> length 1) (char=? (string-ref cell (- length 1)) #\:)))
         (dashes (substring cell (if left? 1 0) (- length (if right? 1 0)))))
    (and (> (string-length dashes) 0)
         (= (string-count dashes (lambda (c) (char=? c #\-))) (string-length dashes))
         (cond [(and left? right?) 'center]
               [left? 'left]
               [right? 'right]
               [else 'none]))))

(define (%markdown-delimiter-row? cells)
  (and (pair? cells)
       (let loop ((cells cells))
         (or (null? cells)
             (and (%markdown-alignment (car cells)) (loop (cdr cells)))))))

(define (%markdown-pad cell width alignment)
  (let* ((extra (max 0 (- width (string-width cell))))
         (before (case alignment
                   [(right) extra]
                   [(center) (quotient extra 2)]
                   [else 0])))
    (string-append (make-string before #\space) cell
                   (make-string (- extra before) #\space))))

(define (%markdown-dashes width alignment)
  (case alignment
    [(left) (string-append ":" (make-string (- width 1) #\-))]
    [(right) (string-append (make-string (- width 1) #\-) ":")]
    [(center) (string-append ":" (make-string (- width 2) #\-) ":")]
    [else (make-string width #\-)]))

(define (%markdown-nth items index default)
  (cond [(null? items) default]
        [(= index 0) (car items)]
        [else (%markdown-nth (cdr items) (- index 1) default)]))

(define (markdown-format-table lines)
  "Takes the lines of a table and returns them with the cells of each
column made equally wide, and lined up as the row of dashes says: `:--`
left, `--:` right, `:-:` in the middle. Rows that are short of cells get
empty ones. Every line starts as the first one does."
  (let* ((indent (substring (car lines) 0 (%markdown-indentation (car lines))))
         (rows (map markdown-table-cells lines))
         (columns (fold max 1 (map length rows)))
         (indices (iota columns))
         (delimiter (let loop ((rows rows))
                      (cond [(null? rows) '()]
                            [(%markdown-delimiter-row? (car rows)) (car rows)]
                            [else (loop (cdr rows))])))
         (alignments (map (lambda (index)
                            (or (%markdown-alignment (%markdown-nth delimiter index "-"))
                                'none))
                          indices))
         (widths (map (lambda (index)
                        (fold max 3
                              (map (lambda (row)
                                     (if (%markdown-delimiter-row? row)
                                         0
                                         (string-width (%markdown-nth row index ""))))
                                   rows)))
                      indices)))
    (map (lambda (row)
           (string-append
            indent "| "
            (string-join
             (map (lambda (index width alignment)
                    (if (%markdown-delimiter-row? row)
                        (%markdown-dashes width alignment)
                        (%markdown-pad (%markdown-nth row index "") width alignment)))
                  indices widths alignments)
             " | ")
            " |"))
         rows)))

(define (markdown-table-bounds)
  "The start of the first line and the end of the last line of the table
that point is in, as a pair, or #f if point is not in a table."
  (and (markdown-table-line? (%markdown-line (point)))
       (let ((start (let loop ((pos (line-beginning-position)))
                      (let ((before (%markdown-line-before pos)))
                        (if (and before (markdown-table-line? (%markdown-line before)))
                            (loop before)
                            pos))))
             (end (let loop ((pos (line-beginning-position)))
                    (let ((after (%markdown-line-after pos)))
                      (if (and after (markdown-table-line? (%markdown-line after)))
                          (loop after)
                          (line-end-position pos))))))
         (cons start end))))

;; Where in a string the bars are that divide cells: those that no
;; backslash stands before.
(define (%markdown-bars text)
  (let loop ((index 0) (found '()))
    (cond [(>= index (string-length text)) (reverse found)]
          [(char=? (string-ref text index) #\\) (loop (+ index 2) found)]
          [(char=? (string-ref text index) #\|) (loop (+ index 1) (cons index found))]
          [else (loop (+ index 1) found)])))

;; Where point is in a table that starts at a position: a list of the row,
;; the cell and how far into the text of the cell, all counted from 0.
(define (%markdown-table-place start)
  (let* ((row (- (line-number-at-pos) (line-number-at-pos start)))
         (before (buffer-substring (line-beginning-position) (point)))
         (bars (%markdown-bars before)))
    (if (null? bars)
        (list row 0 0)
        (list row
              (- (length bars) 1)
              (string-length
               (string-trim-left
                (substring before (+ (last bars) 1) (string-length before))))))))

;; Puts point in a cell of a table whose lines are as markdown-format-table
;; makes them.
(define (%markdown-table-goto start row cell in-cell)
  (goto-char start)
  (let loop ((row row))
    (when (> row 0)
      (let ((after (%markdown-line-after (point))))
        (when after
          (goto-char after)
          (loop (- row 1))))))
  (let* ((line (%markdown-line (point)))
         (cells (markdown-table-cells line))
         (cell (min cell (max 0 (- (length cells) 1)))))
    (let ((bars (%markdown-bars line)))
      (if (>= cell (length bars))
          (goto-char (line-end-position))
          (let* ((bar (%markdown-nth bars cell 0))
                 (text (%markdown-nth cells cell ""))
                 (rest (substring line (+ bar 1) (string-length line)))
                 (blanks (%markdown-indentation rest))
                 ;; An empty cell is all blanks: point goes after one.
                 (blanks (if (string-null? text) (min blanks 1) blanks)))
            (goto-char (+ (line-beginning-position) bar 1 blanks
                          (min in-cell (string-length text)))))))))

;; Lines up the table at point and returns where it starts, with point in
;; the cell it was in.
(define (%markdown-align-table)
  (let ((bounds (markdown-table-bounds)))
    (unless bounds
      (error "Point is not in a table"))
    (let* ((start (car bounds))
           (place (%markdown-table-place start))
           (old (buffer-substring start (cdr bounds)))
           (new (string-join (markdown-format-table (string-split old #\newline)) "\n")))
      (unless (equal? old new)
        (delete-region start (cdr bounds))
        (goto-char start)
        (insert new))
      (apply %markdown-table-goto start place)
      start)))

(define-command (markdown-table-align)
  "Line up the columns of the table at point."
  (%markdown-align-table))

;; The rows of the table at point that hold cells, as numbers counted from
;; the first line of the table: all but the row of dashes.
(define (%markdown-table-rows)
  (let* ((bounds (markdown-table-bounds))
         (lines (string-split (buffer-substring (car bounds) (cdr bounds)) #\newline)))
    (let loop ((lines lines) (index 0) (rows '()))
      (cond [(null? lines) (reverse rows)]
            [(%markdown-delimiter-row? (markdown-table-cells (car lines)))
             (loop (cdr lines) (+ index 1) rows)]
            [else (loop (cdr lines) (+ index 1) (cons index rows))]))))

(define (%markdown-table-columns)
  (length (markdown-table-cells (%markdown-line (point)))))

(define-command (markdown-table-next-cell)
  "Line up the table at point and go to the next cell. After the last
cell of the table, a new row is made."
  (let* ((start (%markdown-align-table))
         (place (%markdown-table-place start))
         (row (car place))
         (cell (cadr place))
         (columns (%markdown-table-columns))
         (rows (%markdown-table-rows))
         (later (filter (lambda (other) (> other row)) rows)))
    (cond
     [(and (memv row rows) (< (+ cell 1) columns))
      (%markdown-table-goto start row (+ cell 1) 0)]
     [(pair? later)
      (%markdown-table-goto start (car later) 0 0)]
     [else
      (goto-char (cdr (markdown-table-bounds)))
      (insert "\n|")
      (%markdown-align-table)
      (%markdown-table-goto start (+ (fold max row rows) 1) 0 0)])))

(define-command (markdown-table-previous-cell)
  "Line up the table at point and go to the cell before this one."
  (let* ((start (%markdown-align-table))
         (place (%markdown-table-place start))
         (row (car place))
         (cell (cadr place))
         (earlier (filter (lambda (other) (< other row)) (%markdown-table-rows))))
    (cond
     [(> cell 0) (%markdown-table-goto start row (- cell 1) 0)]
     [(pair? earlier)
      (%markdown-table-goto start (fold max 0 earlier) (- (%markdown-table-columns) 1) 0)]
     [else (%markdown-table-goto start row 0 0)])))

(define-command (markdown-table-insert)
  "Insert a table with a number of columns, asked for, and go to its
first cell."
  (let* ((answer (read-string "Columns: " "3"))
         (columns (or (string->number answer) 0)))
    (unless (and (integer? columns) (<= 1 columns 50))
      (error "A table has from 1 to 50 columns"))
    (unless (= (point) (line-beginning-position))
      (goto-char (line-end-position))
      (insert "\n"))
    (let ((start (point))
          (row (lambda (cell)
                 (string-append "|" (apply string-append
                                           (map (lambda (index) (string-append cell "|"))
                                                (iota columns)))))))
      (insert (row "   ") "\n" (row "---") "\n" (row "   "))
      (goto-char start)
      (%markdown-align-table)
      (%markdown-table-goto start 0 0 0))))

;;; Lists

;; The parts of a line that is an item of a list: a list of the blanks
;; before the marker, the marker (`-`, `*`, `+`, `1.` or `1)`), the box
;; (`[ ]` or `[x]`) or #f, and the text. #f for other lines.
(define (%markdown-item line)
  (let* ((indent (%markdown-indentation line))
         (length (string-length line))
         (digits (let loop ((index indent))
                   (if (and (< index length) (char-numeric? (string-ref line index)))
                       (loop (+ index 1))
                       (- index indent))))
         (marker-end
          (cond [(and (> digits 0) (< digits 10)
                      (< (+ indent digits) length)
                      (memv (string-ref line (+ indent digits)) '(#\. #\))))
                 (+ indent digits 1)]
                [(and (< indent length) (memv (string-ref line indent) '(#\- #\* #\+)))
                 (+ indent 1)]
                [else #f])))
    (and marker-end
         (or (= marker-end length) (char=? (string-ref line marker-end) #\space))
         (let* ((rest (if (= marker-end length)
                          ""
                          (substring line (+ marker-end 1) length)))
                (box (and (>= (string-length rest) 3)
                          (member (substring rest 0 3) '("[ ]" "[x]" "[X]"))
                          (or (= (string-length rest) 3)
                              (char=? (string-ref rest 3) #\space))
                          (substring rest 0 3)))
                (text (cond [(not box) rest]
                            [(= (string-length rest) 3) ""]
                            [else (substring rest 4 (string-length rest))])))
           (list (substring line 0 indent) (substring line indent marker-end) box text)))))

;; The marker of the item after one: the next number, or the same bullet.
(define (%markdown-next-marker marker)
  (let* ((last (- (string-length marker) 1))
         (number (string->number (substring marker 0 last))))
    (if number
        (string-append (number->string (+ number 1)) (substring marker last (+ last 1)))
        marker)))

(define-command (markdown-newline)
  "Start a new line. In an item of a list the new line is the next item,
with the next number and an empty box if this one has them; in an item
without text the list ends instead."
  (let* ((line (%markdown-line (point)))
         (item (%markdown-item line))
         (start (line-beginning-position)))
    (cond
     [(or (not item)
          ;; Before the text of the item, a line is opened above it.
          (< (- (point) start) (- (string-length line) (string-length (cadddr item)))))
      (call-command 'newline)]
     [(string-null? (string-trim (cadddr item)))
      (delete-region start (line-end-position))]
     [else
      (insert "\n" (car item) (%markdown-next-marker (cadr item)) " "
              (if (caddr item) "[ ] " ""))])))

(define-command (markdown-toggle-checkbox)
  "Tick the box of the item of a list that point is in, or clear it. An
item without a box gets an empty one."
  (let* ((line (%markdown-line (point)))
         (item (%markdown-item line)))
    (unless item
      (error "Point is not in an item of a list"))
    (let* ((start (line-beginning-position))
           (box-start (+ start (string-length (car item)) (string-length (cadr item)) 1))
           (box (caddr item)))
      (save-excursion
        (cond
         [box
          (delete-region box-start (+ box-start 3))
          (goto-char box-start)
          (insert (if (equal? box "[ ]") "[x]" "[ ]"))]
         [else
          ;; A marker alone on its line has no blank after it yet.
          (goto-char (min box-start (line-end-position)))
          (when (< (point) box-start) (insert " "))
          (insert "[ ] ")])))))

;;; Headings

;; The headings of the buffer: a list of lists of the position, the level
;; and the text, in order. Lines that start with # in fenced code are not
;; headings.
(define (markdown-headings)
  "The headings of the current buffer that are written with #, in order:
a list of lists of the position of the line, the level from 1 to 6, and
the text."
  (let loop ((lines (string-split (buffer-string) #\newline))
             (pos 0) (fence #f) (found '()))
    (if (null? lines)
        (reverse found)
        (let* ((line (car lines))
               (next (+ pos (string-length line) 1))
               (trimmed (string-trim-left line))
               (fence-here (cond [(string-prefix? "```" trimmed) "```"]
                                 [(string-prefix? "~~~" trimmed) "~~~"]
                                 [else #f])))
          (cond
           [fence-here
            (loop (cdr lines) next
                  (cond [(not fence) fence-here]
                        [(equal? fence fence-here) #f]
                        [else fence])
                  found)]
           [fence (loop (cdr lines) next fence found)]
           [else
            (let ((level (let count ((index 0))
                           (if (and (< index (string-length line))
                                    (char=? (string-ref line index) #\#))
                               (count (+ index 1))
                               index))))
              (if (and (<= 1 level 6)
                       (or (= level (string-length line))
                           (char=? (string-ref line level) #\space)))
                  (loop (cdr lines) next fence
                        (cons (list pos level
                                    (string-trim (substring line level (string-length line))))
                              found))
                  (loop (cdr lines) next fence found)))])))))

;; Moving by headings, their levels and moving them with what is under
;; them are in runtime/lib/outline.scm.
(define-outline 'markdown-mode 'headings markdown-headings 'marker "#" 'max-level 6)

;;; Links

;; The address that a label stands for: that of the line `[label]: address`
;; of the buffer, or #f. Labels are the same whatever the case of their
;; letters.
(define (%markdown-definition label)
  (let ((label (string-downcase (string-trim label)))
        (text (buffer-string)))
    (let loop ((from 0))
      (let ((match (regexp-search "(?m)^ {0,3}\\[([^\\]]+)\\]:[ \\t]*<?([^\\s>]+)" text from)))
        (and match
             (if (equal? label
                         (string-downcase
                          (string-trim (apply substring text (cadr match)))))
                 (apply substring text (caddr match))
                 (loop (cadr (car match)))))))))

;; The target that the address of a link stands for. An address that is
;; no web address and does not start with # is the name of a file, and the
;; # before a place in that file is written :: and # for the library of
;; links.
(define (%markdown-target address)
  (let ((hash (string-search address "#")))
    (cond [(or (eqv? hash 0) (regexp-search "^[A-Za-z][A-Za-z0-9+.-]*:" address))
           address]
          [hash (string-append "file:" (substring address 0 hash) "::"
                               (substring address hash (string-length address)))]
          [else (string-append "file:" address)])))

(define (markdown-link-at pos)
  "The link of Markdown that a position is in: a list of where it starts,
where it ends, its target and its text, or #f. A link is [text](address),
a picture ![text](address), [text][label] or [label] where a line
`[label]: address` of the buffer gives the address, or the address of
such a line."
  (let* ((start (line-beginning-position pos))
         (line (buffer-substring start (line-end-position pos)))
         (index (- pos start))
         (part (lambda (match group) (apply substring line (list-ref match group))))
         (link (lambda (match address text)
                 (and address
                      (list (+ start (car (car match))) (+ start (cadr (car match)))
                            (%markdown-target address) text)))))
    (cond
     [(regexp-search-around
       "!?\\[([^\\]]*)\\]\\(\\s*<?([^)\\s>]+)>?(?:\\s+\"[^\"]*\")?\\s*\\)" line index)
      => (lambda (match) (link match (part match 2) (part match 1)))]
     [(regexp-search-around "\\[([^\\]]+)\\]\\[([^\\]]*)\\]" line index)
      => (lambda (match)
           (let ((label (part match 2)))
             (link match
                   (%markdown-definition (if (string-null? label) (part match 1) label))
                   (part match 1))))]
     [(regexp-search-around "^ {0,3}\\[([^\\]]+)\\]:[ \\t]*<?([^\\s>]+)" line index)
      => (lambda (match) (link match (part match 2) (part match 1)))]
     [(regexp-search-around "\\[([^\\]]+)\\]" line index)
      => (lambda (match)
           (link match (%markdown-definition (part match 1)) (part match 1)))]
     [else #f])))

(define (markdown-slug text)
  "The name that a link gives a heading with a text, as in [the plan](#the-plan):
the text in small letters with a hyphen for each blank and without
punctuation."
  (list->string
   (filter-map (lambda (char)
                 (cond [(char=? char #\space) #\-]
                       [(or (char-alphabetic? char) (char-numeric? char)
                            (memv char '(#\- #\_)))
                        char]
                       [else #f]))
               (string->list (string-downcase (string-trim text))))))

(define (markdown-link-place target)
  "The position of the heading that a target names: #name is the heading
whose text gives that name with markdown-slug, and other text is the text
of a heading. #f if there is no such heading."
  (let* ((name? (string-prefix? "#" target))
         (wanted (if name?
                     (string-downcase (substring target 1 (string-length target)))
                     target))
         (found (filter (lambda (heading)
                          (equal? wanted (if name?
                                             (markdown-slug (caddr heading))
                                             (caddr heading))))
                        (markdown-headings))))
    (and (pair? found) (car (car found)))))

(define-links 'markdown-mode 'at-point markdown-link-at 'internal markdown-link-place)

;;; Keys

(define-command (markdown-tab)
  "In a table, line it up and go to the next cell; elsewhere, indent."
  (if (markdown-table-bounds)
      (markdown-table-next-cell)
      (call-command 'indent-for-tab-command)))

(define-command (markdown-backtab)
  "In a table, line it up and go to the cell before this one."
  (when (markdown-table-bounds)
    (markdown-table-previous-cell)))

(define-command (markdown-do)
  "Do what fits where point is: line up a table, or tick the box of an
item of a list."
  (if (markdown-table-bounds)
      (markdown-table-align)
      (markdown-toggle-checkbox)))

(for-each
 (lambda (binding) (define-key 'markdown-mode (car binding) (cadr binding)))
 '(("TAB" markdown-tab) ("S-TAB" markdown-backtab) ("RET" markdown-newline)
   ("C-c C-c" markdown-do) ("C-c |" markdown-table-insert)
   ("C-c C-n" outline-next-heading) ("C-c C-p" outline-previous-heading)
   ("C-c C-u" outline-up-heading)
   ("C-c C-f" outline-forward-same-level) ("C-c C-b" outline-backward-same-level)
   ("C-c C-j" outline-goto-heading)
   ("M-<left>" outline-promote) ("M-<right>" outline-demote)
   ("M-S-<left>" outline-promote-subtree) ("M-S-<right>" outline-demote-subtree)
   ("M-<up>" outline-move-subtree-up) ("M-<down>" outline-move-subtree-down)
   ("C-c C-o" open-link-at-point)
   ("<mouse-2>" open-link-at-mouse) ("C-<mouse-1>" open-link-at-mouse)))
