;;; Jumping to a place on screen by naming it: every candidate place gets a
;;; label of a key or two, and pressing the label goes there. This is how
;;; avy and ace-window work in Emacs.

(define-option 'avy-keys "asdfghjkl"
  "The keys that jump labels are made of.")

(define (avy-labels count)
  "Labels for a number of places, the shortest first, as strings. With more
places than keys, some keys lead on to a second key, and so on."
  (let* ((keys (string->list (option 'avy-keys)))
         (available (length keys)))
    (when (< available 2)
      (error "avy-keys needs at least two keys"))
    (if (<= count available)
        (map string (list-head keys count))
        ;; Each key that leads on stands for up to as many places as it
        ;; takes to cover the rest, spread evenly.
        (let* ((leading (min available
                             (quotient (+ (- count available) (- available 2))
                                       (- available 1))))
               (single (- available leading))
               (rest (- count single)))
          (append
           (map string (list-head keys single))
           (let loop ((leaders (list-tail keys single)) (index 0) (labels '()))
             (if (null? leaders)
                 (apply append (reverse labels))
                 (let ((share (+ (quotient rest leading)
                                 (if (< index (remainder rest leading)) 1 0))))
                   (loop (cdr leaders) (+ index 1)
                         (cons (map (lambda (tail)
                                      (string-append (string (car leaders)) tail))
                                    (avy-labels share))
                               labels))))))))))

(define (list-head items count)
  "The first count items of a list."
  (if (or (= count 0) (null? items))
      '()
      (cons (car items) (list-head (cdr items) (- count 1)))))

(define (%avy-matches? wanted found)
  ;; A lowercase letter also matches its capital.
  (or (char=? wanted found)
      (and (char-lower-case? wanted) (char=? wanted (char-downcase found)))))

(define (%avy-word-char? c)
  (or (char-alphabetic? c) (char-numeric? c) (char=? c #\_)))

;; The positions in a window where `match?` holds, nearest to point first.
;; `match?` gets the window's text as a vector of chars, an index into it,
;; and the position of the first char.
(define (%avy-window-candidates window selected? match?)
  (with-selected-window window
    (let* ((start (window-start))
           (end (window-end))
           (chars (string->vector (buffer-substring start end)))
           (here (point)))
      (let loop ((index (vector-length chars)) (found '()))
        (if (< index 0)
            (list-sort (lambda (a b) (< (abs (- a here)) (abs (- b here)))) found)
            (loop (- index 1)
                  (if (and (match? chars index start)
                           ;; Where point already is, is nowhere to go.
                           (not (and selected? (= (+ start index) here))))
                      (cons (+ start index) found)
                      found)))))))

(define (avy-candidates match? . windows)
  "The places on screen where match? holds, as (window position) lists, the
nearest to point first. The optional argument is the windows to look in;
without it, every window of the frame. Match? is called with the text a
window shows as a vector of chars, an index into it (which can be one past
the end, for the place after the last char), and the position of the first
char."
  (let ((selected (selected-window)))
    (append-map
     (lambda (window)
       (map (lambda (position) (list window position))
            (%avy-window-candidates window (= window selected) match?)))
     (if (null? windows) (window-list) (car windows)))))

(define (avy-pick candidates . dim)
  "Shows a label on each candidate, a (window position) list whose position
may be #f for the window itself, and waits for the user to type one. Returns
the candidate. A single candidate is returned at once, and it is an error if
there is none."
  (cond
    [(null? candidates) (error "Nothing on screen matches")]
    [(null? (cdr candidates)) (car candidates)]
    [else
     (let loop ((labelled (map cons (avy-labels (length candidates)) candidates)))
       (show-labels! (map (lambda (entry)
                            (list (cadr entry) (caddr entry) (car entry)))
                          labelled)
                     (or (null? dim) (car dim)))
       (let* ((key (unwind-protect (read-key "Jump to: ") (hide-labels!)))
              (left (filter-map
                     (lambda (entry)
                       (and (string-prefix? key (car entry))
                            (cons (substring (car entry) (string-length key))
                                  (cdr entry))))
                     (if (= (string-length key) 1) labelled '()))))
         (cond
           [(null? left) (error "No such label")]
           [(and (null? (cdr left)) (string-null? (car (car left))))
            (cdr (car left))]
           [else (loop left)])))]))

(define (avy-jump candidates)
  "Lets the user pick one of the candidates and goes there. The mark is
left where point was, unless the region is active."
  (let* ((picked (avy-pick candidates))
         (window (car picked))
         (position (cadr picked)))
    (when (and (= window (selected-window)) (not (region-active?)))
      (set-mark! (point)))
    (select-window window)
    (goto-char position)))

(define (%avy-forbid-minibuffer)
  (when (in-minibuffer?)
    (error "Not available in the minibuffer")))

(define-command (avy-goto-char)
  "Jump to a char on screen: type it, then the label that appears on the one you mean."
  (%avy-forbid-minibuffer)
  (let ((wanted (read-char "Jump to char: ")))
    (avy-jump
     (avy-candidates
      (lambda (chars index start)
        (and (< index (vector-length chars))
             (%avy-matches? wanted (vector-ref chars index))))))))

(define-command (avy-goto-char-2)
  "Jump to a pair of chars on screen."
  (%avy-forbid-minibuffer)
  (let* ((first (read-char "Jump to chars: "))
         (second (read-char (string-append "Jump to chars: " (string first)))))
    (avy-jump
     (avy-candidates
      (lambda (chars index start)
        (and (< (+ index 1) (vector-length chars))
             (%avy-matches? first (vector-ref chars index))
             (%avy-matches? second (vector-ref chars (+ index 1)))))))))

(define-command (avy-goto-word)
  "Jump to a word on screen by its first char."
  (%avy-forbid-minibuffer)
  (let ((wanted (read-char "Jump to word starting with: ")))
    (avy-jump
     (avy-candidates
      (lambda (chars index start)
        (and (< index (vector-length chars))
             (%avy-matches? wanted (vector-ref chars index))
             (if (= index 0)
                 ;; What is before the window's first char decides.
                 (or (= start 0)
                     (not (%avy-word-char?
                           (string-ref (buffer-substring (- start 1) start) 0))))
                 (not (%avy-word-char? (vector-ref chars (- index 1)))))))))))

(define-command (avy-goto-line)
  "Jump to a line on screen."
  (%avy-forbid-minibuffer)
  (avy-jump
   (avy-candidates
    (lambda (chars index start)
      (if (= index 0)
          (= (line-beginning-position start) start)
          (char=? (vector-ref chars (- index 1)) #\newline))))))

(define (%zap through?)
  (%avy-forbid-minibuffer)
  (let* ((wanted (read-char "Zap to char: "))
         (picked (avy-pick
                  (avy-candidates
                   (lambda (chars index start)
                     (and (< index (vector-length chars))
                          (%avy-matches? wanted (vector-ref chars index))))
                   (list (selected-window)))))
         (position (cadr picked))
         (here (point))
         (extra (if through? 1 0)))
    (if (>= position here)
        (kill-region here (+ position extra))
        (kill-region (- (+ position 1) extra) here))))

(define-command (zap-to-char)
  "Kill from point through a char picked on screen."
  (%zap #t))

(define-command (zap-up-to-char)
  "Kill from point up to a char picked on screen."
  (%zap #f))

(define-command (ace-window)
  "Select another window, by label if there are several."
  (%avy-forbid-minibuffer)
  (let ((others (cdr (window-list))))
    (when (null? others)
      (error "There is no other window"))
    (select-window
     (car (avy-pick (map (lambda (window) (list window #f)) others) #f)))))
