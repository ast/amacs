;;; Working in buffers and windows, and waiting.

(define (%call-in-buffer name thunk)
  (let ((previous (buffer-name)))
    (if (equal? name previous)
        (thunk)
        (begin
          (%set-buffer name)
          (unwind-protect (thunk) (%set-buffer previous))))))

(define-syntax with-current-buffer
  (syntax-rules ()
    [(_ buffer body ...)
     (%call-in-buffer buffer (lambda () body ...))]))

(set-documentation! 'with-current-buffer
  "(with-current-buffer name body ...)
Runs the body with the named buffer as the current one, and returns what
it returns. The buffer's own point is used, and is left where the body
puts it.")

(define (%call-in-window window thunk)
  ;; The window may be in another frame: that frame gets its own selected
  ;; window back too. A window that the body closed is not selected again.
  (let ((previous (selected-window))
        (theirs (frame-selected-window window)))
    (select-window window)
    (unwind-protect
     (thunk)
     (begin
       (when (and (not (equal? theirs previous)) (window-live? theirs))
         (select-window theirs))
       (when (window-live? previous)
         (select-window previous))))))

(define-syntax with-selected-window
  (syntax-rules ()
    [(_ window body ...)
     (%call-in-window window (lambda () body ...))]))

(set-documentation! 'with-selected-window
  "(with-selected-window window body ...)
Runs the body with a window selected, and so its buffer current, and
selects the window that was selected before again. The window may be one
of another frame.")

(define-syntax save-excursion
  (syntax-rules ()
    [(_ body ...)
     (let ((buffer (buffer-name)) (saved (point)))
       (unwind-protect
        (begin body ...)
        (when (member buffer (buffer-list))
          (with-current-buffer buffer
            (goto-char (min saved (point-max)))))))]))

(set-documentation! 'save-excursion
  "(save-excursion body ...)
Runs the body and puts point back where it was. The place is remembered
as a position, so text the body inserts before it moves it.")

(define (read-key . prompt)
  "Waits for a key and returns it as a string such as \"a\", \"C-x\" or
\"RET\". The optional argument is shown in the echo area meanwhile. C-g
abandons the running command."
  (await (%read-key (if (null? prompt) "" (car prompt)))))

(define (read-char prompt)
  "Waits for a key that stands for a char and returns the char. Any other
key is an error."
  (let ((key (read-key prompt)))
    (cond [(equal? key "SPC") #\space]
          [(= (string-length key) 1) (string-ref key 0)]
          [else (error "That is not a character:" key)])))

;; Files on other hosts
;;
;; Their names are tramp's, /ssh:user@host:/path. Reading and writing them
;; takes the time that the way there takes, so both happen in the background
;; and are waited for here, while the editor goes on working.

;; The editor comes here to visit a file on another host. With `ask?`, a
;; person asked for it at a prompt: what they type meanwhile waits for the
;; file, and a file above large-file-threshold is asked about, which is
;; else an error unless `anyway`.
(define (%visit-remote path anyway ask?)
  (message "Reading " path "...")
  (let* ((found (await (%fetch-file path anyway ask?)))
         (what (car found)))
    (cond ((eq? what 'buffer)
           (switch-to-buffer (cadr found))
           (message (or (caddr found) "")))
          ((eq? what 'directory)
           (message "")
           (%visit-directory (cadr found)))
          ((and (eq? what 'large) ask?)
           (if (y-or-n? (string-append (cadr found) "; open it anyway?"))
               (%visit-remote path #t #t)
               (message "")))
          ((eq? what 'large)
           (error (string-append (cadr found) ", above large-file-threshold")))
          (else (error (cadr found))))))

(define (find-file name . anyway)
  "Visits a file in the selected window. A file that is not text is an
error, and so is one above the size of the large-file-threshold option,
unless the second argument is given and not #f. A file on another host,
named as tramp names it, /ssh:user@host:/path, is read while the editor
goes on working, and is there when this returns."
  (let ((anyway (and (pair? anyway) (car anyway))))
    (if (file-remote? name)
        (%visit-remote (expand-file-name name) anyway #f)
        (%find-file name anyway))))

;; Does work on files in the background and waits for it: see
;; %start-file-job. Returns what the work gives, or nothing.
(define (%file-job . what)
  (let ((outcome (await (apply %start-file-job what))))
    (cond ((not (car outcome)) (error (cadr outcome)))
          ((pair? (cdr outcome)) (cadr outcome)))))

(define (directory-entries directory)
  "What a directory holds, in order of name: for each entry a list of its
name, 'directory or 'file (a link counts as what it leads to), its size in
bytes, when it was last changed in seconds since 1970, what it links to if
it is a link (else #f), and whether it can be run as a program. A directory
on another host is read while the editor goes on working."
  (if (file-remote? directory)
      (%file-job 'list directory)
      (%directory-entries directory)))

(define (delete-file name)
  "Deletes a file or a link. A directory is deleted with everything in it.
On another host that is done while the editor goes on working, and is done
when this returns."
  (if (file-remote? name)
      (%file-job 'delete name)
      (%delete-file name)))

(define (rename-file from to)
  "Gives a file or directory another name, which can be in another
directory on the same file system, and on the same host. It is an error if
something has the new name already. See move-file for what goes anywhere."
  (if (or (file-remote? from) (file-remote? to))
      (%file-job 'rename from to)
      (%rename-file from to)))

(define (copy-file from to)
  "Copies a file to a new name, on this machine or on another host: a
directory with everything in it, a link as a link, with permissions and
times. The editor goes on working meanwhile. It is an error if something
has the new name already."
  (%file-job 'copy from to))

(define (move-file from to)
  "Moves a file or directory to a new name: by renaming it where that can
be, and else, as between hosts, by copying it and then deleting it. The
editor goes on working meanwhile. It is an error if something has the new
name already."
  (%file-job 'move from to))

(define (sleep-for milliseconds)
  "Waits, while the editor goes on working."
  (await (%timer milliseconds)))

(define (run-process program arguments . input)
  "Runs a program with a list of arguments, in the working directory and
with the environment of the project, and waits for it while the editor
goes on working. The optional argument is text for its standard input.
Returns a list of the exit status, what it wrote to standard output and
what it wrote to standard error. The status is #f if the program could not
be run."
  (await (%run-process program arguments (if (null? input) "" (car input)))))

(define (process-lines program . arguments)
  "Runs a program with arguments and returns the lines it wrote. It is an
error if the program fails."
  (let ((result (run-process program arguments)))
    (if (eqv? (car result) 0)
        (let ((lines (string-split (cadr result) #\newline)))
          ;; What ends with a newline has no line after it.
          (if (and (pair? lines) (equal? (last lines) ""))
              (reverse (cdr (reverse lines)))
              lines))
        (error (string-append program " failed: " (string-trim (caddr result)))))))

(define-command (scratch-buffer)
  "Switch to the *scratch* buffer, making it anew if it was killed."
  (switch-to-buffer "*scratch*"))
