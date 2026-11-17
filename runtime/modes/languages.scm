;;; Major modes for programming and configuration languages.
;;;
;;; A mode sets the language of its buffer, which gives it syntax
;;; highlighting and indentation, and how far a level of indentation goes.

;; Defines a major mode for a language:
;;
;;   (define-language-mode rust-mode prog-mode "Rust" "rust" 4)
;;
;; The arguments are the mode, its parent, its name on the mode line, the
;; language and the indentation offset. Forms after them run when the mode
;; is selected.
(define-syntax define-language-mode
  (syntax-rules ()
    [(_ name parent pretty language offset body ...)
     (define-major-mode name parent pretty
       (string-append "Major mode for " pretty ".")
       (set-buffer-language! language)
       (set-local! 'indent-offset offset)
       body ...)]))

;; For languages that are indented with one tab per level.
(define (%indent-with-tabs!)
  (set-local! 'indent-tabs-mode #t)
  (set-local! 'indent-offset (option 'tab-width)))

(define-language-mode c-mode prog-mode "C" "c" 2)
(define-language-mode c++-mode prog-mode "C++" "cpp" 2)
(define-language-mode cmake-mode prog-mode "CMake" "cmake" 2)
(define-language-mode css-mode prog-mode "CSS" "css" 2)
(define-language-mode devicetree-mode prog-mode "Devicetree" "devicetree" 8
  (%indent-with-tabs!))
(define-language-mode dockerfile-mode prog-mode "Dockerfile" "dockerfile" 4)
(define-language-mode emacs-lisp-mode prog-mode "ELisp" "elisp" 2)
(define-language-mode go-mode prog-mode "Go" "go" 8
  (%indent-with-tabs!))
(define-language-mode html-mode prog-mode "HTML" "html" 2)
(define-language-mode js-mode prog-mode "JavaScript" "javascript" 2)
(define-language-mode json-mode prog-mode "JSON" "json" 2)
(define-language-mode makefile-mode prog-mode "Makefile" "make" 8
  (%indent-with-tabs!))
(define-language-mode markdown-mode text-mode "Markdown" "markdown" 4)
(define-language-mode nix-mode prog-mode "Nix" "nix" 2)
(define-language-mode python-mode prog-mode "Python" "python" 4)
(define-language-mode rust-mode prog-mode "Rust" "rust" 4)
(define-language-mode scheme-mode prog-mode "Scheme" "scheme" 2)
(define-language-mode sh-mode prog-mode "Shell" "bash" 4)
(define-language-mode terraform-mode prog-mode "Terraform" "hcl" 2)
(define-language-mode toml-mode conf-mode "TOML" "toml" 2)
(define-language-mode tsx-mode prog-mode "TSX" "tsx" 2)
(define-language-mode typescript-mode prog-mode "TypeScript" "typescript" 2)
(define-language-mode yaml-mode conf-mode "YAML" "yaml" 2)

(for-each
 (lambda (entry) (add-auto-mode! (car entry) (car (cdr entry))))
 '((".c" c-mode) (".h" c-mode)
   (".cc" c++-mode) (".cpp" c++-mode) (".cxx" c++-mode)
   (".hh" c++-mode) (".hpp" c++-mode) (".hxx" c++-mode)
   ("CMakeLists.txt" cmake-mode) (".cmake" cmake-mode)
   (".css" css-mode)
   (".dts" devicetree-mode) (".dtsi" devicetree-mode) (".dtso" devicetree-mode)
   ("Dockerfile" dockerfile-mode) ("Containerfile" dockerfile-mode)
   (".el" emacs-lisp-mode)
   (".go" go-mode)
   (".html" html-mode) (".htm" html-mode)
   (".js" js-mode) (".mjs" js-mode) (".cjs" js-mode)
   (".json" json-mode)
   ("Makefile" makefile-mode) ("makefile" makefile-mode) (".mk" makefile-mode)
   (".md" markdown-mode) (".markdown" markdown-mode)
   (".nix" nix-mode)
   (".py" python-mode)
   (".rs" rust-mode)
   (".scm" scheme-mode) (".ss" scheme-mode) (".sld" scheme-mode)
   (".sh" sh-mode) (".bash" sh-mode) (".zsh" sh-mode)
   (".bashrc" sh-mode) (".zshrc" sh-mode)
   (".tf" terraform-mode) (".tfvars" terraform-mode) (".hcl" terraform-mode)
   (".tofu" terraform-mode)
   (".toml" toml-mode) ("Cargo.lock" toml-mode)
   (".tsx" tsx-mode) (".jsx" tsx-mode)
   (".ts" typescript-mode)
   (".yaml" yaml-mode) (".yml" yaml-mode)))
