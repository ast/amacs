;;; Example init file for amacs.
;;;
;;; Copy it to ~/.config/amacs/init.scm, or try it with
;;;   amacs --init examples/init.scm
;;;
;;; It ports the parts of an Emacs configuration that amacs can express so
;;; far. Key sequences use Emacs notation. `H-` stands for Hyper, which is
;;; sent as Ctrl+Meta+Super.

;;; Editing defaults

(set-option! 'tab-width 4)

;;; Commands

(define-command (kill-region-or-backward-kill-word)
  "Kill the region if it is active, else the word before point."
  (if (region-active?)
      (call-command 'kill-region)
      (call-command 'backward-kill-word)))

(define-command (insert-date)
  "Insert today's date as YYYY-MM-DD."
  (insert (format-current-time "%Y-%m-%d")))

(define-command (show-file-name)
  "Show the full path of the current buffer's file."
  (message (or (buffer-file-name) "This buffer is not visiting a file")))

;; The comment prefix to use for a one-line comment in the current buffer.
(define (comment-prefix)
  (let ((mode (major-mode)))
    (cond
      [(equal? mode 'scheme-mode) ";;; "]
      [else "# "])))

(define-command (copyright-sm6wjm)
  "Insert a copyright line. With a prefix argument, use the full name."
  (insert (comment-prefix)
          "Copyright (c) "
          (if (prefix-arg-given?) "Albin Stigo" "SM6WJM")
          " "
          (format-current-time "%Y")))

;; The start and end of the run of characters around point for which
;; KEEP? is true.
(define (bounds-around-point keep?)
  (let ((start (let loop ((pos (point)))
                 (if (and (> pos 0) (keep? (char-after (- pos 1))))
                     (loop (- pos 1))
                     pos)))
        (end (let loop ((pos (point)))
               (if (and (< pos (point-max)) (keep? (char-after pos)))
                   (loop (+ pos 1))
                   pos))))
    (list start end)))

(define (hex-digit? s)
  (string-contains? "0123456789abcdefABCDEF" s))

(define-command (decimal-to-hexadecimal)
  "Replace the decimal number at point with its hexadecimal form."
  (let* ((bounds (bounds-around-point (lambda (s) (string-contains? "0123456789" s))))
         (start (car bounds))
         (end (car (cdr bounds)))
         (number (string->number (buffer-substring start end))))
    (if number
        (begin
          (delete-region start end)
          (goto-char start)
          (insert (number->string number 16)))
        (message "No decimal number at point"))))

(define-command (hexadecimal-to-decimal)
  "Show the decimal value of the hexadecimal number at point."
  (let* ((bounds (bounds-around-point hex-digit?))
         (digits (buffer-substring (car bounds) (car (cdr bounds))))
         (number (string->number digits 16)))
    (if number
        (message "Hex " digits " is " number)
        (message "No hexadecimal number at point"))))

;;; Keys

(global-set-key "C-w" 'kill-region-or-backward-kill-word)
(global-set-key "C-h" 'delete-backward-char)
(global-set-key "C-x C-m" 'execute-extended-command)

(global-set-key "H-c" 'switch-to-buffer)
(global-set-key "H-a" 'backward-paragraph)
(global-set-key "H-e" 'forward-paragraph)
(global-set-key "H-v" 'beginning-of-buffer)
(global-set-key "H-o" 'find-file)

;;; Not available yet. These wait for later milestones:
;;;   consult-style buffer switching, line search and ripgrep (completion UI)
;;;   sexp motion, bookmarks, ibuffer, window resizing and windmove
;;;   scrolling without moving point (H-p, H-n)
;;;   LSP, tree-sitter modes, formatters, snippets, git, the file manager
