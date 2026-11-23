;;; Programs that run while the editor goes on.
;;;
;;; `run-process` runs a program to its end and waits for it. A process
;;; made with `make-process` goes on as long as it likes, a shell or a
;;; server, and the editor hears from it as it writes: its filter gets the
;;; text and its sentinel the end. Processes are numbers, as windows are.
;;;
;;;   (make-process "shell" '("sh")
;;;                 'buffer "*shell*"
;;;                 'sentinel (lambda (p status) (message "shell: " status)))
;;;
;;; A process given a buffer writes into it unless it has a filter of its
;;; own, and is ended when the buffer is killed.

;; The buffer of each process that has one, by process.
(define %process-buffers (make-hash-table))

(define (%property properties name default)
  (let loop ((rest properties))
    (cond [(or (null? rest) (null? (cdr rest))) default]
          [(eq? (car rest) name) (cadr rest)]
          [else (loop (cddr rest))])))

(define (make-process name command . properties)
  "Starts a program that goes on running and returns the process, a number.
`command` is a list of the program and its arguments. After it come
properties, each a symbol and a value:

  'filter      a procedure called with the process and the text its
               program writes, as it writes it
  'sentinel    a procedure called with the process and the exit status
               when the program ends; the status is #f after a signal
  'buffer      the name of a buffer the process belongs to: without a
               filter, the output is inserted at its end, and killing
               the buffer ends the process
  'directory   where to run; the working directory unless given
  'environment environment variables to set, over those of the
               directory's project: a list of lists of a name and a value
  'pty         #t, or a list of columns and rows, to run the program on a
               pseudo-terminal of that size (80 by 24 unless given), as a
               shell or a full screen program needs; its standard error
               then comes with its output
  'stderr      a procedure called with the process and the text the program
               writes to standard error, keeping it apart from the output,
               which a protocol on standard output needs; without it the
               error is merged into the output

The program runs in a process group of its own. Nothing waits for it, so
typing goes on meanwhile. An error if the program cannot be started."
  (when (or (null? command) (not (string? (car command))))
    (error "make-process: the command must be a list starting with the program:" command))
  (let* ((buffer (%property properties 'buffer #f))
         (filter (or (%property properties 'filter #f)
                     (and buffer (%process-buffer-filter buffer))))
         (process (%make-process name (car command) (cdr command)
                                 (%property properties 'directory #f)
                                 (%property properties 'environment '())
                                 filter
                                 (%property properties 'sentinel #f)
                                 (let ((pty (%property properties 'pty #f)))
                                   (cond [(pair? pty) pty]
                                         [pty '(80 24)]
                                         [else #f]))
                                 (%property properties 'stderr #f))))
    (when buffer
      (hash-table-set! %process-buffers process buffer))
    process))

(define (%process-buffer-filter buffer)
  (lambda (process text)
    (when (member buffer (buffer-list))
      (with-current-buffer buffer
        (save-excursion
          (goto-char (point-max))
          (insert text))))))

(define (process-buffer process)
  "The name of the buffer a process was given, or #f."
  (hash-table-ref %process-buffers process #f))

(define (get-buffer-process buffer)
  "The live process that was given the buffer, or #f."
  (let loop ((processes (process-list)))
    (cond [(null? processes) #f]
          [(and (equal? (process-buffer (car processes)) buffer)
                (process-live? (car processes)))
           (car processes)]
          [else (loop (cdr processes))])))

(define (kill-process process)
  "Ends a process's program with the kill signal. The sentinel is called
when it has ended."
  (signal-process process 'kill))

(define (interrupt-process process)
  "Interrupts a process's program, as C-c in a terminal does."
  (signal-process process 'int))

;; A buffer that goes takes its process with it.
(add-hook! 'kill-buffer-hook
  (lambda (name)
    (for-each
     (lambda (process)
       (when (equal? (process-buffer process) name)
         (hash-table-delete! %process-buffers process)
         (delete-process process)))
     (process-list))))

;;; The list of processes

(define %process-list-buffer "*Process List*")

;; The processes shown, one per row, in order.
(define %process-list-rows '())

(set-face! 'process-list-header 'bold #t)

(define-major-mode process-list-mode #f "Processes"
  "The processes started by Scheme code, one per line: name, status, the
system's id, the buffer and the program. `k` ends the one at point, `d`
forgets it, `g` lists again."
  (set-buffer-read-only! #t))

(define (%process-status-label process)
  (let ((status (process-exit-status process)))
    (cond [(eq? status 'running) "run"]
          [(not status) "signal"]
          [else (string-append "exit " (number->string status))])))

(define (%process-list-render!)
  (let ((rows (map (lambda (process)
                     (list process
                           (process-name process)
                           (%process-status-label process)
                           (number->string (process-id process))
                           (or (process-buffer process) "")
                           (process-program process)))
                   (process-list))))
    (%replace-buffer-text!
     (apply string-append
            "Name              Status    PID      Buffer                Program\n"
            (map (lambda (row)
                   (string-append (string-pad-right (list-ref row 1) 18)
                                  (string-pad-right (list-ref row 2) 10)
                                  (string-pad-right (list-ref row 3) 9)
                                  (string-pad-right (list-ref row 4) 22)
                                  (list-ref row 5) "\n"))
                 rows)))
    (clear-faces!)
    (add-face! 0 (string-length "Name              Status    PID      Buffer                Program")
               'process-list-header)
    (set! %process-list-rows rows)))

(define (%process-at-point)
  (let ((line (- (line-number-at-pos) 2))
        (rows %process-list-rows))
    (and (>= line 0) (< line (length rows))
         (car (list-ref rows line)))))

(define-command (list-processes)
  "Show the processes started by Scheme code in a buffer."
  (switch-to-buffer %process-list-buffer)
  (unless (eq? (major-mode) 'process-list-mode)
    (process-list-mode))
  (%process-list-render!)
  (goto-line 2))

(define-command (process-list-refresh)
  "List the processes again."
  (let ((line (line-number-at-pos)))
    (%process-list-render!)
    (goto-line line)))

(define-command (process-list-kill)
  "End the program of the process at point."
  (let ((process (%process-at-point)))
    (unless process (error "No process on this line"))
    (kill-process process)
    (message "Killed " (process-name process))))

(define-command (process-list-delete)
  "Forget the process at point, ending its program if it runs."
  (let ((process (%process-at-point)))
    (unless process (error "No process on this line"))
    (delete-process process)
    (hash-table-delete! %process-buffers process)
    (process-list-refresh)))

(define-key 'process-list-mode "g" 'process-list-refresh)
(define-key 'process-list-mode "k" 'process-list-kill)
(define-key 'process-list-mode "d" 'process-list-delete)
(define-key 'process-list-mode "n" 'next-line)
(define-key 'process-list-mode "p" 'previous-line)
(define-command (process-list-quit)
  "Leave the list of processes."
  (kill-buffer))

(define-key 'process-list-mode "q" 'process-list-quit)
