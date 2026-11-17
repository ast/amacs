;;; Example init file for amacs.
;;;
;;; To use it:
;;;   mkdir -p ~/.config/amacs && cp examples/init.scm ~/.config/amacs/init.scm
;;; or try it without installing:
;;;   amacs --init examples/init.scm
;;; The server reads the init file when it starts, so restart it after a
;;; change: amacsclient --eval '(exit-editor)'.
;;;
;;; It ports the parts of an Emacs configuration that amacs can express so
;;; far. Key sequences use Emacs notation. `H-` stands for Hyper, which is
;;; sent as Ctrl+Meta+Super.

;;; Appearance

;; The font of graphical frames. The size is in pixels, before the output's
;; scale is applied, and depends on the machine.
(set-option! 'font-family "JetBrainsMono Nerd Font Mono")
(set-option! 'font-size (if (equal? (system-name) "evo") 16 13))

;; The theme. Palette entries can be replaced in every theme at once; the
;; values are other palette names or colours such as "#aa0000".
(set-palette-overrides! '((comment yellow-faint)
                          (string green-warmer)))
(load-theme 'modus-operandi-tinted)
(set-option! 'themes-to-toggle '(modus-operandi-tinted modus-vivendi-tinted))

;;; Editing defaults

(set-option! 'tab-width 4)

;; Relative line numbers in programming modes only.
(add-hook! 'prog-mode-hook
  (lambda () (set-local! 'line-numbers 'relative)))

;; Brackets and quotes come in pairs. In Lisp modes paredit takes over and
;; keeps them balanced.
(enable-mode! 'auto-pair-mode)
(add-hook! 'scheme-mode-hook (lambda () (enable-mode! 'paredit-mode)))
(add-hook! 'emacs-lisp-mode-hook (lambda () (enable-mode! 'paredit-mode)))

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
  (let ((language (buffer-language)))
    (cond
      [(member language '("scheme" "elisp")) ";;; "]
      [(member language '("c" "cpp" "rust" "go" "javascript" "typescript" "tsx")) "// "]
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
(global-set-key "H-y" 'toggle-theme)

;; These are bound by default the way the Emacs configuration binds them:
;;   C-s      search-lines        (consult-line)
;;   C-c /    search-files        (consult-ripgrep)
;;   C-x b    switch-to-buffer    (consult-buffer)
;;   C-x C-r  recent-file         (consult-recent-file)
;;   M-y      yank-pop            (consult-yank-pop)
;;   C-.      act-on-candidate    (embark-act, in the minibuffer)
;;   TAB      indent-for-tab-command; RET indents the new line
;;   C-M-f, C-M-b, C-M-k, C-M-u, C-M-d, C-M-SPC  work on whole expressions
;; With paredit-mode: C-) and C-} slurp and barf, M-s splices, M-r raises,
;; M-( wraps, and DEL, C-d and C-k keep the brackets balanced.

;;; Not available yet. These wait for later milestones:
;;;   bookmarks, ibuffer, window resizing and windmove
;;;   scrolling without moving point (H-p, H-n)
;;;   rainbow delimiters, matching-bracket highlight
;;;   LSP, formatters, snippets, git, the file manager
