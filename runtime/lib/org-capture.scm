;;; Capture and the journal of Org: writing something down without
;;; leaving what one is doing.
;;;
;;; C-c x asks for a template by its key, shows it in a buffer to fill in,
;;; and C-c C-c there puts the entry in the file and under the heading that
;;; the template names. The file need not be open. C-c j opens the journal
;;; of today at a new entry with the time.
;;;
;;; The words that the user sees are in Simplified Technical English.

(define-option 'org-directory "~/org"
  "The directory of the files of Org that capture writes to: the names of
files in the org-capture-templates option are taken from here.")

(define-option 'org-capture-templates
  '(("t" "Todo" (file+headline "notes.org" "Tasks") "* TODO %?\n  %u\n  %a")
    ("n" "Note" (file+datetree "notes.org") "* %?\n  Entered on %U\n  %i"))
  "What C-c x can capture: lists of a key, a name, where the entry goes and
its text, and then keys and values. Where it goes is one of
  (file \"notes.org\")                   the end of the file
  (file+headline \"notes.org\" \"Tasks\")  under that heading
  (file+datetree \"journal.org\")        under headings for the year, the
                                       month and the day of today
and headings that are missing are made. In the text, %? is where point
goes, %U the date and time, %u the date, %T and %t the same as dates that
an agenda shows, %i the text of the region, %a a link to where the capture
started, and %% a percent sign. After the text, prepend #t puts the entry
first under its heading, not last.")

(define-option 'org-journal-dir "~/org/journal"
  "The directory of the journal of Org, which has a file for each day.")

(define-option 'org-journal-file-format "%Y%m%d"
  "The name of the file of a day of the journal of Org, as a format for
dates: %Y is the year, %m the month and %d the day.")

(define-option 'org-journal-date-format "%A, %d %B %Y"
  "The heading of a day of the journal of Org, as a format for dates.")

;;; Where an entry goes

;; The end of what is under a heading, in a list of headings as
;; org-headings-in gives them, for a text of some length.
(define (%org-capture-subtree-end headings heading length)
  (let loop ((later (let ((rest (member heading headings)))
                      (if rest (cdr rest) '()))))
    (cond [(null? later) length]
          [(<= (cadr (car later)) (cadr heading)) (car (car later))]
          [else (loop (cdr later))])))

;; An entry with its headings moved to a level: the first heading of the
;; entry gets that level, and the others keep their distance to it. Text
;; that starts with no heading stays as it is.
(define (%org-capture-at-level entry level)
  (let* ((first (regexp-search "^(\\*+) " entry))
         (shift (if (and first (= (car (car first)) 0))
                    (- level (cadr (cadr first)))
                    0)))
    (string-join
     (map (lambda (line)
            (let ((stars (regexp-search "^(\\*+) " line)))
              (cond [(or (not stars) (= shift 0)) line]
                    [(> shift 0) (string-append (make-string shift #\*) line)]
                    [else (substring line (min (- shift) (- (cadr (cadr stars)) 1))
                                     (string-length line))])))
          (string-split entry #\newline))
     "\n")))

(define (org-capture-edit text path entry . prepend?)
  "Where an entry goes in a text of Org, and what is put there: a pair of
a position and a string. The path is the titles of the headings that the
entry goes under, from the outermost: '() for the file itself. Headings of
the path that the text does not have are part of the string. The entry
goes last under its heading, or first if the optional argument is true,
with its headings at the level under that heading."
  (let* ((prepend? (and (pair? prepend?) (car prepend?)))
         (size (string-length text))
         (headings (org-headings-in text))
         (entry (string-append (string-trim-right entry) "\n")))
    (let loop ((path path) (parent #f) (start 0) (end size) (level 0))
      (let ((found (and (pair? path)
                        (let find ((headings headings))
                          (cond [(null? headings) #f]
                                [(and (>= (car (car headings)) start)
                                      (< (car (car headings)) end)
                                      (equal? (caddr (car headings)) (car path)))
                                 (car headings)]
                                [else (find (cdr headings))])))))
        (cond
         [found
          (loop (cdr path) found (car found)
                (%org-capture-subtree-end headings found size)
                (cadr found))]
         [else
          ;; The headings that are missing, and then the entry.
          (let* ((made (let make ((titles path) (level (+ level 1)))
                         (if (null? titles)
                             ""
                             (string-append (make-string level #\*) " " (car titles) "\n"
                                            (make (cdr titles) (+ level 1))))))
                 (pos (cond
                       [(and prepend? (null? path) parent)
                        ;; After the line of the heading.
                        (let ((line-end (string-search text "\n" (car parent))))
                          (if line-end (+ line-end 1) size))]
                       [(and prepend? (null? path))
                        (let ((first (filter (lambda (heading) (>= (car heading) start))
                                             headings)))
                          (if (null? first) end (car (car first))))]
                       [else end]))
                 (fresh? (or (= pos 0) (char=? (string-ref text (- pos 1)) #\newline))))
            (cons pos
                  (string-append
                   (if fresh? "" "\n")
                   made
                   (%org-capture-at-level entry (+ level (length path) 1)))))])))))

;; The path of headings for a target of a template, as org-capture-edit
;; takes it.
(define (%org-capture-path target)
  (case (car target)
    [(file) '()]
    [(file+headline) (list (caddr target))]
    [(file+datetree)
     (list (format-current-time "%Y")
           (format-current-time "%Y-%m %B")
           (format-current-time "%Y-%m-%d %A"))]
    [else (error "A template goes to file, file+headline or file+datetree")]))

(define (%org-capture-file target)
  (expand-file-name (cadr target) (expand-file-name (option 'org-directory))))

;; The buffer that visits a file, by its name, or #f.
(define (%org-capture-buffer file)
  (let loop ((buffers (buffer-list)))
    (cond [(null? buffers) #f]
          [(equal? (buffer-file-name (car buffers)) file) (car buffers)]
          [else (loop (cdr buffers))])))

(define (org-capture-write file path entry . prepend?)
  "Puts an entry in a file of Org, as org-capture-edit says, and saves the
file. A buffer that visits the file gets the entry too; a file that is not
there is made, with its directory."
  (let ((buffer (%org-capture-buffer file)))
    (cond
     [buffer
      (with-current-buffer buffer
        (let ((edit (apply org-capture-edit (buffer-string) path entry prepend?)))
          (save-excursion
            (goto-char (car edit))
            (insert (cdr edit)))))
      (%save-buffer buffer file #f)]
     [else
      (let* ((text (if (file-exists? file) (read-file file) ""))
             (edit (apply org-capture-edit text path entry prepend?))
             (directory (file-name-directory file)))
        (when (and directory (not (file-exists? directory)))
          (make-directory directory))
        (write-file file (string-append (substring text 0 (car edit))
                                        (cdr edit)
                                        (substring text (car edit) (string-length text)))))])))

;;; Templates

(define (org-capture-expand template region link)
  "The text of a template with its marks filled in: a pair of the text and
where point goes in it, or #f if the template has no %?. The region and the
link are the strings for %i and %a. A line that holds only a mark for
which there is nothing is left out."
  (define (expand-line line)
    ;; The text of the line, where %? is in it, and whether a mark came
    ;; to nothing.
    (let loop ((index 0) (out '()) (width 0) (point #f) (empty? #f))
      (define (add text)
        (loop (+ index 2) (cons text out) (+ width (string-length text)) point
              (or empty? (string-null? text))))
      (cond
       [(>= index (string-length line))
        (list (apply string-append (reverse out)) point empty?)]
       [(and (char=? (string-ref line index) #\%) (< (+ index 1) (string-length line)))
        (case (string-ref line (+ index 1))
          [(#\?) (loop (+ index 2) out width width empty?)]
          [(#\U) (add (format-current-time "[%Y-%m-%d %a %H:%M]"))]
          [(#\u) (add (format-current-time "[%Y-%m-%d %a]"))]
          [(#\T) (add (format-current-time "<%Y-%m-%d %a %H:%M>"))]
          [(#\t) (add (format-current-time "<%Y-%m-%d %a>"))]
          [(#\i) (add region)]
          [(#\a) (add link)]
          [(#\%) (loop (+ index 2) (cons "%" out) (+ width 1) point empty?)]
          [else (loop (+ index 1) (cons "%" out) (+ width 1) point empty?)])]
       [else
        (loop (+ index 1) (cons (string (string-ref line index)) out) (+ width 1)
              point empty?)])))
  (let loop ((lines (string-split template #\newline)) (out '()) (width 0) (point #f))
    (if (null? lines)
        (cons (string-join (reverse out) "\n") point)
        (let* ((expanded (expand-line (car lines)))
               (text (car expanded)))
          (if (and (caddr expanded) (string-null? (string-trim text)) (not (cadr expanded)))
              (loop (cdr lines) out width point)
              (loop (cdr lines) (cons text out)
                    (+ width (string-length text) 1)
                    (or point (and (cadr expanded) (+ width (cadr expanded))))))))))

;; A link to where point is, for %a: to the heading that point is under
;; in a file of Org, and else to the line. Empty in a buffer without a
;; file.
(define (%org-capture-link)
  (let ((file (buffer-file-name)))
    (if (not file)
        ""
        (let ((heading (and (eq? (major-mode) 'org-mode) (outline-current-heading)))
              (name (abbreviate-file-name file)))
          (if heading
              (let ((title (caddr (org-parse-heading
                                   (buffer-substring (car heading)
                                                     (line-end-position (car heading)))))))
                (string-append "[[file:" name "::*" title "][" title "]]"))
              (string-append "[[file:" name "::" (number->string (line-number-at-pos))
                             "][" (file-name-nondirectory file) "]]"))))))

;;; Capturing

;; What is being captured: the template, and the buffer to go back to.
(define %org-capture #f)

(define %org-capture-buffer-name "*capture*")

(define-minor-mode org-capture-mode "Capture"
  "Writing an entry to capture. C-c C-c puts it in its file and C-c C-k
abandons it.")

(define (%org-capture-leave)
  (let ((origin (cadr %org-capture)))
    (set! %org-capture #f)
    (when (member %org-capture-buffer-name (buffer-list))
      (kill-buffer %org-capture-buffer-name))
    (when (member origin (buffer-list))
      (switch-to-buffer origin))))

(define-command (org-capture)
  "Capture an entry of Org: choose a template by its key, write the entry,
and put it in its file with C-c C-c, without leaving what you do. The
org-capture-templates option holds the templates."
  (let* ((templates (option 'org-capture-templates))
         (origin (buffer-name))
         (region (if (region-active?)
                     (buffer-substring (min (point) (mark)) (max (point) (mark)))
                     ""))
         (link (%org-capture-link))
         (template (car (key-menu "Capture" '()
                                  (map (lambda (template)
                                         (list (car template) (cadr template) template))
                                       templates))))
         (expanded (org-capture-expand (cadddr template) region link)))
    (when (member %org-capture-buffer-name (buffer-list))
      (kill-buffer %org-capture-buffer-name))
    (set! %org-capture (list template origin))
    (switch-to-buffer %org-capture-buffer-name)
    (org-mode)
    (enable-mode! 'org-capture-mode)
    (insert (car expanded))
    (goto-char (or (cdr expanded) (point-max)))
    (set-mode-line-text! (string-append "[" (cadr template) " for "
                                        (file-name-nondirectory
                                         (%org-capture-file (caddr template)))
                                        "]"))
    (message "C-c C-c puts the entry in its file. C-c C-k abandons it.")))

(define-command (org-capture-finish)
  "Put the entry of this buffer in the file of its template, and go back to
where the capture started."
  (unless (and %org-capture (equal? (buffer-name) %org-capture-buffer-name))
    (error "This buffer is not an entry to capture"))
  (let* ((template (car %org-capture))
         (target (caddr template))
         (file (%org-capture-file target))
         (entry (buffer-string)))
    (when (string-null? (string-trim entry))
      (error "The entry is empty"))
    (org-capture-write file (%org-capture-path target) entry
                       (property-ref (cdr (cdddr template)) 'prepend))
    (%org-capture-leave)
    (message "Captured to " (abbreviate-file-name file))))

(define-command (org-capture-abandon)
  "Abandon the entry of this buffer, and go back to where the capture
started."
  (unless (and %org-capture (equal? (buffer-name) %org-capture-buffer-name))
    (error "This buffer is not an entry to capture"))
  (%org-capture-leave)
  (message "The entry was abandoned"))

(define-key 'org-capture-mode "C-c C-c" 'org-capture-finish)
(define-key 'org-capture-mode "C-c C-k" 'org-capture-abandon)

;;; The journal

(define-command (org-journal-new-entry)
  "Start a new entry in the journal of today, with the time: the file of
the day is visited, and made if this is its first entry."
  (let* ((directory (expand-file-name (option 'org-journal-dir)))
         (file (expand-file-name (format-current-time (option 'org-journal-file-format))
                                 directory)))
    (unless (file-exists? directory)
      (make-directory directory))
    (find-file file)
    ;; The name of the file need not say that it is Org.
    (unless (eq? (major-mode) 'org-mode)
      (org-mode))
    (when (string-null? (string-trim (buffer-string)))
      (delete-region 0 (point-max))
      (insert "* " (format-current-time (option 'org-journal-date-format)) "\n"))
    (goto-char (point-max))
    (unless (= (point) (line-beginning-position))
      (insert "\n"))
    (insert "** " (format-current-time "%H:%M") " ")))

(global-set-key "C-c x" 'org-capture)
(global-set-key "C-c j" 'org-journal-new-entry)
