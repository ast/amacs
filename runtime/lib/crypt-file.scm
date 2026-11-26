;;; Encrypted files, visited and saved as any file: C-x C-f on a file
;;; whose name ends in .gpg, .asc or .age shows its plain text, and C-x C-s
;;; encrypts the text again. The buffer is a secret one: nothing of it is
;;; written but the encrypted file. runtime/lib/crypt.scm does the work.

;; Who each file was encrypted to when it was read or written, by its
;; name: a list of recipients, or #f for a file that does not say.
(define %crypt-file-recipients (make-hash-table))

;; The file that names the recipients for the files of a directory and of
;; those under it, by backend: the password store's convention.
(define (%crypt-recipients-file-name backend)
  (case backend
    [(gpg) ".gpg-id"]
    [(age) ".age-recipients"]
    [else #f]))

;; The nearest such file at or above a directory, or #f.
(define (%crypt-recipients-file directory name)
  (let loop ((directory directory))
    (let ((candidate (expand-file-name name directory))
          (above (file-name-directory directory)))
      (cond
       [(file-exists? candidate) candidate]
       [(or (not above) (equal? above directory) (string-null? above)) #f]
       [else (loop above)]))))

;; The lines of a file that name somebody: not the empty ones, and not
;; those that start with #.
(define (%crypt-recipient-lines file)
  (filter (lambda (line) (not (or (string-null? line) (string-prefix? "#" line))))
          (map string-trim (string-split (read-file file) #\newline))))

;; Asks who a new file is for.
(define (%crypt-ask-recipients path)
  (let ((name (file-name-nondirectory path)))
    (case (crypt-backend path)
      [(gpg)
       (let* ((keys (crypt-keys 'gpg))
              (chosen (completing-read
                       (string-append "Encrypt " name " to: ")
                       (map (lambda (key) (list (cadr key) (car key))) keys)
                       'crypt-key))
              (key (find (lambda (key) (equal? (cadr key) chosen)) keys)))
         (list (if key (car key) chosen)))]
      [else
       (list (read-string
              (string-append "Encrypt " name
                             " to (a public key, or a file of them): ")))])))

(define (crypt-file-recipients path)
  "Who an encrypted file is written for when it is saved, as a list for
crypt-encrypt. A file named .gpg-id, or .age-recipients for age, in the
file's directory or one above it decides, a name on each line, as in the
password store. Without one, they are those that the file had when it was
read. A new file with neither is asked about."
  (let* ((backend (crypt-backend path))
         (list-name (%crypt-recipients-file-name backend))
         (list-file (and list-name
                         (%crypt-recipients-file (file-name-directory path) list-name))))
    (cond
     ;; age reads a file of recipients itself.
     [(and list-file (eq? backend 'age)) (list list-file)]
     [list-file (%crypt-recipient-lines list-file)]
     [(hash-table-ref %crypt-file-recipients path) => (lambda (had) had)]
     [else (%crypt-ask-recipients path)])))

(define (%crypt-file-visit path)
  (let ((text (crypt-decrypt path)))
    (hash-table-set! %crypt-file-recipients path (crypt-recipients path))
    text))

(define (%crypt-file-save path text)
  (let ((recipients (crypt-file-recipients path)))
    (crypt-encrypt text recipients path)
    (hash-table-set! %crypt-file-recipients path recipients)))

(for-each
 (lambda (suffix)
   (define-file-handler suffix %crypt-file-visit %crypt-file-save #t))
 (crypt-suffixes))
