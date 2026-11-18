;;; Git: a status buffer to stage, commit and look around from, in the
;;; manner of Magit. Everything is done by running git itself, so hooks,
;;; signing and credentials work as they do in a shell.
;;;
;;; C-x g shows the status of the repository that the current buffer is in.

(set-face! 'git-heading 'fg 'blue 'bold #t)
(set-face! 'git-branch 'fg 'cyan 'bold #t)
(set-face! 'git-file 'bold #t)
(set-face! 'git-hash 'fg 'fg-dim)
(set-face! 'git-hunk 'fg 'fg-dim 'bg 'bg-dim)
(set-face! 'git-added 'fg 'fg-added 'bg 'bg-added)
(set-face! 'git-removed 'fg 'fg-removed 'bg 'bg-removed)

;;; Running git

;; What git was asked and what it said, the latest first, for looking at
;; when something goes wrong.
(define %git-runs '())

(define (git-run root input . arguments)
  "Runs git in a repository with some arguments and text for its standard
input. Returns a list of the exit status, what it wrote and what it wrote
to standard error."
  (let ((result (run-process "git" (append (list "-C" root) arguments) input)))
    (set! %git-runs
          (list-head (cons (cons arguments result) %git-runs) 40))
    result))

(define (%git-complaint result)
  (let ((text (string-trim (string-append (caddr result) (cadr result)))))
    (if (string-null? text)
        "git failed without saying why"
        (car (string-split text #\newline)))))

(define (git root . arguments)
  "Runs git in a repository and returns what it wrote. It is an error, with
git's own words, if git fails."
  (let ((result (apply git-run root "" arguments)))
    (if (eqv? (car result) 0)
        (cadr result)
        (error (%git-complaint result)))))

(define (%lines text)
  (let ((lines (string-split text #\newline)))
    (if (and (pair? lines) (equal? (last lines) ""))
        (reverse (cdr (reverse lines)))
        lines)))

(define (git-lines root . arguments)
  "Runs git in a repository and returns the lines it wrote."
  (%lines (apply git root arguments)))

;; Like `git`, with a patch for git to read.
(define (%git-apply root patch . arguments)
  (let ((result (apply git-run root patch "apply" arguments)))
    (unless (eqv? (car result) 0)
      (error (%git-complaint result)))))

;;; What a git buffer knows

(define-record-type git-view
  (%make-git-view kind root sections argument)
  git-view?
  ;; 'status, 'log, 'commit or 'blame.
  (kind git-view-kind)
  (root git-view-root)
  (sections git-view-sections)
  ;; What the view is of, beyond the repository: the arguments of a log, a
  ;; commit, a file.
  (argument git-view-argument))

;; The view of each git buffer, by buffer name.
(define %git-views (make-hash-table))

(define (git-view)
  "What git knows about the current buffer. It is an error if the buffer is
not one of git's."
  (or (hash-table-ref %git-views (buffer-name))
      (error "This is not a git buffer")))

(define (git-root)
  "The top directory of the repository that the current buffer is in."
  (let ((view (hash-table-ref %git-views (buffer-name))))
    (if view
        (git-view-root view)
        (let ((result (run-process "git" '("rev-parse" "--show-toplevel"))))
          (if (eqv? (car result) 0)
              (string-trim (cadr result))
              (error "This is not in a git repository"))))))

(define (%git-name root)
  (file-name-nondirectory root))

;; Shows a git buffer, making it one if it is not, and returns its view.
(define (%git-buffer name kind root argument)
  (switch-to-buffer name)
  (unless (and (hash-table-contains? %git-views name)
               (eq? (major-mode) 'git-mode))
    (hash-table-set! %git-views name
                     (%make-git-view kind root (make-section-view) argument))
    (git-mode)
    (set-buffer-read-only! #t)
    (set-buffer-directory! root))
  (hash-table-ref %git-views name))

(define-major-mode git-mode #f "Git"
  "Major mode of the git buffers: the status of a repository, a log, a
commit. Start with M-x git-status.")

;;; Diffs

;; A diff as git writes it, taken apart: a list of files, each a list of
;; its name, what happened to it, the lines before its first hunk, and its
;; hunks; a hunk is its lines, the one that starts with @@ first.
(define (git-parse-diff text)
  "Takes a diff apart into files: lists of the file's name, what happened to
it (\"modified\", \"new file\", \"deleted\" or \"renamed\"), the lines of its
header, and its hunks, each a list of lines that starts with the @@ line."
  (define (finish-file header hunks files)
    (if (null? header)
        files
        (let* ((header (reverse header))
               (hunks (reverse (map reverse hunks)))
               (after (lambda (prefix)
                        (let ((line (find (lambda (line) (string-prefix? prefix line))
                                          header)))
                          (and line (substring line (string-length prefix))))))
               (new (after "+++ b/"))
               (old (after "--- a/"))
               (first (car header))
               (split (string-search first " b/"))
               (name (or new old
                         (and split (substring first (+ split 3)))
                         first))
               (kind (cond [(after "new file mode") "new file"]
                           [(after "deleted file mode") "deleted"]
                           [(after "rename from ") "renamed"]
                           [else "modified"])))
          (cons (list name kind header hunks) files))))
  (let loop ((lines (%lines text)) (header '()) (hunks '()) (files '()))
    (cond
      [(null? lines) (reverse (finish-file header hunks files))]
      [(string-prefix? "diff --git " (car lines))
       (loop (cdr lines) (list (car lines)) '() (finish-file header hunks files))]
      [(string-prefix? "@@" (car lines))
       (loop (cdr lines) header (cons (list (car lines)) hunks) files)]
      [(null? hunks) (loop (cdr lines) (cons (car lines) header) hunks files)]
      [else
       (loop (cdr lines) header
             (cons (cons (car lines) (car hunks)) (cdr hunks))
             files)])))

;; The patch that is one hunk of a file, for git apply.
(define (%git-patch header hunk)
  (string-append (string-join (append header hunk) "\n") "\n"))

;; The patch that is a whole file's changes.
(define (%git-file-patch header hunks)
  (string-append (string-join (append header (apply append hunks)) "\n") "\n"))

;; The line of the new file that a line of a hunk is, given its index in
;; the hunk (the @@ line is 0).
(define (%git-hunk-line hunk index)
  (let* ((heading (car hunk))
         (plus (string-search heading "+"))
         (rest (substring heading (+ plus 1)))
         (stop (let loop ((at 0))
                 (if (and (< at (string-length rest))
                          (char-numeric? (string-ref rest at)))
                     (loop (+ at 1))
                     at)))
         (start (string->number (substring rest 0 stop))))
    (let loop ((lines (cdr hunk)) (at 1) (line start))
      (cond
        [(or (null? lines) (>= at index)) (max line 1)]
        [(string-prefix? "-" (car lines)) (loop (cdr lines) (+ at 1) line)]
        [else (loop (cdr lines) (+ at 1) (+ line 1))]))))

(define (%git-line-face line)
  (cond [(string-prefix? "+" line) 'git-added]
        [(string-prefix? "-" line) 'git-removed]
        [else #f]))

;; Sections for the files of a diff. `where` says which diff it is:
;; 'unstaged, 'staged, or 'commit for one that cannot be changed.
(define (%git-file-sections files where folded)
  (map
   (lambda (file)
     (let ((name (car file)) (kind (cadr file))
           (header (caddr file)) (hunks (cadddr file)))
       (make-section
        name
        (list (cons (string-pad-right kind 10) #f) (cons name 'git-file))
        (map
         (lambda (hunk number)
           (make-section
            number
            (list (cons (car hunk) 'git-hunk))
            (map (lambda (line index)
                   (make-section index
                                 (list (cons line (%git-line-face line)))
                                 '()
                                 (list 'line where name header hunk index)))
                 (cdr hunk)
                 (iota (length (cdr hunk)) 1))
            (list 'hunk where name header hunk)))
         hunks
         (iota (length hunks)))
        (list 'file where name header hunks)
        folded)))
   files))

;;; The status buffer

(define (%git-blank key)
  (make-section key "" '() #f))

(define (%git-group key title count children value . folded)
  (make-section key
                (list (cons title 'git-heading)
                      (cons (string-append " (" (number->string count) ")") #f))
                children value
                (and (pair? folded) (car folded))))

;; Asks git everything the status buffer shows. This waits for git, so it
;; is done before the buffer is touched.
(define (%git-status-sections root)
  (let* ((status (git-lines root "status" "--porcelain=v1" "--branch"))
         (branch-line (if (pair? status) (car status) "## ?"))
         (untracked (filter-map
                     (lambda (line)
                       (and (string-prefix? "?? " line) (substring line 3)))
                     status))
         (unstaged (git-parse-diff (git root "diff" "--no-color" "--no-ext-diff")))
         (staged (git-parse-diff
                  (git root "diff" "--cached" "--no-color" "--no-ext-diff")))
         (stashes (git-lines root "stash" "list"))
         ;; A repository without commits has no log, which is no error here.
         (commits (guard (e (#t '()))
                    (git-lines root "log" "-n" "10" "--format=%h%x09%s")))
         (head (substring branch-line 3))
         (dots (string-search head "..."))
         (space (string-search head " ")))
    (append
     (list
      (make-section
       'head
       (list (cons "Head:     " #f)
             (cons (cond [dots (substring head 0 dots)]
                         [else head])
                   'git-branch)
             (cons (if (pair? commits)
                       (string-append "  " (cadr (string-split (car commits) #\tab)))
                       "")
                   #f))
       '() #f))
     (if dots
         (list (make-section
                'upstream
                (list (cons "Upstream: " #f)
                      (cons (substring head (+ dots 3)) 'git-branch))
                '() #f))
         '())
     (list (%git-blank 'blank-head))
     (if (null? untracked)
         '()
         (list
          (%git-group 'untracked "Untracked files" (length untracked)
                      (map (lambda (path)
                             (make-section path path '() (list 'untracked path)))
                           untracked)
                      '(group untracked))
          (%git-blank 'blank-untracked)))
     (if (null? unstaged)
         '()
         (list
          (%git-group 'unstaged "Unstaged changes" (length unstaged)
                      (%git-file-sections unstaged 'unstaged #t)
                      '(group unstaged))
          (%git-blank 'blank-unstaged)))
     (if (null? staged)
         '()
         (list
          (%git-group 'staged "Staged changes" (length staged)
                      (%git-file-sections staged 'staged #t)
                      '(group staged))
          (%git-blank 'blank-staged)))
     (if (null? stashes)
         '()
         (list
          (%git-group 'stashes "Stashes" (length stashes)
                      (map (lambda (line)
                             (let ((colon (string-search line ":")))
                               (make-section line line '()
                                             (list 'stash (substring line 0 colon)))))
                           stashes)
                      '(group stashes) #t)
          (%git-blank 'blank-stashes)))
     (if (null? commits)
         '()
         (list
          (%git-group 'commits "Recent commits" (length commits)
                      (map %git-commit-section commits)
                      '(group commits)))))))

;; A line of a log: the short hash, a tab, and the rest.
(define (%git-commit-section line)
  (let* ((tab (string-search line "\t"))
         (hash (substring line 0 tab)))
    (make-section hash
                  (list (cons hash 'git-hash)
                        (cons (string-append " " (string-replace (substring line (+ tab 1))
                                                                 "\t" "  "))
                              #f))
                  '()
                  (list 'commit hash))))

;; Sections for what `git show` or `git stash show -p` wrote: the lines
;; before the diff, then the diff.
(define (%git-show-sections text)
  (let* ((at (string-search text "\ndiff --git "))
         (preamble (%lines (if at (substring text 0 at) text)))
         (diff (if at (substring text (+ at 1)) "")))
    (append
     (map (lambda (line number) (make-section (list 'text number) line '() #f))
          preamble (iota (length preamble)))
     (list (%git-blank 'blank))
     (%git-file-sections (git-parse-diff diff) 'commit #f))))

;; Works out what a view shows. This waits for git.
(define (%git-sections view)
  (let ((root (git-view-root view))
        (argument (git-view-argument view)))
    (case (git-view-kind view)
      [(status) (%git-status-sections root)]
      [(log)
       (map %git-commit-section
            (apply git-lines root "log" "-n" "200"
                   "--format=%h%x09%ad%x09%an%x09%s%d" "--date=short" argument))]
      [(commit) (%git-show-sections (apply git root argument))]
      [(blame)
       (let ((lines (git-lines root "blame" "--date=short" "--" argument)))
         (map (lambda (line number)
              (let* ((space (string-search line " "))
                     (hash (string-trim (string-replace (substring line 0 space) "^" ""))))
                (make-section number
                              (list (cons (substring line 0 space) 'git-hash)
                                    (cons (substring line space) #f))
                              '()
                              (list 'commit hash))))
              lines
              (iota (length lines) 1)))]
      [else '()])))

(define (git-refresh)
  "Asks git again and shows what it says now in the current git buffer."
  (let* ((name (buffer-name))
         (view (git-view))
         (sections (%git-sections view)))
    ;; Git took a while; the buffer may be gone or not shown any more.
    (when (member name (buffer-list))
      (with-current-buffer name
        (section-view-render! (git-view-sections view) sections)))))

;; After something changed the repository: every status buffer of it that
;; exists is brought up to date.
(define (%git-refresh-status root)
  (let ((name (%git-status-name root)))
    (when (and (hash-table-contains? %git-views name)
               (member name (buffer-list)))
      (let ((sections (%git-status-sections root)))
        (when (member name (buffer-list))
          (with-current-buffer name
            (section-view-render! (git-view-sections (git-view)) sections)))))))

(define (%git-status-name root)
  (string-append "*git: " (%git-name root) "*"))

(define-command (git-status)
  "Show the status of the git repository that the current buffer is in."
  (let ((root (git-root)))
    (%git-buffer (%git-status-name root) 'status root #f)
    (git-refresh)))

(define-command (git-refresh-buffer)
  "Ask git again and show what it says now."
  (git-refresh))

(define-command (git-toggle-section)
  "Fold or open the section at point."
  (unless (section-view-toggle! (git-view-sections (git-view)))
    (error "There is nothing here to fold")))

(define-command (git-quit)
  "Leave this git buffer."
  (let ((name (buffer-name)))
    (hash-table-delete! %git-views name)
    (kill-buffer name)))

;;; Acting on what is at point

;; The value of the section at point that says what it is, looking
;; outwards from the line: a list that starts with 'file, 'hunk and so on.
(define (%git-thing)
  (let ((section (section-at-point (git-view-sections (git-view)))))
    (or (and section (section-value section))
        (error "There is nothing here to act on"))))

(define (%git-done root)
  (%git-refresh-status root)
  (%git-gutter-update-all))

(define-command (git-stage)
  "Stage what is at point: a file, a hunk, or all of a group."
  (let* ((root (git-view-root (git-view)))
         (thing (%git-thing)))
    (case (car thing)
      [(untracked) (git root "add" "--" (cadr thing))]
      [(file)
       (unless (eq? (cadr thing) 'unstaged) (error "That is staged already"))
       (git root "add" "--" (caddr thing))]
      [(hunk line)
       (unless (eq? (cadr thing) 'unstaged) (error "That is staged already"))
       (%git-apply root (%git-patch (cadddr thing) (list-ref thing 4)) "--cached")]
      [(group)
       (case (cadr thing)
         [(untracked) (git root "add" "--all")]
         [(unstaged) (git root "add" "--update")]
         [else (error "That cannot be staged")])]
      [else (error "That cannot be staged")])
    (%git-done root)))

(define-command (git-unstage)
  "Unstage what is at point: a file, a hunk, or everything staged."
  (let* ((root (git-view-root (git-view)))
         (thing (%git-thing)))
    (case (car thing)
      [(file)
       (unless (eq? (cadr thing) 'staged) (error "That is not staged"))
       (%git-apply root (%git-file-patch (cadddr thing) (list-ref thing 4))
                   "--cached" "--reverse")]
      [(hunk line)
       (unless (eq? (cadr thing) 'staged) (error "That is not staged"))
       (%git-apply root (%git-patch (cadddr thing) (list-ref thing 4))
                   "--cached" "--reverse")]
      [(group)
       (unless (eq? (cadr thing) 'staged) (error "That is not staged"))
       (git root "reset" "--quiet")]
      [else (error "That is not staged")])
    (%git-done root)))

(define-command (git-discard)
  "Throw away what is at point, after asking: a change that is not staged,
a file that is not tracked, or a stash."
  (let* ((root (git-view-root (git-view)))
         (thing (%git-thing)))
    (case (car thing)
      [(untracked)
       (when (y-or-n? (string-append "Delete " (cadr thing) "?"))
         (delete-file (expand-file-name (cadr thing) root)))]
      [(file)
       (unless (eq? (cadr thing) 'unstaged)
         (error "Unstage it first; only what is not staged is discarded"))
       (when (y-or-n? (string-append "Discard the changes to " (caddr thing) "?"))
         (%git-apply root (%git-file-patch (cadddr thing) (list-ref thing 4))
                     "--reverse"))]
      [(hunk line)
       (unless (eq? (cadr thing) 'unstaged)
         (error "Unstage it first; only what is not staged is discarded"))
       (when (y-or-n? "Discard this hunk?")
         (%git-apply root (%git-patch (cadddr thing) (list-ref thing 4)) "--reverse"))]
      [(stash)
       (when (y-or-n? (string-append "Drop " (cadr thing) "?"))
         (git root "stash" "drop" (cadr thing)))]
      [else (error "That cannot be discarded")])
    (%git-done root)))

(define (git-show-commit root . arguments)
  "Shows what git show, or another command that writes a commit and its
diff, says: in a buffer of its own."
  (%git-buffer (string-append "*git: " (last arguments) "*") 'commit root arguments)
  (git-refresh))

(define-command (git-visit)
  "Go to what is at point: the file and line of a change, a commit, a stash."
  (let* ((root (git-view-root (git-view)))
         (thing (%git-thing)))
    (case (car thing)
      [(untracked) (find-file (expand-file-name (cadr thing) root))]
      [(file) (find-file (expand-file-name (caddr thing) root))]
      [(hunk)
       (find-file (expand-file-name (caddr thing) root))
       (goto-line (%git-hunk-line (list-ref thing 4) 1))]
      [(line)
       (find-file (expand-file-name (caddr thing) root))
       (goto-line (%git-hunk-line (list-ref thing 4) (list-ref thing 5)))]
      [(commit) (git-show-commit root "show" "--no-color" "--stat" "--patch" (cadr thing))]
      [(stash) (git-show-commit root "stash" "show" "--no-color" "--patch" (cadr thing))]
      [else (git-toggle-section)])))

;;; Committing

;; What each buffer with a commit message is for: the repository, further
;; arguments to git commit, and the file of the message.
(define %git-commits (make-hash-table))

(define-minor-mode git-commit-mode "Commit"
  "Writing the message of a commit. C-c C-c commits and C-c C-k cancels.")

(define (%git-edit-message root arguments initial)
  (let* ((directory (string-trim (git root "rev-parse" "--absolute-git-dir")))
         (path (string-append directory "/COMMIT_EDITMSG"))
         (status (git-lines root "status" "--short")))
    (when (member "COMMIT_EDITMSG" (buffer-list))
      (kill-buffer "COMMIT_EDITMSG"))
    (write-file
     path
     (string-append
      initial
      "\n# Write the message above. C-c C-c commits, C-c C-k cancels.\n"
      "# Lines that start with # are left out.\n#\n"
      (string-join (map (lambda (line) (string-append "#   " line)) status) "\n")
      "\n"))
    (find-file path)
    (enable-mode! 'git-commit-mode)
    (hash-table-set! %git-commits (buffer-name) (list root arguments path))))

(define-command (git-commit-finish)
  "Commit with the message in this buffer."
  (let* ((name (buffer-name))
         (pending (or (hash-table-ref %git-commits name)
                      (error "This buffer is not the message of a commit")))
         (root (car pending)))
    (call-command 'save-buffer)
    (let ((said (apply git root "commit" "--cleanup=strip" "--file" (caddr pending)
                       (cadr pending))))
      (hash-table-delete! %git-commits name)
      (kill-buffer name)
      (when (member (%git-status-name root) (buffer-list))
        (switch-to-buffer (%git-status-name root)))
      (%git-done root)
      (message (car (%lines said))))))

(define-command (git-commit-cancel)
  "Give up the commit whose message this buffer holds."
  (let ((name (buffer-name)))
    (hash-table-delete! %git-commits name)
    (kill-buffer name)
    (message "Commit cancelled")))

(define-key 'git-commit-mode "C-c C-c" 'git-commit-finish)
(define-key 'git-commit-mode "C-c C-k" 'git-commit-cancel)

(define-command (git-commit)
  "Commit: choose between a new commit, amending the last, and adding what
is staged to the last without touching its message."
  (let* ((root (git-root))
         (choice (key-menu "Commit"
                           '(("-a" "Stage all changes to tracked files (--all)" "--all")
                             ("-n" "Skip the hooks (--no-verify)" "--no-verify"))
                           '(("c" "Commit" commit)
                             ("a" "Amend the last commit" amend)
                             ("e" "Extend the last commit, keeping its message" extend))))
         (switches (cdr choice)))
    (case (car choice)
      [(commit) (%git-edit-message root switches "")]
      [(amend)
       (%git-edit-message root (cons "--amend" switches)
                          (git root "log" "-1" "--format=%B"))]
      [(extend)
       (apply git root "commit" "--amend" "--no-edit" switches)
       (%git-done root)
       (message "Extended the last commit")])))

;;; Branches and remotes

(define-command (git-branch)
  "Branches: check one out, create one, delete one."
  (let* ((root (git-root))
         (choice (car (key-menu "Branch" '()
                                '(("b" "Check out a branch" checkout)
                                  ("c" "Create a branch and check it out" create)
                                  ("k" "Delete a branch" delete)))))
         (branches (lambda arguments
                     (apply git-lines root "branch" "--format=%(refname:short)"
                            arguments))))
    (case choice
      [(checkout)
       (let* ((remote (branches "--remotes"))
              (name (completing-read "Check out: " (append (branches) remote) 'git-branch)))
         (if (member name remote)
             (git root "checkout" "--track" name)
             (git root "checkout" name))
         (message "On " name))]
      [(create)
       (let ((name (read-string "Create branch: ")))
         (git root "checkout" "-b" name)
         (message "On " name))]
      [(delete)
       (let ((name (completing-read "Delete branch: " (branches) 'git-branch)))
         (git root "branch" "--delete" name)
         (message "Deleted " name))])
    (%git-done root)))

;; The branch that is checked out, and whether it has an upstream.
(define (%git-branch-state root)
  (let* ((line (car (git-lines root "status" "--porcelain=v1" "--branch")))
         (head (substring line 3))
         (dots (string-search head "...")))
    (cons (if dots (substring head 0 dots) head) (and dots #t))))

(define-command (git-push)
  "Push the branch that is checked out."
  (let* ((root (git-root))
         (choice (key-menu "Push"
                           '(("-f" "Overwrite, if nobody else has pushed (--force-with-lease)"
                              "--force-with-lease"))
                           '(("p" "Push to the upstream, or to origin if there is none" push))))
         (state (%git-branch-state root)))
    (message "Pushing…")
    ;; The network takes its time, and nothing is asked afterwards.
    (release-input!)
    (if (cdr state)
        (apply git root "push" (cdr choice))
        (apply git root "push" "--set-upstream" "origin" (car state) (cdr choice)))
    (%git-done root)
    (message "Pushed " (car state))))

(define-command (git-pull)
  "Pull into the branch that is checked out."
  (let* ((root (git-root))
         (choice (key-menu "Pull" '()
                           '(("u" "Pull from the upstream" merge)
                             ("r" "Pull and rebase onto the upstream" rebase)))))
    (message "Pulling…")
    ;; The network takes its time, and nothing is asked afterwards.
    (release-input!)
    (if (eq? (car choice) 'rebase)
        (git root "pull" "--rebase")
        (git root "pull"))
    (%git-done root)
    (message "Pulled")))

(define-command (git-fetch)
  "Fetch from the remotes."
  (let* ((root (git-root))
         (choice (key-menu "Fetch" '()
                           '(("f" "Fetch from the upstream's remote" one)
                             ("a" "Fetch from all remotes, and prune" all)))))
    (message "Fetching…")
    ;; The network takes its time, and nothing is asked afterwards.
    (release-input!)
    (if (eq? (car choice) 'all)
        (git root "fetch" "--all" "--prune")
        (git root "fetch"))
    (%git-done root)
    (message "Fetched")))

(define-command (git-stash)
  "Stashes: put the changes away, or bring the latest back."
  (let* ((root (git-root))
         (choice (key-menu "Stash"
                           '(("-u" "Also files that are not tracked (--include-untracked)"
                              "--include-untracked"))
                           '(("z" "Stash the changes" push)
                             ("p" "Pop the latest stash" pop)
                             ("a" "Apply the latest stash, keeping it" apply)))))
    (case (car choice)
      [(push) (apply git root "stash" "push" (cdr choice))]
      [(pop) (git root "stash" "pop")]
      [(apply) (git root "stash" "apply")])
    (%git-done root)))

(define-command (git-rebase)
  "Rebase: onto the upstream, or go on with or give up one that stopped."
  (let* ((root (git-root))
         (choice (car (key-menu "Rebase" '()
                                '(("u" "Rebase onto the upstream" upstream)
                                  ("c" "Continue" continue)
                                  ("s" "Skip the commit that stopped it" skip)
                                  ("a" "Abort" abort))))))
    (case choice
      [(upstream) (git root "rebase" "@{upstream}")]
      ;; Continuing wants to show the message in an editor; it stands.
      [(continue) (git root "-c" "core.editor=true" "rebase" "--continue")]
      [(skip) (git root "rebase" "--skip")]
      [(abort) (git root "rebase" "--abort")])
    (%git-done root)
    (message "Rebase: done")))

;;; Looking around

(define-command (git-log)
  "Show the history: of the branch that is checked out, or of everything."
  (let* ((root (git-root))
         (choice (car (key-menu "Log" '()
                                '(("l" "The branch that is checked out" current)
                                  ("a" "All branches" all)))))
         (arguments (if (eq? choice 'all) '("--all") '())))
    (%git-buffer (string-append "*git-log: " (%git-name root) "*") 'log root arguments)
    (git-refresh)
    (goto-char 0)))

(define-command (git-blame)
  "Show who last changed each line of the file in the current buffer."
  (let* ((file (or (buffer-file-name) (error "This buffer has no file")))
         (line (line-number-at-pos))
         (root (git-root)))
    (%git-buffer (string-append "*git-blame: " (file-name-nondirectory file) "*")
                 'blame root file)
    (git-refresh)
    (goto-line line)))

(define-command (git-show-process)
  "Show what git was last asked and what it answered."
  (let ((text (string-join
               (map (lambda (run)
                      (string-append "$ git " (string-join (car run) " ") "\n"
                                     (caddr run) (cadddr run)
                                     (if (eqv? (cadr run) 0)
                                         ""
                                         (string-append "[exit " (->string (cadr run)) "]\n"))))
                    %git-runs)
               "\n")))
    (switch-to-buffer "*git-process*")
    (%replace-buffer-text! text)
    (set-buffer-read-only! #t)))

;;; Marks beside the lines that differ from what is committed

(set-face! 'git-gutter-added 'fg 'fg-added-intense)
(set-face! 'git-gutter-changed 'fg 'fg-changed-intense)
(set-face! 'git-gutter-removed 'fg 'fg-removed-intense)

(define-global-minor-mode git-gutter-mode ""
  "Mark, beside the line numbers, the lines of a file that differ from what
git has: added, changed, or with lines removed after them. The marks are
worked out when a file is visited and when it is saved.")

;; The marks that a diff without context comes to: lists of a line and a
;; face. Each hunk heading says which lines of the new file it is about.
(define (%git-gutter-signs diff)
  (define (numbers text)
    ;; "12,3" or "12": the first line and how many.
    (let ((parts (string-split text #\,)))
      (cons (string->number (car parts))
            (if (pair? (cdr parts)) (string->number (cadr parts)) 1))))
  (append-map
   (lambda (line)
     (if (string-prefix? "@@ -" line)
         (let* ((fields (string-split line #\space))
                (old (numbers (substring (cadr fields) 1)))
                (new (numbers (substring (caddr fields) 1))))
           (cond
             ;; Only removed: the mark goes on the line they came after.
             [(= (cdr new) 0) (list (list (max (car new) 1) 'git-gutter-removed))]
             [else
              (map (lambda (offset)
                     (list (+ (car new) offset)
                           (if (= (cdr old) 0) 'git-gutter-added 'git-gutter-changed)))
                   (iota (cdr new)))]))
         '()))
   (%lines diff)))

(define (%git-gutter-update-buffer name file)
  (let* ((result (run-process "git" (list "-C" (file-name-directory file)
                                          "diff" "--no-color" "--no-ext-diff"
                                          "--unified=0" "--" file)))
         ;; A file that git does not know, or no repository: no marks.
         (signs (if (eqv? (car result) 0) (%git-gutter-signs (cadr result)) '())))
    ;; Git took a moment; the buffer may be gone.
    (when (member name (buffer-list))
      (with-current-buffer name (set-line-signs! signs)))))

(define (git-gutter-update)
  "Works out again the marks beside the lines of the current buffer."
  (let ((file (buffer-file-name)))
    (when (and file (minor-mode-enabled? 'git-gutter-mode))
      (%git-gutter-update-buffer (buffer-name) file))))

(add-hook! 'find-file-hook git-gutter-update)
(add-hook! 'after-save-hook git-gutter-update)

;; After git changed what is committed or staged, the marks of the files
;; that are open in buffers are stale.
(define (%git-gutter-update-all)
  (when (minor-mode-enabled? 'git-gutter-mode)
    (for-each
     (lambda (name)
       (when (member name (buffer-list))
         (let ((file (buffer-file-name name)))
           (when file (%git-gutter-update-buffer name file)))))
     (buffer-list))))

;;; Keys

(for-each
 (lambda (binding) (define-key 'git-mode (car binding) (cadr binding)))
 '(("TAB" git-toggle-section) ("RET" git-visit)
   ("g" git-refresh-buffer) ("q" git-quit)
   ("n" next-line) ("p" previous-line)
   ("s" git-stage) ("u" git-unstage) ("k" git-discard)
   ("c" git-commit) ("b" git-branch) ("P" git-push) ("F" git-pull)
   ("f" git-fetch) ("z" git-stash) ("r" git-rebase) ("l" git-log)
   ("$" git-show-process)))

(global-set-key "C-x g" 'git-status)
