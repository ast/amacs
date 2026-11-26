;;; Asking the user for things. Each of these waits for the answer with
;;; `await`, so the editor goes on working while the question is open.

(define (message . parts)
  "Shows the parts, joined, in the echo area. A part that is not a string
is shown as `display` would show it."
  (%message (apply string-append (map value->string parts))))

(define (read-string prompt . initial)
  "Reads a line in the minibuffer and returns it. The optional argument is
the text the line starts with."
  (await (%read-string prompt (if (null? initial) "" (car initial)) 'none)))

(define (read-passwd prompt)
  "Reads a secret, such as a password, in the minibuffer and returns it.
What is typed is shown as stars. It is not kept in a history, and no hook
sees it."
  (await (%read-secret prompt)))

(define (read-file-name prompt . initial)
  "Reads a file name, with completion. The input starts as the optional
argument, or else as the working directory. As in Emacs, // and /~ in what
is typed start the name anew, and what is before them is not returned."
  (%typed-file-name
   (await (%read-string prompt
                        (if (null? initial)
                            (string-append (working-directory) "/")
                            (car initial))
                        'file))))

(define (read-directory-name prompt . initial)
  "Reads where to put something: the name of a directory, or a new name in
one. Directories are offered as completions; RET takes a name that ends in
a slash as it stands, and otherwise completes it. As in Emacs, // and /~
in what is typed start the name anew."
  (%typed-file-name
   (await (%read-string prompt
                        (if (null? initial)
                            (string-append (working-directory) "/")
                            (car initial))
                        'directory))))

(define (read-buffer-name prompt)
  "Reads a buffer name, with completion."
  (await (%read-string prompt "" 'buffer)))

(define (completing-read prompt candidates . category)
  "Lets the user choose one of the candidates, narrowing them by typing,
and returns the choice as a string. A candidate is a string, or a list of a
string and an annotation shown beside it. If nothing matches, the text
typed is returned. The optional category names the kind of thing, which
keeps a separate input history for it."
  (await (%completing-read prompt candidates (if (null? category) 'choice (car category)))))

(define (y-or-n? prompt)
  "Asks a yes-or-no question and returns #t or #f."
  (await (%y-or-n prompt)))
