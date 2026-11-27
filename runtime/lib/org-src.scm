;;; The source blocks of Org: running the code of a block and putting what
;;; it wrote under the block, and writing the code of the blocks of a file
;;; to the files that they name ("tangling"). A block is
;;;
;;;   #+begin_src sh :tangle setup.sh
;;;   echo hello
;;;   #+end_src
;;;
;;; C-c C-c in a block runs it. This file adds that to Org through
;;; define-org-action, as other code can add what it has.
;;;
;;; The words that the user sees are in Simplified Technical English.

(define-option 'org-src-programs
  '(("sh" "sh") ("shell" "sh") ("bash" "bash") ("zsh" "zsh")
    ("python" "python3") ("perl" "perl") ("ruby" "ruby") ("js" "node")
    ("awk" "awk" "-f" "/dev/stdin"))
  "The programs that run the source blocks of Org: lists of the name of a
language as it is written after #+begin_src, the program, and arguments for
it. The program is given the code of the block on its standard input.")

(define-option 'org-confirm-run #t
  "Whether Org asks before it runs the code of a source block. A file from
somebody else can hold code that does harm.")

;;; The parts of a block

(define (org-src-header block)
  "What the first line of a block says after its name: a list of the
language, or #f, and then keys and values, as in
(\"sh\" tangle \"setup.sh\" results \"silent\") for
#+begin_src sh :tangle setup.sh :results silent. The block is as
org-element-at gives it."
  (let loop ((words (filter (lambda (word) (not (string-null? word)))
                            (string-split (list-ref block 4) #\space)))
             (language #f) (key #f) (value '()) (found '()))
    (define (with-key)
      (if key
          (append found (list key (string-join (reverse value) " ")))
          found))
    (cond
     [(null? words) (cons language (with-key))]
     [(string-prefix? ":" (car words))
      (loop (cdr words) language
            (string->symbol (substring (car words) 1 (string-length (car words))))
            '() (with-key))]
     [key (loop (cdr words) language key (cons (car words) value) found)]
     [(not language) (loop (cdr words) (car words) key value found)]
     ;; Switches such as -n, before the first key, say nothing here.
     [else (loop (cdr words) language key value found)])))

;; The blanks that a line starts with.
(define (%org-src-indent line)
  (let loop ((index 0))
    (if (and (< index (string-length line))
             (memv (string-ref line index) '(#\space #\tab)))
        (loop (+ index 1))
        index)))

(define (org-src-code block)
  "The code of a block, as a string that ends in a newline if there is
code: the lines between its first and its last line, without the
indentation that all of them share, and without the comma that Org puts
before a line of code that starts with a star or with #+."
  (let* ((from (+ (line-end-position (cadr block)) 1))
         (to (line-beginning-position (caddr block)))
         (lines (if (< from to)
                    (string-split (buffer-substring from (- to 1)) #\newline)
                    '()))
         (shared (fold min 1000
                       (map %org-src-indent
                            (filter (lambda (line) (not (string-null? (string-trim line))))
                                    lines)))))
    (apply string-append
           (map (lambda (line)
                  (let* ((line (if (>= (string-length line) shared)
                                   (substring line shared (string-length line))
                                   (string-trim-left line)))
                         (line (if (or (string-prefix? ",*" line) (string-prefix? ",#+" line))
                                   (substring line 1 (string-length line))
                                   line)))
                    (string-append line "\n")))
                lines))))

;; The block of source code that a position is in, or an error.
(define (%org-src-block-at pos)
  (let ((element (org-element-at pos)))
    (unless (and (eq? (car element) 'block) (equal? (cadddr element) "src"))
      (error "Point is not in a source block"))
    element))

;;; Results

;; Where the result of a block is, as a pair of the start of its line
;; #+RESULTS: and the end of its last line, or #f. It follows the block,
;; after blank lines.
(define (%org-src-result block)
  (let* ((text (buffer-string))
         (after (min (string-length text) (+ (caddr block) 1)))
         (found (regexp-search "(?i)\\A[ \\t\\n]*^([ \\t]*#\\+RESULTS:.*)$"
                               (substring text after (string-length text)))))
    (and found
         (let* ((start (+ after (car (cadr found))))
                (rest-start (min (string-length text) (+ after (cadr (cadr found)) 1)))
                (rest (substring text rest-start (string-length text)))
                ;; What the result is: lines that start with a colon, or
                ;; an example block.
                (body (or (regexp-search
                           "(?is)\\A[ \\t]*#\\+begin_example.*?^[ \\t]*#\\+end_example[ \\t]*$" rest)
                          (regexp-search "\\A(?:[ \\t]*:(?: .*)?(?:\\n|\\z))+" rest))))
           (cons start
                 (if body
                     (let ((end (+ rest-start (cadr (car body)))))
                       ;; Without the newline that ends the last line.
                       (if (and (> end rest-start)
                                (char=? (string-ref text (- end 1)) #\newline))
                           (- end 1)
                           end))
                     (+ after (cadr (cadr found)))))))))

;; What a program wrote, as Org writes a result: lines after a colon, or
;; an example block if the lines are many.
(define (%org-src-format output indent)
  (let* ((lines (string-split (string-trim-right output) #\newline))
         (prefix (lambda (line) (string-append indent line))))
    (string-join
     (cons (prefix "#+RESULTS:")
           (if (> (length lines) 10)
               (append (list (prefix "#+begin_example"))
                       (map prefix lines)
                       (list (prefix "#+end_example")))
               (map (lambda (line)
                      (prefix (if (string-null? line) ":" (string-append ": " line))))
                    lines)))
     "\n")))

;; Puts what a block wrote under it, in place of what was there. Without
;; output, the old result is taken away.
(define (%org-src-put-result block output)
  (let ((old (%org-src-result block))
        (indent (make-string (%org-src-indent (buffer-substring
                                                (cadr block)
                                                (line-end-position (cadr block))))
                             #\space)))
    (save-excursion
      (cond
       [old
        (delete-region (car old) (cdr old))
        (goto-char (car old))
        (if (string-null? (string-trim output))
            ;; The blank line before the result goes with it.
            (delete-region (caddr block) (point))
            (insert (%org-src-format output indent)))]
       [(not (string-null? (string-trim output)))
        (goto-char (caddr block))
        (insert "\n\n" (%org-src-format output indent))]))))

;;; Running

;; The program and its arguments for a language, or an error.
(define (%org-src-program language)
  (let ((entry (and language (assoc language (option 'org-src-programs)))))
    (unless entry
      (error (string-append "There is no program for the language "
                            (or language "of this block")
                            ": see the org-src-programs option")))
    (cdr entry)))

(define (%org-src-first-line text)
  (let ((lines (filter (lambda (line) (not (string-null? (string-trim line))))
                       (string-split text #\newline))))
    (if (null? lines) "" (string-trim (car lines)))))

(define-command (org-run-block)
  "Run the code of the source block at point with the program for its
language, and put what the program wrote under the block, after
#+RESULTS:. The program runs in the directory of the file, or in the one
that the block names with :dir. With :results silent the output is shown
and not put in the text."
  (let* ((block (%org-src-block-at (point)))
         (header (org-src-header block))
         (language (car header))
         (command (%org-src-program language))
         (code (org-src-code block))
         (file (buffer-file-name))
         (directory (expand-file-name
                     (or (property-ref (cdr header) 'dir) ".")
                     (or (and file (file-name-directory file)) (working-directory))))
         (buffer (buffer-name)))
    (when (and (option 'org-confirm-run)
               (not (y-or-n? (string-append "Run this block of " language "?"))))
      (error "The block was not run"))
    (message "Running " language "…")
    (let* ((result (run-process "sh"
                                (append (list "-c" "cd \"$1\" && shift && exec \"$@\""
                                              "sh" directory)
                                        command)
                                code))
           (status (car result))
           (output (cadr result))
           (said (%org-src-first-line (caddr result))))
      (cond
       [(not (eqv? status 0))
        (error (string-append
                (car command)
                (if status
                    (string-append " failed with status " (number->string status))
                    " could not be run")
                (if (string-null? said) "" (string-append ": " said))))]
       [(member (property-ref (cdr header) 'results "") '("silent" "none"))
        (message (if (string-null? (string-trim output))
                     "The block wrote nothing"
                     (%org-src-first-line output)))]
       [(not (member buffer (buffer-list)))
        (message "The buffer of the block is gone")]
       [else
        (with-current-buffer buffer
          ;; The text can have changed while the program ran: the block is
          ;; the one that now starts where this one did.
          (let ((now (org-element-at (min (cadr block) (point-max)))))
            (if (and (eq? (car now) 'block) (equal? (cdddr now) (cdddr block)))
                (%org-src-put-result now output)
                (error "The block is not where it was; its result was not put in"))))
        (message (if (string-null? (string-trim output))
                     "The block wrote nothing"
                     "Done"))]))))

(define-org-action 'block
  (lambda (block)
    (and (equal? (cadddr block) "src")
         (begin (org-run-block) #t))))

;;; Tangling

(define (org-src-blocks)
  "The source blocks of the current buffer, in order, each as
org-element-at gives it."
  (let ((text (buffer-string)))
    (let loop ((from 0) (found '()))
      (let ((match (regexp-search "(?mi)^[ \\t]*#\\+begin_src" text from)))
        (if (not match)
            (reverse found)
            (let ((element (org-element-at (car (car match)))))
              (if (and (eq? (car element) 'block) (equal? (cadddr element) "src")
                       (= (cadr element) (line-beginning-position (car (car match)))))
                  (loop (+ (caddr element) 1) (cons element found))
                  (loop (cadr (car match)) found))))))))

;; The name of the file that a block is written to, or #f: what its
;; :tangle says, where yes is the name of the buffer's file with the
;; language as its ending.
(define (%org-src-tangle-file block)
  (let* ((header (org-src-header block))
         (name (property-ref (cdr header) 'tangle))
         (file (buffer-file-name))
         (directory (or (and file (file-name-directory file)) (working-directory))))
    (cond
     [(or (not name) (equal? name "no")) #f]
     [(equal? name "yes")
      (and file (car header)
           (string-append (car (regexp-replace "\\.[^./]*$" file ""))
                          "." (car header)))]
     [else (expand-file-name (car (regexp-replace "^\"|\"$" name "")) directory)])))

(define-command (org-tangle)
  "Write the code of the source blocks of this buffer to the files that
they name with :tangle, as in #+begin_src sh :tangle setup.sh. The blocks
of one file are written in their order, with an empty line between them.
What the files held is replaced. A directory that is missing is made if
the block says :mkdirp yes."
  (let loop ((blocks (org-src-blocks)) (files '()) (count 0))
    (if (pair? blocks)
        (let ((file (%org-src-tangle-file (car blocks))))
          (if (not file)
              (loop (cdr blocks) files count)
              (let ((code (org-src-code (car blocks)))
                    (mkdirp? (equal? (property-ref (cdr (org-src-header (car blocks))) 'mkdirp)
                                     "yes"))
                    (entry (assoc file files)))
                (if entry
                    (begin
                      (set-cdr! entry (list (string-append (cadr entry) "\n" code)
                                            (or (caddr entry) mkdirp?)))
                      (loop (cdr blocks) files (+ count 1)))
                    (loop (cdr blocks)
                          (append files (list (list file code mkdirp?)))
                          (+ count 1))))))
        (begin
          (when (null? files)
            (error "No source block of this buffer names a file with :tangle"))
          (for-each
           (lambda (entry)
             (let ((directory (file-name-directory (car entry))))
               (when (and directory (not (file-exists? directory)))
                 (if (caddr entry)
                     (make-directory directory)
                     (error (string-append "There is no directory " directory
                                           ": make it, or add :mkdirp yes to the block")))))
             (write-file (car entry) (cadr entry)))
           files)
          (message (number->string count) (if (= count 1) " block" " blocks")
                   " written to "
                   (if (= (length files) 1)
                       (abbreviate-file-name (car (car files)))
                       (string-append (number->string (length files)) " files")))))))

(for-each
 (lambda (binding) (define-key 'org-mode (car binding) (cadr binding)))
 '(("C-c C-v e" org-run-block) ("C-c C-v C-e" org-run-block)
   ("C-c C-v t" org-tangle) ("C-c C-v C-t" org-tangle)))
