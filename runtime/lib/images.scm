;;; Pictures in the text.
;;;
;;; add-image! shows the picture of a file under a line, in a frame that
;;; draws pictures. inline-images-mode does that for the pictures that
;;; the text of a buffer names: ![text](file) in Markdown, [[file]] in Org,
;;; and whatever a mode says with define-image-finder. The text is not
;;; changed, and in a terminal it is all that there is to see.
;;;
;;; The words that the user sees are in Simplified Technical English.

(define-option 'inline-images-max-rows 16
  "The rows that a picture gets at most where inline-images-mode shows it.
A larger picture is made smaller.")

(define %image-finders (make-hash-table))

(define (define-image-finder mode finder)
  "Says how a major mode, and the modes derived from it, name pictures in
their text. The finder is a procedure of no arguments that returns, for the
current buffer, a list of pairs of a position and the file name written
there. inline-images-mode shows each of the files that is a picture on
this host under the line of its position."
  (hash-table-set! %image-finders mode finder))

(define (image-file-name? name)
  "Whether a file name is that of a picture that add-image! can show: a
PNG, JPEG, GIF or WebP file by its ending, on this host."
  (and (not (file-remote? name))
       (regexp-search "(?i)\\.(png|jpe?g|gif|webp)$" name)
       #t))

(define (image-links-matching regexp group)
  "The matches of a regular expression in the current buffer, for a finder
of define-image-finder: a list of pairs of the position that a match starts
at and the text of its group of that number, which is the file name."
  (let ((text (buffer-string)))
    (let loop ((from 0) (found '()))
      (let ((match (and (<= from (string-length text))
                        (regexp-search regexp text from))))
        (if (not match)
            (reverse found)
            (let ((whole (car match))
                  (part (list-ref match group)))
              ;; An empty match would be found again at the same place.
              (loop (max (cadr whole) (+ (car whole) 1))
                    (if part
                        (cons (cons (car whole) (substring text (car part) (cadr part)))
                              found)
                        found))))))))

(define-minor-mode inline-images-mode "Img"
  "Show the pictures that the text names, each under the line that names
it: ![text](file) in Markdown and [[file]] in Org. Only files on this host
are shown, and only in a frame that draws pictures, which a terminal does
not. The text is not changed. Pictures follow the text as it is edited.")

;; What the mode shows in each buffer, by the buffer's name: a pair of the
;; buffer's tick when its text was last read and the numbers of the pictures.
(define %inline-images (make-hash-table))

;; The timers that read buffers again after their text changed, by name.
(define %inline-images-timers (make-hash-table))

(define (%inline-images-clear!)
  (let ((state (hash-table-ref %inline-images (buffer-name))))
    (when state
      (for-each remove-image! (cdr state))
      (hash-table-delete! %inline-images (buffer-name)))))

(define (%inline-images-show!)
  (%inline-images-clear!)
  (let ((finder (mode-lookup %image-finders))
        (rows (option 'inline-images-max-rows)))
    (hash-table-set!
     %inline-images (buffer-name)
     (cons (buffer-tick)
           (if finder
               (filter-map
                (lambda (found)
                  (let ((file (expand-file-name (cdr found))))
                    (and (image-file-name? file)
                         (file-exists? file)
                         (add-image! (car found) file rows))))
                (finder))
               '())))))

(define (%inline-images-toggled)
  (if (minor-mode-enabled? 'inline-images-mode)
      (begin
        (%inline-images-show!)
        (unless (mode-lookup %image-finders)
          (message "This mode does not name pictures.")))
      (%inline-images-clear!)))

(add-hook! 'inline-images-mode-hook %inline-images-toggled)

;; Reads the text again a moment after it last changed: not at each key.
(define (%inline-images-after-command)
  (when (minor-mode-enabled? 'inline-images-mode)
    (let ((name (buffer-name))
          (state (hash-table-ref %inline-images (buffer-name))))
      (unless (and state (= (car state) (buffer-tick)))
        (let ((timer (hash-table-ref %inline-images-timers name)))
          (when timer (cancel-timer! timer)))
        (hash-table-set!
         %inline-images-timers name
         (run-after 300
                    (lambda ()
                      (hash-table-delete! %inline-images-timers name)
                      (when (member name (buffer-list))
                        (with-current-buffer name
                          (when (minor-mode-enabled? 'inline-images-mode)
                            (%inline-images-show!)))))))))))

(add-hook! 'post-command-hook %inline-images-after-command)

(define (%inline-images-forget name)
  (hash-table-delete! %inline-images name))

(add-hook! 'kill-buffer-hook %inline-images-forget)
