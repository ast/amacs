;;; Buffers made of sections: lines with a heading that can be folded away
;;; with what is under them. The git views are made of these.
;;;
;;; Code builds a tree of sections and hands it to a view, which writes the
;;; buffer and afterwards says which section a line belongs to.

(define-record-type section
  (%make-section key heading children value folded)
  section?
  ;; Tells the section from the others under the same parent, also after
  ;; the tree has been built anew.
  (key section-key)
  ;; A string, or a list of (text . face) pairs, the face possibly #f.
  (heading section-heading)
  (children section-children)
  ;; Whatever the code that made the section wants to find again.
  (value section-value)
  ;; Whether what is under it is hidden until asked for.
  (folded section-folded-at-first?))

(define (make-section key heading children value . folded)
  "A section: a key that tells it from its siblings, a heading (a string, or
a list of (text . face) pairs), the sections under it, and a value for the
code that made it to find again. With a fifth argument that is not #f, what
is under it is hidden until the user opens it."
  (%make-section key heading children value (and (pair? folded) (car folded))))

(define-record-type section-view
  (%make-section-view sections chains paths states)
  section-view?
  ;; The sections last written, to write again when one is folded.
  (sections section-view-sections %section-view-set-sections!)
  ;; For each line of the buffer, the section on it and the ones it is in,
  ;; innermost first; and the keys from the outermost in.
  (chains section-view-chains %section-view-set-chains!)
  (paths section-view-paths %section-view-set-paths!)
  ;; Sections the user has opened or closed, by their path as text.
  (states section-view-states))

(define (make-section-view)
  "A view, which remembers what the user has folded in a buffer of sections."
  (%make-section-view '() (vector) (vector) (make-hash-table)))

(define (%section-folded? view path section)
  (hash-table-ref (section-view-states view) (write->string path)
                  (section-folded-at-first? section)))

(define (%section-line view)
  (let ((index (- (line-number-at-pos) 1)))
    (and (< index (vector-length (section-view-chains view))) index)))

(define (section-chain-at-point view)
  "The section on the line that point is on, and the sections it is in,
innermost first. Empty if the line belongs to none."
  (let ((index (%section-line view)))
    (if index (vector-ref (section-view-chains view) index) '())))

(define (section-at-point view)
  "The section on the line that point is on, or #f."
  (let ((chain (section-chain-at-point view)))
    (and (pair? chain) (car chain))))

(define (section-view-render! view sections)
  "Writes sections into the current buffer as the view has them folded.
Point stays on the section it was on, if that is still there."
  (let* ((index (%section-line view))
         (keep (and index (vector-ref (section-view-paths view) index)))
         (parts '())
         (faces '())
         (chains '())
         (paths '())
         (at 0))
    (define (emit! text face)
      (let ((end (+ at (string-length text))))
        (when face (set! faces (cons (list at end face) faces)))
        (set! parts (cons text parts))
        (set! at end)))
    (define (walk section chain path)
      (let* ((chain (cons section chain))
             (path (append path (list (section-key section))))
             (parent? (pair? (section-children section)))
             (folded (and parent? (%section-folded? view path section)))
             (heading (section-heading section)))
        (when parent? (emit! (if folded "▸ " "▾ ") 'section-fold))
        (if (string? heading)
            (emit! heading #f)
            (for-each (lambda (span) (emit! (car span) (cdr span))) heading))
        (emit! "\n" #f)
        (set! chains (cons chain chains))
        (set! paths (cons path paths))
        (unless folded
          (for-each (lambda (child) (walk child chain path))
                    (section-children section)))))
    (for-each (lambda (section) (walk section '() '())) sections)
    (%replace-buffer-text! (apply string-append (reverse parts)))
    (for-each (lambda (face) (apply add-face! face)) faces)
    (%section-view-set-sections! view sections)
    (%section-view-set-chains! view (list->vector (reverse chains)))
    (%section-view-set-paths! view (list->vector (reverse paths)))
    ;; Back to the same section; failing that, to the one that holds it,
    ;; and so on outwards; failing all, to the same line.
    (let ((all (reverse paths)))
      (let loop ((wanted keep))
        (cond
          [(and wanted (pair? wanted)
                (list-index (lambda (path) (equal? path wanted)) all))
           => (lambda (line) (goto-line (+ line 1)))]
          [(and wanted (pair? wanted))
           (loop (reverse (cdr (reverse wanted))))]
          [index (goto-line (+ index 1))]
          [else (goto-char 0)])))))

(define (section-view-toggle! view)
  "Folds or opens the section at point that has something under it: the one
on the line, or else the nearest that holds it. Returns whether there was
one."
  (let* ((index (%section-line view))
         (chain (if index (vector-ref (section-view-chains view) index) '()))
         (path (if index (vector-ref (section-view-paths view) index) '())))
    (let loop ((chain chain) (path path))
      (cond
        [(null? chain) #f]
        [(pair? (section-children (car chain)))
         (hash-table-set! (section-view-states view) (write->string path)
                          (not (%section-folded? view path (car chain))))
         (section-view-render! view (section-view-sections view))
         #t]
        [else (loop (cdr chain) (reverse (cdr (reverse path))))]))))

(set-face! 'section-fold 'fg 'fg-dim)
