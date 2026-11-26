;;; Compressed files and archives in the file manager: Z compresses or
;;; uncompresses each file where it is, z packs files into one archive, and
;;; visiting an archive lists what is in it. The work is done by tar, zip,
;;; unzip and the compressors, which must be installed; files on another
;;; host are not handled, as programs do not run there.

(define-option 'archive-compress-extension "zst"
  "What the file manager compresses with when Z is pressed on a file that
is not compressed, as the extension without the dot: \"zst\", \"gz\", \"xz\"
or \"bz2\". A directory becomes a tar archive compressed that way.")

;; Lists of a suffix and what handles it: tar, zip, or the name of a
;; program that compresses one file. An earlier suffix wins over a later
;; one that it ends in.
(define %archive-formats
  '((".tar.zst" tar) (".tar.gz" tar) (".tgz" tar) (".tar.xz" tar)
    (".tar.bz2" tar) (".tar" tar) (".zip" zip)
    (".zst" "zstd") (".gz" "gzip") (".xz" "xz") (".bz2" "bzip2")))

;; The entry of %archive-formats for a file name, or #f.
(define (%archive-format path)
  (find (lambda (format) (string-suffix? (car format) path)) %archive-formats))

(define (archive-file? path)
  "Whether a file name is that of an archive that can be listed: a tar
archive, compressed or not, or a zip file."
  (let ((format (%archive-format path)))
    (and format (symbol? (cadr format)))))

;; A file name without the suffix that says how it is packed.
(define (%archive-stem path)
  (let ((format (%archive-format path)))
    (if format
        (substring path 0 (- (string-length path) (string-length (car format))))
        path)))

