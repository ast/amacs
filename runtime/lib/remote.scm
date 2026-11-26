;;; Files on other hosts, and files as another user.
;;;
;;; A remote file has a name in tramp's syntax: /ssh:evo|sudo::/etc/nixos
;;; is a file on the host evo, as root. Each step of such a name starts
;;; with a method. The methods are defined here, with
;;; define-remote-method, and so are the things that make the names easy
;;; to type: what the prompt for file names offers at each point of a
;;; name, and the line that says a name in plain words. The editor itself
;;; reads the names, runs the commands and speaks SFTP.
;;;
;;; The words that the user sees are in Simplified Technical English.

;;; Methods

;; What Scheme knows of each method, by name: the words about it and how
;; to find its hosts. The commands are in the editor's own table.
(define %remote-methods (make-hash-table))

;; The names of the methods that are offered, in the order of definition.
(define %remote-method-order '())

;; The value after a key in a list of keys and values, or a default.
(define (%remote-property properties key default)
  (cond [(or (null? properties) (null? (cdr properties))) default]
        [(eq? (car properties) key) (cadr properties)]
        [else (%remote-property (cddr properties) key default)]))

(define (define-remote-method name . properties)
  "Defines a method: a way to reach a place, which a step of the name of a
remote file starts with, as ssh in /ssh:evo:/etc. A method of the same name
is replaced. After the name come keys and values:

  'command  The program that enters the place, and its arguments: a list
            of strings and of the symbols host, user and port, which stand
            for those parts of the step. A list in the list is left out
            if a part that it names is not there: (\"-l\" user). A
            list that starts with the symbol unasked holds words that
            are there only if nobody can answer what the program asks:
            (unasked \"-o\" \"BatchMode=yes\"). The command to run in
            the place is added at the end.
  'line     #t if the command to run in the place is one argument, a line
            for a shell there, as ssh takes it. Else it is added as words.
  'sftp     The full command for the last step of a name, if the method
            has its own SFTP server, as ssh has. Without this, the editor
            finds and runs the sftp-server program in the place.
  'jump     How a step is written when the next step, of the same method,
            goes through it without help: a list such as
            ((user \"@\") host (\":\" port)), made into one word. The
            symbol jumps in the command stands for these words.
  'host     'optional if a step can have no host, as sudo:: has.
  'user     The user if a step names none.
  'stays    #t if the place has the files of the place before it: the
            same host, as a different user.
  'here     A sentence about the method as the first step of a name.
  'there    A sentence about the method after a different step.
  'usage    A sentence that tells how to type a step.
  'words    A procedure of the user, the host and the port of a step (#f
            if not given), which returns words for the step: \"host evo\".
  'hosts    A procedure of no arguments that returns the hosts to offer:
            a list of lists of a name and a sentence about it."
  (%define-remote-method
   name
   (%remote-property properties 'command '())
   (%remote-property properties 'sftp #f)
   (%remote-property properties 'jump #f)
   (%remote-property properties 'line #f)
   (eq? (%remote-property properties 'host 'required) 'optional)
   (%remote-property properties 'user #f)
   (%remote-property properties 'stays #f))
  (hash-table-set! %remote-methods name properties)
  (unless (member name %remote-method-order)
    (set! %remote-method-order (append %remote-method-order (list name)))))

;; A fact about a method, or a default.
(define (%remote-method-ref name key default)
  (%remote-property (hash-table-ref %remote-methods name '()) key default))

;; Whether a step of a method can have no host.
(define (%remote-host-optional? name)
  (eq? (%remote-method-ref name 'host 'required) 'optional))

;; The configuration that every ssh of the editor reads, as text. ssh
;; takes the first value that it finds for an option. The configuration of
;; the user comes first, so that it wins: its control connections are the
;; ones that are used, and so is its compression. Then come the values of
;; the editor for what the user did not set: one connection for each host,
;; shared and kept for ten minutes, and a host that does not answer is
;; given up. Last comes the configuration of the system, which ssh does
;; not read by itself when it is given a file.
(define (%ssh-config user-config control-directory)
  (string-append
   "Include " user-config "\n"
   "Host *\n"
   "  ControlMaster auto\n"
   "  ControlPersist 600\n"
   "  ControlPath " control-directory "/ssh-%C\n"
   "  ConnectTimeout 10\n"
   "  ServerAliveInterval 10\n"
   "  ServerAliveCountMax 3\n"
   "Include /etc/ssh/ssh_config\n"))

;; What every ssh of the editor is told. Where nobody can answer a
;; question, as in a batch run, none is asked, whatever a configuration
;; says. Else ssh asks through the editor, see runtime/lib/askpass.scm.
;; The rest is in a file of the editor, see %ssh-config, which ssh also
;; gives to the ssh that it starts for a host that it goes through. The
;; file is where the editor has its sockets. Without a place for that
;; file, ssh reads the configuration of the user and nothing is added.
(define %ssh-options
  (append
   '((unasked "-o" "BatchMode=yes"))
   (let* ((runtime (getenv "XDG_RUNTIME_DIR"))
          (user (getenv "USER"))
          (directory (cond [(and runtime (not (string-null? runtime)))
                            (string-append runtime "/amacs")]
                           [user (string-append (or (getenv "TMPDIR") "/tmp")
                                                "/amacs-" user)]
                           [else #f]))
          (file (and directory (string-append directory "/ssh_config"))))
     (if (and file
              (guard (e (#t #f))
                (make-directory directory)
                (write-file file (%ssh-config "~/.ssh/config" directory))
                #t))
         (list "-F" file)
         '("-o" "ConnectTimeout=10"
           "-o" "ServerAliveInterval=10"
           "-o" "ServerAliveCountMax=3")))))

;; The parts of a sentence that are there, with commas between them.
(define (%remote-parts . parts)
  (string-join (filter (lambda (part) part) parts) ", "))

;; The hosts that ssh knows, each with a sentence made from what its
;; configuration says.
(define (%ssh-host-offers)
  (map (lambda (host)
         (let* ((name (car host))
                (address (cadr host))
                (user (caddr host))
                (port (list-ref host 3))
                (jump (list-ref host 4))
                (address (and address (not (equal? address name)) address))
                (parts (%remote-parts
                        (cond [(and user address) (string-append user "@" address)]
                              [user (string-append user "@" name)]
                              [else address])
                        (and port (string-append "port " port))
                        (and jump (string-append "through " jump)))))
           (list name
                 (cond [(list-ref host 5) "From known_hosts."]
                       [(string-null? parts) ""]
                       [else (string-append parts ".")]))))
       (ssh-hosts)))

(let ((ssh (append '("ssh") %ssh-options '(("-J" jumps) ("-l" user) ("-p" port)))))
  (define-remote-method "ssh"
    'command (append ssh '("--" host))
    'sftp (append ssh '("-s" "--" host "sftp"))
    'jump '((user "@") host (":" port))
    'line #t
    'here "Connect to a different host with ssh."
    'there "Connect from that host to a different host."
    'usage "Type ssh:HOST: or ssh:USER@HOST#PORT: and put a colon after the host."
    'words (lambda (user host port)
             (%remote-parts (string-append "host " host)
                            (and user (string-append "user " user))
                            (and port (string-append "port " (number->string port)))))
    'hosts %ssh-host-offers))

(define-remote-method "sudo"
  'command '("sudo" "-n" "-u" user "--")
  'host 'optional
  'user "root"
  'stays #t
  'here "Become a different user on this machine. Default: root."
  'there "Become a different user on that host. Default: root."
  'usage "Type sudo:: for root. Type sudo:USER@: for a different user."
  'words (lambda (user host port) (string-append "user " (or user "root"))))

;; A container holds the files only if it has the sftp-server program.
(define (%define-container-method name)
  (define-remote-method name
    'command (list name "exec" "-i" '("-u" user) 'host)
    'here (string-append "Open a container with " name ".")
    'there (string-append "Open a container on that host with " name ".")
    'usage (string-append "Type " name ":NAME: and put a colon after the name.")
    'words (lambda (user host port)
             (%remote-parts (string-append "container " host)
                            (and user (string-append "user " user))))))

(%define-container-method "podman")
(%define-container-method "docker")

;;; What a name means

;; Where a character is last in a string, or #f.
(define (%remote-last-index text char)
  (let loop ((at (- (string-length text) 1)))
    (cond [(< at 0) #f]
          [(char=? (string-ref text at) char) at]
          [else (loop (- at 1))])))

;; The methods as they are typed, in a sentence: "ssh:, sudo: or podman:".
(define (%remote-method-list suffix)
  (let* ((names (map (lambda (name) (string-append name suffix)) %remote-method-order))
         (count (length names)))
    (if (< count 2)
        (string-join names "")
        (string-append (string-join (reverse (cdr (reverse names))) ", ")
                       " or " (last names)))))

;; What a name that is not whole needs: `problem` is what
;; file-remote-steps gave for it.
(define (%remote-missing name problem)
  (let* ((kind (cadr problem))
         (text (caddr problem))
         (bar (%remote-last-index name #\|))
         (current (substring name (if bar (+ bar 1) 1) (string-length name)))
         (colon (string-search current ":" 0))
         (method (and colon (substring current 0 colon))))
    (cond
     [(eq? kind 'unknown-method)
      (string-append text " is not a method. Use " (%remote-method-list "") ".")]
     [(eq? kind 'bad-host) (string-append text " cannot be the name of a host.")]
     [(eq? kind 'bad-port) (string-append text " is not a port number.")]
     [(eq? kind 'no-host)
      (%remote-method-ref text 'usage (string-append text " needs a host."))]
     [(not method) (string-append "Type a method: " (%remote-method-list ":"))]
     [else
      (%remote-method-ref method 'usage
                          (string-append "Type " method ":HOST: and put a colon after the host."))])))

(define (file-remote-description name)
  "Says in plain words what the name of a file on another host means:
\"Host evo. Then user root. Path: /etc/nixos\" for
\"/ssh:evo|sudo::/etc/nixos\". For a name that starts as one and is not
whole, says what to type. Returns #f for the name of a file of this
machine. The prompt for file names shows this while a name is typed."
  (let ((parts (file-remote-steps name)))
    (cond
     [(not parts) #f]
     [(eq? (car parts) 'error) (%remote-missing name parts)]
     [else
      (let* ((path (cadr parts))
             (sentences
              (let loop ((steps (car parts)) (first? #t))
                (if (null? steps)
                    '()
                    (let* ((step (car steps))
                           (method (car step))
                           (words ((%remote-method-ref
                                    method 'words
                                    ;; A method that says nothing of
                                    ;; itself is a user or a host.
                                    (lambda (user host port)
                                      (if (string-null? host)
                                          (string-append
                                           "user "
                                           (or user (%remote-method-ref method 'user "root")))
                                          (string-append method " " host))))
                                   (cadr step) (caddr step) (cadddr step))))
                      (cons
                       (cond
                        [(not first?) (string-append "Then " words)]
                        [(%remote-method-ref method 'stays #f)
                         (string-append "This machine, " words)]
                        [else (string-append (string-upcase (substring words 0 1))
                                             (substring words 1 (string-length words)))])
                       (loop (cdr steps) #f)))))))
        (string-append
         (string-join sentences ". ")
         ". Path: "
         (cond [(member path '("" "~" "~/")) "home directory"]
               [(or (string-prefix? "/" path) (string-prefix? "~/" path)) path]
               [else (string-append "~/" path)])))])))

;;; What the prompt offers

(define-option 'remote-places-offered 5
  "How many of the places on other hosts that were used before the prompt
for file names offers first, where a name starts.")

;; The first items of a list, at most so many.
(define (%remote-first items count)
  (if (or (null? items) (<= count 0))
      '()
      (cons (car items) (%remote-first (cdr items) (- count 1)))))

;; Whether there is a connection to a place: /ssh:evo:.
(define (%remote-connected? place)
  (equal? (assoc place (remote-hosts)) (list place "connected")))

(define (remote-name-offers name)
  "What can stand where the first part of a file name stands, if that part
is, or can become, the start of the name of a remote file. The argument is
the part as far as it is typed, without the / before it: \"ssh:ev\", or
\"ssh:evo|su\". Returns #f, or a list for set-prompt-offers!: a name for
what is offered, the text that narrows it, the candidates, and whether the
files at the top of this machine are left out.

At the start, the places used before and the methods are offered. After a
method, its hosts. After a host, the directories there and the steps that
can follow, so that a chain such as ssh:evo|sudo:: needs no memory."
  (let* ((bar (%remote-last-index name #\|))
         (before (if bar (substring name 0 (+ bar 1)) ""))
         (current (substring name (string-length before) (string-length name)))
         (there? (not (string-null? before)))
         (colon (string-search current ":" 0))
         ;; The offers stay the same until one more step sign is typed.
         (stage (let ((end (max (or bar -1) (or (%remote-last-index name #\:) -1))))
                  (substring name 0 (+ end 1))))
         (found (lambda (pattern offers) (list stage pattern offers (not (string-null? stage))))))
    (cond
     ;; A method, or at the start a place used before.
     [(not colon)
      (found
       name
       (append
        (if there?
            '()
            (map (lambda (place)
                   (list (substring place 1 (string-length place))
                         (if (%remote-connected? place)
                             "Used before. Connected."
                             "Used before.")
                         #t))
                 (%remote-first (remote-places) (option 'remote-places-offered))))
        (map (lambda (method)
               (list (string-append before method ":")
                     (%remote-method-ref method (if there? 'there 'here) "")
                     #t))
             %remote-method-order)))]
     [else
      (let* ((method (substring current 0 colon))
             (rest (substring current (+ colon 1) (string-length current)))
             (end (string-search rest ":" 0)))
        (cond
         [(not (member method (remote-method-names))) #f]
         ;; A whole step. With text after it, the name is that of a file
         ;; in the home directory there.
         [end
          (and (= end (- (string-length rest) 1))
               (not (string-search rest "[" 0))
               (let ((stem (substring name 0 (- (string-length name) 1)))
                     (stays? (%remote-method-ref method 'stays #f)))
                 (found
                  stem
                  (append
                   (list (list (string-append name "/") "Top directory of that host." #f)
                         (list (string-append name "~/")
                               (if stays?
                                   "Home directory of that user."
                                   "Your home directory on that host.")
                               #f))
                   ;; From a different user there is no step further.
                   (if stays?
                       '()
                       (map (lambda (next)
                              (if (%remote-host-optional? next)
                                  (list (string-append stem "|" next "::")
                                        (string-append
                                         "Become "
                                         (%remote-method-ref next 'user "a different user")
                                         " on that host.")
                                        #t)
                                  (list (string-append stem "|" next ":")
                                        (%remote-method-ref next 'there "")
                                        #t)))
                            %remote-method-order))))))]
         ;; A host. The user, if one is typed, stays as typed.
         [else
          (let* ((at (%remote-last-index rest #\@))
                 (user (if at (substring rest 0 at) ""))
                 (prefix (string-append before method ":"
                                        (if (string-null? user) "" (string-append user "@")))))
            (found
             name
             (cond
              [(not (%remote-host-optional? method))
               (map (lambda (host)
                      (let ((text (string-append prefix (car host) ":")))
                        (list text
                              (string-append
                               (cadr host)
                               (cond [(not (%remote-connected? (string-append "/" text))) ""]
                                     [(string-null? (cadr host)) "Connected."]
                                     [else " Connected."]))
                              #t)))
                    ((%remote-method-ref method 'hosts (lambda () '()))))]
              [(string-null? user)
               (list (list (string-append prefix ":")
                           (string-append "User " (%remote-method-ref method 'user "root")
                                          ". For a different user, type " method ":NAME@:")
                           #t))]
              [else
               (list (list (string-append prefix ":")
                           (string-append "User " user ".")
                           #t))])))]))])))

;; The prompt for a file name tells what a remote name means, and offers
;; what can follow, each time its input changes.
(define (%remote-prompt-changed kind input)
  (when (member kind '("file" "directory"))
    (let ((typed (%typed-file-name input)))
      (set-prompt-hint! input (file-remote-description typed))
      (let ((offers
             ;; Only for the first part of a name from the top.
             (and (string-prefix? "/" typed)
                  (not (string-search typed "/" 1))
                  (remote-name-offers (substring typed 1 (string-length typed))))))
        (if offers
            (apply set-prompt-offers! input offers)
            (set-prompt-offers! input #f))))))

(add-hook! 'prompt-change-hook %remote-prompt-changed)

;;; Passwords

;; The value after a word at the start of a line of text, or #f: what
;; ssh -G prints is such lines.
(define (%ssh-setting text word)
  (let loop ((lines (string-split text #\newline)))
    (cond [(null? lines) #f]
          [(string-prefix? (string-append word " ") (car lines))
           (substring (car lines) (+ (string-length word) 1) (string-length (car lines)))]
          [else (loop (cdr lines))])))

;; What ssh makes of a host, by its own account (ssh -G, which connects
;; to nothing), as the text that it prints: a line for each setting. #f
;; if ssh does not say. `options` are the words for ssh before the host,
;; `user` and `port` are those of the step, or #f.
(define (%ssh-settings options user port host)
  (let ((result (run-process
                 "ssh"
                 (append options
                         (if user (list "-l" user) '())
                         (if port (list "-p" (number->string port)) '())
                         (list "-G" "--" host)))))
    (and (eqv? (car result) 0) (cadr result))))

;; Whether a setting of ssh that names a way through something else is
;; not set: ssh leaves the line out or says none.
(define (%ssh-unset? settings word)
  (member (%ssh-setting settings word) '(#f "none")))

;; Whether the key of a host is in one of the files where ssh keeps the
;; keys that it knows, by the account of ssh-keygen -F. Then ssh has no
;; question about the host.
(define (%ssh-host-known? settings)
  (let* ((host (%ssh-setting settings "hostname"))
         (port (or (%ssh-setting settings "port") "22"))
         (name (or (let ((alias (%ssh-setting settings "hostkeyalias")))
                     (and alias (not (equal? alias "none")) alias))
                   (if (equal? port "22")
                       host
                       (string-append "[" host "]:" port))))
         (files (append
                 (string-split (or (%ssh-setting settings "userknownhostsfile") "") #\space)
                 (string-split (or (%ssh-setting settings "globalknownhostsfile") "") #\space))))
    (let loop ((files files))
      (cond [(null? files) #f]
            [(and (not (string-null? (car files)))
                  (file-exists? (expand-file-name (car files)))
                  (eqv? 0 (car (run-process
                                "ssh-keygen"
                                (list "-F" name "-f" (expand-file-name (car files)))))))
             #t]
            [else (loop (cdr files))]))))

;; The host, the user and the port to look a password up for, if the
;; first question of the ssh of a place can only be the one for that
;; password, and else #f. The questions of ssh are not read to see what
;; they ask, so this is worked out before: the place is one host, with
;; no host on the way to it, in the name or in the configuration, since
;; such a host asks first; and ssh knows the key of the host, since it
;; asks about one that it does not know. (A key with a passphrase and no
;; agent asks first too. That question is answered by ssh on this
;; machine, and the password goes no further.)
(define (%ssh-password-facts place options)
  (let* ((parts (file-remote-steps place))
         (steps (and (pair? parts) (not (eq? (car parts) 'error)) (car parts))))
    (and steps
         (null? (cdr steps))
         (equal? (car (car steps)) "ssh")
         (let* ((step (car steps))
                (settings (%ssh-settings options (cadr step) (cadddr step) (caddr step))))
           (and settings
                (%ssh-setting settings "hostname")
                (%ssh-unset? settings "proxyjump")
                (%ssh-unset? settings "proxycommand")
                (%ssh-host-known? settings)
                (list (%ssh-setting settings "hostname")
                      (%ssh-setting settings "user")
                      (%ssh-setting settings "port")))))))

;; For the first question of the ssh of a place, the password that a
;; source of secrets has for the host, under its full name, where that
;; question can only be for it. A function of askpass-functions.
(define (%ssh-stored-password who host user prompt kind before)
  (and (eq? kind 'secret)
       (eqv? before 0)
       (guard (e (#t #f))
         (let ((facts (%ssh-password-facts who (filter string? %ssh-options))))
           (and facts
                (secret-get 'host (car facts) 'user (cadr facts) 'port (caddr facts)))))))

(add-hook! 'askpass-functions %ssh-stored-password)

;;; As root

;; Whether the last step of the start of a remote name, such as
;; /ssh:evo|sudo::, is a different user on the same host.
(define (%remote-as-user? prefix)
  (let ((parts (file-remote-steps prefix)))
    (and (pair? parts)
         (not (eq? (car parts) 'error))
         (%remote-method-ref (car (last (car parts))) 'stays #f))))

(define (file-as-root name)
  "The name under which root reads and writes a file: /sudo::/etc/hosts
for /etc/hosts, and /ssh:evo|sudo::/etc/hosts for /ssh:evo:/etc/hosts. A
name that goes through sudo already is returned as it is."
  (let* ((name (expand-file-name name))
         (prefix (file-remote-prefix name)))
    (cond
     [(not prefix) (string-append "/sudo::" name)]
     [(%remote-as-user? prefix) name]
     [else
      ;; The colon that ends the host gives way to the next step.
      (string-append (substring prefix 0 (- (string-length prefix) 1))
                     "|sudo::"
                     (file-local-name name))])))

(define-command (sudo-edit)
  "Visit the file of this buffer again as root, through sudo on the host
that it is on, with point where it was. In the file manager, show the
same directory as root. sudo must not ask for a password there."
  (cond
   [(eq? (major-mode) 'dired-mode)
    (let ((directory (dired-directory)))
      (when (equal? (file-as-root directory) directory)
        (error "This directory is shown through sudo already"))
      (dired-open (file-as-root directory)))]
   [(buffer-file-name)
    => (lambda (file)
         (let ((at (point))
               (root (file-as-root file)))
           (when (equal? root file)
             (error "This file is visited through sudo already"))
           (find-file root)
           (goto-char (min at (point-max)))))]
   [else (error "This buffer has no file")]))
