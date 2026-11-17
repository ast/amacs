;;; Language servers, formatters and snippets.

;;; Language servers
;;;
;;; `lsp-server!` says which program serves which languages; `lsp-ensure`
;;; in a mode hook attaches the buffers of that mode to it:
;;;
;;;   (lsp-server! "rust" '("rust-analyzer")
;;;                "{\"check\": {\"command\": \"clippy\"}}")
;;;   (add-hook! 'rust-mode-hook lsp-ensure)
;;;
;;; The servers below are only used where a hook asks for them.

(lsp-server! '("c" "cpp") '("clangd"))
(lsp-server! "rust" '("rust-analyzer"))
(lsp-server! "nix" '("nixd"))
(lsp-server! "go" '("gopls"))
(lsp-server! "python" '("pyright-langserver" "--stdio"))
(lsp-server! "hcl" '("terraform-ls" "serve"))
(lsp-server! '("typescript" "tsx" "javascript")
             '("typescript-language-server" "--stdio"))

;;; Formatters
;;;
;;; A formatter is a program that reads text and writes it formatted.
;;; `format-on-save!` makes a mode use one whenever its buffers are saved:
;;;
;;;   (format-on-save! 'rust-mode 'rustfmt)
;;;
;;; M-x format-buffer (C-c C-f) runs the buffer's formatter at once.

(define-formatter 'black "black" '("-q" "-"))
(define-formatter 'clang-format "clang-format" '("--assume-filename" "{file}"))
(define-formatter 'cmake-format "cmake-format" '("-"))
(define-formatter 'gofmt "gofmt" '())
(define-formatter 'goimports "goimports" '())
(define-formatter 'nixfmt "nixfmt" '())
(define-formatter 'prettier "prettier" '("--stdin-filepath" "{file}"))
(define-formatter 'rustfmt "rustfmt" '("--edition" "2024" "--emit" "stdout" "--quiet"))
(define-formatter 'shfmt "shfmt" '("-"))
(define-formatter 'tofu-fmt "tofu" '("fmt" "-"))

(define (use-formatter! mode formatter)
  "Buffers in the mode are formatted with the formatter by `format-buffer`."
  (add-hook! (%mode-hook-name mode)
    (lambda () (set-local! 'formatter formatter))))

(define (format-on-save! mode formatter)
  "Buffers in the mode are formatted with the formatter, also whenever
they are saved."
  (add-hook! (%mode-hook-name mode)
    (lambda ()
      (set-local! 'formatter formatter)
      (set-local! 'format-on-save #t))))

;;; Snippets
;;;
;;; A snippet is a name and a template: strings, and symbols that stand for
;;;
;;;   p            a field to fill in; (p "text") one that starts with text
;;;   (s name)     a field whose text every other (s name) repeats
;;;   q            where point ends up
;;;   n  n>  >     a line break; one with indentation; indentation
;;;   r  r>        like p, the second with indentation
;;;
;;; A procedure in a template is called for a string when the snippet is
;;; expanded. Snippets belong to a mode and the modes derived from it; those
;;; of fundamental-mode apply everywhere.
;;;
;;;   (define-snippets 'rust-mode
;;;     `((fn "fn " p "(" p ") {" n> r> n "}")
;;;       (today ,(lambda () (format-current-time "%Y-%m-%d")))))
;;;
;;; M-+ completes a snippet name at point, M-* inserts one by name, and
;;; snippet names are among the completions that pop up. TAB moves to the
;;; next field and S-TAB to the previous.

(define (define-snippets mode snippets)
  "Defines snippets for a mode and the modes derived from it. Each is a
list of a name and the elements of its template; see runtime/core/tools.scm
for what the elements are."
  (for-each
   (lambda (snippet)
     (%define-snippet mode (car snippet) (cdr snippet)))
   snippets))

;; Expands a template at point. This is also where the editor sends the
;; templates that have procedures in them.
(define (%expand-snippet elements)
  (%expand-snippet-data
   (map (lambda (element)
          (if (procedure? element) (value->string (element)) element))
        elements)))

(define (expand-snippet . elements)
  "Expands a snippet template, given as its elements, at point."
  (%expand-snippet elements))
