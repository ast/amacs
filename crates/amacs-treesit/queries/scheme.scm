; Highlighting for Scheme, in the manner of Emacs: special forms and the
; names being defined stand out, ordinary calls do not.

(comment) @comment
(block_comment) @comment
(directive) @comment

(string) @string
(escape_sequence) @escape
(character) @constant
(boolean) @constant
(number) @number
(keyword) @constant.builtin

(list
  .
  (symbol) @keyword
  (#match? @keyword
   "^(define|define-syntax|define-values|define-record-type|define-command|define-major-mode|define-minor-mode|define-global-minor-mode|define-option|define-test|lambda|λ|let|let\\*|letrec|letrec\\*|let-values|let\\*-values|let-syntax|letrec-syntax|named-lambda|case-lambda|if|cond|case|when|unless|and|or|not|else|=>|begin|do|set!|quote|quasiquote|unquote|unquote-splicing|syntax-rules|syntax-case|delay|delay-force|parameterize|guard|dynamic-wind|call/cc|call-with-current-continuation|await|require|provide|import|export|library|define-library|include|assert|error|raise)$"))

; (define (name args) ...) and (define name ...)
(list
  .
  (symbol) @_define
  .
  (list . (symbol) @function)
  (#match? @_define "^define"))

(list
  .
  (symbol) @_define
  .
  (symbol) @variable.parameter
  (#match? @_define "^define"))

(quote (symbol) @constant)
