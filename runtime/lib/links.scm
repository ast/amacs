;;; Links: going where a piece of text points. A link has a target, such
;;; as https://example.org, notes/plan.md or a heading of the same file.
;;; This file knows how to go to a target. How a link is written is the
;;; business of each mode, which says so with define-links; a bare web
;;; address is a link in every mode. C-c C-o in Markdown and Org follows
;;; the link at point.
;;;
;;; The words that the user sees are in Simplified Technical English.

(define-option 'browse-url-program #f
  "The program that opens web addresses: it is given the address. #f is the
program of the dired-open-program option, which is xdg-open unless changed:
that hands the address to the browser of the desktop.")

(define (browse-url url)
  "Opens a web address, or another address that the desktop knows a program
for, with the program of the browse-url-program option."
  (start-process (or (option 'browse-url-program) (option 'dired-open-program))
                 (list url))
  (message "Opened " url))

;;; Kinds of link

;; The procedure that opens the links of each kind, by the name of the
;; kind: the word before the colon of a target.
(define %link-types (make-hash-table))

(define (define-link-type name opener)
  "Defines a kind of link: targets that start with the name and a colon, as
https in https://example.org. The opener is a procedure that is given the
rest of the target, after the colon, and goes there. A kind of the same
name is replaced."
  (hash-table-set! %link-types name opener))

;; The opener and the rest of a target that names a kind of link, as a
;; pair, or #f.
(define (%link-typed target)
  (let ((match (regexp-search "^([A-Za-z][A-Za-z0-9+.-]*):" target)))
    (and match
         (let* ((name (string-downcase (substring target 0 (- (cadr (car match)) 1))))
                (opener (hash-table-ref %link-types name)))
           (and opener
                (cons opener
                      (substring target (cadr (car match)) (string-length target))))))))

(for-each
 (lambda (name)
   (define-link-type name (lambda (rest) (browse-url (string-append name ":" rest)))))
 '("http" "https" "ftp" "mailto"))

;;; What each mode knows of links

(define %link-modes (make-hash-table))

(define (define-links mode . properties)
  "Says how links are written in a major mode and in the modes derived from
it, as keys and values:
  'at-point  a procedure that is given a position and returns the link
             there, as a list of where it starts, where it ends, its target
             and its description or #f; or #f if no link is there.
  'internal  a procedure that is given a target that names a place in the
             current buffer, such as a heading, and returns its position,
             or #f if there is no such place."
  (hash-table-set! %link-modes mode properties))

(define (%link-procedure key)
  (let ((entry (mode-lookup %link-modes)))
    (and entry (property-ref entry key))))

;;; Links in text

(define (regexp-search-around regexp text index)
  "The match of a regular expression in a string that an index is in or at
the end of, as regexp-search gives it, or #f. Of matches that follow one
another, the first that reaches the index is taken."
  (let loop ((from 0))
    (let ((match (and (<= from (string-length text))
                      (regexp-search regexp text from))))
      (and match
           (let ((start (car (car match))) (end (cadr (car match))))
             (cond [(> start index) #f]
                   [(<= index end) match]
                   ;; An empty match would be found again at the same place.
                   [else (loop (max end (+ from 1)))]))))))

;; A web address ends before the punctuation of the sentence it is in.
(define (%link-trim-url url)
  (let loop ((end (string-length url)))
    (if (and (> end 0)
             (memv (string-ref url (- end 1)) '(#\. #\, #\; #\: #\! #\? #\' #\")))
        (loop (- end 1))
        (substring url 0 end))))

(define (url-at . pos)
  "The web address that a position (default point) is in, written out in
the text: a list of where it starts, where it ends, the address and #f, or
#f if there is none. Addresses start with http://, https://, ftp:// or
mailto:."
  (let* ((pos (if (pair? pos) (car pos) (point)))
         (start (line-beginning-position pos))
         (line (buffer-substring start (line-end-position pos)))
         (match (regexp-search-around
                 ;; Brackets that are closed belong to the address, as in
                 ;; the addresses of Wikipedia; one that closes what was
                 ;; opened before the address does not.
                 "(?:https?://|ftp://|mailto:)[^\\s<>\\[\\]()\"]+(?:\\([^\\s<>()]*\\)[^\\s<>\\[\\]()\"]*)*"
                 line (- pos start))))
    (and match
         (let* ((from (car (car match)))
                (url (%link-trim-url (substring line from (cadr (car match))))))
           (and (<= (- pos start) (+ from (string-length url)))
                (list (+ start from) (+ start from (string-length url)) url #f))))))

(define (link-at . pos)
  "The link that a position (default point) is in: a list of where it
starts, where it ends, its target and its description or #f; or #f if no
link is there. The major mode says how its links are written; a web
address written out is a link in every mode."
  (let ((pos (if (pair? pos) (car pos) (point)))
        (at-point (%link-procedure 'at-point)))
    (or (and at-point (at-point pos))
        (url-at pos))))

;;; Going to a target

;; The directory that the relative names of files in the current buffer
;; start from.
(define (%link-directory)
  (let ((file (buffer-file-name)))
    (or (and file (file-name-directory file))
        (working-directory))))

(define (%link-goto pos)
  (set-mark! (point))
  (goto-char pos))

(define (link-search text)
  "Goes to the place in the current buffer that the part of a link after
the name of a file names: a line by its number, a place that the major
mode knows, such as a heading, or else the first occurrence of the text.
The mark is left where point was."
  (let ((internal (%link-procedure 'internal)))
    (cond
     [(regexp-search "^[0-9]+$" text)
      (set-mark! (point))
      (goto-line (string->number text))]
     [(and internal (internal text)) => %link-goto]
     [(string-search (buffer-string) text) => %link-goto]
     [else (error (string-append "Found no place for " text))])))

(define (open-file-link target)
  "Visits the file that a target names, relative to the file of the current
buffer. After the name there can be :: and a place in the file: the number
of a line, a place that the mode of the file knows, or text to search for.
A directory is shown in the file manager, and a file that is not text goes
to the program that opens such files."
  (let* ((cut (string-search target "::"))
         (name (if cut (substring target 0 cut) target))
         (search (and cut (substring target (+ cut 2) (string-length target))))
         (path (and (not (string-null? name))
                    (expand-file-name name (%link-directory)))))
    (cond
     [(not path) #t]
     [(file-remote? path) (%dired-visit path)]
     [(file-directory? path) (dired-open path)]
     [(file-exists? path) (%dired-visit path)]
     [else (find-file path)])
    (when (and search (not (string-null? search)))
      (cond
       [(not path) (link-search search)]
       ;; A file that was not visited before gets its mode when this code
       ;; has returned to the editor, and the mode knows the places.
       [(equal? (buffer-file-name) path)
        (let ((buffer (buffer-name)))
          (run-later (lambda ()
                       (when (equal? (buffer-name) buffer)
                         (link-search search)))))]))))

(define-link-type "file" open-file-link)

(define (%link-file? target)
  (or (string-prefix? "/" target)
      (string-prefix? "./" target)
      (string-prefix? "../" target)
      (string-prefix? "~" target)))

(define (open-link target)
  "Goes where the target of a link leads. A target of a kind made with
define-link-type, such as https://example.org or file:notes.org, goes to
the opener of its kind. A target that starts with /, ./, ../ or ~ is a
file. Any other target names a place in the current buffer, which the
major mode finds."
  (let ((typed (%link-typed target))
        (internal (%link-procedure 'internal)))
    (cond
     [typed ((car typed) (cdr typed))]
     [(%link-file? target) (open-file-link target)]
     [(and internal (internal target)) => %link-goto]
     [else (error (string-append "Found no place for " target))])))

(define-command (open-link-at-point)
  "Follow the link at point: open a web address, visit a file, or go to
the place in this buffer that the link names. The mark is left where point
was."
  (let ((link (link-at)))
    (unless link
      (error "There is no link at point"))
    (open-link (caddr link))))

(define-command (open-link-at-mouse)
  "Put point where the button was pressed, and follow the link there if
there is one."
  (call-command 'mouse-set-point)
  (let ((link (link-at)))
    (when link
      (open-link (caddr link)))))
