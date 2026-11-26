;;; What programs ask the user: the password of a host, the passphrase of
;;; a key, a code, whether a host is the right one.
;;;
;;; ssh, sudo and git are not watched for text that looks like a question.
;;; Each of them runs a helper if its environment names one, with the
;;; question as the argument, and reads the answer from what the helper
;;; prints. The editor has such a helper. A program that is started with
;;; the variables that askpass-environment returns asks through it, and
;;; the question comes here, to %askpass-asked, which finds the answer:
;;; from askpass-functions, from the sources of secrets (the password
;;; store), or from the user, in the minibuffer of the frame that the
;;; program was started from. A question of several lines has its last
;;; line as the prompt and the lines before it under the prompt.
;;;
;;; Only a program that the editor gave a ticket can ask, and each
;;; question starts with the name that the ticket was made for.
;;;
;;; The words that the user sees are in Simplified Technical English.

(define (askpass-environment who . properties)
  "The environment variables with which a program asks the user its
questions through the editor, as the 'environment of make-process and
run-process takes them. ssh, sudo -A and git obey them. `who` is a short
name for the program, which starts each question: \"git push\". After it
can come keys and values:

  'host  The host that the program can ask a password for.
  'user  The user on that host.

With these, the sources of secrets are asked for the password first, see
secret-get. Call (askpass-done! environment) when the program has ended.
An error if nobody can answer in this session: see askpass-served?."
  (%askpass-variables
   (%askpass-ticket who
                    (%property properties 'host #f)
                    (%property properties 'user #f))))

(define (askpass-done! environment)
  "Ends the permission to ask that askpass-environment gave: the argument
is what that returned. Returns whether the permission was there."
  (let ((ticket (assoc "AMACS_ASKPASS_TICKET" environment)))
    (and ticket (%askpass-release! (cadr ticket)))))

;;; The answer to a question

;; The lines of a text that are not empty, each without spaces at its
;; ends.
(define (%askpass-lines text)
  (filter (lambda (line) (not (string-null? line)))
          (map string-trim (string-split text #\newline))))

;; Puts a question of a program to the user and returns the answer. The
;; last line of the question is the prompt, after the name of the
;; program. The lines before it, as ssh has them about a host that it
;; does not know, show under the prompt until something is typed.
;; `start` opens the prompt and returns its promise.
(define (%askpass-put start who lines)
  (let* ((turned (reverse lines))
         (promise (start (string-append who " " (if (null? turned) "" (car turned)) " "))))
    (when (and (pair? turned) (pair? (cdr turned)))
      (set-prompt-hint! "" (string-join (reverse (cdr turned)) "\n")))
    (await promise)))

;; The first answer that a function of askpass-functions gives, or #f.
(define (%askpass-from-hook who host user prompt kind before)
  (let loop ((functions (hook-functions 'askpass-functions)))
    (and (pair? functions)
         (or ((car functions) who host user prompt kind before)
             (loop (cdr functions))))))

(define (askpass-answer who host user prompt kind before)
  "The answer to a question of a program, as a string, or #f for none.
`who` is the name of the program, `host` and `user` are what it can want
a password for (each can be #f), and `prompt` is the question in the words
of the program. `kind` is what the program says of the question: confirm
for yes or no, none for words that want no answer, and else secret, for
something to type. `before` is the number of questions that the program
asked before this one.

The words of a question are shown to the user and are not read for what
they mean: a program does not say more of a question than its kind. So
what is typed is always hidden, and only the first question of a program
is taken to be the one for its password.

The functions of the hook askpass-functions are called first, with the
same arguments, and the first answer that is not #f is used. Then, for
the first question of a program with a host, the sources of secrets (see
secret-get). Then the user."
  (let* ((lines (%askpass-lines prompt))
         (line (string-join lines " ")))
    (cond
     [(%askpass-from-hook who host user prompt kind before)]
     [(eq? kind 'none)
      (message who " " line)
      ""]
     [(eq? kind 'confirm) (and (y-or-n? (string-append who " " line)) "yes")]
     [(and (eqv? before 0)
           host
           (guard (e (#t (message "Cannot get the password from a source: "
                                  (error-object-message e))
                         #f))
             (if user
                 (secret-get 'host host 'user user)
                 (secret-get 'host host))))]
     [else (%askpass-put %read-secret who lines)])))

;;; Questions, one at a time

;; The questions that are not put yet, the oldest first. Each is the list
;; of arguments of %askpass-asked.
(define %askpass-waiting '())

;; Whether a question is put now.
(define %askpass-busy #f)

;; Whether a look at the questions is to come.
(define %askpass-timer #f)

;; Puts the next question, if none is put. While the user types in a
;; different prompt, the question waits.
(define (%askpass-next)
  (cond
   [(or %askpass-busy (null? %askpass-waiting)) #f]
   [(in-minibuffer?)
    (unless %askpass-timer
      (set! %askpass-timer #t)
      (message (cadr (car %askpass-waiting))
               " has a question. Complete this prompt first.")
      (run-after 500 (lambda ()
                       (set! %askpass-timer #f)
                       (%askpass-next))))]
   [else
    (let* ((question (car %askpass-waiting))
           (id (car question))
           (answered? #f))
      (set! %askpass-waiting (cdr %askpass-waiting))
      (set! %askpass-busy #t)
      ;; A prompt that the user leaves with C-g, and an error, give no
      ;; answer: the program that asked gives up.
      (unwind-protect
          (let ((answer (apply askpass-answer (cdr question))))
            (set! answered? #t)
            (%askpass-answer id answer))
        (unless answered?
          (%askpass-answer id #f))
        (set! %askpass-busy #f)
        (run-later %askpass-next)))]))

;; The editor's entry point: a program asks. The arguments are the number
;; of the question and those of askpass-answer.
(define (%askpass-asked id who host user prompt kind before)
  (set! %askpass-waiting
        (append %askpass-waiting (list (list id who host user prompt kind before))))
  (%askpass-next))
