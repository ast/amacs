;;; Org: notes under headings, written as plain text. This is the part of
;;; Org that edits such a file: its headings and what is under them, the
;;; states of headings, lists, tables, links and dates. How Org is coloured
;;; is in crates/amacs-treesit/src/org.rs.
;;;
;;; Most of the work is done by libraries that Markdown uses too: outlines
;;; (outline.scm), tables and lists (prose.scm) and links (links.scm). What
;;; is here is what Org writes in its own way, and the places where more of
;;; Org can be added without changes to this file:
;;;
;;;   org-parse-heading, org-headings-in   read Org from a string, for code
;;;                                        that reads files it does not show
;;;   org-element-at                       says what is at a position
;;;   define-org-action                    gives C-c C-c more to do, as
;;;                                        org-src.scm does to run blocks
;;;   define-link-type                     adds a kind of link
;;;   org-mode-hook                        runs in every buffer of Org
;;;
;;; The words that the user sees are in Simplified Technical English.

(define-option 'org-startup-folded #f
  "How a file of Org is folded when it is visited: #f shows all of it,
'overview shows its headings of the highest level, and 'content shows all
its headings without the text under them. A line #+STARTUP: in the file
with the word overview, content or showall says it for that file.")

(define-option 'org-todo-keywords '(("TODO") ("DONE"))
  "The words for the states of a heading in Org: a list of the words for
what is still to do and a list of the words for what is done, as in
'((\"TODO\" \"WAITING\") (\"DONE\" \"CANCELLED\")). C-c C-t goes through
them in this order.")

;;; Reading Org

(define (%org-states)
  (apply append (option 'org-todo-keywords)))

(define (org-parse-heading line . states)
  "The parts of a line that is a heading of Org, or #f for another line: a
list of the level, the word for its state or #f, the title, and the tags
as a list of strings. The words that are states are those of the
org-todo-keywords option, or the list given."
  (let ((match (regexp-search "^(\\*+) (.*)$" line)))
    (and match
         (let* ((states (if (pair? states) (car states) (%org-states)))
                (level (cadr (cadr match)))
                (rest (apply substring line (caddr match)))
                (tagged (regexp-search "(?:^|[ \\t]+)(:[\\w@#%:]+:)[ \\t]*$" rest))
                (tags (if tagged
                          (filter (lambda (tag) (not (string-null? tag)))
                                  (string-split (apply substring rest (cadr tagged)) #\:))
                          '()))
                (rest (string-trim (if tagged (substring rest 0 (car (car tagged))) rest)))
                (blank (string-search rest " "))
                (word (if blank (substring rest 0 blank) rest))
                (state (and (member word states) word)))
           (list level
                 state
                 (if state
                     (string-trim (substring rest (string-length word) (string-length rest)))
                     rest)
                 tags)))))

(define (org-headings-in text)
  "The headings of a text of Org, in order: a list of lists of the
position of the line, the level from 1, and the text of the heading with
its state and without its tags."
  (let loop ((lines (string-split text #\newline)) (pos 0) (found '()))
    (if (null? lines)
        (reverse found)
        (let* ((line (car lines))
               (next (+ pos (string-length line) 1))
               (parts (and (string-prefix? "*" line) (org-parse-heading line))))
          (loop (cdr lines) next
                (if parts
                    (cons (list pos (car parts)
                                (if (cadr parts)
                                    (string-append (cadr parts) " " (caddr parts))
                                    (caddr parts)))
                          found)
                    found))))))

(define (org-headings)
  "The headings of the current buffer, as org-headings-in gives them."
  (org-headings-in (buffer-string)))

(define (%org-line pos)
  (buffer-substring (line-beginning-position pos) (line-end-position pos)))

;; The block that a position is in, from its #+begin_ line to its #+end_
;; line, or #f: a list of the word block, where it starts, where it ends,
;; its name in small letters and what follows the name on its first line.
(define (%org-block-around pos)
  (let loop ((lines (string-split (buffer-string) #\newline)) (start 0) (open #f))
    (and (pair? lines)
         (or open (<= start pos))
         (let* ((line (car lines))
                (end (+ start (string-length line)))
                (begins (regexp-search "(?i)^[ \\t]*#\\+begin_(\\S+)[ \\t]*(.*)$" line))
                (ends (and open (regexp-search "(?i)^[ \\t]*#\\+end_(\\S+)" line))))
           (cond
            [(and ends (equal? (string-downcase (apply substring line (cadr ends)))
                               (cadr open)))
             (if (<= (car open) pos end)
                 (list 'block (car open) end (cadr open) (caddr open))
                 (loop (cdr lines) (+ end 1) #f))]
            ;; A block does not reach past a heading.
            [(and open (string-prefix? "*" line) (org-parse-heading line))
             (loop (cdr lines) (+ end 1) #f)]
            [(and begins (not open))
             (loop (cdr lines) (+ end 1)
                   (list start
                         (string-downcase (apply substring line (cadr begins)))
                         (string-trim (apply substring line (caddr begins)))))]
            [else (loop (cdr lines) (+ end 1) open)])))))

(define (org-element-at . pos)
  "What the line of a position (default point) is in a buffer of Org, as a
list that starts with the kind:
  (block start end name rest)   a block from #+begin_ to #+end_: its name
                                in small letters, such as \"src\", and what
                                follows the name, such as the language
  (heading start level state title tags)
  (table start end)
  (item start blanks marker box text)
  (keyword start name value)    a line such as #+title: Notes
  (paragraph start)             any other line"
  (let* ((pos (if (pair? pos) (car pos) (point)))
         (start (line-beginning-position pos))
         (line (%org-line pos)))
    (cond
     [(%org-block-around pos) => (lambda (block) block)]
     [(org-parse-heading line) => (lambda (parts) (cons 'heading (cons start parts)))]
     [(table-line? line)
      (let ((bounds (save-excursion (goto-char pos) (table-bounds))))
        (list 'table (car bounds) (cdr bounds)))]
     [(list-item-parts line) => (lambda (parts) (cons 'item (cons start parts)))]
     [(regexp-search "^[ \\t]*#\\+([A-Za-z_]+):[ \\t]*(.*)$" line)
      => (lambda (match)
           (list 'keyword start
                 (string-downcase (apply substring line (cadr match)))
                 (string-trim (apply substring line (caddr match)))))]
     [else (list 'paragraph start)])))

;;; What C-c C-c does

;; The procedures that act on each kind of element, the newest first.
(define %org-actions (make-hash-table))

(define (define-org-action kind action)
  "Gives C-c C-c something to do on a kind of element of Org, such as
'block or 'heading: a procedure that is given the element, as
org-element-at returns it, and returns #f if it did nothing. Of the
actions of a kind, the newest is tried first."
  (hash-table-set! %org-actions kind
                   (cons action (or (hash-table-ref %org-actions kind) '()))))

(define-org-action 'table (lambda (element) (table-align) #t))
(define-org-action 'item (lambda (element) (list-item-toggle-checkbox) #t))

(define-command (org-do)
  "Do what fits where point is: line up a table, or tick the box of an
item of a list."
  (let ((element (org-element-at)))
    (let loop ((actions (or (hash-table-ref %org-actions (car element)) '())))
      (cond [(null? actions) (message "There is nothing to do here")]
            [((car actions) element) #t]
            [else (loop (cdr actions))]))))

;;; Headings

(define-outline 'org-mode 'headings org-headings 'marker "*" 'max-level #f)

(define-command (org-meta-return)
  "Start a new item of a list after the item at point. Elsewhere, start a
new heading of the level of the heading that point is under, after all
that is under it."
  (let ((item (list-item-parts (%org-line (point))))
        (heading (outline-current-heading)))
    (cond
     [item
      (goto-char (line-end-position))
      (insert "\n" (car item) (%list-next-marker (cadr item)) " "
              (if (caddr item) "[ ] " ""))]
     [else
      (goto-char (if heading (outline-subtree-end heading) (line-end-position)))
      ;; The new heading gets a line of its own.
      (unless (= (point) (line-beginning-position))
        (insert "\n"))
      (insert (make-string (if heading (cadr heading) 1) #\*) " ")
      (unless (= (point) (line-end-position))
        (save-excursion (insert "\n")))])))

(define-command (org-metaleft)
  "On a heading, make it one level more important. Elsewhere, go back a
word."
  (if (outline-heading-at)
      (outline-promote)
      (call-command 'backward-word)))

(define-command (org-metaright)
  "On a heading, make it one level less important. Elsewhere, go forward a
word."
  (if (outline-heading-at)
      (outline-demote)
      (call-command 'forward-word)))

;;; States

(define-command (org-todo)
  "Give the heading that point is under its next state: from none to the
first word of the org-todo-keywords option, through the words, and to none
again."
  (let* ((heading (or (outline-current-heading)
                      (error "Point is not under a heading")))
         (start (car heading))
         (parts (org-parse-heading (%org-line start)))
         (level (car parts))
         (state (cadr parts))
         (states (%org-states))
         (next (if state
                   (let ((later (cdr (member state states))))
                     (and (pair? later) (car later)))
                   (and (pair? states) (car states))))
         (at (+ start level 1))
         (line-end (line-end-position start))
         (removed (if state (- (min line-end (+ at (string-length state) 1)) at) 0))
         (added (if next
                    (string-append next (if (= (+ at removed) line-end) "" " "))
                    ""))
         (here (point)))
    (delete-region at (+ at removed))
    (goto-char at)
    (insert added)
    ;; Point stays in the text that it was in.
    (goto-char (if (< here at)
                   here
                   (max (+ at (string-length added))
                        (+ here (- (string-length added) removed)))))
    (message (or next "No state"))))

;;; Dates and links

(define-command (org-time-stamp)
  "Insert the date of today as Org writes dates, such as <2026-11-26 Thu>."
  (insert (format-current-time "<%Y-%m-%d %a>")))

(define-command (org-insert-link)
  "Insert a link: its target is asked for, and then its text. If the
region is active, its text is the text of the link."
  (let* ((marked (and (region-active?)
                      (cons (min (point) (mark)) (max (point) (mark)))))
         (target (let ((target (read-string "Link to: ")))
                   (when (string-null? target)
                     (error "A link must have a target"))
                   target))
         (text (read-string "Text of the link: "
                            (if marked (buffer-substring (car marked) (cdr marked)) ""))))
    (when marked
      (delete-region (car marked) (cdr marked))
      (goto-char (car marked))
      (deactivate-mark!))
    (insert "[[" target (if (string-null? text) "" (string-append "][" text)) "]]")))

(define (org-link-at pos)
  "The link of Org that a position is in: a list of where it starts, where
it ends, its target and its text or #f; or #f. A link is [[target]] or
[[target][text]]."
  (let* ((start (line-beginning-position pos))
         (line (%org-line pos))
         (match (regexp-search-around "\\[\\[([^\\]]+)\\](?:\\[([^\\]]*)\\])?\\]"
                                      line (- pos start))))
    (and match
         (list (+ start (car (car match))) (+ start (cadr (car match)))
               (apply substring line (cadr match))
               (and (caddr match) (apply substring line (caddr match)))))))

;; The start of the first line of the buffer that a regular expression
;; matches, or #f.
(define (%org-line-matching regexp)
  (let ((match (regexp-search regexp (buffer-string))))
    (and match (line-beginning-position (car (car match))))))

;; The position of the first heading with a title, or #f.
(define (%org-heading-titled title)
  (let loop ((headings (org-headings)))
    (cond [(null? headings) #f]
          [(equal? title (caddr (org-parse-heading (%org-line (car (car headings))))))
           (car (car headings))]
          [else (loop (cdr headings))])))

;; A text with the chars that mean something in a regular expression made
;; plain.
(define (%org-regexp-quote text)
  (car (regexp-replace "[\\\\.+*?()|\\[\\]{}^$#&~-]" text "\\\\\\&")))

(define (org-link-place target)
  "The position that a target names in the current buffer of Org, or #f:
*Title is a heading, #name is the entry whose CUSTOM_ID property is the
name, and other text is a heading with that title or a place marked
<<text>>."
  (cond
   [(string-prefix? "*" target)
    (%org-heading-titled (string-trim (substring target 1 (string-length target))))]
   [(string-prefix? "#" target)
    (%org-line-matching
     (string-append "(?m)^[ \\t]*:CUSTOM_ID:[ \\t]+"
                    (%org-regexp-quote (substring target 1 (string-length target)))
                    "[ \\t]*$"))]
   [else
    (or (%org-heading-titled target)
        (%org-line-matching (string-append "<<" (%org-regexp-quote target) ">>")))]))

(define-links 'org-mode 'at-point org-link-at 'internal org-link-place)

;; A link by id leads to the entry of the current buffer whose ID property
;; is the id. Finding entries in other files is for an index to do.
(define-link-type "id"
  (lambda (id)
    (let ((found (%org-line-matching
                  (string-append "(?mi)^[ \\t]*:ID:[ \\t]+" (%org-regexp-quote id)
                                 "[ \\t]*$"))))
      (unless found
        (error (string-append "Found no entry with the id " id " in this buffer")))
      (set-mark! (point))
      (goto-char (let ((heading (outline-current-heading found)))
                   (if heading (car heading) found))))))

;;; Tables and lists

(define-prose-style 'org-mode 'rule 'plus 'tick "[X]" 'star-items? #f)

(define-command (org-cycle)
  "In a table, line it up and go to the next cell. On a heading, fold what
is under it, or show it again. Elsewhere, indent."
  (cond [(table-bounds) (table-next-cell)]
        [(outline-heading-at) (outline-cycle)]
        [else (call-command 'indent-for-tab-command)]))

(define-command (org-shifttab)
  "In a table, line it up and go to the cell before this one. Elsewhere,
fold the whole buffer to its headings, or show it again."
  (if (table-bounds)
      (table-previous-cell)
      (outline-cycle-buffer)))

;;; The mode

;; How a buffer of Org is folded when it is first shown: what its line
;; #+STARTUP: says, or else the org-startup-folded option.
(define (%org-startup)
  (let* ((text (buffer-string))
         (line (regexp-search "(?mi)^#\\+STARTUP:(.*)$" text))
         (words (if line (string-split (apply substring text (cadr line)) #\space) '())))
    (cond [(or (member "overview" words) (member "fold" words)) 'overview]
          [(member "content" words) 'content]
          [(or (member "showall" words) (member "showeverything" words)
               (member "nofold" words))
           #f]
          [else (option 'org-startup-folded)])))

(define (%org-setup)
  (let ((keywords (option 'org-todo-keywords)))
    (set-syntax-keywords! (car keywords) (cadr keywords)))
  (case (%org-startup)
    [(overview) (outline-fold-overview)]
    [(content) (outline-fold-contents)]))

(add-hook! 'org-mode-hook %org-setup)

(define-snippets 'org-mode
  '((src "#+begin_src " p n r n "#+end_src")
    (quote "#+begin_quote" n r n "#+end_quote")
    (example "#+begin_example" n r n "#+end_example")))

(for-each
 (lambda (binding) (define-key 'org-mode (car binding) (cadr binding)))
 '(("TAB" org-cycle) ("S-TAB" org-shifttab) ("RET" list-item-newline)
   ("M-RET" org-meta-return)
   ("C-c C-c" org-do) ("C-c |" table-insert)
   ("C-c C-t" org-todo) ("C-c ." org-time-stamp)
   ("C-c C-l" org-insert-link) ("C-c C-o" open-link-at-point)
   ("<mouse-2>" open-link-at-mouse) ("C-<mouse-1>" open-link-at-mouse)
   ("C-c C-n" outline-next-heading) ("C-c C-p" outline-previous-heading)
   ("C-c C-u" outline-up-heading)
   ("C-c C-f" outline-forward-same-level) ("C-c C-b" outline-backward-same-level)
   ("C-c C-j" outline-goto-heading)
   ("M-<left>" org-metaleft) ("M-<right>" org-metaright)
   ("M-S-<left>" outline-promote-subtree) ("M-S-<right>" outline-demote-subtree)
   ("M-<up>" outline-move-subtree-up) ("M-<down>" outline-move-subtree-down)))
