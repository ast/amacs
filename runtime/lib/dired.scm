;;; The file manager: a buffer that lists a directory, with keys that act
;;; on the files. Two of them side by side work as two panes: copying and
;;; moving go to the other one unless told otherwise.

(define-option 'dired-show-hidden #t
  "Whether the file manager lists the files whose names start with a dot.")

(define-option 'dired-icons #t
  "Whether the file manager shows an icon before each name. The icons are
glyphs of the Nerd Fonts.")

(define-option 'dired-reuse-buffer #t
  "Whether going to another directory in the file manager replaces the
buffer, rather than leaving a buffer for each directory visited.")

(define-option 'dired-quick-access
  '(("h" "~" "Home") ("d" "~/Downloads" "Downloads") ("s" "~/src" "Source"))
  "Directories that the file manager goes to by a key: lists of the key, the
directory and a name for it.")

(define-option 'dired-confirm-drop #t
  "Whether the file manager asks before it moves or copies the files that
were dragged with the pointer.")

(set-face! 'dired-header 'bold #t)
(set-face! 'dired-directory 'fg 'blue 'bold #t)
(set-face! 'dired-symlink 'fg 'cyan)
(set-face! 'dired-executable 'fg 'green)
(set-face! 'dired-detail 'fg 'fg-dim)
(set-face! 'dired-mark 'fg 'magenta 'bold #t)
(set-face! 'dired-marked 'fg 'magenta 'bold #t)
(set-face! 'dired-flag 'fg 'red 'bold #t)
(set-face! 'dired-flagged 'fg 'red 'bold #t)

;;; What a buffer of the file manager knows

(define-record-type dired-row
  (%make-dired-row path name kind size changed link runs depth)
  dired-row?
  (path dired-row-path)
  (name dired-row-name)
  (kind dired-row-kind)
  (size dired-row-size)
  (changed dired-row-changed)
  (link dired-row-link)
  (runs dired-row-runs?)
  (depth dired-row-depth))

(define-record-type dired-state
  (%make-dired-state directory rows positions marks expanded filter hidden order)
  dired-state?
  (directory dired-state-directory)
  ;; The rows as shown, and where the name of each starts in the buffer.
  (rows dired-state-rows %dired-set-rows!)
  (positions dired-state-positions %dired-set-positions!)
  ;; The mark of each marked path: #\* or #\D.
  (marks dired-state-marks)
  ;; The directories whose contents are shown under them.
  (expanded dired-state-expanded)
  (filter dired-state-filter %dired-set-filter!)
  (hidden dired-state-hidden? %dired-set-hidden!)
  (order dired-state-order %dired-set-order!))

;; The state of each buffer of the file manager, by buffer name.
(define %dired-buffers (make-hash-table))

;; Directories visited, the latest first.
(define %dired-history '())

(define (dired-state)
  "What the file manager knows about the current buffer. It is an error if
the buffer is not one of its own."
  (or (hash-table-ref %dired-buffers (buffer-name))
      (error "This buffer does not list a directory")))

(define (dired-directory)
  "The directory that the current buffer lists."
  (dired-state-directory (dired-state)))

;;; Listing

(define (%dired-join directory name)
  (if (string-suffix? "/" directory)
      (string-append directory name)
      (string-append directory "/" name)))

(define (%dired-size bytes)
  (let loop ((size bytes) (units '("" "K" "M" "G" "T" "P")))
    (cond
      [(and (< size 1024) (equal? (car units) "")) (number->string size)]
      [(or (< size 1024) (null? (cdr units)))
       (string-append
        (if (< size 10)
            (number->string (/ (round (* size 10.0)) 10))
            (number->string (exact (round size))))
        (car units))]
      [else (loop (/ size 1024.0) (cdr units))])))

;; Glyphs of the Nerd Fonts, written by number so that they survive editors
;; and terminals that cannot show them.
(define %dired-icon-table
  '(("rs" . "\xe7a8;") ("py" . "\xe73c;") ("go" . "\xe627;")
    ("c" . "\xe61e;") ("h" . "\xe61e;") ("cpp" . "\xe61d;") ("cc" . "\xe61d;")
    ("hpp" . "\xe61d;") ("js" . "\xe781;") ("ts" . "\xe628;") ("tsx" . "\xe628;")
    ("el" . "\xe632;") ("scm" . "\xe6b1;") ("org" . "\xe633;")
    ("nix" . "\xf313;") ("md" . "\xe73e;") ("json" . "\xe60b;")
    ("toml" . "\xe615;") ("yaml" . "\xe615;") ("yml" . "\xe615;")
    ("conf" . "\xe615;") ("ini" . "\xe615;")
    ("sh" . "\xe795;") ("bash" . "\xe795;") ("zsh" . "\xe795;")
    ("lock" . "\xf023;") ("html" . "\xe736;") ("css" . "\xe749;")
    ("png" . "\xf1c5;") ("jpg" . "\xf1c5;") ("jpeg" . "\xf1c5;")
    ("gif" . "\xf1c5;") ("svg" . "\xf1c5;") ("webp" . "\xf1c5;")
    ("pdf" . "\xf1c1;") ("zip" . "\xf1c6;") ("gz" . "\xf1c6;")
    ("xz" . "\xf1c6;") ("tar" . "\xf1c6;") ("txt" . "\xf0f6;")
    ("log" . "\xf0f6;")))

(define %dired-directory-icon "\xf07b;")
(define %dired-open-directory-icon "\xf07c;")
(define %dired-file-icon "\xf016;")

(define (%dired-icon row open?)
  (if (eq? (dired-row-kind row) 'directory)
      (if open? %dired-open-directory-icon %dired-directory-icon)
      (let* ((parts (string-split (dired-row-name row) #\.))
             (found (and (pair? (cdr parts))
                         (assoc (string-downcase (last parts)) %dired-icon-table))))
        (if found (cdr found) %dired-file-icon))))

(define (%dired-sorted state entries)
  (let* ((less? (case (dired-state-order state)
                  [(time) (lambda (a b) (> (cadddr a) (cadddr b)))]
                  [(size) (lambda (a b) (> (caddr a) (caddr b)))]
                  [else #f]))
         (ordered (if less? (list-sort less? entries) entries)))
    ;; Directories come first, as in a file manager with two panes.
    (append (filter (lambda (entry) (eq? (cadr entry) 'directory)) ordered)
            (remove (lambda (entry) (eq? (cadr entry) 'directory)) ordered))))

;; The rows for a directory and for the expanded directories in it.
(define (%dired-collect state directory depth)
  (let* ((wanted (dired-state-filter state))
         (entries
          (filter
           (lambda (entry)
             (and (or (dired-state-hidden? state)
                      (not (string-prefix? "." (car entry))))
                  ;; Narrowing applies to the directory itself only.
                  (or (> depth 0) (not wanted)
                      (string-contains? (string-downcase (car entry))
                                        (string-downcase wanted)))))
           (directory-entries directory))))
    (append-map
     (lambda (entry)
       (let* ((path (%dired-join directory (car entry)))
              (row (%make-dired-row path (car entry) (cadr entry) (caddr entry)
                                    (cadddr entry) (list-ref entry 4)
                                    (list-ref entry 5) depth)))
         (if (and (eq? (cadr entry) 'directory)
                  (hash-table-contains? (dired-state-expanded state) path))
             (cons row (guard (e (#t '()))
                         (%dired-collect state path (+ depth 1))))
             (list row))))
     (%dired-sorted state entries))))

(define (%dired-name-face row mark)
  (cond [(eqv? mark #\*) 'dired-marked]
        [(eqv? mark #\D) 'dired-flagged]
        [(dired-row-link row) 'dired-symlink]
        [(eq? (dired-row-kind row) 'directory) 'dired-directory]
        [(dired-row-runs? row) 'dired-executable]
        [else #f]))

;; Writes the listing into the current buffer and puts point on the row of
;; a path, or else on the row it was on, or else on the first.
(define (%dired-render state . target)
  (let* ((previous (and (pair? (vector->list (dired-state-rows state)))
                        (%dired-row-index state)))
         (keep (cond [(pair? target) (car target)]
                     [previous (dired-row-path
                                (vector-ref (dired-state-rows state) previous))]
                     [else #f]))
         (rows (%dired-collect state (dired-state-directory state) 0))
         (icons (option 'dired-icons))
         (parts '())
         (faces '())
         (positions '())
         (at 0))
    (define (emit! text face)
      (let ((end (+ at (string-length text))))
        (when face (set! faces (cons (list at end face) faces)))
        (set! parts (cons text parts))
        (set! at end)))
    (emit! (abbreviate-file-name (dired-state-directory state)) 'dired-header)
    (emit! (string-append
            ":"
            (if (dired-state-filter state)
                (string-append "  (narrowed to " (dired-state-filter state) ")")
                "")
            "\n")
           #f)
    (for-each
     (lambda (row)
       (let ((mark (hash-table-ref (dired-state-marks state) (dired-row-path row))))
         (emit! (if mark (string mark) " ")
                (and mark (if (eqv? mark #\D) 'dired-flag 'dired-mark)))
         (emit! (string-append
                 " "
                 (string-pad-left (%dired-size (dired-row-size row)) 6)
                 "  "
                 (format-time (dired-row-changed row) "%Y-%m-%d %H:%M")
                 "  ")
                'dired-detail)
         (emit! (make-string (* 2 (dired-row-depth row)) #\space) #f)
         (emit! (cond [(not (eq? (dired-row-kind row) 'directory)) "  "]
                      [(hash-table-contains? (dired-state-expanded state)
                                             (dired-row-path row))
                       "▾ "]
                      [else "▸ "])
                'dired-detail)
         (when icons
           (emit! (string-append
                   (%dired-icon row (hash-table-contains? (dired-state-expanded state)
                                                          (dired-row-path row)))
                   " ")
                  (%dired-name-face row mark)))
         (set! positions (cons at positions))
         (emit! (dired-row-name row) (%dired-name-face row mark))
         (when (dired-row-link row)
           (emit! (string-append " -> " (dired-row-link row)) 'dired-detail))
         (emit! "\n" #f)))
     rows)
    (%replace-buffer-text! (apply string-append (reverse parts)))
    (for-each (lambda (face) (apply add-face! face)) faces)
    (%dired-set-rows! state (list->vector rows))
    (%dired-set-positions! state (list->vector (reverse positions)))
    (let ((index (or (and keep
                          (list-index (lambda (row) (equal? (dired-row-path row) keep))
                                      rows))
                     (and previous (min previous (- (length rows) 1)))
                     0)))
      (when (pair? rows)
        (goto-char (vector-ref (dired-state-positions state) (max index 0)))))))

;; The index of the row that point is on, or #f on the header.
(define (%dired-row-index state)
  (let ((index (- (line-number-at-pos) 2)))
    (and (>= index 0)
         (< index (vector-length (dired-state-rows state)))
         index)))

(define (dired-row-at-point)
  "The row of the file that point is on in the file manager. It is an error
if point is on no file."
  (let* ((state (dired-state))
         (index (%dired-row-index state)))
    (if index
        (vector-ref (dired-state-rows state) index)
        (error "There is no file on this line"))))

(define (dired-file-at-point)
  "The full name of the file that point is on in the file manager."
  (dired-row-path (dired-row-at-point)))

(define (dired-marked-files)
  "The full names of the files marked with *, in the order listed; if none
is marked, the file that point is on."
  (let* ((state (dired-state))
         (marked (filter (lambda (path)
                           (eqv? (hash-table-ref (dired-state-marks state) path) #\*))
                         (map dired-row-path (vector->list (dired-state-rows state))))))
    (if (null? marked)
        (list (dired-file-at-point))
        marked)))

(define (dired-refresh . target)
  "Reads the directory of the current buffer again. Point stays on the same
file, or goes to the one named by the optional argument."
  (apply %dired-render (dired-state) target))

;; Reads again every directory that is listed in a buffer, after files
;; have changed.
(define (%dired-refresh-all)
  (for-each
   (lambda (name)
     (if (member name (buffer-list))
         (with-current-buffer name
           (guard (e (#t #f)) (dired-refresh)))
         (hash-table-delete! %dired-buffers name)))
   (hash-table-keys %dired-buffers)))

;;; Opening

(define (%dired-buffer-name directory)
  (let ((short (abbreviate-file-name directory)))
    (if (string-suffix? "/" short) short (string-append short "/"))))

(define (dired-open directory . target)
  "Shows a directory in the file manager, in the selected window. The
optional argument is the full name of a file to put point on."
  (let* ((directory (expand-file-name directory))
         (name (%dired-buffer-name directory))
         (from (buffer-name))
         ;; The buffer left is not kept, unless another window shows it.
         (replacing (and (option 'dired-reuse-buffer)
                         (hash-table-contains? %dired-buffers from)
                         (not (equal? from name))
                         (not (member from (map window-buffer (cdr (window-list))))))))
    (unless (file-directory? directory)
      (error (string-append directory " is not a directory")))
    (set! %dired-history
          (cons directory (delete directory %dired-history)))
    (switch-to-buffer name)
    ;; A buffer that was killed behind our back comes back empty and in
    ;; another mode; what was known about it is then no longer true.
    (unless (and (hash-table-contains? %dired-buffers name)
                 (eq? (major-mode) 'dired-mode))
      (hash-table-set!
       %dired-buffers name
       (%make-dired-state directory (vector) (vector) (make-hash-table)
                          (make-hash-table) #f (option 'dired-show-hidden) 'name))
      (dired-mode)
      (set-buffer-read-only! #t)
      (set-buffer-directory! directory)
      (enable-mode! 'hl-line-mode))
    (apply dired-refresh target)
    (when replacing
      (hash-table-delete! %dired-buffers from)
      (kill-buffer from))))

;; The editor comes here when a directory is visited as if it were a file.
(define (%visit-directory directory)
  (dired-open directory))

(define-major-mode dired-mode #f "Dired"
  "Major mode of the file manager: a listing of a directory, with keys that
act on the files. Start it with M-x dired.")

(define-command (dired)
  "Show a directory in the file manager."
  (dired-open (read-directory-name "Directory: ")))

;;; Moving about

(define-command (dired-find-file)
  "Visit the file on this line; a directory is shown in the file manager."
  (let ((row (dired-row-at-point)))
    (if (eq? (dired-row-kind row) 'directory)
        (dired-open (dired-row-path row))
        (find-file (dired-row-path row)))))

(define-command (dired-find-file-other-window)
  "Visit the file on this line in another window."
  (let ((row (dired-row-at-point)))
    (select-other-window)
    (if (eq? (dired-row-kind row) 'directory)
        (dired-open (dired-row-path row))
        (find-file (dired-row-path row)))))

(define-command (dired-up-directory)
  "Show the directory that this one is in, with point on this one."
  (let* ((directory (dired-directory))
         (parent (file-name-directory directory)))
    (if parent
        (dired-open parent directory)
        (error "This is the top directory"))))

(define-command (dired-next-line)
  "Move to the next file."
  (let* ((state (dired-state))
         (index (%dired-row-index state))
         (next (if index (+ index 1) 0)))
    (when (< next (vector-length (dired-state-rows state)))
      (goto-char (vector-ref (dired-state-positions state) next)))))

(define-command (dired-previous-line)
  "Move to the previous file."
  (let* ((state (dired-state))
         (index (%dired-row-index state)))
    (when (and index (> index 0))
      (goto-char (vector-ref (dired-state-positions state) (- index 1))))))

(define-command (dired-revert)
  "Read the directory again, showing all of it."
  (%dired-set-filter! (dired-state) #f)
  (dired-refresh))

(define-command (dired-quit)
  "Leave the file manager."
  (let ((name (buffer-name)))
    (hash-table-delete! %dired-buffers name)
    (kill-buffer name)))

(define-command (dired-subtree-toggle)
  "Show the contents of the directory on this line under it, or hide them."
  (let* ((state (dired-state))
         (row (dired-row-at-point))
         (path (dired-row-path row)))
    (unless (eq? (dired-row-kind row) 'directory)
      (error "That is not a directory"))
    (if (hash-table-contains? (dired-state-expanded state) path)
        (hash-table-delete! (dired-state-expanded state) path)
        (hash-table-set! (dired-state-expanded state) path #t))
    (dired-refresh)))

(define-command (dired-narrow)
  "Show only the files whose names contain a text. g shows all again."
  (let ((text (read-string "Narrow to: ")))
    (%dired-set-filter! (dired-state) (if (string-null? text) #f text))
    (dired-refresh)))

(define-command (dired-toggle-hidden)
  "Show or hide the files whose names start with a dot."
  (let ((state (dired-state)))
    (%dired-set-hidden! state (not (dired-state-hidden? state)))
    (dired-refresh)))

(define-command (dired-sort-toggle)
  "List by name, then by time with the newest first, then by size."
  (let* ((state (dired-state))
         (next (case (dired-state-order state)
                 [(name) 'time]
                 [(time) 'size]
                 [else 'name])))
    (%dired-set-order! state next)
    (dired-refresh)
    (message "Sorted by " next)))

(define-command (dired-quick-access)
  "Go to one of the directories of the dired-quick-access option, by key."
  (let* ((entries (option 'dired-quick-access))
         (key (read-key
               (string-append
                "Go to: "
                (string-join
                 (map (lambda (entry)
                        (string-append "[" (car entry) "] " (caddr entry)))
                      entries)
                 "  "))))
         (entry (assoc key entries)))
    (if entry
        (dired-open (cadr entry))
        (error "No directory has that key"))))

(define-command (dired-history-jump)
  "Go to a directory that the file manager has shown before."
  (dired-open (completing-read "Directory: " %dired-history 'dired-history)))

(define-command (dired-copy-filename)
  "Put the names of the marked files, or of this one, on the kill ring."
  (let ((names (string-join (map file-name-nondirectory (dired-marked-files)) " ")))
    (kill-new names)
    (message names)))

;;; Marks

(define (%dired-set-mark! mark)
  (let* ((state (dired-state))
         (path (dired-file-at-point)))
    (if mark
        (hash-table-set! (dired-state-marks state) path mark)
        (hash-table-delete! (dired-state-marks state) path))
    (dired-refresh)
    (dired-next-line)))

(define-command (dired-mark)
  "Mark the file on this line, for the commands that act on several."
  (%dired-set-mark! #\*))

(define-command (dired-unmark)
  "Take the mark or the flag off the file on this line."
  (%dired-set-mark! #f))

(define-command (dired-flag-file-deletion)
  "Flag the file on this line for deletion; x deletes the flagged files."
  (%dired-set-mark! #\D))

(define-command (dired-unmark-all)
  "Take every mark and flag off."
  (hash-table-clear! (dired-state-marks (dired-state)))
  (dired-refresh))

(define-command (dired-toggle-marks)
  "Mark the files that are not marked and unmark those that are."
  (let ((state (dired-state)))
    (for-each
     (lambda (row)
       (let ((path (dired-row-path row)))
         (if (hash-table-contains? (dired-state-marks state) path)
             (hash-table-delete! (dired-state-marks state) path)
             (hash-table-set! (dired-state-marks state) path #\*))))
     (vector->list (dired-state-rows state)))
    (dired-refresh)))

;;; Acting on files

(define (%dired-describe paths)
  (if (null? (cdr paths))
      (file-name-nondirectory (car paths))
      (string-append (number->string (length paths)) " files")))

(define (%dired-clear-marks! paths)
  (let ((marks (dired-state-marks (dired-state))))
    (for-each (lambda (path) (hash-table-delete! marks path)) paths)))

(define (%dired-delete paths)
  (when (y-or-n? (string-append "Delete " (%dired-describe paths) "?"))
    (let ((failures
           (filter-map
            (lambda (path)
              (guard (e (#t (error-object-message e)))
                (delete-file path)
                #f))
            paths)))
      (%dired-clear-marks! paths)
      (%dired-refresh-all)
      (if (null? failures)
          (message "Deleted " (%dired-describe paths))
          (error (car failures))))))

(define-command (dired-do-delete)
  "Delete the marked files, or the file on this line, after asking."
  (%dired-delete (dired-marked-files)))

(define-command (dired-do-flagged-delete)
  "Delete the files flagged for deletion, after asking."
  (let* ((state (dired-state))
         (flagged (filter (lambda (path)
                            (eqv? (hash-table-ref (dired-state-marks state) path) #\D))
                          (map dired-row-path (vector->list (dired-state-rows state))))))
    (if (null? flagged)
        (error "No files are flagged for deletion")
        (%dired-delete flagged))))

;; The directory of another window of the file manager, the other pane, or
;; else this one's.
(define (%dired-other-directory)
  (let loop ((windows (cdr (window-list))))
    (cond
      [(null? windows) (dired-directory)]
      [(hash-table-ref %dired-buffers (window-buffer (car windows)))
       => dired-state-directory]
      [else (loop (cdr windows))])))

;; Where each of some files goes, given what the user answered: into the
;; destination if that is a directory, else to that name. Returns pairs of
;; a file and its new full name.
(define (%dired-destinations paths answer)
  (let* ((destination (expand-file-name answer))
         (into? (or (string-suffix? "/" answer) (file-directory? destination))))
    (cond
      [into?
       (unless (file-directory? destination)
         (if (y-or-n? (string-append "Create the directory " answer "?"))
             (make-directory destination)
             (error "There is no such directory")))
       (map (lambda (path)
              (cons path (%dired-join destination (file-name-nondirectory path))))
            paths)]
      [(null? (cdr paths)) (list (cons (car paths) destination))]
      [else (error "Several files can only go into a directory")])))

;; Moves or copies files, given pairs of a file and its new full name,
;; with `transfer` doing each one. A file that is in the way is replaced
;; only after asking. `from` is the buffer whose marks on the files go.
(define (%dired-carry-out pairs transfer done from)
  (let ((count 0))
    (for-each
     (lambda (pair)
       (when (or (not (file-exists? (cdr pair)))
                 (and (y-or-n? (string-append (abbreviate-file-name (cdr pair))
                                              " exists; replace it?"))
                      (begin (delete-file (cdr pair)) #t)))
         (transfer (car pair) (cdr pair))
         (set! count (+ count 1))))
     pairs)
    (when (member from (buffer-list))
      (with-current-buffer from (%dired-clear-marks! (map car pairs))))
    (%dired-refresh-all)
    (message done " " count (if (= count 1) " file" " files"))))

;; Copies or moves the marked files to where the user says, with `transfer`
;; doing each one: it gets a file and its new name.
(define (%dired-transfer verb done transfer)
  (let* ((paths (dired-marked-files))
         (from (buffer-name))
         (answer (read-directory-name
                  (string-append verb " " (%dired-describe paths) " to: ")
                  (string-append (abbreviate-file-name (%dired-other-directory)) "/"))))
    (%dired-carry-out (%dired-destinations paths answer) transfer done from)))

(define (%dired-run program . arguments)
  (let ((result (run-process program arguments)))
    (unless (eqv? (car result) 0)
      (error (string-append program ": " (string-trim (caddr result)))))))

(define (%dired-copy from to)
  (%dired-run "cp" "-a" "-T" "--" from to))

(define (%dired-move from to)
  ;; Renaming is at once and cannot leave half a file; between file
  ;; systems it is not possible, and the files are moved instead.
  (guard (e (#t (%dired-run "mv" "-T" "--" from to)))
    (rename-file from to)))

(define-command (dired-do-copy)
  "Copy the marked files, or the file on this line. The other pane of the
file manager is offered as where to."
  (%dired-transfer "Copy" "Copied" %dired-copy))

(define-command (dired-do-rename)
  "Move or rename the marked files, or the file on this line. The other pane
of the file manager is offered as where to."
  (%dired-transfer "Move" "Moved" %dired-move))

(define-command (dired-create-directory)
  "Create a directory in this one."
  (let* ((name (read-string "Create directory: "))
         (path (expand-file-name name (dired-directory))))
    (when (string-null? name)
      (error "A directory needs a name"))
    (make-directory path)
    (%dired-refresh-all)
    ;; Of a/b/c, a is what shows here.
    (dired-refresh (expand-file-name (car (string-split name #\/)) (dired-directory)))))

;;; The pointer
;;;
;;; A press goes to a file, a second one opens it, and with Control or
;;; Shift held it marks. Dragging carries the file under the pointer, or
;;; the marked files if it is one of them, to the directory it is let go
;;; over: a directory in a listing, or the directory of a listing, in this
;;; window or another.

;; What is being dragged: a list of the files and the buffer they were
;; taken from, from the first movement until the button is let go.
(define %dired-drag #f)

;; The index of the row at a position of the current buffer, or #f.
(define (%dired-row-index-at state pos)
  (let ((index (- (line-number-at-pos pos) 2)))
    (and (>= index 0)
         (< index (vector-length (dired-state-rows state)))
         index)))

;; Puts point on the file where the button was pressed. Returns whether
;; there is a file there.
(define (%dired-goto-press)
  (let* ((state (dired-state))
         (press (mouse-press-position))
         (index (and press (%dired-row-index-at state (cadr press)))))
    (deactivate-mark!)
    (when index
      (goto-char (vector-ref (dired-state-positions state) index)))
    index))

(define-command (dired-mouse-select)
  "Go to the file that the pointer is at."
  (set! %dired-drag #f)
  (%dired-goto-press))

(define-command (dired-mouse-open)
  "Visit the file that the pointer is at; a directory is shown in the file
manager."
  (when (%dired-goto-press)
    (dired-find-file)))

(define-command (dired-mouse-open-other-window)
  "Visit the file that the pointer is at in another window."
  (when (%dired-goto-press)
    (dired-find-file-other-window)))

(define-command (dired-mouse-toggle-mark)
  "Mark the file that the pointer is at, or take its mark off."
  (when (%dired-goto-press)
    (let ((marks (dired-state-marks (dired-state)))
          (path (dired-file-at-point)))
      (if (hash-table-contains? marks path)
          (hash-table-delete! marks path)
          (hash-table-set! marks path #\*))
      (dired-refresh))))

(define-command (dired-mouse-mark-range)
  "Mark the files from the one that point is on to the one that the
pointer is at."
  (let* ((state (dired-state))
         (from (%dired-row-index state))
         (to (%dired-goto-press)))
    (when (and from to)
      (let loop ((index (min from to)))
        (when (<= index (max from to))
          (hash-table-set! (dired-state-marks state)
                           (dired-row-path (vector-ref (dired-state-rows state) index))
                           #\*)
          (loop (+ index 1))))
      (dired-refresh))))

(define-command (dired-mouse-menu)
  "Show what can be done with the file that the pointer is at, or with the
marked files if it is one of them."
  (when (%dired-goto-press)
    (let ((choice (key-menu (%dired-describe (%dired-files-at-point))
                            '()
                            '(("o" "Open" dired-find-file)
                              ("O" "Open in another window" dired-find-file-other-window)
                              ("m" "Mark or unmark" dired-mouse-toggle-mark)
                              ("c" "Copy to..." dired-do-copy)
                              ("r" "Move or rename to..." dired-do-rename)
                              ("d" "Delete" dired-do-delete)
                              ("w" "Copy the name" dired-copy-filename)))))
      ((global-ref (car choice))))))

;; The marked files if the file at point is one of them, else that file:
;; what the pointer acts on.
(define (%dired-files-at-point)
  (let* ((path (dired-file-at-point))
         (marked (dired-marked-files)))
    (if (member path marked) marked (list path))))

(define (%dired-without-slash directory)
  (if (and (> (string-length directory) 1) (string-suffix? "/" directory))
      (substring directory 0 (- (string-length directory) 1))
      directory))

;; Where files would go if the button were let go now: a list of the
;; window that the pointer is in, the directory there (#f if the window
;; does not list one) and the index of the row of that directory (#f if
;; it is the directory of the listing).
(define (%dired-drop-target)
  (let ((at (mouse-position)))
    (and at
         (let* ((window (car at))
                (state (hash-table-ref %dired-buffers (window-buffer window))))
           (if (not state)
               (list window #f #f)
               (let* ((index (with-selected-window window
                               (%dired-row-index-at state (cadr at))))
                      (row (and index (vector-ref (dired-state-rows state) index))))
                 (cond
                  [(not row) (list window (dired-state-directory state) #f)]
                  [(eq? (dired-row-kind row) 'directory)
                   (list window (dired-row-path row) index)]
                  ;; A file stands for the directory it is in, which under
                  ;; an opened directory is not that of the listing.
                  [else
                   (list window
                         (%dired-without-slash (file-name-directory (dired-row-path row)))
                         #f)])))))))

;; The pairs of a file and its new full name for the files that a drop
;; into a directory would carry: not those that are there already, nor a
;; directory into itself or into what is within it.
(define (%dired-drop-pairs paths directory)
  (let ((directory (%dired-without-slash directory)))
    (filter-map
     (lambda (path)
       (and (not (equal? (%dired-without-slash (file-name-directory path)) directory))
            (not (equal? path directory))
            (not (string-prefix? (string-append path "/") directory))
            (cons path (%dired-join directory (file-name-nondirectory path)))))
     paths)))

(define (%dired-drop-verb)
  (if (memq 'control (mouse-modifiers)) "Copy" "Move"))

(define-command (dired-mouse-drag)
  "Carry the file that the button was pressed on, or the marked files if
it is one of them, and say where they would go."
  (unless %dired-drag
    (when (%dired-goto-press)
      (set! %dired-drag (list (%dired-files-at-point) (buffer-name)))))
  (when %dired-drag
    (let* ((paths (car %dired-drag))
           (target (%dired-drop-target))
           (directory (and target (cadr target))))
      ;; The line of the directory under the pointer shows where to.
      (when (and target (caddr target))
        (with-selected-window (car target)
          (goto-char (vector-ref (dired-state-positions (dired-state)) (caddr target)))))
      (cond
       [(and directory (pair? (%dired-drop-pairs paths directory)))
        (message (%dired-drop-verb) " " (%dired-describe paths) " to "
                 (abbreviate-file-name directory) "/")]
       [(and target (not directory)
             (null? (cdr paths)) (not (file-directory? (car paths))))
        (message "Open " (%dired-describe paths) " in that window")]
       [else (message "Nowhere to put " (%dired-describe paths) " here")]))))

(define-command (dired-mouse-drop)
  "Move the files that were dragged to the directory that the pointer is
over, or copy them if Control is held. A file let go over a window that
shows something else is visited there."
  (let ((drag %dired-drag)
        (target (%dired-drop-target)))
    (set! %dired-drag #f)
    (when (and drag target)
      (let* ((paths (car drag))
             (directory (cadr target))
             (pairs (if directory (%dired-drop-pairs paths directory) '()))
             (copy? (memq 'control (mouse-modifiers))))
        (cond
         [(pair? pairs)
          (when (or (not (option 'dired-confirm-drop))
                    (y-or-n? (string-append
                              (%dired-drop-verb) " " (%dired-describe (map car pairs))
                              " to " (abbreviate-file-name directory) "/?")))
            (%dired-carry-out pairs
                              (if copy? %dired-copy %dired-move)
                              (if copy? "Copied" "Moved")
                              (cadr drag)))]
         [(and (not directory) (null? (cdr paths)) (not (file-directory? (car paths))))
          (select-window (car target))
          (find-file (car paths))]
         [else (message "Nothing was moved")])))))

;;; Keys

(for-each
 (lambda (binding) (define-key 'dired-mode (car binding) (cadr binding)))
 '(("RET" dired-find-file) ("f" dired-find-file) ("e" dired-find-file)
   ("o" dired-find-file-other-window)
   ("^" dired-up-directory) ("DEL" dired-up-directory)
   ("n" dired-next-line) ("p" dired-previous-line) ("SPC" dired-next-line)
   ("<down>" dired-next-line) ("<up>" dired-previous-line)
   ("C-n" dired-next-line) ("C-p" dired-previous-line)
   ("g" dired-revert) ("q" dired-quit)
   ("TAB" dired-subtree-toggle) ("/" dired-narrow)
   ("." dired-toggle-hidden) ("s" dired-sort-toggle)
   ("b" dired-quick-access) ("r" dired-history-jump) ("w" dired-copy-filename)
   ("m" dired-mark) ("u" dired-unmark) ("U" dired-unmark-all)
   ("t" dired-toggle-marks) ("d" dired-flag-file-deletion)
   ("x" dired-do-flagged-delete) ("D" dired-do-delete)
   ("C" dired-do-copy) ("R" dired-do-rename) ("+" dired-create-directory)))

(for-each
 (lambda (binding) (define-key 'dired-mode (car binding) (cadr binding)))
 '(("<mouse-1>" dired-mouse-select) ("<triple-mouse-1>" dired-mouse-select)
   ("<double-mouse-1>" dired-mouse-open)
   ("<mouse-2>" dired-mouse-open-other-window)
   ("C-<mouse-1>" dired-mouse-toggle-mark) ("S-<mouse-1>" dired-mouse-mark-range)
   ("<mouse-3>" dired-mouse-menu)
   ("<motion-mouse-1>" dired-mouse-drag) ("<drag-mouse-1>" dired-mouse-drop)))

(global-set-key "C-x d" 'dired)
