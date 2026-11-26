;;; Encrypting and decrypting, with gpg and with age behind one interface:
;;; what reads and writes encrypted files and the password store is built
;;; on this. The programs do the work. The plain text goes to them and
;;; comes from them through pipes, and is never in a file; the cipher text
;;; is written under another name first and then given the file's name.
;;;
;;; A backend is found by the end of a file's name. `define-crypt-backend`
;;; adds one.

(define-option 'crypt-gpg-program "gpg"
  "The program that encrypts and decrypts OpenPGP files, those whose names
end in .gpg or .asc.")

(define-option 'crypt-age-program #f
  "The program that encrypts and decrypts age files, those whose names end
in .age. #f takes rage if it is installed, since it can ask for a
passphrase through pinentry, and else age.")

(define-option 'crypt-age-identities '()
  "The files with the keys that age decrypts with. When empty, they are
those that exist of ~/.config/age/keys.txt, ~/.ssh/id_ed25519 and
~/.ssh/id_rsa.")

(define-option 'crypt-pinentry 'agent
  "Who asks for a passphrase or a PIN when gpg needs one: agent leaves it
to gpg's own agent and its pinentry program, and loopback asks in the
minibuffer, which also works in a terminal and over ssh. The editor does not
keep what is typed.")

(define-option 'crypt-environment '()
  "Environment variables for the programs that encrypt and decrypt, as a
list of lists of a name and a value: GNUPGHOME, for one.")

;;; Backends

;; Lists of a name, the suffixes, and the procedures that decrypt, encrypt,
;; say who a file is encrypted to, and list the keys there are.
(define %crypt-backends '())

(define (define-crypt-backend name suffixes decrypt encrypt recipients keys)
  "Adds a way to encrypt and decrypt, for the files whose names end in one
of the suffixes. `decrypt` gets a file's name and returns its plain text.
`encrypt` gets a text, a list of recipients and a file's name, and writes
the file. `recipients` gets a file's name and returns who it is encrypted
to, as `encrypt` takes them, or #f if the file does not say. `keys` returns
what there is to encrypt to, as lists of a recipient and a description."
  (set! %crypt-backends
        (cons (list name suffixes decrypt encrypt recipients keys)
              (filter (lambda (backend) (not (eq? (car backend) name)))
                      %crypt-backends))))

(define (crypt-backend file)
  "The name of the backend for a file, by the end of its name: gpg or age,
or one that was added. #f for a file that is not an encrypted one."
  (let ((found (find (lambda (backend)
                       (any (lambda (suffix) (string-suffix? suffix file))
                            (cadr backend)))
                     %crypt-backends)))
    (and found (car found))))

(define (crypt-suffixes)
  "The ends of the names of the files that can be decrypted."
  (append-map cadr %crypt-backends))

(define (%crypt-entry file)
  (or (and (crypt-backend file) (assq (crypt-backend file) %crypt-backends))
      (error (string-append (file-name-nondirectory file)
                            " is not an encrypted file by its name"))))

(define (%crypt-here! file)
  (when (file-remote? file)
    (error "Files on another host are not encrypted or decrypted: copy them here first")))

(define (crypt-decrypt file)
  "The plain text of an encrypted file. The key's owner is asked for a
passphrase or a PIN if one is needed. It is an error if the file cannot be
decrypted, with the reason."
  (%crypt-here! file)
  (keep-from-program! "what is encrypted")
  ((list-ref (%crypt-entry file) 2) (expand-file-name file)))

(define (crypt-encrypt text recipients file)
  "Writes a text to a file, encrypted to a list of recipients: for gpg,
what names a key, such as a fingerprint or an address; for age, a public
key, or the name of a file of them. The file is replaced only when all of
it is written."
  (%crypt-here! file)
  (when (null? recipients)
    (error (string-append "There is nobody to encrypt "
                          (file-name-nondirectory file) " to")))
  ((list-ref (%crypt-entry file) 3) text recipients (expand-file-name file)))

(define (crypt-recipients file)
  "Who an encrypted file is encrypted to, as crypt-encrypt takes them, or
#f if the file does not say, as an age file does not."
  (%crypt-here! file)
  ((list-ref (%crypt-entry file) 4) (expand-file-name file)))

(define (crypt-keys backend)
  "What can be encrypted to with a backend, gpg or age: lists of a
recipient and a description of it. Empty for a backend that keeps no
keys."
  (let ((entry (assq backend %crypt-backends)))
    (if entry ((list-ref entry 5)) '())))

;;; Running the programs

(define (%crypt-run program arguments input)
  (run-process program arguments input (option 'crypt-environment)))

;; Writes a file through a procedure that is given another name in the
;; same directory, and gives that the file's name if the procedure
;; returns: a file is whole or as it was.
(define (%crypt-write-through file write)
  (let ((temporary (string-append
                    file ".tmp"
                    (apply string-append (map number->string (random-bytes 6))))))
    (guard (e (#t (when (file-exists? temporary) (delete-file temporary))
                  (raise e)))
      (write temporary)
      (replace-file temporary file))))

(define (%crypt-lines text)
  (filter (lambda (line) (not (string-null? line)))
          (string-split text #\newline)))

;;; gpg

(define (%gpg-arguments . more)
  (append '("--batch" "--quiet" "--yes" "--status-fd" "2") more))

;; What went wrong, in words, from what gpg wrote: its status lines say
;; it for a program, and the other lines for a person.
(define (%gpg-complaint what result)
  (let* ((said (caddr result))
         (plain (filter (lambda (line) (not (string-prefix? "[GNUPG:]" line)))
                        (%crypt-lines said))))
    (define (says? text) (string-search said text))
    (string-append
     what ": "
     (cond
      [(and (not (car result)) (null? plain))
       (string-append (option 'crypt-gpg-program) " could not be run")]
      ;; No status: the program did not run, and the line says why.
      [(not (car result)) (car plain)]
      [(or (says? "BAD_PASSPHRASE") (says? "Bad passphrase")) "the passphrase is wrong"]
      [(says? "NO_SECKEY") "no key here can decrypt it"]
      [(or (says? "INV_RECP") (says? "Unusable public key") (says? "No public key"))
       "a recipient has no key here that can be used; a key must be there and trusted"]
      [(or (says? "CANCELED") (says? "cancelled") (says? "canceled"))
       "it was cancelled"]
      [(says? "No pinentry") "a passphrase is needed"]
      [(pair? plain) (car plain)]
      [else "gpg failed without saying why"]))))

(define (%gpg-decrypt file)
  (let* ((program (option 'crypt-gpg-program))
         (loopback? (eq? (option 'crypt-pinentry) 'loopback))
         (name (file-name-nondirectory file))
         ;; With loopback, the first try is one that asks nobody: it does
         ;; if the agent has the key open or the key needs nothing.
         (first (%crypt-run program
                            (apply %gpg-arguments
                                   (append (if loopback? '("--pinentry-mode" "error") '())
                                           (list "--decrypt" "--" file)))
                            "")))
    (cond
     [(eqv? (car first) 0) (cadr first)]
     [(and loopback? (string-search (caddr first) "No pinentry"))
      (let* ((passphrase (read-passwd (string-append "Passphrase for " name ": ")))
             (second (%crypt-run program
                                 (%gpg-arguments "--pinentry-mode" "loopback"
                                                 "--passphrase-fd" "0"
                                                 "--decrypt" "--" file)
                                 (string-append passphrase "\n"))))
        (if (eqv? (car second) 0)
            (cadr second)
            (error (%gpg-complaint name second))))]
     [else (error (%gpg-complaint name first))])))

(define (%gpg-encrypt text recipients file)
  (%crypt-write-through
   file
   (lambda (temporary)
     (let ((result (%crypt-run
                    (option 'crypt-gpg-program)
                    (apply %gpg-arguments
                           (append
                            ;; As pass does: what is short is not made
                            ;; shorter, and nobody is added.
                            '("--compress-algo=none" "--no-encrypt-to")
                            (if (string-suffix? ".asc" file) '("--armor") '())
                            (append-map (lambda (recipient) (list "--recipient" recipient))
                                        recipients)
                            (list "--output" temporary "--encrypt")))
                    text)))
       (unless (eqv? (car result) 0)
         (error (%gpg-complaint (file-name-nondirectory file) result)))))))

;; The keys that a file is encrypted to, by their ids, from the status
;; lines of a run that decrypts nothing.
(define (%gpg-recipients file)
  (let* ((result (%crypt-run (option 'crypt-gpg-program)
                             (%gpg-arguments "--list-only" "--decrypt" "--" file)
                             ""))
         (ids (filter-map
               (lambda (line)
                 (and (string-prefix? "[GNUPG:] ENC_TO " line)
                      (let ((words (string-split line #\space)))
                        (and (> (length words) 2) (list-ref words 2)))))
               (%crypt-lines (caddr result)))))
    ;; An id of all zeros is a recipient that the file does not name.
    (let ((named (filter (lambda (id) (not (string=? id "0000000000000000"))) ids)))
      (and (pair? named) (delete-duplicates named)))))

;; The public keys, as lists of a fingerprint and the first name on the
;; key, from the listing that gpg makes for programs.
(define (%gpg-keys)
  (let ((result (%crypt-run (option 'crypt-gpg-program)
                            '("--batch" "--list-keys" "--with-colons")
                            "")))
    (unless (eqv? (car result) 0)
      (error (%gpg-complaint "The keys" result)))
    (let loop ((lines (%crypt-lines (cadr result)))
               (fingerprint #f)
               (wanted? #f)
               (keys '()))
      (if (null? lines)
          (reverse keys)
          (let* ((fields (string-split (car lines) #\:))
                 (kind (car fields)))
            (cond
             ;; A key starts; the fingerprint that follows is its own, and
             ;; those after it are of its parts.
             [(equal? kind "pub") (loop (cdr lines) #f #t keys)]
             [(and (equal? kind "fpr") wanted? (not fingerprint) (> (length fields) 9))
              (loop (cdr lines) (list-ref fields 9) #t keys)]
             [(and (equal? kind "uid") wanted? fingerprint (> (length fields) 9))
              (loop (cdr lines) #f #f
                    (cons (list fingerprint (list-ref fields 9)) keys))]
             [else (loop (cdr lines) fingerprint wanted? keys)]))))))

(define-crypt-backend 'gpg '(".gpg" ".asc")
  %gpg-decrypt %gpg-encrypt %gpg-recipients %gpg-keys)

;;; age

;; Whether rage is there, found out once.
(define %crypt-rage 'unknown)

(define (%age-program)
  (or (option 'crypt-age-program)
      (begin
        (when (eq? %crypt-rage 'unknown)
          (set! %crypt-rage
                (eqv? (car (%crypt-run "rage" '("--version") "")) 0)))
        (if %crypt-rage "rage" "age"))))

(define (%age-identities)
  (let ((given (option 'crypt-age-identities)))
    (map expand-file-name
         (if (pair? given)
             given
             (filter (lambda (file) (file-exists? (expand-file-name file)))
                     '("~/.config/age/keys.txt" "~/.ssh/id_ed25519" "~/.ssh/id_rsa"))))))

(define (%age-complaint what result)
  (let ((lines (%crypt-lines (caddr result))))
    (string-append
     what ": "
     (cond
      [(and (not (car result)) (null? lines)) "the age program could not be run"]
      [(pair? lines) (car lines)]
      [else "age failed without saying why"]))))

(define (%age-decrypt file)
  (let ((identities (%age-identities)))
    (when (null? identities)
      (error "There is no key to decrypt with: see the crypt-age-identities option"))
    (let ((result (%crypt-run (%age-program)
                              (append '("--decrypt")
                                      (append-map (lambda (identity)
                                                    (list "--identity" identity))
                                                  identities)
                                      (list "--" file))
                              "")))
      (if (eqv? (car result) 0)
          (cadr result)
          (error (%age-complaint (file-name-nondirectory file) result))))))

;; A recipient is a public key as text, or the name of a file of them.
(define (%age-recipient-arguments recipient)
  (if (or (string-prefix? "age1" recipient) (string-prefix? "ssh-" recipient))
      (list "--recipient" recipient)
      (list "--recipients-file" (expand-file-name recipient))))

(define (%age-encrypt text recipients file)
  (%crypt-write-through
   file
   (lambda (temporary)
     (let ((result (%crypt-run (%age-program)
                               (append '("--encrypt")
                                       (append-map %age-recipient-arguments recipients)
                                       (list "--output" temporary))
                               text)))
       (unless (eqv? (car result) 0)
         (error (%age-complaint (file-name-nondirectory file) result)))))))

(define-crypt-backend 'age '(".age")
  %age-decrypt %age-encrypt (lambda (file) #f) (lambda () '()))
