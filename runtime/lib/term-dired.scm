;;; A file manager that follows a terminal.
;;;
;;; `term-dired` (C-c C-d in a terminal) shows the directory that the shell
;;; is in, in a window beside the terminal, and from then on that window
;;; shows the shell's directory whenever the shell changes it. The terminal
;;; leads: moving about in the listing does nothing to the shell, and the
;;; next `cd` brings the listing back. `c` in the listing asks the shell to
;;; come to the directory at point.
;;;
;;; The link is between the terminal and the window, and lasts until the
;;; window is closed or shows something that is not a listing. The core
;;; tells where the shell is (`term-directory-hook`).

;; Window -> the name of the terminal buffer it follows.
(define %term-dired-links (make-hash-table))

(define (%term-dired-linked? window)
  ;; Whether a window still follows a terminal: it is there, in whichever
  ;; frame, it shows a listing, and the terminal's buffer is there.
  (and (window-live? window)
       (member (hash-table-ref %term-dired-links window #f) (buffer-list))
       (with-current-buffer (window-buffer window)
         (eq? (major-mode) 'dired-mode))))

(define (%term-dired-windows terminal)
  ;; The windows that follow a terminal. Links that no longer hold are
  ;; forgotten on the way.
  (filter (lambda (window)
            (cond [(not (%term-dired-linked? window))
                   (hash-table-delete! %term-dired-links window)
                   #f]
                  [else (equal? (hash-table-ref %term-dired-links window) terminal)]))
          (hash-table-keys %term-dired-links)))

(define (term-dired-terminal)
  "The name of the terminal buffer that the listing in the selected window
follows, or #f."
  (let ((window (selected-window)))
    (and (%term-dired-linked? window)
         (hash-table-ref %term-dired-links window))))

(define (%term-dired-follow terminal directory)
  (for-each
   (lambda (window)
     (guard (e (#t (message "The listing cannot follow " terminal ": "
                            (condition-message e))))
       (with-selected-window window
         (unless (equal? (dired-directory) (expand-file-name directory))
           (dired-open directory)))))
   (%term-dired-windows terminal)))

(add-hook! 'term-directory-hook %term-dired-follow)

(define-command (term-dired)
  "Show the directory that the shell of this terminal is in, in a window
beside it that follows the shell from then on: when the shell changes
directory, so does the listing. Selects that window if it is there
already. In the listing, `c` sends the shell to the directory at point."
  (unless (terminal-buffer?)
    (error "This is not a terminal with a running program"))
  (let* ((terminal (buffer-name))
         (directory (working-directory))
         (windows (%term-dired-windows terminal)))
    (if (pair? windows)
        (select-window (car windows))
        (let ((from (selected-window))
              (before (window-list)))
          (call-command 'split-window-right)
          (let ((new (filter (lambda (window) (not (member window before)))
                             (window-list))))
            (when (null? new)
              (error "The window is too small to split"))
            (select-window (car new))
            (dired-open directory)
            (hash-table-set! %term-dired-links (car new) terminal)
            (select-window from))))))

(define (%term-dired-quote text)
  ;; A word for the shell, in single quotes.
  (string-append
   "'"
   (apply string-append
          (map (lambda (c) (if (char=? c #\') "'\\''" (string c)))
               (string->list text)))
   "'"))

(define-command (dired-cd-terminal)
  "Send the shell of the terminal that this listing follows to the
directory at point, or to the listing's own when point is on a file: types
a `cd` at its prompt."
  (let ((terminal (term-dired-terminal)))
    (unless terminal
      (error "This listing follows no terminal: see term-dired"))
    (unless (terminal-buffer? terminal)
      (error "The program of the terminal has ended"))
    (let* ((row (dired-row-at-point))
           (directory (if (and row (eq? (dired-row-kind row) 'directory))
                          (dired-row-path row)
                          (dired-directory))))
      (with-current-buffer terminal
        (terminal-paste (string-append "cd " (%term-dired-quote directory)))
        (terminal-send-key "RET"))
      ;; The shell says nothing when it is there already.
      (unless (equal? directory (dired-directory))
        (dired-open directory)))))

(define-key 'term-mode "C-c C-d" 'term-dired)
(define-key 'dired-mode "c" 'dired-cd-terminal)
