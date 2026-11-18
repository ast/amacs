;;; Finding out what the editor has and what code does: for people at
;;; M-:, and for programs that work on the editor from outside.

;;; Tracing

;; The procedures that are being traced, by name, as they were before.
(define %traced (make-hash-table))

(define (trace! name)
  "Logs every call of a procedure defined at the top level, by name: its
arguments when it is called, and what it returns or raises. The log is the
*Messages* buffer. A key that runs a command does not go through the
command's name; use add-advice! to see those."
  (unless (hash-table-contains? %traced name)
    (let ((original (global-ref name)))
      (unless (procedure? original)
        (error "only procedures can be traced; this is not one:" name))
      (hash-table-set! %traced name original)
      (global-set!
       name
       (lambda arguments
         (log-message (string-append "trace: " (write->string (cons name arguments))))
         (let ((result
                (guard (raised (#t (log-message
                                    (string-append
                                     "trace: " (symbol->string name) " raised "
                                     (if (error-object? raised)
                                         (error-object->string raised)
                                         (write->string raised))))
                                   (raise raised)))
                  (apply original arguments))))
           (log-message (string-append "trace: " (symbol->string name) " => "
                                       (write->string result)))
           result))))))

(define (untrace! name)
  "Stops logging the calls of a procedure."
  (when (hash-table-contains? %traced name)
    (global-set! name (hash-table-ref %traced name))
    (hash-table-delete! %traced name)))

(define (traced)
  "The names of the procedures that are being traced."
  (hash-table-keys %traced))

(define-syntax time
  (syntax-rules ()
    [(_ expression)
     (let* ((start (current-jiffy))
            (value expression))
       (log-message (string-append "time: " (write->string 'expression) " took "
                                   (number->string (/ (- (current-jiffy) start) 1000.0))
                                   " ms"))
       value)]))

(set-documentation! 'time
  "(time expression)
Evaluates the expression, logs how long it took to *Messages*, and returns
its value.")

;;; The log

(define (messages . count)
  "The last lines of the message log, as one string: 40 of them, or as many
as the argument says."
  (let* ((wanted (if (null? count) 40 (car count)))
         (text (if (member "*Messages*" (buffer-list)) (buffer-string "*Messages*") ""))
         (lines (string-split (string-trim-right text) #\newline))
         (skip (max 0 (- (length lines) wanted))))
    (string-join (list-tail lines skip) "\n")))

;;; What there is

(define (%first-line text)
  (car (string-split text #\newline)))

(define (describe-fully name)
  "Everything known about a name, as text: what describe says of it, and
also whether it is a command, with its advice, or an option, with its
value in the current buffer."
  (string-append
   (describe name)
   (if (command? name)
       (string-append
        "\nIt is a command"
        (if (%native-command? name) ", built in" "")
        (if (%script-command name) ", defined or advised in Scheme" "")
        ": " (or (command-documentation name) "")
        (let ((advice (command-advice name)))
          (if (null? advice)
              ""
              (string-append "\nAdvice: " (write->string (map (lambda (entry)
                                                                (list (car entry) (cadr entry)))
                                                              advice))))))
       "")
   (guard (e (#t ""))
     (string-append "\nIt is an option; its value here is "
                    (write->string (option name))))))

(define (apropos-lines pattern)
  "What has a text in its name, a line for each with the start of its
documentation: procedures, macros and variables, then commands. A command
is listed once, as a command. The names that start with %, which are the
inner parts of things, are only counted, unless the text has a % in it."
  (define (line kind name doc)
    (string-append kind " " name (if doc (string-append " — " (%first-line doc)) "")))
  (define (inner? name)
    (and (not (string-contains? pattern "%"))
         (string-prefix? "%" (symbol->string name))))
  (let* ((commands (filter (lambda (command) (string-contains? command pattern))
                           (command-names)))
         (found (filter (lambda (name) (not (member (symbol->string name) commands)))
                        (apropos pattern)))
         (names (filter (lambda (name) (not (inner? name))) found))
         (inner (- (length found) (length names))))
    (string-join
     (append
      (map (lambda (name)
             (line (cond [(macro? name) "macro    "]
                         [(procedure? (global-ref name #f)) "procedure"]
                         [else "variable "])
                   (symbol->string name)
                   (documentation name)))
           names)
      (map (lambda (command)
             (line "command  " command (command-documentation command)))
           commands)
      (if (> inner 0)
          (list (string-append "(and " (number->string inner)
                               " whose names start with %; put a % in the text to see them)"))
          '()))
     "\n")))

(define (buffer-lines)
  "The buffers, a line for each: its name, its major mode, the file it
visits, and whether it has changes that are not saved."
  (string-join
   (map (lambda (name)
          (string-append
           name
           "  (" (symbol->string (with-current-buffer name (major-mode))) ")"
           (let ((file (buffer-file-name name)))
             (if file (string-append "  " file) ""))
           (if (buffer-modified? name) "  [modified]" "")))
        (buffer-list))
   "\n"))

(define (test-report)
  "Runs the tests defined with deftest and says how it went, as text."
  (let* ((result (run-tests))
         (failures (cdr result)))
    (string-append
     (number->string (car result)) " passed, "
     (number->string (length failures)) " failed"
     (apply string-append
            (map (lambda (failure)
                   (string-append "\nFAILED " (symbol->string (car failure))
                                  ": " (cadr failure)))
                 failures)))))
