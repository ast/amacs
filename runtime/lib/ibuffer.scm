;;; The buffer list: every buffer, grouped by project, with marks and keys
;;; that act on many at once, as ibuffer does in Emacs. C-x C-b shows it.
;;;
;;; The list is a buffer of sections (runtime/lib/section.scm): a group for
;;; each project, which TAB folds, and under it a line for each buffer with
;;; its marks, size, mode and file. Buffers that visit no file go under
;;; "Other buffers", unless their mode is one that lives in a project, as a
;;; listing of a directory does.

(define-option 'ibuffer-show-hidden #f
  "Whether the buffer list shows the buffers whose names start with a
space, which the editor keeps for itself.")

(define-option 'ibuffer-project-modes '(dired-mode git-mode)
  "The major modes whose buffers are listed under their project although
they visit no file, as a listing of a directory or the status of a
repository is. Other buffers without a file are listed under Other.")

(set-face! 'ibuffer-header 'bold #t)
(set-face! 'ibuffer-group 'fg 'blue 'bold #t)
(set-face! 'ibuffer-detail 'fg 'fg-dim)
(set-face! 'ibuffer-modified 'fg 'yellow-warmer 'bold #t)
(set-face! 'ibuffer-mark 'fg 'magenta 'bold #t)
(set-face! 'ibuffer-marked 'fg 'magenta 'bold #t)
(set-face! 'ibuffer-flag 'fg 'red 'bold #t)
(set-face! 'ibuffer-flagged 'fg 'red 'bold #t)

;;; What the buffer list knows

(define-record-type ibuffer-row
  (%make-ibuffer-row name file mode size modified read-only group recency)
  ibuffer-row?
  (name ibuffer-row-name)
  (file ibuffer-row-file)
  (mode ibuffer-row-mode)
  ;; Chars.
  (size ibuffer-row-size)
  (modified ibuffer-row-modified?)
  (read-only ibuffer-row-read-only?)
  ;; The root of the project the buffer is listed under, 'files for a
  ;; file in no project, or 'other.
  (group ibuffer-row-group)
  ;; Where the buffer comes in the order of use: 0 for the latest.
  (recency ibuffer-row-recency))

(define-record-type ibuffer-state
  (%make-ibuffer-state view rows marks order filter hidden)
  ibuffer-state?
  ;; The sections, which remember what is folded.
  (view ibuffer-state-view)
  ;; The rows listed, in the order shown, those of folded groups too.
  (rows ibuffer-state-rows %ibuffer-set-rows!)
  ;; The mark on each marked buffer, by name: #\> or #\D.
  (marks ibuffer-state-marks)
  ;; 'recent, 'name, 'size or 'mode.
  (order ibuffer-state-order %ibuffer-set-order!)
  (filter ibuffer-state-filter %ibuffer-set-filter!)
  (hidden ibuffer-state-hidden? %ibuffer-set-hidden!))

(define %ibuffer-buffer "*Ibuffer*")

;; There is one buffer list; this is what it knows, or #f before the first.
(define %ibuffer-state #f)

;; The column that the names start in: an indent of two under the arrow
;; of the group, the mark, the two flags and a space.
(define %ibuffer-name-column 6)

(define (ibuffer-state)
  "What the buffer list knows: its rows, marks and order. It is an error
if the current buffer is not the buffer list."
  (if (and %ibuffer-state (eq? (major-mode) 'ibuffer-mode))
      %ibuffer-state
      (error "This buffer is not the buffer list")))

(define (ibuffer-rows)
  "The rows of the buffer list as shown, those in folded groups too."
  (ibuffer-state-rows (ibuffer-state)))

;;; Collecting

;; What is known about a buffer, looked at from inside it.
(define (%ibuffer-row name recency)
  (with-current-buffer name
    (let* ((file (buffer-file-name))
           (mode (major-mode))
           (root (and (or file (memq mode (option 'ibuffer-project-modes)))
                      (project-root))))
      (%make-ibuffer-row name file mode (point-max) (buffer-modified?)
                         (buffer-read-only?)
                         (cond [root root] [file 'files] [else 'other])
                         recency))))

(define (%ibuffer-matches? row wanted)
  (or (string-contains? (string-downcase (ibuffer-row-name row)) wanted)
      (and (ibuffer-row-file row)
           (string-contains? (string-downcase (ibuffer-row-file row)) wanted))))

;; The rows to list, in the order of use.
(define (%ibuffer-collect state)
  (let* ((wanted (and (ibuffer-state-filter state)
                      (string-downcase (ibuffer-state-filter state))))
         (names (filter (lambda (name)
                          (and (not (equal? name %ibuffer-buffer))
                               (or (ibuffer-state-hidden? state)
                                   (not (string-prefix? " " name)))))
                        (buffer-list)))
         (rows (map %ibuffer-row names (iota (length names)))))
    (if wanted
        (filter (lambda (row) (%ibuffer-matches? row wanted)) rows)
        rows)))

(define (%ibuffer-less order)
  (case order
    [(name) (lambda (a b) (string-ci<? (ibuffer-row-name a) (ibuffer-row-name b)))]
    [(size) (lambda (a b) (> (ibuffer-row-size a) (ibuffer-row-size b)))]
    [(mode) (lambda (a b) (string<? (symbol->string (ibuffer-row-mode a))
                                    (symbol->string (ibuffer-row-mode b))))]
    [else (lambda (a b) (< (ibuffer-row-recency a) (ibuffer-row-recency b)))]))

;; The rows in groups: pairs of a group and its rows in order. The projects
;; come in the order of their latest buffer, then the files in no project,
;; then the rest.
(define (%ibuffer-groups state rows)
  (let ((table (make-hash-table))
        (less? (%ibuffer-less (ibuffer-state-order state))))
    (for-each
     (lambda (row)
       (hash-table-update!/default table (ibuffer-row-group row)
                                   (lambda (rows) (cons row rows)) '()))
     rows)
    (let ((keys (hash-table-keys table)))
      (map (lambda (key)
             (cons key (list-sort less? (reverse (hash-table-ref table key)))))
           (append (filter string? keys)
                   (filter (lambda (key) (eq? key 'files)) keys)
                   (filter (lambda (key) (eq? key 'other)) keys))))))

;;; Writing the buffer

(define (%ibuffer-size chars)
  (cond [(< chars 10000) (number->string chars)]
        [(< chars 1000000)
         (string-append (number->string (exact (round (/ chars 1000.0)))) "k")]
        [else (string-append (number->string (/ (round (/ chars 100000.0)) 10)) "M")]))

(define (%ibuffer-mode-label mode)
  (let ((text (symbol->string mode)))
    (if (string-suffix? "-mode" text)
        (substring text 0 (- (string-length text) 5))
        text)))

;; The file of a row as shown: within its project, or abbreviated.
(define (%ibuffer-file-label row)
  (let ((file (ibuffer-row-file row))
        (group (ibuffer-row-group row)))
    (cond [(not file) ""]
          [(and (string? group) (string-prefix? (string-append group "/") file))
           (substring file (+ (string-length group) 1) (string-length file))]
          [else (abbreviate-file-name file)])))

(define (%ibuffer-group-label group)
  (cond [(string? group) (abbreviate-file-name group)]
        [(eq? group 'files) "Files outside projects"]
        [else "Other buffers"]))

(define (%ibuffer-count n word)
  (string-append (number->string n) " " word (if (= n 1) "" "s")))

(define (%ibuffer-mark-of state name)
  (hash-table-ref (ibuffer-state-marks state) name))

(define (%ibuffer-marked-count state rows)
  (length (filter (lambda (row) (eqv? (%ibuffer-mark-of state (ibuffer-row-name row)) #\>))
                  rows)))

;; The first line: how many buffers, how many unsaved and marked, and
;; whether the list is narrowed.
(define (%ibuffer-summary state rows)
  (let ((unsaved (length (filter ibuffer-row-modified? rows)))
        (marked (%ibuffer-marked-count state rows))
        (filter (ibuffer-state-filter state)))
    (list (cons (%ibuffer-count (length rows) "buffer") 'ibuffer-header)
          (cons (string-append
                 (if (> unsaved 0) (string-append ", " (number->string unsaved) " unsaved") "")
                 (if (> marked 0) (string-append ", " (number->string marked) " marked") "")
                 (if filter (string-append "  (narrowed to " filter ")") ""))
                'ibuffer-detail))))

(define (%ibuffer-group-spans state group rows)
  (let ((marked (%ibuffer-marked-count state rows)))
    (list (cons (%ibuffer-group-label group) 'ibuffer-group)
          (cons (string-append "  " (number->string (length rows))
                               (if (> marked 0)
                                   (string-append ", " (number->string marked) " marked")
                                   ""))
                'ibuffer-detail))))

;; The line of a buffer, as (text . face) pairs.
(define (%ibuffer-row-spans state row name-width mode-width)
  (let* ((name (ibuffer-row-name row))
         (mark (%ibuffer-mark-of state name))
         (name-face (cond [(eqv? mark #\>) 'ibuffer-marked]
                          [(eqv? mark #\D) 'ibuffer-flagged]
                          [else #f]))
         (shown (if (> (string-length name) name-width)
                    (string-append (substring name 0 (- name-width 1)) "…")
                    name))
         (modified? (ibuffer-row-modified? row)))
    (list (cons "  " #f)
          (cons (if mark (string mark) " ")
                (and mark (if (eqv? mark #\D) 'ibuffer-flag 'ibuffer-mark)))
          (cons (if modified? "*" " ") (and modified? 'ibuffer-modified))
          (cons (if (ibuffer-row-read-only? row) "%" " ") 'ibuffer-detail)
          (cons " " #f)
          (cons shown name-face)
          (cons (make-string (- name-width (string-length shown)) #\space) #f)
          (cons (string-append
                 "  " (string-pad-left (%ibuffer-size (ibuffer-row-size row)) 5)
                 "  " (string-pad-right (%ibuffer-mode-label (ibuffer-row-mode row)) mode-width)
                 "  " (%ibuffer-file-label row))
                'ibuffer-detail))))

(define (%ibuffer-sections state groups)
  (let* ((rows (append-map cdr groups))
         (widest (lambda (f least)
                   (fold (lambda (row most) (max most (string-length (f row)))) least rows)))
         (name-width (min 36 (widest ibuffer-row-name 8)))
         (mode-width (widest (lambda (row) (%ibuffer-mode-label (ibuffer-row-mode row))) 4)))
    (cons (make-section 'summary (%ibuffer-summary state rows) '() #f)
          (map (lambda (group)
                 (make-section (car group)
                               (%ibuffer-group-spans state (car group) (cdr group))
                               (map (lambda (row)
                                      (make-section (ibuffer-row-name row)
                                                    (%ibuffer-row-spans state row name-width mode-width)
                                                    '()
                                                    (ibuffer-row-name row)))
                                    (cdr group))
                               (list 'group (car group))))
               groups))))

;; The lines that have a buffer on them: pairs of the line number and the
;; buffer's name, in order. Lines of folded groups are not there.
(define (%ibuffer-row-lines state)
  (let ((chains (section-view-chains (ibuffer-state-view state))))
    (let loop ((index (- (vector-length chains) 1)) (lines '()))
      (if (< index 0)
          lines
          (let* ((chain (vector-ref chains index))
                 (value (and (pair? chain) (section-value (car chain)))))
            (loop (- index 1)
                  (if (string? value)
                      (cons (cons (+ index 1) value) lines)
                      lines)))))))

;; Puts point on the name, if the line has a buffer on it.
(define (%ibuffer-goto-name state)
  (when (string? (%ibuffer-value-at-point state))
    (goto-char (+ (line-beginning-position) %ibuffer-name-column))))

;; What the line point is on stands for: a buffer's name, (group key)
;; for the heading of a group, or #f on the first line.
(define (%ibuffer-value-at-point state)
  (let ((section (section-at-point (ibuffer-state-view state))))
    (and section (section-value section))))

(define (ibuffer-refresh . target)
  "Lists the buffers again in the buffer list, keeping the marks and what
is folded. Point stays on the buffer it was on, or goes to the one named
by the optional argument."
  (let* ((state (ibuffer-state))
         (marks (ibuffer-state-marks state))
         (was-on-buffer? (string? (%ibuffer-value-at-point state))))
    ;; Marks on buffers that are gone go with them.
    (for-each (lambda (name)
                (unless (member name (buffer-list)) (hash-table-delete! marks name)))
              (hash-table-keys marks))
    (let* ((groups (%ibuffer-groups state (%ibuffer-collect state)))
           (rows (append-map cdr groups)))
      (%ibuffer-set-rows! state rows)
      (section-view-render! (ibuffer-state-view state) (%ibuffer-sections state groups))
      (let ((lines (%ibuffer-row-lines state)))
        (cond
          [(and (pair? target)
                (find (lambda (line) (equal? (cdr line) (car target))) lines))
           => (lambda (line) (goto-line (car line)))]
          ;; The first line says how many there are; a buffer is a better
          ;; place to start. A buffer that is gone from under point leaves
          ;; point on the heading of its group: the next buffer is better.
          [(and (pair? lines)
                (or (= (line-number-at-pos) 1)
                    (and was-on-buffer? (not (string? (%ibuffer-value-at-point state))))))
           (let ((here (line-number-at-pos)))
             (goto-line (car (or (find (lambda (line) (> (car line) here)) lines)
                                 (last lines)))))]))
      (%ibuffer-goto-name state))))

;;; Opening

(define-major-mode ibuffer-mode special-mode "Ibuffer"
  "Major mode of the buffer list: the buffers grouped by project, with keys
that mark them and act on the marked ones. Start it with C-x C-b.")

(define-command (ibuffer)
  "List the buffers, grouped by project, with point on the current one.
m marks, D kills, S saves the marked buffers; RET switches to the buffer
on the line, TAB folds a project, / narrows the list and s changes the
order."
  (let ((from (buffer-name)))
    (switch-to-buffer %ibuffer-buffer)
    ;; A buffer that was killed behind our back comes back empty and in
    ;; another mode; what was known about it is then no longer true.
    (unless (and %ibuffer-state (eq? (major-mode) 'ibuffer-mode))
      (set! %ibuffer-state
            (%make-ibuffer-state (make-section-view) '() (make-hash-table)
                                 'recent #f (option 'ibuffer-show-hidden)))
      (ibuffer-mode)
      (set-buffer-read-only! #t)
      (enable-mode! 'hl-line-mode))
    (ibuffer-refresh from)))

;;; The buffers on a line

(define (ibuffer-buffer-at-point)
  "The name of the buffer on the line that point is on in the buffer list.
It is an error if there is none."
  (let ((value (%ibuffer-value-at-point (ibuffer-state))))
    (if (string? value) value (error "There is no buffer on this line"))))

(define (ibuffer-row-at-point)
  "The row of the buffer on the line that point is on in the buffer list."
  (let ((name (ibuffer-buffer-at-point)))
    (find (lambda (row) (equal? (ibuffer-row-name row) name)) (ibuffer-rows))))

;; The buffers that the line point is on stands for: the one on it, or
;; those of the group whose heading it is. It is an error on the first
;; line.
(define (%ibuffer-buffers-at-point)
  (let* ((state (ibuffer-state))
         (value (%ibuffer-value-at-point state)))
    (cond [(string? value) (list value)]
          [(pair? value)
           (map ibuffer-row-name
                (filter (lambda (row) (equal? (ibuffer-row-group row) (cadr value)))
                        (ibuffer-state-rows state)))]
          [else (error "There is no buffer on this line")])))

(define (ibuffer-marked-buffers)
  "The names of the buffers marked with >, in the order listed; if none is
marked, the buffer on the line that point is on, or the buffers of the
group whose heading it is."
  (let* ((state (ibuffer-state))
         (marked (filter (lambda (name) (eqv? (%ibuffer-mark-of state name) #\>))
                         (map ibuffer-row-name (ibuffer-state-rows state)))))
    (if (null? marked) (%ibuffer-buffers-at-point) marked)))

(define (%ibuffer-describe names)
  (if (null? (cdr names))
      (car names)
      (%ibuffer-count (length names) "buffer")))

;;; Moving about

(define-command (ibuffer-visit)
  "Switch to the buffer on this line."
  (switch-to-buffer (ibuffer-buffer-at-point)))

(define-command (ibuffer-visit-other-window)
  "Show the buffer on this line in another window."
  (let ((name (ibuffer-buffer-at-point)))
    (select-other-window)
    (switch-to-buffer name)))

(define-command (ibuffer-next-line)
  "Move to the next buffer, over the headings of groups."
  (let* ((state (ibuffer-state))
         (here (line-number-at-pos))
         (next (find (lambda (line) (> (car line) here)) (%ibuffer-row-lines state))))
    (when next
      (goto-line (car next))
      (%ibuffer-goto-name state))))

(define-command (ibuffer-previous-line)
  "Move to the previous buffer, over the headings of groups."
  (let* ((state (ibuffer-state))
         (here (line-number-at-pos))
         (previous (find (lambda (line) (< (car line) here))
                         (reverse (%ibuffer-row-lines state)))))
    (when previous
      (goto-line (car previous))
      (%ibuffer-goto-name state))))

(define-command (ibuffer-toggle-group)
  "Fold the group that this line belongs to, or open it."
  (let ((state (ibuffer-state)))
    (unless (section-view-toggle! (ibuffer-state-view state))
      (error "There is no group here to fold"))
    (%ibuffer-goto-name state)))

(define-command (ibuffer-revert)
  "List the buffers again, showing all of them."
  (%ibuffer-set-filter! (ibuffer-state) #f)
  (ibuffer-refresh))

(define-command (ibuffer-quit)
  "Leave the buffer list."
  (ibuffer-state)
  (set! %ibuffer-state #f)
  (kill-buffer %ibuffer-buffer))

(define-command (ibuffer-narrow)
  "Show only the buffers whose names or files contain a text. g shows all
again."
  (let ((text (read-string "Narrow to: ")))
    (%ibuffer-set-filter! (ibuffer-state) (if (string-null? text) #f text))
    (ibuffer-refresh)))

(define-command (ibuffer-toggle-hidden)
  "Show or hide the buffers whose names start with a space, which the
editor keeps for itself."
  (let ((state (ibuffer-state)))
    (%ibuffer-set-hidden! state (not (ibuffer-state-hidden? state)))
    (ibuffer-refresh)))

(define-command (ibuffer-sort-toggle)
  "List each group by last use, then by name, then by size with the
largest first, then by mode."
  (let* ((state (ibuffer-state))
         (next (case (ibuffer-state-order state)
                 [(recent) 'name]
                 [(name) 'size]
                 [(size) 'mode]
                 [else 'recent])))
    (%ibuffer-set-order! state next)
    (ibuffer-refresh)
    (message "Sorted by " (if (eq? next 'recent) "last use" (symbol->string next)))))

(define-command (ibuffer-copy-name)
  "Put the files of the marked buffers, or of this one, on the kill ring;
for a buffer without a file, its name."
  (let ((names (string-join (map (lambda (name)
                                   (let ((file (buffer-file-name name)))
                                     (if file (abbreviate-file-name file) name)))
                                 (ibuffer-marked-buffers))
                            " ")))
    (kill-new names)
    (message names)))

;;; Marks

;; Marks the buffers of the line point is on, or takes their mark off
;; with #f, and moves on to the next buffer when the line held one.
(define (%ibuffer-set-mark! mark)
  (let* ((state (ibuffer-state))
         (marks (ibuffer-state-marks state))
         (one? (string? (%ibuffer-value-at-point state))))
    (for-each (lambda (name)
                (if mark
                    (hash-table-set! marks name mark)
                    (hash-table-delete! marks name)))
              (%ibuffer-buffers-at-point))
    (ibuffer-refresh)
    (when one? (ibuffer-next-line))))

(define-command (ibuffer-mark)
  "Mark the buffer on this line, or the buffers of the group whose heading
it is, for the commands that act on several."
  (%ibuffer-set-mark! #\>))

(define-command (ibuffer-unmark)
  "Take the mark or the flag off the buffer on this line, or off the
buffers of the group whose heading it is."
  (%ibuffer-set-mark! #f))

(define-command (ibuffer-flag-for-kill)
  "Flag the buffer on this line to be killed. ibuffer-do-kill-flagged kills
the flagged buffers."
  (%ibuffer-set-mark! #\D))

(define-command (ibuffer-unmark-all)
  "Take every mark and flag off."
  (hash-table-clear! (ibuffer-state-marks (ibuffer-state)))
  (ibuffer-refresh))

(define-command (ibuffer-toggle-marks)
  "Mark the buffers that are not marked and unmark those that are."
  (let* ((state (ibuffer-state))
         (marks (ibuffer-state-marks state)))
    (for-each
     (lambda (row)
       (let ((name (ibuffer-row-name row)))
         (if (hash-table-contains? marks name)
             (hash-table-delete! marks name)
             (hash-table-set! marks name #\>))))
     (ibuffer-state-rows state))
    (ibuffer-refresh)))

(define-command (ibuffer-mark-by)
  "Mark the buffers of a kind: the unsaved ones, those in the mode of the
one on this line, those with or without a file, or those whose names
contain a text."
  (let* ((state (ibuffer-state))
         (marks (ibuffer-state-marks state))
         (choice (car (key-menu "Mark the buffers that are" '()
                                '(("u" "Unsaved" unsaved)
                                  ("m" "In the mode of this one" mode)
                                  ("f" "Visiting a file" file)
                                  ("s" "Visiting no file" special)
                                  ("n" "Named with a text" name)))))
         (wanted?
          (case choice
            [(unsaved) ibuffer-row-modified?]
            [(mode) (let ((mode (ibuffer-row-mode (ibuffer-row-at-point))))
                      (lambda (row) (eq? (ibuffer-row-mode row) mode)))]
            [(file) ibuffer-row-file]
            [(special) (lambda (row) (not (ibuffer-row-file row)))]
            [(name) (let ((text (string-downcase (read-string "Names that contain: "))))
                      (lambda (row)
                        (string-contains? (string-downcase (ibuffer-row-name row)) text)))]))
         (rows (filter wanted? (ibuffer-state-rows state))))
    (for-each (lambda (row) (hash-table-set! marks (ibuffer-row-name row) #\>)) rows)
    (ibuffer-refresh)
    (message "Marked " (%ibuffer-count (length rows) "buffer"))))

;;; Acting on buffers

;; Kills buffers after asking once, and again for each that has changes
;; to a file that are not saved.
(define (%ibuffer-kill names)
  (when (y-or-n? (string-append "Kill " (%ibuffer-describe names) "?"))
    (let ((killed
           (filter
            (lambda (name)
              (and (member name (buffer-list))
                   (or (not (and (buffer-file-name name) (buffer-modified? name)))
                       (y-or-n? (string-append name " has unsaved changes; kill it anyway?")))
                   (begin (kill-buffer name) #t)))
            names)))
      (ibuffer-refresh)
      (message "Killed " (%ibuffer-count (length killed) "buffer")))))

(define-command (ibuffer-do-kill)
  "Kill the marked buffers, or the buffer on this line, after asking."
  (%ibuffer-kill (ibuffer-marked-buffers)))

(define-command (ibuffer-do-kill-flagged)
  "Kill the buffers flagged with d, after asking."
  (let* ((state (ibuffer-state))
         (flagged (filter (lambda (name) (eqv? (%ibuffer-mark-of state name) #\D))
                          (map ibuffer-row-name (ibuffer-state-rows state)))))
    (if (null? flagged)
        (error "No buffers are flagged to be killed")
        (%ibuffer-kill flagged))))

(define-command (ibuffer-do-save)
  "Save the marked buffers, or the buffer on this line: those of them that
have unsaved changes to a file."
  (let ((names (filter (lambda (name)
                         (and (buffer-file-name name) (buffer-modified? name)))
                       (ibuffer-marked-buffers))))
    (if (null? names)
        (message "Nothing here needs saving")
        (begin
          ;; As C-x s saves: with the hooks and the formatter, and without
          ;; the question about the file having changed on disk.
          (for-each (lambda (name) (%save-buffer name (buffer-file-name name) #f))
                    names)
          (ibuffer-refresh)
          (message "Saved " (%ibuffer-describe names))))))

;;; The pointer

;; Puts point where the button was pressed, on the name if the line has a
;; buffer. Returns 'buffer, 'group or #f for what is there.
(define (%ibuffer-goto-press)
  (let* ((state (ibuffer-state))
         (press (mouse-press-position)))
    (deactivate-mark!)
    (when press (goto-char (cadr press)))
    (let ((value (%ibuffer-value-at-point state)))
      (cond [(string? value) (%ibuffer-goto-name state) 'buffer]
            [(pair? value) 'group]
            [else #f]))))

(define-command (ibuffer-mouse-select)
  "Go to the buffer that the pointer is at; on the heading of a group, fold
the group or open it."
  (when (eq? (%ibuffer-goto-press) 'group)
    (ibuffer-toggle-group)))

(define-command (ibuffer-mouse-visit)
  "Switch to the buffer that the pointer is at."
  (when (eq? (%ibuffer-goto-press) 'buffer)
    (ibuffer-visit)))

(define-command (ibuffer-mouse-visit-other-window)
  "Show the buffer that the pointer is at in another window."
  (when (eq? (%ibuffer-goto-press) 'buffer)
    (ibuffer-visit-other-window)))

(define-command (ibuffer-mouse-toggle-mark)
  "Mark the buffer that the pointer is at, or take its mark off."
  (when (%ibuffer-goto-press)
    (let* ((state (ibuffer-state))
           (marks (ibuffer-state-marks state))
           (names (%ibuffer-buffers-at-point))
           (all-marked? (every (lambda (name) (hash-table-contains? marks name)) names)))
      (for-each (lambda (name)
                  (if all-marked?
                      (hash-table-delete! marks name)
                      (hash-table-set! marks name #\>)))
                names)
      (ibuffer-refresh))))

(define-command (ibuffer-mouse-menu)
  "Show what can be done with the buffer that the pointer is at, or with
the marked buffers if it is one of them."
  (when (eq? (%ibuffer-goto-press) 'buffer)
    (let* ((name (ibuffer-buffer-at-point))
           (marked (ibuffer-marked-buffers))
           (choice (key-menu (%ibuffer-describe (if (member name marked) marked (list name)))
                             '()
                             '(("o" "Switch to it" ibuffer-visit)
                               ("O" "Show it in another window" ibuffer-visit-other-window)
                               ("m" "Mark or unmark" ibuffer-mouse-toggle-mark)
                               ("s" "Save" ibuffer-do-save)
                               ("k" "Kill" ibuffer-do-kill)
                               ("w" "Copy the name" ibuffer-copy-name)))))
      ((global-ref (car choice))))))

;;; Keys

(for-each
 (lambda (binding) (define-key 'ibuffer-mode (car binding) (cadr binding)))
 '(("RET" ibuffer-visit) ("f" ibuffer-visit) ("e" ibuffer-visit)
   ("o" ibuffer-visit-other-window)
   ("n" ibuffer-next-line) ("p" ibuffer-previous-line) ("SPC" ibuffer-next-line)
   ("<down>" ibuffer-next-line) ("<up>" ibuffer-previous-line)
   ("C-n" ibuffer-next-line) ("C-p" ibuffer-previous-line)
   ("TAB" ibuffer-toggle-group) ("g" ibuffer-revert) ("q" ibuffer-quit)
   ("/" ibuffer-narrow) ("." ibuffer-toggle-hidden) ("s" ibuffer-sort-toggle)
   ("w" ibuffer-copy-name)
   ("m" ibuffer-mark) ("u" ibuffer-unmark) ("U" ibuffer-unmark-all)
   ("t" ibuffer-toggle-marks) ("*" ibuffer-mark-by)
   ("d" ibuffer-flag-for-kill) ("x" ibuffer-do-kill-flagged)
   ("D" ibuffer-do-kill) ("k" ibuffer-do-kill) ("S" ibuffer-do-save)))

(for-each
 (lambda (binding) (define-key 'ibuffer-mode (car binding) (cadr binding)))
 '(("<mouse-1>" ibuffer-mouse-select) ("<triple-mouse-1>" ibuffer-mouse-select)
   ("<double-mouse-1>" ibuffer-mouse-visit)
   ("<mouse-2>" ibuffer-mouse-visit-other-window)
   ("C-<mouse-1>" ibuffer-mouse-toggle-mark)
   ("<mouse-3>" ibuffer-mouse-menu)))

(global-set-key "C-x C-b" 'ibuffer)
