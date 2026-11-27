;;; Telemetry: what the editor was used for and how long it took.
;;;
;;; The editor counts what runs, by name, and the time it takes. Nothing of
;;; what was typed or of any buffer is kept, and nothing leaves the machine:
;;; the numbers of all sessions are summed in the file `telemetry` of the
;;; state directory. `(set-option! 'telemetry #f)` stops the counting.
;;;
;;; The kinds of things that are counted:
;;;   event      one thing from the user or the system, with all it leads
;;;              to: key, paste, mouse, resize, and poll for a timer or
;;;              the output of a program that Scheme code was waiting for
;;;   redisplay  working out what a frame shows, when that has changed
;;;   command    a command, by its name; one written in Scheme up to where
;;;              it first waits
;;;   hook       the functions of a hook, by the hook's name
;;;   call       other calls into Scheme, by the procedure of the runtime
;;;              they go through; resume is code going on after a wait
;;;   scheme     run is all time in the interpreter, collect the part of
;;;              it that went to collecting garbage
;;;   load       reading the runtime at the start, and files of Scheme
;;;
;;; M-x show-telemetry shows the numbers. A program asks with
;;; (telemetry-report), or with (telemetry) for data to work on.

(define-record-type telemetry-entry
  (%make-telemetry-entry kind name count total max slow)
  telemetry-entry?
  (kind telemetry-entry-kind)
  (name telemetry-entry-name)
  (count telemetry-entry-count)
  (total telemetry-entry-total)
  (max telemetry-entry-max)
  (slow telemetry-entry-slow))

(define (telemetry-entry-mean entry)
  "The milliseconds that one time took on average."
  (/ (telemetry-entry-total entry) (telemetry-entry-count entry)))

(define (telemetry . all?)
  "(telemetry [all?]) What ran and how long it took: a list of entries,
the one that took most time in all first. With a true argument the numbers
are of every session, else of this one. An entry has a kind, a symbol such
as command or hook, a name, a string, a count, the milliseconds in all and
of the longest time, and the count of slow times, those of 16 ms or more;
`telemetry-entry-kind` and its like give them."
  (sort (map (lambda (row)
               (%make-telemetry-entry (list-ref row 0)
                                      (list-ref row 1)
                                      (list-ref row 2)
                                      (/ (list-ref row 3) 1000.0)
                                      (/ (list-ref row 4) 1000.0)
                                      (list-ref row 5)))
             (%telemetry (and (pair? all?) (car all?) #t)))
        (lambda (a b) (> (telemetry-entry-total a) (telemetry-entry-total b)))))

(define (telemetry-of kind . all?)
  "(telemetry-of kind [all?]) The entries of `telemetry` of one kind, such
as 'command."
  (filter (lambda (entry) (eq? (telemetry-entry-kind entry) kind))
          (apply telemetry all?)))

;; The kinds in the order they are shown, with what follows theirs.
(define %telemetry-kinds '(event redisplay command hook call scheme load))

;; A number of milliseconds with two decimals.
(define (%telemetry-ms ms)
  (let* ((hundredths (exact (round (* ms 100))))
         (whole (quotient hundredths 100))
         (rest (remainder hundredths 100)))
    (string-append (number->string whole) "."
                   (string-pad-left (number->string rest) 2 #\0))))

(define (%telemetry-line name . columns)
  (apply string-append
         (string-pad-right name 32)
         (append (map (lambda (column) (string-pad-left column 11)) columns)
                 '("\n"))))

(define (%telemetry-section kind entries)
  (apply string-append
         (%telemetry-line (symbol->string kind) "count" "total ms" "mean ms" "max ms" "slow")
         (append
          (map (lambda (entry)
                 (%telemetry-line
                  (string-append "  " (telemetry-entry-name entry))
                  (number->string (telemetry-entry-count entry))
                  (%telemetry-ms (telemetry-entry-total entry))
                  (%telemetry-ms (telemetry-entry-mean entry))
                  (%telemetry-ms (telemetry-entry-max entry))
                  (number->string (telemetry-entry-slow entry))))
               entries)
          '("\n"))))

(define (telemetry-report . all?)
  "(telemetry-report [all?]) The numbers of `telemetry` as text: a table
for each kind of thing that ran, with what took most time in all first."
  (let* ((entries (apply telemetry all?))
         (kinds (append %telemetry-kinds
                        (filter (lambda (kind) (not (memq kind %telemetry-kinds)))
                                (delete-duplicates (map telemetry-entry-kind entries))))))
    (apply string-append
           (map (lambda (kind)
                  (let ((of-kind (filter (lambda (entry)
                                           (eq? (telemetry-entry-kind entry) kind))
                                         entries)))
                    (if (null? of-kind) "" (%telemetry-section kind of-kind))))
                kinds))))

(define-major-mode telemetry-mode special-mode "Telemetry"
  "Major mode of the buffer that shows what ran and how long it took."
  (set-buffer-read-only! #t))

(define-command (show-telemetry)
  "Show what ran in this session and how long it took. With a prefix argument, in every session."
  (let ((report (telemetry-report (prefix-arg-given?))))
    (switch-to-buffer "*Telemetry*")
    (unless (eq? (major-mode) 'telemetry-mode)
      (telemetry-mode))
    (%replace-buffer-text!
     (if (string=? report "") "Nothing has been counted.\n" report))
    (goto-char 0)))

(define-command (clear-telemetry)
  "Forget what ran and how long it took, in this session and in those before."
  (%telemetry-clear!)
  (message "Telemetry cleared"))
