;;; The password store of pass: a directory of encrypted files, one for
;;; each entry, whose first line is the password and whose other lines say
;;; more, as `key: value`. https://www.passwordstore.org describes it.
;;;
;;; The store is read here as it is, without the pass program: an entry is
;;; a file that runtime/lib/crypt.scm decrypts, and it is written for
;;; those whom the .gpg-id above it names. A store of age files, as
;;; passage keeps, is the same with .age-recipients. Changes are committed
;;; with the messages that pass writes, so that both can work on one
;;; store. What is copied leaves the clipboard again after a time.

(define-option 'pass-store-directory #f
  "The directory of the password store. #f takes that of the
PASSWORD_STORE_DIR environment variable, or else ~/.password-store.")

(define-option 'pass-clip-time 45
  "The seconds after which a password that was copied is taken off the
clipboard again, if the clipboard still holds it.")

(define-option 'pass-generated-length 25
  "The length that is offered for a password that is generated.")

(define-option 'pass-generated-symbols #t
  "Whether a password that is generated has chars that are no letters or
digits.")

(define-option 'pass-commit #t
  "Whether a change to the password store is committed, if the store is a
git repository.")

;;; The store

(define (pass-store)
  "The directory of the password store, as a full name."
  (let ((directory (expand-file-name
                    (or (option 'pass-store-directory)
                        (getenv "PASSWORD_STORE_DIR")
                        "~/.password-store"))))
    (if (and (string-suffix? "/" directory) (> (string-length directory) 1))
        (substring directory 0 (- (string-length directory) 1))
        directory)))

(define (%pass-join directory name)
  (string-append directory "/" name))

;; The suffix of an entry's file, if a name is one.
(define (%pass-suffix name)
  (find (lambda (suffix) (string-suffix? suffix name)) (crypt-suffixes)))

(define (%pass-strip name)
  (let ((suffix (%pass-suffix name)))
    (if suffix
        (substring name 0 (- (string-length name) (string-length suffix)))
        name)))

(define (pass-entries)
  "The names of the entries of the password store, sorted: for the file
mail/work.gpg, mail/work. What starts with a dot is not part of it."
  (let ((root (pass-store)))
    (unless (file-directory? root)
      (error (string-append "There is no password store at "
                            (abbreviate-file-name root))))
    (let walk ((directory root) (prefix ""))
      (sort
       (append-map
        (lambda (entry)
          (let ((name (car entry)))
            (cond
             [(string-prefix? "." name) '()]
             [(eq? (cadr entry) 'directory)
              (walk (%pass-join directory name) (string-append prefix name "/"))]
             [(%pass-suffix name) (list (string-append prefix (%pass-strip name)))]
             [else '()])))
        (directory-entries directory))
       string<?))))

;; The suffix for a new entry: that of age in a store that says who its
;; age files are for, and else that of gpg.
(define (%pass-new-suffix)
  (if (file-exists? (%pass-join (pass-store) ".age-recipients")) ".age" ".gpg"))

(define (pass-entry-file name)
  "The file of an entry of the password store, by the entry's name. An
entry that is not there yet gets the name it would have."
  (let ((base (%pass-join (pass-store) name)))
    (or (find file-exists?
              (map (lambda (suffix) (string-append base suffix)) (crypt-suffixes)))
        (string-append base (%pass-new-suffix)))))

(define (%pass-existing name)
  (let ((file (pass-entry-file name)))
    (unless (file-exists? file)
      (error (string-append name " is not in the password store")))
    file))

;; The name of the entry that a file of the store is, or #f for another
;; file.
(define (%pass-entry-of file)
  (let ((prefix (string-append (pass-store) "/")))
    (and (string-prefix? prefix file)
         (%pass-suffix file)
         (%pass-strip (substring file (string-length prefix) (string-length file))))))

;;; What is in an entry

(define (pass-entry-text name)
  "All of the plain text of an entry of the password store. The owner of
the key is asked for a passphrase if one is needed."
  (keep-from-program! "an entry of the password store")
  (crypt-decrypt (%pass-existing name)))

(define (%pass-lines text)
  (string-split text #\newline))

(define (pass-text-password text)
  "The password in the text of an entry: its first line."
  (car (%pass-lines text)))

(define (pass-text-fields text)
  "What the text of an entry says besides the password: pairs of a key and
a value, from its lines of the form `key: value`, in order. The keys are
in small letters."
  (filter-map
   (lambda (line)
     (let ((colon (string-search line ":")))
       (and colon
            (> colon 0)
            ;; A line that is an address, as otpauth://..., is no field.
            (not (string-prefix? "//" (substring line (+ colon 1) (string-length line))))
            (cons (string-downcase (string-trim (substring line 0 colon)))
                  (string-trim (substring line (+ colon 1) (string-length line)))))))
   (cdr (%pass-lines text))))

(define (pass-password name)
  "The password of an entry of the password store."
  (pass-text-password (pass-entry-text name)))

(define (pass-field name key)
  "The value of a field of an entry of the password store, or #f if the
entry has no such field. The key user also finds a field named username or
login."
  (let* ((fields (pass-text-fields (pass-entry-text name)))
         (keys (if (equal? key "user") '("user" "username" "login") (list key)))
         (found (find (lambda (field) (member (car field) keys)) fields)))
    (and found (cdr found))))

;;; The clipboard

(define %pass-clip-timer #f)

(define (pass-copy-secret text what)
  "Puts a secret on the system clipboard, not on the kill ring, and takes
it off again after the seconds of the pass-clip-time option. `what` says
what it is, for the message."
  (unless (clipboard-set! text)
    (error "There is no clipboard to copy to"))
  (when %pass-clip-timer (cancel-timer! %pass-clip-timer))
  (set! %pass-clip-timer
        (run-after (* 1000 (option 'pass-clip-time))
                   (lambda ()
                     (set! %pass-clip-timer #f)
                     (when (clipboard-clear!)
                       (message "The clipboard was cleared")))))
  (message "Copied " what ". The clipboard is cleared in "
           (option 'pass-clip-time) " seconds."))

;;; git

(define (%pass-git?)
  (file-exists? (%pass-join (pass-store) ".git")))

(define (%pass-git . arguments)
  (let ((result (run-process "git" (append (list "-C" (pass-store)) arguments))))
    (unless (eqv? (car result) 0)
      (error (string-append
              "git: "
              (car (string-split
                    (string-trim (string-append (caddr result) (cadr result)))
                    #\newline)))))
    (cadr result)))

;; Commits what was done to some files of the store, if the store is a
;; repository and commits are wanted.
(define (%pass-commit message . files)
  (when (and (option 'pass-commit) (%pass-git?))
    (apply %pass-git "add" "--all" "--" files)
    (apply %pass-git "commit" "--quiet" "--message" message "--" files)))

;;; Reading a name

(define (%pass-read-entry prompt)
  (completing-read prompt (pass-entries) 'pass-entry))

;; The entry that `pass` found, while a command runs for it.
(define %pass-chosen #f)

;; The entry that a command is about: the one that `pass` found, or the
;; one that the file manager's point is on in the store, or the one of
;; the buffer, or one asked for.
(define (%pass-entry-here prompt)
  (or %pass-chosen
      (and (eq? (major-mode) 'dired-mode)
           (guard (e (#t #f)) (%pass-entry-of (dired-file-at-point))))
      ;; A buffer whose entry was renamed or deleted still has the old
      ;; name of its file.
      (and (buffer-file-name)
           (file-exists? (buffer-file-name))
           (%pass-entry-of (buffer-file-name)))
      (%pass-read-entry prompt)))

;;; Commands

(define-command (pass-copy)
  "Copy the password of an entry of the password store to the clipboard.
The clipboard is cleared again after a time."
  (let ((name (%pass-entry-here "Copy the password of: ")))
    (pass-copy-secret (pass-password name) (string-append "the password of " name))))

(define-command (pass-copy-user)
  "Copy the user name of an entry of the password store to the clipboard."
  (let* ((name (%pass-entry-here "Copy the user of: "))
         (user (pass-field name "user")))
    (unless user
      (error (string-append name " has no user, username or login field")))
    (pass-copy-secret user (string-append "the user of " name))))

(define-command (pass-copy-field)
  "Copy a field of an entry of the password store to the clipboard. The
field is asked for."
  (let* ((name (%pass-entry-here "Copy a field of: "))
         (fields (pass-text-fields (pass-entry-text name))))
    (when (null? fields)
      (error (string-append name " has no fields")))
    (let* ((key (completing-read "Field: " (delete-duplicates (map car fields))
                                 'pass-field))
           (field (assoc key fields)))
      (unless field
        (error (string-append name " has no field " key)))
      (pass-copy-secret (cdr field) (string-append key " of " name)))))

(define-command (pass-open-url)
  "Open the address in the url field of an entry of the password store."
  (let* ((name (%pass-entry-here "Open the address of: "))
         (url (pass-field name "url")))
    (unless url
      (error (string-append name " has no url field")))
    (start-process (option 'dired-open-program) (list url))
    (message "Opened " url)))

(define-command (pass-edit)
  "Visit an entry of the password store, to read it or change it. Saving
encrypts it again and commits the change."
  (find-file (%pass-existing (%pass-entry-here "Entry: "))))

;; Asks for the name of an entry that may be new, starting with the
;; directory that the file manager shows if that is in the store.
(define (%pass-read-new-name prompt)
  (let* ((root (string-append (pass-store) "/"))
         (here (and (eq? (major-mode) 'dired-mode) (dired-directory)))
         (prefix (if (and here (string-prefix? root (string-append here "/")))
                     (substring (string-append here "/")
                                (string-length root)
                                (+ (string-length here) 1))
                     ""))
         (name (or %pass-chosen (string-trim (read-string prompt prefix)))))
    (when (or (string-null? name) (string-suffix? "/" name) (string-prefix? "/" name)
              (string-search name ".."))
      (error "That is no name for an entry"))
    name))

;; Writes the text of an entry for those whom the store says, making the
;; directories on the way.
(define (%pass-write name text)
  (let ((file (pass-entry-file name)))
    (make-directory (file-name-directory file))
    (crypt-encrypt text (crypt-file-recipients file) file)
    file))

(define (%pass-refresh file)
  (%dired-refresh-all)
  (when (eq? (major-mode) 'dired-mode)
    (dired-refresh file)))

(define-command (pass-insert)
  "Add an entry to the password store, with a password that is typed
twice. An entry that is there already is replaced after a question."
  (let* ((name (%pass-read-new-name "New entry: "))
         (file (pass-entry-file name))
         (there? (file-exists? file)))
    (when (and there?
               (not (y-or-n? (string-append name " is there already; replace it?"))))
      (error "Nothing was changed"))
    (let ((first (read-passwd (string-append "Password for " name ": ")))
          (second (read-passwd "The same again: ")))
      (unless (equal? first second)
        (error "The two are not the same"))
      (%pass-write name (string-append first "\n"))
      (%pass-commit (string-append "Add given password for " name " to store.") file)
      (%pass-refresh file)
      (message "Added " name))))

(define %pass-letters "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789")
(define %pass-symbols "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~")

(define (pass-generate-password size . symbols?)
  "A password of a length, from the random numbers of the operating
system, each char as likely as any other. With a second argument, that
says whether it has chars that are no letters or digits; without, the
pass-generated-symbols option does."
  (let* ((symbols? (if (pair? symbols?) (car symbols?) (option 'pass-generated-symbols)))
         (chars (if symbols? (string-append %pass-letters %pass-symbols) %pass-letters))
         (count (string-length chars))
         ;; A byte at or above this would make the first chars more likely.
         (limit (- 256 (modulo 256 count))))
    (let loop ((picked '()) (bytes '()))
      (cond
       [(= (length picked) size) (list->string picked)]
       [(null? bytes) (loop picked (random-bytes (* 2 (+ 1 (- size (length picked))))))]
       [(< (car bytes) limit)
        (loop (cons (string-ref chars (modulo (car bytes) count)) picked) (cdr bytes))]
       [else (loop picked (cdr bytes))]))))

(define-command (pass-generate)
  "Generate a password for an entry of the password store and copy it. A
new entry is made; in one that is there, the password is replaced after a
question and the other lines stay."
  (let* ((name (%pass-read-new-name "Generate a password for: "))
         (file (pass-entry-file name))
         (there? (file-exists? file))
         (size (string->number
                (read-string "Length: " (number->string (option 'pass-generated-length))))))
    (unless (and size (exact? size) (<= 1 size 1024))
      (error "The length must be a number from 1 to 1024"))
    (when (and there?
               (not (y-or-n? (string-append name " is there already; replace its password?"))))
      (error "Nothing was changed"))
    (let* ((password (pass-generate-password size))
           (rest (if there? (cdr (%pass-lines (pass-entry-text name))) '(""))))
      (%pass-write name (string-join (cons password rest) "\n"))
      (%pass-commit (string-append (if there? "Replace" "Add")
                                   " generated password for " name ".")
                    file)
      (%pass-refresh file)
      (pass-copy-secret password (string-append "the new password of " name)))))

(define-command (pass-rename)
  "Give an entry of the password store another name, which can be in
another directory of the store. It is encrypted anew if the new place is
for somebody else."
  (let* ((old (%pass-entry-here "Rename: "))
         (from (%pass-existing old))
         (new (string-trim (read-string (string-append "Rename " old " to: ") old)))
         (to (string-append (%pass-join (pass-store) new) (%pass-suffix from))))
    (when (or (string-null? new) (equal? new old))
      (error "Nothing was changed"))
    (when (file-exists? to)
      (error (string-append new " is there already")))
    (make-directory (file-name-directory to))
    (if (equal? (crypt-file-recipients from) (crypt-file-recipients to))
        (rename-file from to)
        (begin
          (crypt-encrypt (crypt-decrypt from) (crypt-file-recipients to) to)
          (delete-file from)))
    (%pass-commit (string-append "Rename " old " to " new ".") from to)
    (%pass-refresh to)
    (message "Renamed " old " to " new)))

(define-command (pass-delete)
  "Delete an entry of the password store, after asking."
  (let* ((name (%pass-entry-here "Delete: "))
         (file (%pass-existing name)))
    (when (y-or-n? (string-append "Delete " name " from the password store?"))
      (delete-file file)
      (%pass-commit (string-append "Remove " name " from store.") file)
      (%pass-refresh file)
      (message "Deleted " name))))

(define-command (pass-git-pull)
  "Get the changes that others made to the password store, with git pull."
  (message "Pulling...")
  (release-input!)
  (%pass-git "pull" "--quiet")
  (%dired-refresh-all)
  (message "The password store is up to date"))

(define-command (pass-git-push)
  "Send the changes of the password store to where it is kept, with git
push."
  (message "Pushing...")
  (release-input!)
  (%pass-git "push" "--quiet")
  (message "The password store was pushed"))

(define %pass-actions
  '(("c" "Copy the password" pass-copy)
    ("u" "Copy the user" pass-copy-user)
    ("f" "Copy another field..." pass-copy-field)
    ("o" "Open its address" pass-open-url)
    ("e" "Read or edit it" pass-edit)
    ("g" "Generate a new password for it..." pass-generate)
    ("r" "Rename it..." pass-rename)
    ("d" "Delete it..." pass-delete)))

(define-command (pass)
  "Find an entry of the password store by its name and say what to do with
it: copy its password or a field, open its address, read or change it."
  (let* ((name (%pass-read-entry "Password store: "))
         (choice (car (key-menu name '() %pass-actions))))
    ;; The command is run with the entry as the one that it is about.
    (set! %pass-chosen name)
    (unwind-protect ((global-ref choice)) (set! %pass-chosen #f))))

;;; Committing what was saved

;; On the hook that runs after a buffer is saved: an entry that was edited
;; or made by visiting its file is committed, as pass does after its edit.
(define (%pass-after-save)
  (let* ((file (buffer-file-name))
         (name (and file (guard (e (#t #f)) (%pass-entry-of file)))))
    (when (and name (option 'pass-commit) (%pass-git?))
      (run-later
       (lambda ()
         (let ((new? (string-prefix? "??" (%pass-git "status" "--porcelain" "--" file))))
           (%pass-commit (if new?
                             (string-append "Add given password for " name " to store.")
                             (string-append "Edit password for " name " using amacs."))
                         file)))))))

(add-hook! 'after-save-hook %pass-after-save)

;;; Browsing: the file manager on the store

(define-minor-mode pass-store-mode "Pass"
  "Minor mode of the file manager in the password store. Its keys act on
the entry on the line: they copy its password or a field, and add,
generate, rename and delete entries.")

(for-each
 (lambda (binding) (define-key 'pass-store-mode (car binding) (cadr binding)))
 '(("w" pass-copy) ("W" pass-copy-user) ("F" pass-copy-field)
   ("O" pass-open-url) ("I" pass-insert) ("G" pass-generate)
   ("R" pass-rename) ("D" pass-delete)))

;; On the hook of the file manager's mode: its listings of the store, and
;; of what is under it, get the keys of the store.
(define (%pass-in-dired)
  (let ((directory (guard (e (#t #f)) (dired-directory)))
        (root (guard (e (#t #f)) (pass-store))))
    (when (and directory root
               (or (equal? directory root)
                   (string-prefix? (string-append root "/") directory)))
      ;; What starts with a dot is the store's own: .git, .gpg-id.
      (%dired-set-hidden! (dired-state) #f)
      (enable-mode! 'pass-store-mode))))

(add-hook! 'dired-mode-hook %pass-in-dired)

(define-command (pass-browse)
  "Show the password store in the file manager, with keys that act on its
entries. RET reads an entry."
  (dired-open (pass-store)))

;;; As a source of secrets

;; How well the name of an entry fits what is known, as auth-source-pass
;; reads names: host, user@host, host:port, or host/user. Returns a list
;; of a rank, lower is better, and the user that the name gives, or #f. A
;; name that gives the user is better than one that does not, and of two
;; that are else alike, the one with the port is better.
(define (%pass-match name host user port)
  (let* ((parts (string-split name #\/))
         (last (car (reverse parts)))
         (parent (and (pair? (cdr parts)) (cadr (reverse parts))))
         (with-port (and port (string-append host ":" port)))
         (at (string-search last "@")))
    ;; 0 for the host with its port, 1 for the host alone, #f for another.
    (define (host-rank text)
      (cond [(and with-port (equal? text with-port)) 0]
            [(equal? text host) 1]
            [else #f]))
    (cond
     ;; user@host
     [(and at (host-rank (substring last (+ at 1) (string-length last))))
      => (lambda (rank)
           (let ((named (substring last 0 at)))
             (and (or (not user) (equal? user named))
                  (list rank named))))]
     ;; host/user
     [(and parent (or (not user) (equal? user last)) (host-rank parent))
      => (lambda (rank) (list (+ 2 rank) last))]
     ;; host
     [(host-rank last) => (lambda (rank) (list (+ 4 rank) #f))]
     [else #f])))

(define (%pass-secret-search host user port)
  (if (or (not host) (not (file-directory? (pass-store))))
      '()
      (map (lambda (found)
             (let ((name (cadr found)))
               (list name host (or (caddr found) user)
                     (lambda () (pass-password name)))))
           (sort (filter-map (lambda (name)
                               (let ((match (%pass-match name host user port)))
                                 (and match (list (car match) name (cadr match)))))
                             (pass-entries))
                 (lambda (a b) (< (car a) (car b)))))))

(define-secret-source 'pass %pass-secret-search)
