;;; What kinds of prose share: tables that keep their columns in line, and
;;; lists that go on and have boxes to tick. Markdown and Org write these
;;; nearly alike. Where they differ, a mode says how it writes them with
;;; define-prose-style.

;;; What each mode writes

(define %prose-styles (make-hash-table))

(define (define-prose-style mode . properties)
  "Says how a major mode, and the modes derived from it, write tables and
lists, as keys and values:
  'rule         how the row of dashes of a table is written: 'bars for
                `| --- | :-: |`, where colons say how a column is lined up,
                as in Markdown; 'plus for `|---+---|`, as in Org.
  'tick         the box of an item that is done: \"[x]\" or \"[X]\".
  'star-items?  whether a star at the very start of a line starts an item
                of a list. In Org such a line is a heading."
  (hash-table-set! %prose-styles mode properties))

(define %prose-default-style '(rule bars tick "[x]" star-items? #t))

(define (prose-style)
  "How the major mode of the current buffer writes tables and lists: the
keys and values that it gave to define-prose-style, or those of Markdown."
  (or (mode-lookup %prose-styles) %prose-default-style))

;;; Lines

(define (%prose-line pos)
  (buffer-substring (line-beginning-position pos) (line-end-position pos)))

;; The start of the line before or after the one at a position, or #f.
(define (%prose-line-before pos)
  (let ((start (line-beginning-position pos)))
    (and (> start 0) (line-beginning-position (- start 1)))))

(define (%prose-line-after pos)
  (let ((end (line-end-position pos)))
    (and (< end (point-max)) (+ end 1))))

(define (%prose-indentation line)
  (let loop ((index 0))
    (if (and (< index (string-length line))
             (memv (string-ref line index) '(#\space #\tab)))
        (loop (+ index 1))
        index)))

;;; Tables

(define (table-line? line)
  "Whether a line is a row of a table: it starts with a bar."
  (string-prefix? "|" (string-trim-left line)))

(define (table-cells line)
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
(define (%table-alignment cell)
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

;; Whether a line is the row of dashes of a table, in a style.
(define (%table-rule? line style)
  (if (eq? (property-ref style 'rule) 'plus)
      (string-prefix? "|-" (string-trim-left line))
      (let ((cells (table-cells line)))
        (and (pair? cells)
             (let loop ((cells cells))
               (or (null? cells)
                   (and (%table-alignment (car cells)) (loop (cdr cells)))))))))

(define (%table-pad cell width alignment)
  (let* ((extra (max 0 (- width (string-width cell))))
         (before (case alignment
                   [(right) extra]
                   [(center) (quotient extra 2)]
                   [else 0])))
    (string-append (make-string before #\space) cell
                   (make-string (- extra before) #\space))))

(define (%table-dashes width alignment)
  (case alignment
    [(left) (string-append ":" (make-string (- width 1) #\-))]
    [(right) (string-append (make-string (- width 1) #\-) ":")]
    [(center) (string-append ":" (make-string (- width 2) #\-) ":")]
    [else (make-string width #\-)]))

(define (%table-nth items index default)
  (cond [(null? items) default]
        [(= index 0) (car items)]
        [else (%table-nth (cdr items) (- index 1) default)]))

;; Whether a column holds numbers: at least half of its cells that are not
;; empty look like numbers. Org lines such a column up to the right.
(define (%table-numbers? rows index)
  (let* ((cells (filter (lambda (cell) (not (string-null? cell)))
                        (map (lambda (row) (%table-nth row index "")) rows)))
         (numbers (filter (lambda (cell)
                            (regexp-search "^[<>]?[-+^.0-9]*[0-9][-+^.0-9eEdDx()%:]*$" cell))
                          cells)))
    (and (pair? cells)
         (>= (* 2 (length numbers)) (length cells)))))

(define (table-format lines . style)
  "Takes the lines of a table and returns them with the cells of each
column made equally wide. Rows that are short of cells get empty ones, and
every line starts as the first one does. The style, which is that of the
current buffer if none is given, says how the row of dashes is written.
With bars, as in Markdown, that row says how each column is lined up: `:--`
left, `--:` right, `:-:` in the middle. With plus signs, as in Org, a
column of numbers is lined up to the right."
  (let* ((style (if (pair? style) (car style) (prose-style)))
         (plus? (eq? (property-ref style 'rule) 'plus))
         (indent (substring (car lines) 0 (%prose-indentation (car lines))))
         ;; Each row is its cells, or the cells of the row of dashes after
         ;; the word rule. With plus signs that row has no cells.
         (rows (map (lambda (line)
                      (cond [(not (%table-rule? line style)) (table-cells line)]
                            [plus? '(rule)]
                            [else (cons 'rule (table-cells line))]))
                    lines))
         (rule? (lambda (row) (and (pair? row) (eq? (car row) 'rule))))
         (cells (lambda (row) (if (rule? row) (cdr row) row)))
         (columns (fold max 1 (map (lambda (row) (length (cells row))) rows)))
         (indices (iota columns))
         (rule (let loop ((rows rows))
                 (cond [(null? rows) '()]
                       [(rule? (car rows)) (cdr (car rows))]
                       [else (loop (cdr rows))])))
         (alignments (map (lambda (index)
                            (if plus?
                                (if (%table-numbers? (filter (lambda (row) (not (rule? row)))
                                                             rows)
                                                     index)
                                    'right
                                    'none)
                                (or (%table-alignment (%table-nth rule index "-")) 'none)))
                          indices))
         (widths (map (lambda (index)
                        (fold max (if plus? 1 3)
                              (map (lambda (row)
                                     (if (rule? row)
                                         0
                                         (string-width (%table-nth row index ""))))
                                   rows)))
                      indices)))
    (map (lambda (row)
           (cond
            [(and plus? (rule? row))
             (string-append
              indent "|"
              (string-join (map (lambda (width) (make-string (+ width 2) #\-)) widths) "+")
              "|")]
            [else
             (string-append
              indent "| "
              (string-join
               (map (lambda (index width alignment)
                      (if (rule? row)
                          (%table-dashes width alignment)
                          (%table-pad (%table-nth row index "") width alignment)))
                    indices widths alignments)
               " | ")
              " |")]))
         rows)))

(define (table-bounds)
  "The start of the first line and the end of the last line of the table
that point is in, as a pair, or #f if point is not in a table."
  (and (table-line? (%prose-line (point)))
       (let ((start (let loop ((pos (line-beginning-position)))
                      (let ((before (%prose-line-before pos)))
                        (if (and before (table-line? (%prose-line before)))
                            (loop before)
                            pos))))
             (end (let loop ((pos (line-beginning-position)))
                    (let ((after (%prose-line-after pos)))
                      (if (and after (table-line? (%prose-line after)))
                          (loop after)
                          (line-end-position pos))))))
         (cons start end))))

;; Where in a string the bars are that divide cells: those that no
;; backslash stands before.
(define (%table-bars text)
  (let loop ((index 0) (found '()))
    (cond [(>= index (string-length text)) (reverse found)]
          [(char=? (string-ref text index) #\\) (loop (+ index 2) found)]
          [(char=? (string-ref text index) #\|) (loop (+ index 1) (cons index found))]
          [else (loop (+ index 1) found)])))

;; Where point is in a table that starts at a position: a list of the row,
;; the cell and how far into the text of the cell, all counted from 0.
(define (%table-place start)
  (let* ((row (- (line-number-at-pos) (line-number-at-pos start)))
         (before (buffer-substring (line-beginning-position) (point)))
         (bars (%table-bars before)))
    (if (null? bars)
        (list row 0 0)
        (list row
              (- (length bars) 1)
              (string-length
               (string-trim-left
                (substring before (+ (last bars) 1) (string-length before))))))))

;; Puts point in a cell of a table whose lines are as table-format makes
;; them.
(define (%table-goto start row cell in-cell)
  (goto-char start)
  (let loop ((row row))
    (when (> row 0)
      (let ((after (%prose-line-after (point))))
        (when after
          (goto-char after)
          (loop (- row 1))))))
  (let* ((line (%prose-line (point)))
         (cells (table-cells line))
         (cell (min cell (max 0 (- (length cells) 1)))))
    (let ((bars (%table-bars line)))
      (if (>= cell (length bars))
          (goto-char (line-end-position))
          (let* ((bar (%table-nth bars cell 0))
                 (text (%table-nth cells cell ""))
                 (rest (substring line (+ bar 1) (string-length line)))
                 (blanks (%prose-indentation rest))
                 ;; An empty cell is all blanks: point goes after one.
                 (blanks (if (string-null? text) (min blanks 1) blanks)))
            (goto-char (+ (line-beginning-position) bar 1 blanks
                          (min in-cell (string-length text)))))))))

;; Lines up the table at point and returns where it starts, with point in
;; the cell it was in.
(define (%table-align)
  (let ((bounds (table-bounds)))
    (unless bounds
      (error "Point is not in a table"))
    (let* ((start (car bounds))
           (place (%table-place start))
           (old (buffer-substring start (cdr bounds)))
           (new (string-join (table-format (string-split old #\newline)) "\n")))
      (unless (equal? old new)
        (delete-region start (cdr bounds))
        (goto-char start)
        (insert new))
      (apply %table-goto start place)
      start)))

(define-command (table-align)
  "Line up the columns of the table at point."
  (%table-align))

;; The rows of the table at point that hold cells, as numbers counted from
;; the first line of the table: all but the rows of dashes.
(define (%table-rows)
  (let* ((bounds (table-bounds))
         (style (prose-style))
         (lines (string-split (buffer-substring (car bounds) (cdr bounds)) #\newline)))
    (let loop ((lines lines) (index 0) (rows '()))
      (cond [(null? lines) (reverse rows)]
            [(%table-rule? (car lines) style) (loop (cdr lines) (+ index 1) rows)]
            [else (loop (cdr lines) (+ index 1) (cons index rows))]))))

;; How many columns the table at point has: as many as its widest row.
(define (%table-columns)
  (let ((bounds (table-bounds))
        (style (prose-style)))
    (fold max 1
          (map (lambda (line)
                 (if (%table-rule? line style) 0 (length (table-cells line))))
               (string-split (buffer-substring (car bounds) (cdr bounds)) #\newline)))))

(define-command (table-next-cell)
  "Line up the table at point and go to the next cell. After the last
cell of the table, a new row is made."
  (let* ((start (%table-align))
         (place (%table-place start))
         (row (car place))
         (cell (cadr place))
         (columns (%table-columns))
         (rows (%table-rows))
         (later (filter (lambda (other) (> other row)) rows)))
    (cond
     [(and (memv row rows) (< (+ cell 1) columns))
      (%table-goto start row (+ cell 1) 0)]
     [(pair? later)
      (%table-goto start (car later) 0 0)]
     [else
      (goto-char (cdr (table-bounds)))
      (insert "\n|")
      (%table-align)
      (%table-goto start (+ (fold max row rows) 1) 0 0)])))

(define-command (table-previous-cell)
  "Line up the table at point and go to the cell before this one."
  (let* ((start (%table-align))
         (place (%table-place start))
         (row (car place))
         (cell (cadr place))
         (earlier (filter (lambda (other) (< other row)) (%table-rows))))
    (cond
     [(> cell 0) (%table-goto start row (- cell 1) 0)]
     [(pair? earlier)
      (%table-goto start (fold max 0 earlier) (- (%table-columns) 1) 0)]
     [else (%table-goto start row 0 0)])))

(define-command (table-insert)
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
      (%table-align)
      (%table-goto start 0 0 0))))

;;; Lists

(define (list-item-parts line)
  "The parts of a line that is an item of a list: a list of the blanks
before the marker, the marker (`-`, `*`, `+`, `1.` or `1)`), the box (such
as `[ ]` or `[x]`) or #f, and the text. #f for other lines."
  (let* ((style (prose-style))
         (indent (%prose-indentation line))
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
                [(and (< indent length) (memv (string-ref line indent) '(#\- #\+)))
                 (+ indent 1)]
                [(and (< indent length) (char=? (string-ref line indent) #\*)
                      (or (> indent 0) (property-ref style 'star-items? #t)))
                 (+ indent 1)]
                [else #f])))
    (and marker-end
         (or (= marker-end length) (char=? (string-ref line marker-end) #\space))
         (let* ((rest (if (= marker-end length)
                          ""
                          (substring line (+ marker-end 1) length)))
                (box (and (>= (string-length rest) 3)
                          (member (substring rest 0 3) '("[ ]" "[x]" "[X]" "[-]"))
                          (or (= (string-length rest) 3)
                              (char=? (string-ref rest 3) #\space))
                          (substring rest 0 3)))
                (text (cond [(not box) rest]
                            [(= (string-length rest) 3) ""]
                            [else (substring rest 4 (string-length rest))])))
           (list (substring line 0 indent) (substring line indent marker-end) box text)))))

;; The marker of the item after one: the next number, or the same bullet.
(define (%list-next-marker marker)
  (let* ((last (- (string-length marker) 1))
         (number (string->number (substring marker 0 last))))
    (if number
        (string-append (number->string (+ number 1)) (substring marker last (+ last 1)))
        marker)))

(define-command (list-item-newline)
  "Start a new line. In an item of a list the new line is the next item,
with the next number and an empty box if this one has them; in an item
without text the list ends instead."
  (let* ((line (%prose-line (point)))
         (item (list-item-parts line))
         (start (line-beginning-position)))
    (cond
     [(or (not item)
          ;; Before the text of the item, a line is opened above it.
          (< (- (point) start) (- (string-length line) (string-length (cadddr item)))))
      (call-command 'newline)]
     [(string-null? (string-trim (cadddr item)))
      (delete-region start (line-end-position))]
     [else
      (insert "\n" (car item) (%list-next-marker (cadr item)) " "
              (if (caddr item) "[ ] " ""))])))

(define-command (list-item-toggle-checkbox)
  "Tick the box of the item of a list that point is in, or clear it. An
item without a box gets an empty one."
  (let* ((line (%prose-line (point)))
         (item (list-item-parts line)))
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
          (insert (if (member box '("[ ]" "[-]"))
                      (property-ref (prose-style) 'tick "[x]")
                      "[ ]"))]
         [else
          ;; A marker alone on its line has no blank after it yet.
          (goto-char (min box-start (line-end-position)))
          (when (< (point) box-start) (insert " "))
          (insert "[ ] ")])))))
