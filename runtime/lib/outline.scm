;;; Outlines: text under headings of levels, as Markdown and Org have.
;;; This file moves by headings, changes their levels, moves a heading
;;; with all that is under it, and folds away what is under a heading. How a heading is written is the business of
;;; each mode, which says so with define-outline.
;;;
;;; A heading is a list of the position of its line, its level from 1, and
;;; its text. The subtree of a heading is its line and all lines to the
;;; next heading that is not of a deeper level.
;;;
;;; The words that the user sees are in Simplified Technical English.

;;; What each mode knows of headings

(define %outline-modes (make-hash-table))

(define (define-outline mode . properties)
  "Says how headings are written in a major mode and in the modes derived
from it, as keys and values:
  'headings   a procedure of no arguments that returns the headings of the
              current buffer in order: lists of the position of the line,
              the level from 1, and the text.
  'marker     the string that a line of a heading starts with once for each
              level, as \"#\" in Markdown.
  'max-level  the deepest level that there is, or #f if there is none."
  (hash-table-set! %outline-modes mode properties))

(define (%outline-property key)
  (let ((entry (mode-lookup %outline-modes)))
    (unless entry
      (error "This mode has no headings"))
    (property-ref entry key)))

(define (outline-headings)
  "The headings of the current buffer, in order: a list of lists of the
position of the line, the level from 1, and the text. It is an error in a
mode that has no headings."
  ((%outline-property 'headings)))

(define (outline-heading-at . pos)
  "The heading that the line of a position (default point) is, or #f."
  (let ((start (line-beginning-position (if (pair? pos) (car pos) (point)))))
    (let loop ((headings (outline-headings)))
      (cond [(or (null? headings) (> (car (car headings)) start)) #f]
            [(= (car (car headings)) start) (car headings)]
            [else (loop (cdr headings))]))))

(define (outline-current-heading . pos)
  "The heading that the text at a position (default point) is under: the
last one that starts at or before the position, or #f if the position is
before all headings."
  (let ((pos (if (pair? pos) (car pos) (point))))
    (let loop ((headings (outline-headings)) (found #f))
      (if (or (null? headings) (> (car (car headings)) pos))
          found
          (loop (cdr headings) (car headings))))))

;; The headings after one, as outline-headings gives them.
(define (%outline-after heading)
  (let ((rest (member heading (outline-headings))))
    (if rest (cdr rest) '())))

(define (outline-subtree-end heading)
  "Where the subtree of a heading ends: the start of the next heading that
is not of a deeper level, or the end of the buffer."
  (let loop ((later (%outline-after heading)))
    (cond [(null? later) (point-max)]
          [(<= (cadr (car later)) (cadr heading)) (car (car later))]
          [else (loop (cdr later))])))

;; The heading at point, or an error.
(define (%outline-here)
  (or (outline-heading-at)
      (error "Point is not on a heading")))

;;; Moving

(define-command (outline-next-heading)
  "Go to the next heading."
  (let ((later (filter (lambda (heading) (> (car heading) (point))) (outline-headings))))
    (if (null? later)
        (message "No heading after this")
        (goto-char (car (car later))))))

(define-command (outline-previous-heading)
  "Go to the heading before point."
  (let ((earlier (filter (lambda (heading) (< (car heading) (line-beginning-position)))
                         (outline-headings))))
    (if (null? earlier)
        (message "No heading before this")
        (goto-char (car (last earlier))))))

;; The heading that one is under: the last before it of a higher level.
(define (%outline-parent heading)
  (let loop ((headings (outline-headings)) (found #f))
    (cond [(or (null? headings) (>= (car (car headings)) (car heading))) found]
          [(< (cadr (car headings)) (cadr heading)) (loop (cdr headings) (car headings))]
          [else (loop (cdr headings) found)])))

(define-command (outline-up-heading)
  "Go to the heading that this text is under: from text to its heading,
from a heading to the heading of a higher level above it."
  (let* ((here (outline-heading-at))
         (above (if here (%outline-parent here) (outline-current-heading))))
    (if above
        (goto-char (car above))
        (message "No heading above this"))))

;; The next heading of the level of one and under the same heading, or #f.
(define (%outline-next-sibling heading)
  (let loop ((later (%outline-after heading)))
    (cond [(or (null? later) (< (cadr (car later)) (cadr heading))) #f]
          [(= (cadr (car later)) (cadr heading)) (car later)]
          [else (loop (cdr later))])))

(define (%outline-previous-sibling heading)
  (let loop ((earlier (reverse (filter (lambda (other) (< (car other) (car heading)))
                                       (outline-headings)))))
    (cond [(or (null? earlier) (< (cadr (car earlier)) (cadr heading))) #f]
          [(= (cadr (car earlier)) (cadr heading)) (car earlier)]
          [else (loop (cdr earlier))])))

(define-command (outline-forward-same-level)
  "Go to the next heading of the level of this one, under the same
heading."
  (let ((next (%outline-next-sibling (or (outline-current-heading)
                                         (error "Point is before the first heading")))))
    (if next
        (goto-char (car next))
        (message "No heading of this level after this"))))

(define-command (outline-backward-same-level)
  "Go to the heading of the level of this one before it, under the same
heading."
  (let* ((here (or (outline-current-heading)
                   (error "Point is before the first heading")))
         (previous (%outline-previous-sibling here)))
    (cond [(< (car here) (line-beginning-position)) (goto-char (car here))]
          [previous (goto-char (car previous))]
          [else (message "No heading of this level before this")])))

(define-command (outline-goto-heading)
  "Go to a heading of the buffer, chosen by its text."
  (let* ((headings (outline-headings))
         (names (map (lambda (heading)
                       (string-append (make-string (* 2 (- (cadr heading) 1)) #\space)
                                      (caddr heading)))
                     headings)))
    (when (null? headings)
      (error "The buffer has no headings"))
    (let ((chosen (completing-read "Heading: " names)))
      (let loop ((headings headings) (names names))
        (cond [(null? headings) (error "No such heading:" chosen)]
              [(equal? (car names) chosen)
               (set-mark! (point))
               (goto-char (car (car headings)))]
              [else (loop (cdr headings) (cdr names))])))))

;;; Levels

;; Changes the level of headings by one step, down for a positive delta.
;; None is changed if one of them cannot be.
(define (%outline-shift headings delta)
  (let ((marker (%outline-property 'marker))
        (deepest (%outline-property 'max-level)))
    (for-each
     (lambda (heading)
       (let ((level (+ (cadr heading) delta)))
         (when (or (< level 1) (and deepest (> level deepest)))
           (error (if deepest
                      (string-append "Headings have levels 1 to " (number->string deepest))
                      "A heading of level 1 cannot go up")))))
     headings)
    (save-excursion
      ;; From the last one, so that the positions of the others stay.
      (for-each
       (lambda (heading)
         (goto-char (car heading))
         (if (> delta 0)
             (insert marker)
             (delete-region (point) (+ (point) (string-length marker)))))
       (reverse headings)))))

;; A heading and the headings under it.
(define (%outline-subtree heading)
  (let ((end (outline-subtree-end heading)))
    (cons heading
          (filter (lambda (other) (< (car other) end)) (%outline-after heading)))))

(define-command (outline-promote)
  "Make the heading at point one level more important."
  (%outline-shift (list (%outline-here)) -1))

(define-command (outline-demote)
  "Make the heading at point one level less important."
  (%outline-shift (list (%outline-here)) 1))

(define-command (outline-promote-subtree)
  "Make the heading at point and the headings under it one level more
important."
  (%outline-shift (%outline-subtree (%outline-here)) -1))

(define-command (outline-demote-subtree)
  "Make the heading at point and the headings under it one level less
important."
  (%outline-shift (%outline-subtree (%outline-here)) 1))

;;; Moving subtrees

;; Folds the headings at some positions again that were folded before
;; their text was moved: moving text shows it.
(define (%outline-fold-again positions folded)
  (for-each (lambda (pos folded?)
              (when folded?
                (outline-fold (outline-heading-at pos))))
            positions folded))

;; Makes two pieces of text that follow one another change places: the
;; first is from START to MIDDLE, the second from MIDDLE to END. Only the
;; last piece of a buffer can be without a newline at its end, and the
;; piece that comes last afterwards is then without one.
(define (%outline-swap start middle end)
  (let* ((first (buffer-substring start middle))
         (second (buffer-substring middle end))
         (open? (not (string-suffix? "\n" second)))
         (second (if open? (string-append second "\n") second))
         (first (if open? (substring first 0 (- (string-length first) 1)) first)))
    (delete-region start end)
    (goto-char start)
    (insert second first)
    (string-length second)))

(define-command (outline-move-subtree-down)
  "Move the heading that point is under, with all that is under it, after
the next heading of its level."
  (let* ((here (or (outline-current-heading)
                   (error "Point is before the first heading")))
         (next (or (%outline-next-sibling here)
                   (error "No heading of this level after this")))
         (offset (- (point) (car here)))
         (folded (map outline-folded? (list next here)))
         (moved (%outline-swap (car here) (car next) (outline-subtree-end next))))
    (%outline-fold-again (list (car here) (+ (car here) moved)) folded)
    (goto-char (+ (car here) moved offset))))

(define-command (outline-move-subtree-up)
  "Move the heading that point is under, with all that is under it, before
the heading of its level that is before it."
  (let* ((here (or (outline-current-heading)
                   (error "Point is before the first heading")))
         (previous (or (%outline-previous-sibling here)
                       (error "No heading of this level before this")))
         (offset (- (point) (car here)))
         (folded (map outline-folded? (list here previous)))
         (moved (%outline-swap (car previous) (car here) (outline-subtree-end here))))
    (%outline-fold-again (list (car previous) (+ (car previous) moved)) folded)
    (goto-char (+ (car previous) offset))))

;;; Folding
;;;
;;; What is under a heading can be folded away: its lines are hidden, with
;;; hide-lines!, and the heading shows a mark. Nothing is kept here of what
;;; is folded: the hidden lines themselves say it.

;; The lines under a heading, as a pair of the start of the first and the
;; end of its subtree, or #f if nothing is under it.
(define (%outline-body heading)
  (let ((start (+ (line-end-position (car heading)) 1))
        (end (outline-subtree-end heading)))
    (and (< start end) (cons start end))))

(define (outline-folded? heading)
  "Whether what is under a heading is folded away: its first line is
hidden."
  (let ((body (%outline-body heading)))
    (and body (hidden? (car body)))))

(define (outline-fold heading)
  "Folds away all that is under a heading."
  (let ((body (%outline-body heading)))
    (when body
      (hide-lines! (car body) (cdr body)))))

(define (outline-unfold heading)
  "Shows all that is under a heading, with what is folded under the
headings under it."
  (let ((body (%outline-body heading)))
    (when body
      (show-lines! (car body) (cdr body)))))

;; The headings directly under a heading.
(define (%outline-children heading)
  (let ((under (cdr (%outline-subtree heading))))
    (if (null? under)
        '()
        (let ((level (fold min (cadr (car under)) (map cadr under))))
          (filter (lambda (other) (= (cadr other) level)) under)))))

;; What the last step of folding left, to know the next step by: a list of
;; the buffer, what was stepped (the position of a heading, or the word
;; buffer), the state it came to, and the hidden lines after it. A step
;; goes on from that state only if all of these are as they were.
(define %outline-last-step #f)

(define (%outline-step! what state)
  (set! %outline-last-step (list (buffer-name) what state (hidden-ranges))))

(define (%outline-after-step? what state)
  (equal? %outline-last-step (list (buffer-name) what state (hidden-ranges))))

(define-command (outline-cycle)
  "Fold and unfold what is under the heading at point: fold it away; at
the next use show the headings under it, with their text folded; at the
next show all of it."
  (let* ((heading (%outline-here))
         (body (%outline-body heading))
         (under (cdr (%outline-subtree heading))))
    (cond
     [(not body) (message "Nothing is under this heading")]
     [(outline-folded? heading)
      ;; The fold of the heading itself: what is folded under it stays so.
      (show-lines! (car body) (car body))
      (for-each outline-fold (%outline-children heading))
      (%outline-step! (car heading) 'headings)
      (message (if (null? under) "All shown" "Headings shown"))]
     [(and (pair? under) (%outline-after-step? (car heading) 'headings))
      (outline-unfold heading)
      (message "All shown")]
     [else
      (outline-fold heading)
      (message "Folded")])))

(define (outline-fold-overview)
  "Folds the current buffer to its headings of the highest level: all that
is under them is hidden."
  (show-lines!)
  (let ((headings (outline-headings)))
    (unless (null? headings)
      (let ((top (fold min (cadr (car headings)) (map cadr headings))))
        (for-each (lambda (heading)
                    (when (= (cadr heading) top)
                      (outline-fold heading)))
                  headings)))))

(define (outline-fold-contents)
  "Folds the current buffer to its headings: all headings are shown, and
the text under each is hidden."
  (show-lines!)
  (let loop ((headings (outline-headings)))
    (unless (null? headings)
      (let ((start (+ (line-end-position (car (car headings))) 1))
            (end (if (null? (cdr headings)) (point-max) (car (cadr headings)))))
        (when (< start end)
          (hide-lines! start end)))
      (loop (cdr headings)))))

(define-command (outline-cycle-buffer)
  "Fold and unfold the whole buffer: from all shown to the headings of the
highest level, then to all headings, then to all shown again."
  (cond
   [(%outline-after-step? 'buffer 'overview)
    (outline-fold-contents)
    (%outline-step! 'buffer 'contents)
    (message "All headings")]
   [(%outline-after-step? 'buffer 'contents)
    (show-lines!)
    (message "All shown")]
   [else
    (outline-fold-overview)
    (%outline-step! 'buffer 'overview)
    (message "Headings of the highest level")]))

(define-command (outline-show-all)
  "Show all lines of the buffer."
  (show-lines!))
