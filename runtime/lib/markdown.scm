;;; Editing Markdown: moving by headings, and links that are followed.
;;; Its tables and lists are those of runtime/lib/prose.scm, and its
;;; headings are moved and changed by runtime/lib/outline.scm. How Markdown
;;; is coloured is the business of its grammar, in crates/amacs-treesit.

;;; Tables and lists

(define %markdown-prose-style '(rule bars tick "[x]" star-items? #t))

(define-prose-style 'markdown-mode 'rule 'bars 'tick "[x]" 'star-items? #t)

(define (markdown-format-table lines)
  "Takes the lines of a table of Markdown and returns them with the cells
of each column made equally wide, and lined up as the row of dashes says:
`:--` left, `--:` right, `:-:` in the middle. Rows that are short of cells
get empty ones. Every line starts as the first one does."
  (table-format lines %markdown-prose-style))

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
  "In a table, line it up and go to the next cell. On a heading, fold what
is under it, or show it again. Elsewhere, indent."
  (cond [(table-bounds) (table-next-cell)]
        [(outline-heading-at) (outline-cycle)]
        [else (call-command 'indent-for-tab-command)]))

(define-command (markdown-backtab)
  "In a table, line it up and go to the cell before this one. Elsewhere,
fold the whole buffer to its headings, or show it again."
  (if (table-bounds)
      (table-previous-cell)
      (outline-cycle-buffer)))

(define-command (markdown-do)
  "Do what fits where point is: line up a table, or tick the box of an
item of a list."
  (if (table-bounds)
      (table-align)
      (list-item-toggle-checkbox)))

(for-each
 (lambda (binding) (define-key 'markdown-mode (car binding) (cadr binding)))
 '(("TAB" markdown-tab) ("S-TAB" markdown-backtab) ("RET" list-item-newline)
   ("C-c C-c" markdown-do) ("C-c |" table-insert)
   ("C-c C-n" outline-next-heading) ("C-c C-p" outline-previous-heading)
   ("C-c C-u" outline-up-heading)
   ("C-c C-f" outline-forward-same-level) ("C-c C-b" outline-backward-same-level)
   ("C-c C-j" outline-goto-heading)
   ("M-<left>" outline-promote) ("M-<right>" outline-demote)
   ("M-S-<left>" outline-promote-subtree) ("M-S-<right>" outline-demote-subtree)
   ("M-<up>" outline-move-subtree-up) ("M-<down>" outline-move-subtree-down)
   ("C-c C-o" open-link-at-point)
   ("<mouse-2>" open-link-at-mouse) ("C-<mouse-1>" open-link-at-mouse)))
