;;; Asking the user for things. Each of these waits for the answer with
;;; `await`, so the editor goes on working while the question is open.

(define (message . parts)
  (%message (apply string-append (map value->string parts))))

;; Reads a line in the minibuffer. The optional argument is the initial input.
(define (read-string prompt . initial)
  (await (%read-string prompt (if (null? initial) "" (car initial)) 'none)))

;; Reads a file name, with completion, starting in the working directory.
(define (read-file-name prompt)
  (await (%read-string prompt (string-append (working-directory) "/") 'file)))

;; Reads a buffer name, with completion.
(define (read-buffer-name prompt)
  (await (%read-string prompt "" 'buffer)))

;; Lets the user choose one of CANDIDATES, narrowing them by typing, and
;; returns the choice as a string. A candidate is a string, or a list of a
;; string and an annotation shown beside it. If nothing matches, the text
;; typed is returned. The optional category names the kind of thing, which
;; keeps a separate input history for it.
(define (completing-read prompt candidates . category)
  (await (%completing-read prompt candidates (if (null? category) 'choice (car category)))))

;; Asks a yes-or-no question and returns a boolean.
(define (y-or-n? prompt)
  (await (%y-or-n prompt)))
