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
;; Text in shades of grey. "rgb" draws it per subpixel: three times the
;; resolution across, with coloured fringes. Grey looks better here.
(set-option! 'subpixel-order "none")
;; Draw windows with the GPU, through Vulkan. Without it they are drawn on
;; the CPU, as they are anyway where there is no Vulkan.
(set-option! 'gpu-renderer #t)

;; The theme. Palette entries can be replaced in every theme at once; the
;; values are other palette names or colours such as "#aa0000". The
;; fg-term-* and bg-term-* entries are the sixteen colours that programs
;; in a terminal buffer name; here they are Catppuccin Mocha, to match the
;; Alacritty config. This one call sets the whole override list.
(set-palette-overrides! '((comment yellow-faint)
                          (string green-warmer)
                          ;; Terminal: normal colours, then bright.
                          (fg-term-black "#45475a") (bg-term-black "#45475a")
                          (fg-term-red "#f38ba8") (fg-term-green "#a6e3a1")
                          (fg-term-yellow "#f9e2af") (fg-term-blue "#89b4fa")
                          (fg-term-magenta "#f5c2e7") (fg-term-cyan "#94e2d5")
                          (fg-term-white "#bac2de")
                          (fg-term-black-bright "#585b70") (fg-term-red-bright "#f38ba8")
                          (fg-term-green-bright "#a6e3a1") (fg-term-yellow-bright "#f9e2af")
                          (fg-term-blue-bright "#89b4fa") (fg-term-magenta-bright "#f5c2e7")
                          (fg-term-cyan-bright "#94e2d5") (fg-term-white-bright "#a6adc8")))
(load-theme 'modus-operandi-tinted)
(set-option! 'themes-to-toggle '(modus-operandi-tinted modus-vivendi-tinted))

;; Terminal buffers get their own dark background and foreground, apart
;; from the light editor theme, like the Alacritty terminal (Catppuccin
;; Mocha). The sixteen named colours above are what programs draw with.
(set-face! 'term 'bg "#1e1e2e" 'fg "#cdd6f4")

;;; Editing defaults

(set-option! 'tab-width 4)

;; In programming modes: relative line numbers, the current line marked,
;; the other occurrences of the symbol at point marked, TODO and its kin
;; standing out, and trailing whitespace and text past column 80 shown.
(add-hook! 'prog-mode-hook
  (lambda ()
    (set-local! 'line-numbers 'relative)
    (enable-mode! 'hl-line-mode)
    (enable-mode! 'symbol-highlight-mode)
    (enable-mode! 'hl-todo-mode)
    (enable-mode! 'whitespace-mode)))
(add-hook! 'text-mode-hook (lambda () (enable-mode! 'hl-line-mode)))

;; Mark, beside the line numbers, the lines that differ from what git has.
(enable-mode! 'git-gutter-mode)

;; Spelling is checked in these languages at once: a word is right if the
;; dictionary of one of them knows it. They are Hunspell's, by name.
(set-option! 'spell-languages '("en_US" "sv_SE" "da_DK" "pt_BR"))

;; A project starts at the nearest of these, so that a crate or a Go module
;; inside a larger repository is a project of its own, with its own
;; language server.
(set-option! 'project-root-markers '(".git" ".projectile" "Cargo.toml" "go.mod"))

;; Brackets and quotes come in pairs. In Lisp modes paredit takes over and
;; keeps them balanced.
(enable-mode! 'auto-pair-mode)
(add-hook! 'scheme-mode-hook (lambda () (enable-mode! 'paredit-mode)))
(add-hook! 'emacs-lisp-mode-hook (lambda () (enable-mode! 'paredit-mode)))

;;; Files and other hosts
;;;
;;; A file on another host is named as tramp names it and needs nothing set
;;; here: C-x C-f /ssh:host:/path visits it and C-x d /ssh:host: lists its
;;; home directory, over the ssh of ~/.ssh/config. At a prompt that holds
;;; a name already, //ssh: starts anew.

;; In the file manager, b and one of these keys goes to a directory. One
;; on another host is as good as any; put your own hosts here.
(set-option! 'dired-quick-access
             '(("h" "~" "Home")
               ("d" "~/Downloads" "Downloads")
               ("s" "~/src" "Source")
               ;; ("e" "/ssh:evo:" "evo")
               ;; ("n" "/ssh:evo:/etc/nixos" "evo's NixOS")
               ))

;;; Language servers
;;;
;;; A server is started for a project when the first of its files is opened
;;; in a mode with `lsp-ensure` on its hook. The program comes from the
;;; project's .envrc if there is one, as everything run for a file does.

(lsp-server! '("c" "cpp")
             '("clangd" "--fallback-style=llvm" "--header-insertion=never"
               "--background-index"))
(lsp-server! "rust" '("rust-analyzer")
             "{\"check\": {\"command\": \"clippy\"}}")
;; The nixpkgs expression powers package names, the options expression
;; NixOS option names and their documentation.
(lsp-server! "nix" '("nixd")
             "{\"nixpkgs\": {\"expr\": \"import <nixpkgs> { }\"},
               \"options\": {\"nixos\": {\"expr\": \"(import <nixpkgs/nixos> { }).options\"}}}")

(for-each (lambda (hook) (add-hook! hook lsp-ensure))
          '(c-mode-hook c++-mode-hook python-mode-hook rust-mode-hook
            typescript-mode-hook tsx-mode-hook go-mode-hook nix-mode-hook
            terraform-mode-hook))

;;; Formatting on save

(define-formatter 'clang-format "clang-format"
  '("--style=llvm" "--assume-filename" "{file}"))

(format-on-save! 'rust-mode 'rustfmt)
(format-on-save! 'c-mode 'clang-format)
(format-on-save! 'c++-mode 'clang-format)
(format-on-save! 'go-mode 'goimports)
(format-on-save! 'typescript-mode 'prettier)
(format-on-save! 'tsx-mode 'prettier)
(format-on-save! 'nix-mode 'nixfmt)
(format-on-save! 'terraform-mode 'tofu-fmt)
(format-on-save! 'cmake-mode 'cmake-format)
(use-formatter! 'python-mode 'black)

;;; Snippets
;;;
;;; M-+ completes a snippet name at point and M-* inserts one by name.

(define (comment-start)
  (let ((language (buffer-language)))
    (cond
      [(member language '("scheme" "elisp")) ";; "]
      [(member language '("c" "cpp" "rust" "go" "javascript" "typescript" "tsx")) "// "]
      [else "# "])))

(define-snippets 'fundamental-mode
  `((today ,(lambda () (format-current-time "%Y-%m-%d")))
    (now ,(lambda () (format-current-time "%Y-%m-%d %H:%M")))))

(define-snippets 'prog-mode
  `((todo ,comment-start "TODO " q)
    (fixme ,comment-start "FIXME " q)
    (note ,comment-start "NOTE " q)))

(define-snippets 'emacs-lisp-mode
  '((lam "(lambda (" p ")" n> r> ")")
    (let "(let ((" p "))" n> r> ")")
    (defun "(defun " p " (" p ")" n> "\"" p "\"" n> r> ")")
    (usepkg "(use-package " p n> ":straight nil" n> r> ")")
    (when "(when " p n> r> ")")
    (dolist "(dolist (" p " " p ")" n> r> ")")))

(define-snippets 'scheme-mode
  '((lam "(lambda (" p ")" n> r> ")")
    (let "(let ((" p "))" n> r> ")")
    (def "(define (" p ")" n> r> ")")
    (defcmd "(define-command (" p ")" n> "\"" p "\"" n> r> ")")
    (when "(when " p n> r> ")")))

(define-snippets 'rust-mode
  '((fn "fn " p "(" p ") {" n> r> n "}")
    (pfn "pub fn " p "(" p ") -> " p " {" n> r> n "}")
    (struct "#[derive(Debug, Clone)]" n "pub struct " p " {" n> r> n "}")
    (enum "#[derive(Debug, Clone)]" n "pub enum " p " {" n> r> n "}")
    (imp "impl " p " {" n> r> n "}")
    (test "#[cfg(test)]" n "mod tests {" n> "use super::*;" n n> "#[test]" n>
          "fn " p "() {" n> r> n> "}" n "}")
    (match "match " p " {" n> r> n "}")
    (derive "#[derive(" p ")]" q)))

(define-snippets 'c-mode
  '((inc "#include <" p ">" q)
    (incl "#include \"" p "\"" q)
    (guard "#ifndef " (s name) n "#define " (s name) n n r> n n
           "#endif /* " (s name) " */")
    (fori "for (size_t i = 0; i < " p "; i++) {" n> r> n "}")
    (main "int main(int argc, char **argv) {" n> r> n> "return 0;" n "}")
    (st "typedef struct {" n> r> n "} " p ";")))

(define-snippets 'go-mode
  '((fn "func " p "(" p ") " p " {" n> r> n "}")
    (iferr "if err != nil {" n> r> n "}")
    (test "func Test" p "(t *testing.T) {" n> r> n "}")))

(define-snippets 'nix-mode
  '((let "let" n> p n "in" n> r>)
    (mkopt "mkOption {" n> "type = types." p ";" n> "default = " p ";" n>
           "description = \"" p "\";" n "};")
    (pkg "{ config, pkgs, ... }:" n n "{" n> r> n "}")))

;;; Commands

(define-command (kill-region-or-backward-kill-word)
  "Kill the region if it is active, else the word before point."
  (if (region-active?)
      (call-command 'kill-region)
      (call-command 'backward-kill-word)))

(define-command (insert-date)
  "Insert today's date as YYYY-MM-DD."
  (insert (format-current-time "%Y-%m-%d")))

(define-command (scroll-up-some)
  "Scroll the text up four lines."
  (scroll-lines 4))

(define-command (scroll-down-some)
  "Scroll the text down four lines."
  (scroll-lines -4))

(define-command (show-file-name)
  "Show the full path of the current buffer's file."
  (message (or (buffer-file-name) "This buffer is not visiting a file")))

(define-command (copyright-sm6wjm)
  "Insert a copyright line. With a prefix argument, use the full name."
  (insert (comment-start)
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
(global-set-key "H-o" 'find-file-other-window)
(global-set-key "H-y" 'toggle-theme)
(global-set-key "H-f" 'forward-sexp)
(global-set-key "H-b" 'backward-sexp)
(global-set-key "H-u" 'backward-up-list)
(global-set-key "H-d" 'down-list)
(global-set-key "H-k" 'kill-sexp)
(global-set-key "H-j" 'bookmark-jump)
(global-set-key "H-m" 'bookmark-set)
(global-set-key "H-!" 'shell-command-on-region)
(global-set-key "H-r" 'read-only-mode)
(global-set-key "H-p" 'scroll-down-some)
(global-set-key "H-n" 'scroll-up-some)
(global-set-key "H-<left>" 'shrink-window-horizontally)
(global-set-key "H-<right>" 'enlarge-window-horizontally)
(global-set-key "H-<down>" 'shrink-window)
(global-set-key "H-<up>" 'enlarge-window)

(global-set-key "C-c -" 'comment-or-uncomment-region)
(global-set-key "C-c h k" 'describe-key)
(global-set-key "C-c h f" 'describe-command)
(global-set-key "C-c h x" 'describe-command)

;; Windows: move between them with Ctrl and an arrow, or M-o.
(global-set-key "C-<left>" 'windmove-left)
(global-set-key "C-<right>" 'windmove-right)
(global-set-key "C-<up>" 'windmove-up)
(global-set-key "C-<down>" 'windmove-down)

;; Jumping: type the key, then a char, then the label on the place meant.
(global-set-key "C-ö" 'avy-goto-char)
(global-set-key "M-ö" 'avy-goto-char)
(global-set-key "C-ä" 'avy-goto-char-2)
(global-set-key "M-ä" 'avy-goto-char-2)
(global-set-key "C-å" 'avy-goto-line)
(global-set-key "M-å" 'avy-goto-line)

(global-set-key "C-x u" 'undo-tree-visualize)
(global-set-key "C-S-p" 'move-text-up)
(global-set-key "C-S-n" 'move-text-down)
(global-set-key "C-c s n" 'symbol-next)
(global-set-key "C-c s p" 'symbol-previous)
(global-set-key "C-c s r" 'symbol-rename)

(global-set-key "C-x C-." 'xref-find-definitions)
(global-set-key "C-x C-," 'xref-go-back)
(global-set-key "M-n" 'next-error)
(global-set-key "M-p" 'previous-error)
(global-set-key "C-c f" 'list-diagnostics)

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
;; With a language server:
;;   C-c C-r  lsp-rename            (eglot-rename)
;;   C-c C-a  lsp-code-actions      (eglot-code-actions)
;;   C-c C-s  lsp-workspace-symbols (consult-eglot-symbols)
;;   C-c C-d  lsp-hover             M-g i  lsp-document-symbols
;;   M-.  M-,  M-?  definitions, back, references
;;   C-M-i    completion-at-point; completions also pop up as you type
;;   C-c C-f  format-buffer         C-x s  save-some-buffers
;; Projects, as with projectile:
;;   C-c p f  a file of the project   C-c p p  another project
;;   C-c p b  a buffer of the project C-c p k  kill its buffers
;;   C-c p s  search-files
;; Also: M-o ace-window, M-z and M-Z zap, M-g w a word by its first char,
;; C-x SPC and C-x r k, y, d, t, o for rectangles, M-% query-replace,
;; M-; comments, M-! and M-| shell commands, C-x r m and C-x r b bookmarks.

;;; Not available yet. These wait for later milestones:
;;;   ibuffer, repeat (H-z), help beyond describe-key and describe-command
;;;   rainbow delimiters, matching-bracket highlight
;;;   the breadcrumb header line, the debugger, Copilot
;;;   on other hosts: programs, passwords, sudo, git and language servers
