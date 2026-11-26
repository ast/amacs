;;; Spelling, as jinx does it in Emacs: the misspelled words on screen have
;;; a wave under them, and M-$ corrects the nearest one.
;;;
;;; The editor checks the words itself, with Hunspell dictionaries, in a
;;; thread of its own, and only those it shows. What is here is what is
;;; done about them: the commands, the lists of words that are right
;;; whatever the dictionaries say, and where spelling is checked.
;;;
;;;   (set-option! 'spell-languages '("en_US" "sv_SE"))
;;;   (remove-hook! 'prog-mode-hook spell-in-code)
;;;
;;; The options spell-only-in and spell-not-in say which kinds of syntax
;;; are checked, and a hook can set them for a mode with set-local!.

(define-option 'spell-personal-file #f
  "The file of words that are right whatever the dictionaries say, a word
on each line. #f for spelling/words in the directory of the init file.")

(define-option 'spell-project-file ".spelling"
  "The name of the file of words that are right in a project, in its root.")

(define-minor-mode spell-mode ""
  "Mark the misspelled words of the buffer. M-$ corrects the nearest one,
C-M-$ all of them in turn.")

;; Whether the user's own words have been read. They are when spelling
;; is first checked somewhere, not when the editor starts.
(define %spell-personal-loaded #f)

(define (spell-in-text)
  "Checks the spelling of the whole text of the current buffer. On the
hook of text-mode."
  (unless %spell-personal-loaded (spell-load-personal-words))
  (enable-mode! 'spell-mode))

(define (spell-in-code)
  "Checks the spelling of the comments and strings of the current buffer.
On the hooks of prog-mode and conf-mode."
  (set-local! 'spell-only-in '("comment" "doc" "string"))
  (spell-in-text))

(add-hook! 'text-mode-hook spell-in-text)
(add-hook! 'prog-mode-hook spell-in-code)
(add-hook! 'conf-mode-hook spell-in-code)

;;; Asking the dictionaries

(define (spell-suggest word . languages)
  "What a word may have been meant to be: a list of words, the likeliest
first, from the dictionaries of the buffer's languages or of the list of
languages given."
  (await (%spell-suggest word (if (null? languages)
                                  (option 'spell-languages)
                                  (car languages)))))

(define (spell-misspellings . range)
  "The misspelled words of the current buffer, or of it between two
positions: a list of (start end) lists, in the order of the text. It is
the words that spell-mode would mark, whether the mode is on or not."
  (await (%spell-misspellings (if (null? range) (point-min) (car range))
                              (if (null? range) (point-max) (cadr range)))))

;;; Lists of words

(define (spell-personal-file)
  "The name of the file of the user's own words."
  (or (option 'spell-personal-file)
      (string-append (or (getenv "XDG_CONFIG_HOME")
                         (expand-file-name "~/.config"))
                     "/amacs/spelling/words")))

(define (spell-project-file)
  "The name of the file of the words of the current buffer's project, or
#f if it is in none."
  (let ((root (project-root)))
    (and root (string-append root "/" (option 'spell-project-file)))))

(define (spell-read-words file)
  "The words in a file of words: one on each line. Empty lines and lines
that start with # are not words. A file that is not there has none."
  (if (file-exists? file)
      (filter (lambda (line) (not (or (string-null? line) (string-prefix? "#" line))))
              (map string-trim (string-split (read-file file) #\newline)))
      '()))

(define (spell-save-word word file)
  "Adds a word to a file of words, which is kept sorted, and makes the
file if it is not there."
  (let* ((lines (if (file-exists? file)
                    (filter (lambda (line) (not (string-null? line)))
                            (string-split (read-file file) #\newline))
                    '()))
         (comment? (lambda (line) (string-prefix? "#" line)))
         (words (sort (delete-duplicates (cons word (remove comment? lines))) string<?)))
    (make-directory (file-name-directory file))
    (write-file file (string-append (string-join (append (filter comment? lines) words) "\n")
                                    "\n"))))

(define (spell-load-personal-words)
  "Accepts the words of the user's own file. Done when spelling is first
checked in a buffer, and by M-x spell-reload."
  (set! %spell-personal-loaded #t)
  (for-each spell-accept! (spell-read-words (spell-personal-file))))

(define (spell-load-project-words)
  "Accepts the words of the project's file in the current buffer. On
find-file-hook."
  (let ((file (spell-project-file)))
    (when file
      (for-each (lambda (word) (spell-accept! word #t)) (spell-read-words file)))))

(add-hook! 'find-file-hook spell-load-project-words)

;;; Correcting

;; A word in small letters, if it is not already.
(define (%spell-lower word)
  (let ((lower (string-downcase word)))
    (if (equal? lower word) '() (list lower))))

;; The things to do with a word other than to replace it, as candidates:
;; a key, the word, and what it does.
(define (%spell-actions word)
  (let ((forms (cons word (%spell-lower word))))
    (append
     (map (lambda (form) (list (string-append "@" form) "save as a word of mine")) forms)
     (map (lambda (form) (list (string-append "*" form) "accept until the editor exits")) forms)
     (if (spell-project-file)
         (map (lambda (form) (list (string-append "+" form) "save as a word of the project"))
              forms)
         '()))))

;; Does what a candidate of %spell-actions says, if the choice is one.
(define (%spell-act choice word)
  (and (> (string-length choice) 1)
       (member (substring choice 1 (string-length choice)) (cons word (%spell-lower word)))
       (let ((form (substring choice 1 (string-length choice)))
             (key (string-ref choice 0)))
         (cond [(char=? key #\@)
                (spell-save-word form (spell-personal-file))
                (spell-accept! form)
                (message "Saved " form " in " (abbreviate-file-name (spell-personal-file)))
                #t]
               [(char=? key #\*)
                (spell-accept! form)
                (message "Accepted " form)
                #t]
               [(and (char=? key #\+) (spell-project-file))
                (spell-save-word form (spell-project-file))
                (spell-accept! form #t)
                (message "Saved " form " in " (abbreviate-file-name (spell-project-file)))
                #t]
               [else #f]))))

(define (spell-correct-word start end)
  "Asks what the word between two positions should be, with what the
dictionaries suggest to choose from, and puts the answer in its place.
After the suggestions come other things to do with the word: @word saves
it in the user's file of words, *word accepts it until the editor exits,
and +word saves it in the project's file. Returns the position after the
word as it then is."
  (let* ((word (buffer-substring start end))
         (suggestions (spell-suggest word)))
    (goto-char start)
    (add-face! start end 'region)
    (let ((choice (unwind-protect
                   (completing-read (string-append "Correct " word ": ")
                                    (append suggestions (%spell-actions word))
                                    'spelling)
                   (clear-faces! start end))))
      (cond [(or (equal? choice "") (equal? choice word) (%spell-act choice word))
             end]
            [else
             (delete-region start end)
             (goto-char start)
             (insert choice)
             (point)]))))

;; Where point belongs after the text from start to end became text that
;; ends at new-end, if it was at a place before that.
(define (%spell-moved place start end new-end)
  (cond [(<= place start) place]
        [(< place end) (min place new-end)]
        [else (+ place (- new-end end))]))

;; The misspelling that point is in or at the end of, else the nearest.
(define (%spell-nearest wrong place)
  (let ((distance (lambda (word)
                    (cond [(< place (car word)) (- (car word) place)]
                          [(> place (cadr word)) (- place (cadr word))]
                          [else 0]))))
    (fold (lambda (word best)
            (if (or (not best) (< (distance word) (distance best))) word best))
          #f
          wrong)))

(define-command (spell-correct)
  "Correct the misspelled word at point, or else the nearest one in the
window: choose among what the dictionaries suggest, or type what it should
be. The choices @word, *word and +word keep the word as it is and save it
as a word of yours, accept it until the editor exits, or save it as a
word of the project. With a prefix argument, correct every misspelled
word of the buffer in turn."
  (if (prefix-arg-given?)
      (spell-correct-all)
      (let* ((place (point))
             (word (%spell-nearest (spell-misspellings (window-start) (window-end)) place)))
        (if word
            (let ((new-end (spell-correct-word (car word) (cadr word))))
              (goto-char (%spell-moved place (car word) (cadr word) new-end)))
            (message "No misspelled word in sight")))))

(define-command (spell-correct-all)
  "Correct every misspelled word of the buffer in turn, from its start.
C-g stops."
  (let loop ((from (point-min)) (count 0))
    (let ((wrong (spell-misspellings from (point-max))))
      (if (null? wrong)
          (message (if (= count 0) "No misspelled words" "No more misspelled words"))
          (loop (spell-correct-word (car (car wrong)) (cadr (car wrong)))
                (+ count 1))))))

(define-command (spell-next)
  "Move to the next misspelled word."
  (let ((wrong (filter (lambda (word) (> (car word) (point)))
                       (spell-misspellings (point) (point-max)))))
    (if (null? wrong)
        (message "No misspelled word after this")
        (goto-char (car (car wrong))))))

(define-command (spell-previous)
  "Move to the misspelled word before point."
  (let ((wrong (filter (lambda (word) (< (car word) (point)))
                       (spell-misspellings (point-min) (point)))))
    (if (null? wrong)
        (message "No misspelled word before this")
        (goto-char (car (last wrong))))))

(define-command (spell-set-languages)
  "Choose the languages that the spelling of this buffer is checked in:
the names of dictionaries, with spaces between them. With a prefix
argument, choose them for every buffer that has none of its own."
  (let* ((there (spell-dictionaries))
         (everywhere? (prefix-arg-given?))
         (answer (read-string (string-append "Languages (of " (string-join there " ") "): ")
                              (string-join (option 'spell-languages) " ")))
         (languages (filter (lambda (name) (not (string-null? name)))
                            (string-split answer #\space))))
    (for-each (lambda (name)
                (unless (member name there)
                  (error (string-append "There is no dictionary " name))))
              languages)
    (if everywhere?
        (set-option! 'spell-languages languages)
        (set-local! 'spell-languages languages))
    (message (if (null? languages)
                 "Spelling is not checked"
                 (string-append "Spelling is checked in " (string-join languages ", "))))))

(define-command (spell-reload)
  "Read the dictionaries and the files of words again, as after one was
installed or changed outside the editor."
  (spell-reload!)
  (spell-load-personal-words)
  (spell-load-project-words)
  (message "Spelling: " (string-join (spell-dictionaries) ", ")))

;; C-. is the key of flyspell, and easier to reach than M-$ where $ is on
;; a layer of the keyboard. In the minibuffer it acts on a candidate.
(global-set-key "M-$" 'spell-correct)
(global-set-key "C-." 'spell-correct)
(global-set-key "C-M-$" 'spell-correct-all)
