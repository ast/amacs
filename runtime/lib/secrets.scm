;;; Where code gets secrets from: a password for a host, a key for a
;;; service. Code asks with what it knows, a host, a user, a port, and
;;; sources answer; the password store is one, runtime/lib/pass.scm, and
;;; others can be added. A secret is not decrypted until it is asked for.

(define-option 'secret-sources '(pass)
  "The sources that are asked for a secret, in order, by name. pass is the
password store. Others are added with define-secret-source.")

;; Pairs of a name and the procedure that searches.
(define %secret-sources '())

(define (define-secret-source name search)
  "Adds a source of secrets under a name, which the secret-sources option
can then list. `search` is called with what is known, each #f if it is
not: a host, a user and a port. It returns a list of entries. An entry is
a list of a name for it, the host, the user or #f, and a procedure of no
arguments that returns the secret: so that nothing is decrypted until the
secret is used."
  (set! %secret-sources
        (cons (cons name search)
              (filter (lambda (source) (not (eq? (car source) name)))
                      %secret-sources))))

(define (%secret-keyword spec key)
  (let ((rest (memq key spec)))
    (and rest (pair? (cdr rest)) (cadr rest))))

(define (secret-search . spec)
  "The entries that the sources have for what is known: (secret-search
'host \"example.org\" 'user \"me\" 'port \"22\"), each of them optional. An
entry is a list of its name, the host, the user or #f, and a procedure that
returns the secret when it is called. The entries of the first source that
has any are returned, the best first."
  (keep-from-program! "a secret")
  (let ((host (%secret-keyword spec 'host))
        (user (%secret-keyword spec 'user))
        (port (let ((port (%secret-keyword spec 'port)))
                (if (number? port) (number->string port) port))))
    (let loop ((names (option 'secret-sources)))
      (if (null? names)
          '()
          (let* ((source (assq (car names) %secret-sources))
                 (found (if source ((cdr source) host user port) '())))
            (if (null? found)
                (loop (cdr names))
                found))))))

(define (secret-get . spec)
  "The secret of the best entry for what is known, as a string, or #f if
no source has one: (secret-get 'host \"example.org\" 'user \"me\"). The
owner of the key is asked for a passphrase if one is needed."
  (let ((found (apply secret-search spec)))
    (and (pair? found) ((list-ref (car found) 3)))))
