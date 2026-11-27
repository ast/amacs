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

;; Files that take a while to read
;;
;; A file on another host, whose name is tramp's, /ssh:user@host:/path,
;; takes the time that the way there takes to read and to write, so both
;; happen in the background and are waited for here, while the editor goes
;; on working. A large file of this machine is read the same way.

;; The editor comes here to visit such a file. With `ask?`, a person asked
;; for it at a prompt: what they type meanwhile waits for the file, and a
;; file above large-file-threshold is asked about, which is else an error
;; unless `anyway`. With a fourth argument that is not #f, what is typed
;; waits too, without the question.
(define (%visit-remote path anyway ask? . hold?)
  (message "Reading " path "...")
  (let* ((found (await (%fetch-file path anyway (or ask? (and (pair? hold?) (car hold?))))))
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
goes on working, and so is a file of some megabytes; either is there when
this returns."
  (let ((anyway (and (pair? anyway) (car anyway))))
    (cond
     [(or (file-remote? name) (%file-read-in-background? name))
      (%visit-remote (expand-file-name name) anyway #f)]
     [(file-handler name) (%visit-handled (expand-file-name name))]
     [else (%find-file name anyway)])))

;;; Files whose bytes are not their text

;; Lists of the end of a name, the procedure that reads, the one that
;; writes, and whether the text is a secret.
(define %file-handlers '())

(define (define-file-handler suffix visit save . secret?)
  "Makes procedures read and write the files whose names end in a suffix,
such as \".gpg\": files whose bytes are not their text. `visit` is called
with the name of a file and returns its text, and `save` is called with the
name and the text and writes the file; both may wait for a program. Such a
file is visited, saved and read again as any other. With a fourth argument
that is not #f, the text is a secret: the buffer has the buffer-secret
option set from the start. A file on another host is not handled."
  (set! %file-handlers
        (cons (list suffix visit save (and (pair? secret?) (car secret?) #t))
              (filter (lambda (handler) (not (equal? (car handler) suffix)))
                      %file-handlers)))
  (%set-file-handler-suffixes! (map car %file-handlers)))

(define (file-handler name)
  "The handler of a file, by its name: a list of the suffix, the procedure
that reads, the one that writes, and whether the text is a secret. #f for
a file that the editor reads itself."
  (and (not (file-remote? name))
       (find (lambda (handler) (string-suffix? (car handler) name))
             %file-handlers)))

;; The editor comes here to visit a file that has a handler. A file that
;; is not there yet gives an empty buffer, as any new file does. The mode
;; is the one that the name asks for without the handler's suffix.
(define (%visit-handled path)
  (let ((visiting (find (lambda (buffer) (equal? (buffer-file-name buffer) path))
                        (buffer-list))))
    (if visiting
        (switch-to-buffer visiting)
        (let* ((handler (file-handler path))
               (new? (not (file-exists? path)))
               (text (if new? "" ((cadr handler) path)))
               (buffer (%adopt-text! path text (list-ref handler 3))))
          (switch-to-buffer buffer)
          (when (eq? (major-mode) 'fundamental-mode)
            (let ((mode (%auto-mode-for
                         (substring path 0 (- (string-length path)
                                              (string-length (car handler)))))))
              (when mode (call-command mode))))
          (when new? (message "(New file)"))))))

;; The editor comes here to save a buffer to a file that has a handler.
;; The text is taken while the buffer is current, and written while the
;; editor goes on working.
(define (%save-handled buffer path kill?)
  (let* ((handler (file-handler path))
         (problem #f)
         (taken (%call-in-buffer
                 buffer
                 (lambda ()
                   (set! problem
                         (guard (e (#t (if (error-object? e)
                                           (error-object->string e)
                                           (write->string e))))
                           (run-hooks 'before-save-hook)
                           #f))
                   (cons (buffer-tick) (buffer-string))))))
    ((caddr handler) path (cdr taken))
    (%call-in-buffer buffer (lambda () (%mark-written! path (car taken))))
    (when problem
      (message "Saved, but before-save-hook failed: " problem))
    (when kill? (kill-buffer buffer))))

;; The editor comes here to read a file that has a handler again.
(define (%revert-handled buffer)
  (let* ((path (%call-in-buffer buffer buffer-file-name))
         (handler (and path (file-handler path))))
    (when handler
      (%adopt-text! path ((cadr handler) path) (list-ref handler 3)))))

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

(define (run-process program arguments . more)
  "Runs a program with a list of arguments, in the working directory and
with the environment of the project, and waits for it while the editor
goes on working. The first optional argument is text for its standard
input. The second is environment variables to set for it, as a list of
lists of a name and a value. Returns a list of the exit status, what it
wrote to standard output and what it wrote to standard error. The status
is #f if the program could not be run."
  (await (%run-process program arguments
                       (if (null? more) "" (car more))
                       (if (or (null? more) (null? (cdr more))) '() (cadr more)))))

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