;; The suffix that Z gives a file, and the program that compresses to it.
(define (%archive-compressor)
  (let* ((suffix (string-append "." (option 'archive-compress-extension)))
         (format (assoc suffix %archive-formats)))
    (unless (and format (string? (cadr format)))
      (error (string-append "The archive-compress-extension option is "
                            (option 'archive-compress-extension)
                            ": use zst, gz, xz or bz2")))
    format))

;;; Running the programs

;; zip and unzip read and write names that are not ASCII rightly only
;; when the locale is one of UTF-8, whatever the editor was started with.
(define %archive-utf-8 "LC_ALL=C.UTF-8")

;; Runs a program and returns what it wrote. It is an error if it failed.
(define (%archive-run program . arguments)
  (let ((result (if (equal? program "unzip")
                    (run-process "env" (cons %archive-utf-8 (cons program arguments)))
                    (run-process program arguments))))
    (cond
      [(eqv? (car result) 0) (cadr result)]
      [(string-null? (string-trim (caddr result)))
       (error (string-append program " could not be run, or failed without saying why"))]
      [else (error (string-append program ": " (string-trim (caddr result))))])))

;; Programs run on this machine only.
(define (%archive-here! paths)
  (when (any file-remote? (cons (working-directory) paths))
    (error "Files on another host cannot be packed or unpacked: copy them here first")))

(define (%archive-free! path)
  (when (file-exists? path)
    (error (string-append (file-name-nondirectory path) " is in the way"))))

;;; What is in an archive

(define-record-type archive-member
  (%make-archive-member name kind size changed link)
  archive-member?
  ;; The name as the archive has it, without a slash at the end.
  (name archive-member-name)
  ;; directory, link or file.
  (kind archive-member-kind)
  (size archive-member-size)
  ;; The time as text, YYYY-MM-DD HH:MM.
  (changed archive-member-changed)
  (link archive-member-link))

;; The first `count` words of a line and then the rest of it after one
;; space, as a list, or #f if the line has fewer parts.
(define (%archive-fields line count)
  (let ((end (string-length line)))
    (define (skip-spaces at)
      (if (and (< at end) (char=? (string-ref line at) #\space))
          (skip-spaces (+ at 1))
          at))
    (define (skip-word at)
      (if (and (< at end) (not (char=? (string-ref line at) #\space)))
          (skip-word (+ at 1))
          at))
    (let loop ((at (skip-spaces 0)) (left count) (fields '()))
      (if (>= at end)
          #f
          (let* ((stop (skip-word at))
                 (fields (cons (substring line at stop) fields)))
            (cond
              [(> left 1) (loop (skip-spaces stop) (- left 1) fields)]
              [(< (+ stop 1) end)
               (reverse (cons (substring line (+ stop 1) end) fields))]
              [else #f]))))))

(define (%archive-kind mode)
  (case (string-ref mode 0)
    [(#\d) 'directory]
    [(#\l #\h) 'link]
    [else 'file]))

(define (%archive-trim-slash name)
  (if (and (string-suffix? "/" name) (> (string-length name) 1))
      (substring name 0 (- (string-length name) 1))
      name))

;; A line of `tar -tv`: mode, owner, size, date, time, name.
(define (%archive-tar-member line)
  (let ((fields (%archive-fields line 5)))
    (and fields
         (let* ((kind (%archive-kind (list-ref fields 0)))
                (name (list-ref fields 5))
                (arrow (and (eq? kind 'link)
                            (or (string-search name " -> ")
                                (string-search name " link to "))))
                (target (and arrow
                             (substring name
                                        (+ arrow (if (string-search name " -> " arrow) 4 9))
                                        (string-length name)))))
           (%make-archive-member
            (%archive-trim-slash (if arrow (substring name 0 arrow) name))
            kind
            (or (string->number (list-ref fields 2)) 0)
            (string-append (list-ref fields 3) " " (list-ref fields 4))
            target)))))

;; unzip writes a question mark for each char of a name that is not ASCII,
;; unless told with -U to write #U and four hex digits, or #L and six. This
;; puts the chars back.
(define (%archive-zip-name text)
  (let ((end (string-length text)))
    (let loop ((at 0) (from 0) (parts '()))
      (let* ((hash (string-search text "#" at))
             (digits (and hash
                          (< (+ hash 1) end)
                          (case (string-ref text (+ hash 1))
                            [(#\U) 4]
                            [(#\L) 6]
                            [else #f])))
             (stop (and digits (+ hash 2 digits)))
             (code (and stop
                        (<= stop end)
                        (string->number (substring text (+ hash 2) stop) 16))))
        (cond
          [(not hash) (apply string-append (reverse (cons (substring text from end) parts)))]
          [(and code (or (< code #xD800) (< #xDFFF code #x110000)))
           (loop stop stop
                 (cons (string (integer->char code))
                       (cons (substring text from hash) parts)))]
          [else (loop (+ hash 1) from parts)])))))

;; A line of `unzip -Z -T -U`: mode, version, system, size, kind, method,
;; time as YYYYMMDD.HHMMSS, name. The lines above and below the members
;; start with something else than a mode.
(define (%archive-zip-member line)
  (let ((fields (and (> (string-length line) 0)
                     (memv (string-ref line 0) '(#\- #\d #\l))
                     (%archive-fields line 7))))
    (and fields
         (let ((stamp (list-ref fields 6)))
           (%make-archive-member
            (%archive-trim-slash (%archive-zip-name (list-ref fields 7)))
            (%archive-kind (list-ref fields 0))
            (or (string->number (list-ref fields 3)) 0)
            (if (>= (string-length stamp) 13)
                (string-append (substring stamp 0 4) "-" (substring stamp 4 6) "-"
                               (substring stamp 6 8) " " (substring stamp 9 11) ":"
                               (substring stamp 11 13))
                stamp)
            #f)))))

(define (archive-members path)
  "What is in an archive, in the order that the archive has it: a list of
records with archive-member-name, archive-member-kind (directory, link or
file), archive-member-size, archive-member-changed and archive-member-link."
  (let ((format (%archive-format path)))
    (unless (and format (symbol? (cadr format)))
      (error (string-append (file-name-nondirectory path) " is not an archive")))
    (%archive-here! (list path))
    (filter-map
     (if (eq? (cadr format) 'zip) %archive-zip-member %archive-tar-member)
     (string-split
      (if (eq? (cadr format) 'zip)
          (%archive-run "unzip" "-Z" "-T" "-U" path)
          (%archive-run "tar" "--quoting-style=literal" "-tvf" path))
      #\newline))))

;; unzip takes names as patterns: these chars stand for themselves after a
;; backslash. A dash at the start would make the name an option, and a
;; set of the dash alone is the dash.
(define (%archive-zip-pattern name)
  (let ((pattern (apply string-append
                        (map (lambda (char)
                               (if (memv char '(#\[ #\] #\* #\? #\\))
                                   (string #\\ char)
                                   (string char)))
                             (string->list name)))))
    (if (string-prefix? "-" pattern)
        (string-append "[-]" (substring pattern 1 (string-length pattern)))
        pattern)))

;; The names to give a program for some members: unzip needs the contents
;; of a directory asked for, tar takes them with it.
(define (%archive-zip-names members)
  (append-map
   (lambda (member)
     (let ((pattern (%archive-zip-pattern (archive-member-name member))))
       (if (eq? (archive-member-kind member) 'directory)
           (list (string-append pattern "/") (string-append pattern "/*"))
           (list pattern))))
   members))

;; Takes members out of an archive into a directory, or all of them if
;; `members` is empty. What is in the way is replaced.
(define (%archive-extract path members directory)
  (if (eq? (cadr (%archive-format path)) 'zip)
      (apply %archive-run "unzip" "-q" "-o" path
             (append (%archive-zip-names members) (list "-d" directory)))
      (apply %archive-run "tar" "-xf" path "-C" directory "--"
             (map archive-member-name members))))

;; The text of a member that is a file.
(define (%archive-member-text path member)
  (if (eq? (cadr (%archive-format path)) 'zip)
      (%archive-run "unzip" "-p" path (%archive-zip-pattern (archive-member-name member)))
      (%archive-run "tar" "-xOf" path "--" (archive-member-name member))))

;; The first part of a member's name: what unpacking it leaves in a
;; directory.
(define (%archive-top name)
  (let* ((name (cond [(string-prefix? "./" name) (substring name 2 (string-length name))]
                     [(string-prefix? "/" name) (substring name 1 (string-length name))]
                     [else name]))
         (slash (string-search name "/")))
    (if slash (substring name 0 slash) name)))

;; Unpacks a whole archive next to itself and returns the name of what
;; that made. An archive that holds one thing, as most do, leaves that
;; thing; one that holds several, or a thing whose name is taken, is
;; unpacked into a new directory with the archive's name.
(define (%archive-unpack path)
  (let* ((directory (file-name-directory path))
         (tops (delete-duplicates
                (filter (lambda (top) (not (member top '("" "."))))
                        (map (lambda (member) (%archive-top (archive-member-name member)))
                             (archive-members path)))))
         (single (and (pair? tops) (null? (cdr tops))
                      (expand-file-name (car tops) directory))))
    (cond
      [(null? tops) (error (string-append (file-name-nondirectory path) " is empty"))]
      [(and single (not (file-exists? single)))
       (%archive-extract path '() directory)
       single]
      [else
       (let ((target (%archive-stem path)))
         (%archive-free! target)
         (make-directory target)
         (%archive-extract path '() target)
         target)])))

;;; Z and z in the file manager

;; Compresses what is not compressed and unpacks what is. Returns a list
;; of what was done, as a word, and the name of what there is now.
(define (%archive-toggle path)
  (let ((format (%archive-format path))
        (compressor (%archive-compressor)))
    (cond
      [(file-directory? path)
       (let ((out (string-append path ".tar" (car compressor))))
         (%archive-free! out)
         (%archive-run "tar" "-caf" out "-C" (file-name-directory path)
                       "--" (file-name-nondirectory path))
         (list "Packed" out))]
      [(not format)
       (let ((out (string-append path (car compressor))))
         (%archive-free! out)
         (if (equal? (cadr compressor) "zstd")
             (%archive-run "zstd" "-q" "--rm" "--" path)
             (%archive-run (cadr compressor) "--" path))
         (list "Compressed" out))]
      [(symbol? (cadr format)) (list "Unpacked" (%archive-unpack path))]
      [else
       (let ((out (%archive-stem path)))
         (%archive-free! out)
         (if (equal? (cadr format) "zstd")
             (%archive-run "zstd" "-d" "-q" "--rm" "--" path)
             (%archive-run (cadr format) "-d" "--" path))
         (list "Uncompressed" out))])))

;; After files have changed: drops the marks on them in a buffer of the
;; file manager, reads the listings again and puts point on a file.
(define (%archive-settle from paths target)
  (when (member from (buffer-list))
    (with-current-buffer from (%dired-clear-marks! paths)))
  (%dired-refresh-all)
  (when (and target (equal? (buffer-name) from))
    (dired-refresh target)))

(define-command (dired-do-compress)
  "Compress or uncompress the marked files, or the file on this line. The
result is put where each file is. A compressed file is uncompressed and an
archive is unpacked:
into the directory if it holds one thing whose name is free, else into a
new directory with the archive's name; the archive stays. Any other file is
compressed the way the archive-compress-extension option says, and a
directory is packed into a tar archive compressed that way and stays."
  (let ((paths (dired-marked-files))
        (from (buffer-name)))
    (%archive-here! paths)
    (message "Working on " (%dired-describe paths) "...")
    ;; A large file takes its time, and nothing is asked afterwards.
    (release-input!)
    (let* ((done '())
           (failures
            (filter-map
             (lambda (path)
               (guard (e (#t (error-object-message e)))
                 (set! done (cons (%archive-toggle path) done))
                 #f))
             paths)))
      (%archive-settle from paths (and (pair? done) (cadr (car done))))
      (cond
        [(pair? failures) (error (car failures))]
        [(null? (cdr done))
         (message (car (car done)) " " (file-name-nondirectory (car paths))
                  ": " (file-name-nondirectory (cadr (car done))))]
        [else (message "Compressed or unpacked " (length done) " files")]))))

;; A file name inside a directory, as told from the directory.
(define (%archive-relative path directory)
  (let ((prefix (if (string-suffix? "/" directory) directory (string-append directory "/"))))
    (if (string-prefix? prefix path)
        (substring path (string-length prefix) (string-length path))
        (error (string-append (file-name-nondirectory path) " is not in " directory)))))

(define (%archive-suffixes)
  (string-join (map car (filter (lambda (format) (symbol? (cadr format)))
                                %archive-formats))
               ", "))

(define-command (dired-do-compress-to)
  "Pack the marked files, or the file on this line, into one archive. Its
name is asked for, and how the name ends says what kind: .tar.zst, .tar.gz,
.tgz, .tar.xz, .tar.bz2, .tar or .zip. One file that is no directory can
also go to a name that ends in .zst, .gz, .xz or .bz2. The files stay."
  (let* ((paths (dired-marked-files))
         (from (buffer-name))
         (directory (dired-directory))
         (base (file-name-nondirectory (if (null? (cdr paths)) (car paths) directory)))
         (answer (read-string
                  (string-append "Pack " (%dired-describe paths) " into: ")
                  (string-append
                   (abbreviate-file-name
                    (%dired-join directory (if (string-null? base) "archive" base)))
                   ".tar" (car (%archive-compressor)))))
         (out (expand-file-name answer))
         (format (%archive-format out))
         (names (map (lambda (path) (%archive-relative path directory)) paths)))
    (%archive-here! (cons out paths))
    (when (member out paths)
      (error "An archive cannot hold itself"))
    (unless (or (and format (symbol? (cadr format)))
                (and format (null? (cdr paths)) (not (file-directory? (car paths)))))
      (error (string-append "The name must end in one of " (%archive-suffixes))))
    (when (file-exists? out)
      (if (y-or-n? (string-append (abbreviate-file-name out) " exists; replace it?"))
          (delete-file out)
          (error "Nothing was packed")))
    (message "Packing " (%dired-describe paths) "...")
    (release-input!)
    (case (cadr format)
      [(tar) (apply %archive-run "tar" "-caf" out "-C" directory "--" names)]
      ;; zip stores the names it is given, so it runs in the directory.
      [(zip) (apply %archive-run "sh" "-c"
                    (string-append "cd \"$1\" && shift && " %archive-utf-8
                                   " exec zip -q -r \"$@\"")
                    "sh" directory out "--" names)]
      [else (%archive-run "sh" "-c" "exec \"$1\" -c -- \"$2\" > \"$3\""
                          "sh" (cadr format) (car paths) out)])
    (%archive-settle from paths out)
    (message "Packed " (%dired-describe paths) " into " (file-name-nondirectory out))))

;; Emacs has c for packing, which here asks a terminal to come to a
;; directory: runtime/lib/term-dired.scm.
(define-key 'dired-mode "Z" 'dired-do-compress)
(define-key 'dired-mode "z" 'dired-do-compress-to)

;;; The listing of an archive

(define-record-type archive-state
  (%make-archive-state path members positions marks)
  archive-state?
  (path archive-state-path)
  ;; The members as shown, and where the name of each starts in the buffer.
  (members archive-state-members %archive-set-members!)
  (positions archive-state-positions %archive-set-positions!)
  ;; The names of the marked members.
  (marks archive-state-marks))

;; The state of each buffer that lists an archive, by buffer name.
(define %archive-buffers (make-hash-table))

(define (%archive-forget name)
  (hash-table-delete! %archive-buffers name))

(add-hook! 'kill-buffer-hook %archive-forget)

(define (archive-state)
  "What is known about the archive that the current buffer lists. It is an
error if the buffer lists none."
  (or (hash-table-ref %archive-buffers (buffer-name))
      (error "This buffer does not list an archive")))

(define-major-mode archive-mode special-mode "Archive"
  "Major mode of the listing of an archive: the members of a tar archive
or a zip file, one on each line with its size and time. A member that is
text can be read but not saved back; members can be copied out."
  (set-buffer-read-only! #t))

;; Writes the members into the current buffer and puts point on the row
;; with an index, or the nearest there is.
(define (%archive-show state members index)
  (let ((parts '())
        (faces '())
        (positions '())
        (at 0))
    (define (emit! text face)
      (let ((end (+ at (string-length text))))
        (when face (set! faces (cons (list at end face) faces)))
        (set! parts (cons text parts))
        (set! at end)))
    (emit! (abbreviate-file-name (archive-state-path state)) 'dired-header)
    (emit! ":\n" #f)
    (for-each
     (lambda (member)
       (let* ((marked (hash-table-contains? (archive-state-marks state)
                                            (archive-member-name member)))
              (face (cond [marked 'dired-marked]
                          [(eq? (archive-member-kind member) 'directory) 'dired-directory]
                          [(eq? (archive-member-kind member) 'link) 'dired-symlink]
                          [else #f])))
         (emit! (if marked "*" " ") (and marked 'dired-mark))
         (emit! (string-append
                 " "
                 (string-pad-left (%dired-size (archive-member-size member)) 6)
                 "  "
                 (string-pad-right (archive-member-changed member) 16)
                 "  ")
                'dired-detail)
         (set! positions (cons at positions))
         (emit! (archive-member-name member) face)
         (when (eq? (archive-member-kind member) 'directory) (emit! "/" face))
         (when (archive-member-link member)
           (emit! (string-append " -> " (archive-member-link member)) 'dired-detail))
         (emit! "\n" #f)))
     members)
    (%replace-buffer-text! (apply string-append (reverse parts)))
    (for-each (lambda (face) (apply add-face! face)) faces)
    (%archive-set-members! state (list->vector members))
    (%archive-set-positions! state (list->vector (reverse positions)))
    (when (pair? members)
      (goto-char (vector-ref (archive-state-positions state)
                             (max 0 (min index (- (length members) 1))))))))

;; The index of the member that point is on, or #f on the first line.
(define (%archive-index state)
  (let ((index (- (line-number-at-pos) 2)))
    (and (>= index 0)
         (< index (vector-length (archive-state-members state)))
         index)))

(define (archive-member-at-point)
  "The member of the archive that point is on. It is an error if point is
on none."
  (let* ((state (archive-state))
         (index (%archive-index state)))
    (if index
        (vector-ref (archive-state-members state) index)
        (error "There is no member on this line"))))

(define (archive-marked-members)
  "The marked members of the archive that the current buffer lists, in the
order listed; if none is marked, the member that point is on."
  (let* ((state (archive-state))
         (marked (filter (lambda (member)
                           (hash-table-contains? (archive-state-marks state)
                                                 (archive-member-name member)))
                         (vector->list (archive-state-members state)))))
    (if (null? marked)
        (list (archive-member-at-point))
        marked)))

(define (archive-open path)
  "Shows what is in an archive, a tar archive or a zip file, in the
selected window."
  (let* ((path (expand-file-name path))
         (members (archive-members path))
         (name (string-append (abbreviate-file-name path) ":")))
    (switch-to-buffer name)
    ;; A buffer that was killed behind our back comes back empty and in
    ;; another mode.
    (unless (and (hash-table-contains? %archive-buffers name)
                 (eq? (major-mode) 'archive-mode))
      (hash-table-set! %archive-buffers name
                       (%make-archive-state path (vector) (vector) (make-hash-table)))
      (archive-mode)
      (set-buffer-read-only! #t)
      (set-buffer-directory! (file-name-directory path))
      (enable-mode! 'hl-line-mode))
    (let ((state (archive-state)))
      (%archive-show state members (or (%archive-index state) 0)))))

;; Shows the current buffer again from what is known: for what changes
;; the marks only.
(define (%archive-redraw)
  (let ((state (archive-state)))
    (%archive-show state
                   (vector->list (archive-state-members state))
                   (or (%archive-index state) 0))))

(define (%archive-move by)
  (let* ((state (archive-state))
         (count (vector-length (archive-state-members state)))
         (index (%archive-index state)))
    (when (> count 0)
      (goto-char (vector-ref (archive-state-positions state)
                             (max 0 (min (- count 1) (+ (or index -1) by))))))))

(define-command (archive-next-line)
  "Go to the next member of the archive."
  (%archive-move 1))

(define-command (archive-previous-line)
  "Go to the previous member of the archive."
  (%archive-move -1))

(define-command (archive-revert)
  "List the archive again, as it is now."
  (let ((state (archive-state)))
    (hash-table-clear! (archive-state-marks state))
    (%archive-show state
                   (archive-members (archive-state-path state))
                   (or (%archive-index state) 0))))

(define-command (archive-quit)
  "Leave the listing of the archive."
  (kill-buffer))

(define-command (archive-up)
  "Show the directory that the archive is in, with point on the archive."
  (let ((path (archive-state-path (archive-state)))
        (from (buffer-name)))
    (dired-open (file-name-directory path) path)
    (kill-buffer from)))

(define-command (archive-mark)
  "Mark the member on this line and go to the next."
  (hash-table-set! (archive-state-marks (archive-state))
                   (archive-member-name (archive-member-at-point))
                   #t)
  (%archive-redraw)
  (%archive-move 1))

(define-command (archive-unmark)
  "Take the mark off the member on this line and go to the next."
  (hash-table-delete! (archive-state-marks (archive-state))
                      (archive-member-name (archive-member-at-point)))
  (%archive-redraw)
  (%archive-move 1))

(define-command (archive-unmark-all)
  "Take the marks off all members."
  (hash-table-clear! (archive-state-marks (archive-state)))
  (%archive-redraw))

(define-command (archive-find-member)
  "Show the member on this line in a buffer of its own. The mode is the
one that its name asks for. The buffer has no file: it cannot be saved back into
the archive. A member that is not text is not shown; C copies it out."
  (let* ((path (archive-state-path (archive-state)))
         (member (archive-member-at-point))
         (name (archive-member-name member)))
    (unless (eq? (archive-member-kind member) 'file)
      (error (string-append name " is not a file")))
    (let ((text (%archive-member-text path member))
          (mode (%auto-mode-for name)))
      (when (string-search text "\x0;")
        (error (string-append name " is not text: copy it out with C")))
      (switch-to-buffer (string-append (file-name-nondirectory path) ":" name))
      (set-buffer-read-only! #f)
      (when (and mode (not (eq? (major-mode) mode)))
        (call-command mode))
      (%replace-buffer-text! text)
      (set-buffer-read-only! #t)
      (set-buffer-directory! (file-name-directory path))
      (goto-char 0))))

;; The directory of a window of the file manager, or else the one that
;; the archive is in: where members are offered to go.
(define (%archive-other-directory path)
  (let loop ((windows (window-list)))
    (cond
      [(null? windows) (file-name-directory path)]
      [(hash-table-ref %dired-buffers (window-buffer (car windows)))
       => dired-state-directory]
      [else (loop (cdr windows))])))

(define-command (archive-copy-out)
  "Copy the marked members, or the member on this line, out of the archive.
They go into a directory that is asked for. A directory comes with what is in it,
and each member keeps the directories of its name."
  (let* ((state (archive-state))
         (path (archive-state-path state))
         (members (archive-marked-members))
         (label (if (null? (cdr members))
                    (archive-member-name (car members))
                    (string-append (number->string (length members)) " members")))
         (answer (read-directory-name
                  (string-append "Copy " label " to: ")
                  (string-append (abbreviate-file-name (%archive-other-directory path)) "/")))
         (directory (expand-file-name answer))
         (taken (filter (lambda (member)
                          (file-exists? (expand-file-name (archive-member-name member)
                                                          directory)))
                        members)))
    (%archive-here! (list directory))
    (unless (file-directory? directory)
      (if (y-or-n? (string-append "Create the directory " answer "?"))
          (make-directory directory)
          (error "There is no such directory")))
    (unless (or (null? taken)
                (y-or-n? (string-append
                          (if (null? (cdr taken))
                              (archive-member-name (car taken))
                              (string-append (number->string (length taken)) " of them"))
                          " exists there; replace?")))
      (error "Nothing was copied"))
    (%archive-extract path members directory)
    (hash-table-clear! (archive-state-marks state))
    (%archive-redraw)
    (%dired-refresh-all)
    (message "Copied " label " to " (abbreviate-file-name directory))))

(define-command (archive-unpack)
  "Unpack the whole archive next to itself. The result is shown in the file
manager: it is in the directory if the archive holds one thing whose name
is free, else in a new directory with the archive's name."
  (let* ((path (archive-state-path (archive-state)))
         (result (%archive-unpack path)))
    (%dired-refresh-all)
    (dired-open (file-name-directory path) result)
    (message "Unpacked " (file-name-nondirectory path) ": "
             (file-name-nondirectory result))))

(for-each
 (lambda (binding) (define-key 'archive-mode (car binding) (cadr binding)))
 '(("RET" archive-find-member) ("f" archive-find-member) ("e" archive-find-member)
   ("n" archive-next-line) ("p" archive-previous-line) ("SPC" archive-next-line)
   ("<down>" archive-next-line) ("<up>" archive-previous-line)
   ("C-n" archive-next-line) ("C-p" archive-previous-line)
   ("m" archive-mark) ("u" archive-unmark) ("U" archive-unmark-all)
   ("C" archive-copy-out) ("Z" archive-unpack)
   ("g" archive-revert) ("q" archive-quit)
   ("^" archive-up) ("DEL" archive-up)))
