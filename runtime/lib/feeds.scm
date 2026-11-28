;;; Feeds: news from RSS and Atom, read in the editor as elfeed reads it
;;; in Emacs.
;;;
;;; The feeds option lists the addresses, each with the tags that its
;;; entries get. C-x w shows the entries of all feeds in one buffer, the
;;; newest first, as far as a filter lets them through: the default is
;;; "@6-months-ago +unread". G fetches the feeds, and a timer does that
;;; too. RET shows an entry, b opens it in the browser. Read, unread and
;;; any other mark are tags: + and - put them on and take them off.
;;;
;;; curl fetches, one program for each feed, some at the same time. The
;;; feed is read by feed-parse-file and the HTML of an entry is laid out
;;; by html-render, both in the core. What is fetched is kept in
;;; feeds-directory: an index of the feeds and entries, and the content of
;;; each entry in a file of its own, which is read only when the entry is
;;; shown.
;;;
;;; A filter is words with spaces between them; an entry must satisfy all:
;;;   +tag -tag             has, or does not have, the tag
;;;   @6-months-ago         is newer than that; the units are days, weeks,
;;;                         months and years
;;;   @2-weeks-ago--1-week-ago   is between the two
;;;   =regexp ~regexp       its feed's title or address matches, or not
;;;   !regexp               its title and address do not match
;;;   #20                   only the first 20 entries
;;;   regexp                its title or address matches
;;; Letters of the regular expressions match in either case.
;;;
;;; The words that the user sees are in Simplified Technical English.

(define-option 'feeds '()
  "The feeds to fetch: a list of lists of an address and the tags that the
feed's new entries get, as symbols. For example:
  (set-option! 'feeds
    '((\"https://planet.emacslife.com/atom.xml\" emacs)
      (\"https://lwn.net/headlines/rss\" linux news)))")

(define-option 'feeds-default-filter "@6-months-ago +unread"
  "The filter that the list of entries starts with, and that c goes back
to.")

(define-option 'feeds-refresh-minutes 60
  "How often the feeds are fetched while the editor runs, in minutes. #f
is never: only G fetches them.")

(define-option 'feeds-initial-tags '(unread)
  "The tags that every new entry gets, besides those of its feed.")

(define-option 'feeds-curl-program "curl"
  "The program that fetches the feeds. It is given the arguments of curl.")

(define-option 'feeds-parallel 6
  "How many feeds are fetched at the same time.")

(define-option 'feeds-directory #f
  "The directory where the feeds and their entries are kept. #f is
amacs/feeds in $XDG_DATA_HOME, which is ~/.local/share if not set.")

(set-face! 'feeds-date 'fg 'fg-dim)
(set-face! 'feeds-title)
(set-face! 'feeds-unread 'bold #t)
(set-face! 'feeds-feed 'fg 'yellow)
(set-face! 'feeds-tag 'fg 'green)
(set-face! 'feeds-header 'fg 'fg-dim)
(set-face! 'feeds-entry-title 'bold #t 'fg 'blue)
(set-face! 'feeds-bold 'bold #t)
(set-face! 'feeds-italic 'italic #t)
(set-face! 'feeds-strikeout 'fg 'fg-dim)
(set-face! 'feeds-code 'fg 'cyan)
(set-face! 'feeds-heading 'bold #t 'fg 'blue)
(set-face! 'feeds-link 'fg 'blue 'underline #t)
(set-face! 'feeds-error 'fg 'red)

;;; The database

(define-record-type feed-entry
  (%make-feed-entry number feed id title link author date tags)
  feed-entry?
  (number feed-entry-number)
  (feed feed-entry-feed)
  (id feed-entry-id)
  (title feed-entry-title %set-feed-entry-title!)
  (link feed-entry-link %set-feed-entry-link!)
  (author feed-entry-author %set-feed-entry-author!)
  (date feed-entry-date %set-feed-entry-date!)
  (tags feed-entry-tags %set-feed-entry-tags!))

(set-documentation! 'feed-entry-number "The number of an entry: what its content file is named by.")
(set-documentation! 'feed-entry-feed "The address of the feed of an entry.")
(set-documentation! 'feed-entry-id "What tells an entry from the others of its feed.")
(set-documentation! 'feed-entry-title "The title of an entry.")
(set-documentation! 'feed-entry-link "The address of the page of an entry, or #f.")
(set-documentation! 'feed-entry-author "Who wrote an entry, or #f.")
(set-documentation! 'feed-entry-date "When an entry was published, in seconds since 1970.")
(set-documentation! 'feed-entry-tags "The tags of an entry, as a list of symbols.")
(set-documentation! 'feed-entry? "Whether a value is an entry of a feed.")

;; Entries by the address of their feed and their id, joined.
(define %feeds-entries (make-hash-table))
;; What is known of each feed, by its address: an alist of title, etag,
;; modified (the Last-Modified that the server gave) and error.
(define %feeds-info (make-hash-table))
;; The size of the content of each entry, by its number, to tell whether
;; a fetch changed it.
(define %feeds-sizes (make-hash-table))
(define %feeds-next-number 0)
(define %feeds-loaded #f)
(define %feeds-save-pending #f)

(define (feeds-directory)
  "The directory where the feeds and their entries are kept: the
feeds-directory option, or amacs/feeds in $XDG_DATA_HOME."
  (or (option 'feeds-directory)
      (string-append (or (getenv "XDG_DATA_HOME") (expand-file-name "~/.local/share"))
                     "/amacs/feeds")))

(define (%feeds-file . parts)
  (apply string-append (feeds-directory) "/" parts))

(define (%feeds-key feed id)
  (string-append feed "\x0;" id))

(define (feeds-entries)
  "All entries of all feeds, in no order."
  (%feeds-load!)
  (hash-table-values %feeds-entries))

(define (feeds-feed-title feed)
  "The title of a feed, given its address: the one it gives itself, or the
address if it gives none or has not been fetched."
  (let ((info (hash-table-ref %feeds-info feed #f)))
    (or (and info (let ((title (assq 'title info))) (and title (cadr title))))
        feed)))

(define (%feeds-info-ref feed key)
  (let* ((info (hash-table-ref %feeds-info feed #f))
         (field (and info (assq key info))))
    (and field (cadr field))))

(define (%feeds-info-set! feed key value)
  (let ((info (or (hash-table-ref %feeds-info feed #f) '())))
    (hash-table-set! %feeds-info feed
                     (cons (list key value)
                           (filter (lambda (field) (not (eq? (car field) key))) info)))))

(define (%feeds-load!)
  ;; Reads the index, once.
  (unless %feeds-loaded
    (set! %feeds-loaded #t)
    (let ((file (%feeds-file "index.scm")))
      (when (file-exists? file)
        (let* ((data (read-from-string (read-file file)))
               (field (lambda (key) (cadr (assq key (cdr data))))))
          (set! %feeds-next-number (field 'next))
          (for-each (lambda (feed) (hash-table-set! %feeds-info (car feed) (cadr feed)))
                    (field 'feeds))
          (for-each
           (lambda (row)
             (let ((entry (apply %make-feed-entry (list-head row 8))))
               (hash-table-set! %feeds-sizes (feed-entry-number entry) (list-ref row 8))
               (hash-table-set! %feeds-entries
                                (%feeds-key (feed-entry-feed entry) (feed-entry-id entry))
                                entry)))
           (field 'entries)))))))

(define (list-head list count)
  "The first count elements of a list."
  (if (or (= count 0) (null? list))
      '()
      (cons (car list) (list-head (cdr list) (- count 1)))))

(define (feeds-save!)
  "Writes what is known of the feeds and their entries to the index in
feeds-directory, as one step: a power failure leaves the old index or the
new one."
  (set! %feeds-save-pending #f)
  (make-directory (feeds-directory))
  (let ((file (%feeds-file "index.scm"))
        (new (%feeds-file "index.scm.new")))
    (write-file
     new
     (write->string
      (list 'feeds-index
            (list 'version 1)
            (list 'next %feeds-next-number)
            (list 'feeds (map (lambda (feed) (list feed (hash-table-ref %feeds-info feed)))
                              (hash-table-keys %feeds-info)))
            (list 'entries
                  (map (lambda (entry)
                         (list (feed-entry-number entry) (feed-entry-feed entry)
                               (feed-entry-id entry) (feed-entry-title entry)
                               (feed-entry-link entry) (feed-entry-author entry)
                               (feed-entry-date entry) (feed-entry-tags entry)
                               (hash-table-ref %feeds-sizes (feed-entry-number entry) 0)))
                       (hash-table-values %feeds-entries))))))
    (replace-file new file)))

(define (%feeds-changed!)
  ;; Saves the index once the running code is done, however many changes
  ;; it made.
  (unless %feeds-save-pending
    (set! %feeds-save-pending #t)
    (run-later feeds-save!)))

(define (feed-entry-content entry)
  "The content of an entry, as HTML, or #f if it has none."
  (let ((file (%feeds-file "content/" (number->string (feed-entry-number entry)) ".html")))
    (and (file-exists? file) (read-file file))))

(define (%feeds-merge! feed parsed)
  ;; Takes what a fetch of a feed found into the database. Returns how
  ;; many entries are new.
  (let ((tags (append (option 'feeds-initial-tags) (%feeds-tags-of feed)))
        (now (exact (floor (current-second))))
        (new 0)
        (field (lambda (alist key) (cadr (assq key alist)))))
    (%feeds-info-set! feed 'title (field parsed 'title))
    (for-each
     (lambda (item)
       (let* ((key (%feeds-key feed (field item 'id)))
              (old (hash-table-ref %feeds-entries key #f))
              (content (field item 'content))
              (entry
               (or old
                   (begin
                     (set! new (+ new 1))
                     (set! %feeds-next-number (+ %feeds-next-number 1))
                     (%make-feed-entry %feeds-next-number feed (field item 'id) "" #f #f
                                       (or (field item 'date) now) tags)))))
         (%set-feed-entry-title! entry (field item 'title))
         (%set-feed-entry-link! entry (field item 'link))
         (%set-feed-entry-author! entry (field item 'author))
         (when (field item 'date) (%set-feed-entry-date! entry (field item 'date)))
         (hash-table-set! %feeds-entries key entry)
         (when content
           (let ((number (feed-entry-number entry)))
             (unless (eqv? (hash-table-ref %feeds-sizes number #f) (string-length content))
               (make-directory (%feeds-file "content"))
               (write-file (%feeds-file "content/" (number->string number) ".html") content)
               (hash-table-set! %feeds-sizes number (string-length content)))))))
     (field parsed 'entries))
    (%feeds-changed!)
    new))

(define (%feeds-tags-of feed)
  (let ((item (assoc feed (option 'feeds))))
    (if item (cdr item) '())))

(define (feeds-tag! entry tag)
  "Puts a tag on an entry, if it does not have it."
  (unless (memq tag (feed-entry-tags entry))
    (%set-feed-entry-tags! entry (append (feed-entry-tags entry) (list tag)))
    (%feeds-changed!)))

(define (feeds-untag! entry tag)
  "Takes a tag off an entry."
  (when (memq tag (feed-entry-tags entry))
    (%set-feed-entry-tags! entry (filter (lambda (t) (not (eq? t tag))) (feed-entry-tags entry)))
    (%feeds-changed!)))

(define (feeds-all-tags)
  "Every tag that some entry or feed has, sorted by name."
  (sort (fold (lambda (tag tags) (if (memq tag tags) tags (cons tag tags)))
              '()
              (append (option 'feeds-initial-tags)
                      (apply append (map cdr (option 'feeds)))
                      (apply append (map feed-entry-tags (feeds-entries)))))
        (lambda (a b) (string<? (symbol->string a) (symbol->string b)))))

;;; Fetching

(define %feeds-updating #f)
(define %feeds-done 0)
(define %feeds-total 0)

(define (%feeds-header headers name)
  ;; The value of a header in the last response of what curl wrote with
  ;; -D: after redirects, the last block is the one of the page.
  (let* ((blocks (filter (lambda (block) (not (string-null? (string-trim block))))
                         (string-split (string-replace headers "\r" "") "\n\n")))
         (lines (if (null? blocks) '() (string-split (last blocks) #\newline)))
         (prefix (string-append name ":")))
    (let loop ((lines lines))
      (cond [(null? lines) #f]
            [(string-prefix? prefix (string-downcase (car lines)))
             (string-trim (substring (car lines) (string-length prefix) (string-length (car lines))))]
            [else (loop (cdr lines))]))))

(define (%feeds-fetch! feed slot)
  ;; Fetches one feed with curl and takes in what it has. A problem is
  ;; kept with the feed, for M-x feeds-show-errors.
  (let* ((body (%feeds-file "fetch/" (number->string slot) ".xml"))
         (headers (%feeds-file "fetch/" (number->string slot) ".headers"))
         (etag (%feeds-info-ref feed 'etag))
         (modified (%feeds-info-ref feed 'modified))
         (arguments (append (list "-sS" "-L" "--compressed" "--max-time" "60"
                                  "-A" "amacs-feeds" "-o" body "-D" headers "-w" "%{http_code}")
                            (if etag (list "-H" (string-append "If-None-Match: " etag)) '())
                            (if modified (list "-H" (string-append "If-Modified-Since: " modified)) '())
                            (list "--" feed))))
    (make-directory (%feeds-file "fetch"))
    (when (file-exists? body) (delete-file body))
    (let* ((result (run-process (option 'feeds-curl-program) arguments))
           (status (car result))
           (code (string-trim (cadr result))))
      (cond
       [(not status)
        (%feeds-info-set! feed 'error (string-append "Cannot run " (option 'feeds-curl-program)))]
       [(not (eqv? status 0))
        (%feeds-info-set! feed 'error
                          (let ((said (string-trim (caddr result))))
                            (if (string-null? said)
                                (string-append (option 'feeds-curl-program) " ended with status "
                                               (number->string status))
                                said)))]
       [(equal? code "304")
        (%feeds-info-set! feed 'error #f)]
       [(or (string-prefix? "2" code)
            (and (equal? code "000") (file-exists? body)))
        (guard (e (#t (%feeds-info-set! feed 'error
                                        (if (error-object? e) (error-object-message e) "Cannot read the feed."))))
          (%feeds-merge! feed (feed-parse-file body feed))
          (let ((text (if (file-exists? headers) (read-file headers) "")))
            (%feeds-info-set! feed 'etag (%feeds-header text "etag"))
            (%feeds-info-set! feed 'modified (%feeds-header text "last-modified")))
          (%feeds-info-set! feed 'error #f))]
       [else
        (%feeds-info-set! feed 'error (string-append "The server answered " code))])
      (%feeds-changed!))))

(define (feeds-update)
  "Fetches all feeds of the feeds option, some at the same time, while the
editor goes on working. The list of entries shows the new ones when all
are fetched. Does nothing if a fetch is in progress."
  (%feeds-load!)
  (unless (or %feeds-updating (null? (option 'feeds)))
    (let ((queue (map car (option 'feeds)))
          (workers (max 1 (min (option 'feeds-parallel) (length (option 'feeds)))))
          (before (hash-table-count %feeds-entries)))
      (set! %feeds-updating workers)
      (set! %feeds-done 0)
      (set! %feeds-total (length queue))
      (%feeds-show-progress!)
      (do ((slot 0 (+ slot 1))) ((= slot workers))
        (let ((slot slot))
          (run-later
           (lambda ()
             (let loop ()
               (unless (null? queue)
                 (let ((feed (car queue)))
                   (set! queue (cdr queue))
                   (guard (e (#t (%feeds-info-set! feed 'error
                                                   (if (error-object? e) (error-object-message e) "Error"))))
                     (%feeds-fetch! feed slot))
                   (set! %feeds-done (+ %feeds-done 1))
                   (%feeds-show-progress!)
                   (loop))))
             (set! %feeds-updating (- %feeds-updating 1))
             (when (= %feeds-updating 0)
               (set! %feeds-updating #f)
               (let ((new (- (hash-table-count %feeds-entries) before))
                     (failed (length (feeds-failed))))
                 (message "Feeds: " (number->string new) " new "
                          (if (= new 1) "entry" "entries")
                          (if (> failed 0)
                              (string-append ", " (number->string failed)
                                             " with problems (M-x feeds-show-errors)")
                              "")))
               (%feeds-redraw!)))))))))

(define-command (feeds-fetch)
  "Fetch all feeds now. The list of entries shows the new ones when all are
fetched."
  (if (null? (option 'feeds))
      (message "No feeds. Set the feeds option to a list of addresses.")
      (feeds-update)))

(define (feeds-failed)
  "The feeds whose last fetch had a problem: a list of lists of the address
and the problem."
  (filter cadr (map (lambda (item) (list (car item) (%feeds-info-ref (car item) 'error)))
                    (option 'feeds))))

(define-command (feeds-show-errors)
  "Show the feeds whose last fetch had a problem, with the problem."
  (%feeds-load!)
  (let ((failed (feeds-failed)))
    (if (null? failed)
        (message "No feed has a problem.")
        (begin
          (switch-to-buffer "*feeds errors*")
          (unless (eq? (major-mode) 'special-mode) (special-mode))
          (%replace-buffer-text!
           (apply string-append
                  (map (lambda (item) (string-append (car item) "\n  " (cadr item) "\n"))
                       failed)))
          (goto-char 0)))))

;; The timer that fetches the feeds every feeds-refresh-minutes. It is set
;; by the first command after init has given the feeds option some feeds,
;; so that an editor without feeds has no timer. The first fetch is a
;; minute after that. A load of this file again stops the timer of the
;; load before.
(define %feeds-timer (guard (e (#t #f)) %feeds-timer))
(when %feeds-timer (cancel-timer! %feeds-timer))
(set! %feeds-timer #f)

(define (%feeds-schedule! minutes)
  (set! %feeds-timer
        (run-after (* minutes 60000)
                   (lambda ()
                     (let ((minutes (option 'feeds-refresh-minutes)))
                       (when (and minutes (pair? (option 'feeds)))
                         (feeds-update))
                       (%feeds-schedule! (or minutes 10)))))))

(define (%feeds-arm-timer)
  (when (and (not %feeds-timer) (pair? (option 'feeds)))
    (remove-hook! 'post-command-hook %feeds-arm-timer)
    (%feeds-schedule! 1)))

(add-hook! 'post-command-hook %feeds-arm-timer)

;;; The filter

(define (%feeds-age word)
  ;; Seconds of a word as 6-months-ago, or #f.
  (let ((match (regexp-search "^([0-9]+)-(day|week|month|year)s?-ago$" word)))
    (and match
         (let ((count (string->number (substring word (car (cadr match)) (cadr (cadr match)))))
               (unit (substring word (car (caddr match)) (cadr (caddr match)))))
           (* count (cdr (assoc unit '(("day" . 86400) ("week" . 604800)
                                       ("month" . 2629800) ("year" . 31557600)))))))))

(define (%feeds-matcher regexp)
  (lambda (text) (and text (regexp-search (string-append "(?i)" regexp) text) #t)))

(define (feeds-parse-filter text)
  "A filter as a list of two: a predicate on entries, and the most entries
to show or #f. See the comment at the start of feeds.scm for how a filter
is written. A word that is not understood matches nothing."
  (let ((now (exact (floor (current-second))))
        (tests '())
        (limit #f))
    (define (test! predicate) (set! tests (cons predicate tests)))
    (for-each
     (lambda (word)
       (let ((rest (substring word 1 (string-length word)))
             (first (string-ref word 0)))
         (cond
          [(char=? first #\+)
           (let ((tag (string->symbol rest)))
             (test! (lambda (entry) (memq tag (feed-entry-tags entry)))))]
          [(char=? first #\-)
           (let ((tag (string->symbol rest)))
             (test! (lambda (entry) (not (memq tag (feed-entry-tags entry))))))]
          [(char=? first #\@)
           (let* ((split (string-search rest "--"))
                  (older (%feeds-age (if split (substring rest 0 split) rest)))
                  (newer (and split (%feeds-age (substring rest (+ split 2) (string-length rest))))))
             (if (and older (or newer (not split)))
                 (test! (lambda (entry)
                          (let ((date (feed-entry-date entry)))
                            (and (>= date (- now older))
                                 (or (not newer) (<= date (- now newer)))))))
                 (test! (lambda (entry) #f))))]
          [(char=? first #\#)
           (set! limit (string->number rest))]
          [(memv first '(#\= #\~))
           (let ((matches? (%feeds-matcher rest)))
             (test! (lambda (entry)
                      (let ((feed (feed-entry-feed entry)))
                        (eq? (char=? first #\=)
                             (or (matches? feed) (matches? (feeds-feed-title feed))))))))]
          [(char=? first #\!)
           (let ((matches? (%feeds-matcher rest)))
             (test! (lambda (entry)
                      (not (or (matches? (feed-entry-title entry))
                               (matches? (feed-entry-link entry)))))))]
          [else
           (let ((matches? (%feeds-matcher word)))
             (test! (lambda (entry)
                      (or (matches? (feed-entry-title entry))
                          (matches? (feed-entry-link entry))))))])))
     (filter (lambda (word) (not (string-null? word)))
             (string-split text #\space)))
    (list (lambda (entry) (every (lambda (test) (test entry)) tests))
          limit)))

(define (feeds-search text)
  "The entries that a filter lets through, the newest first."
  (let* ((parsed (feeds-parse-filter text))
         (found (sort (filter (car parsed) (feeds-entries))
                      (lambda (a b) (> (feed-entry-date a) (feed-entry-date b))))))
    (if (and (cadr parsed) (< (cadr parsed) (length found)))
        (list-head found (cadr parsed))
        found)))

;;; The list of entries

(define %feeds-buffer "*feeds*")
;; The filter of the list, and the entries that it shows, one a line.
(define %feeds-filter #f)
(define %feeds-rows '())
;; The buffer that the window showed before the list.
(define %feeds-previous-buffer #f)

(define-major-mode feeds-search-mode special-mode "Feeds"
  "Major mode of the list of entries of feeds: one on each line, the newest
first, with its date, title, feed and tags. The filter is on the mode
line. RET shows the entry, b opens it in the browser, r and u mark it read
and unread, + and - put on and take off a tag, s changes the filter and c
sets the default one again, g lists the entries again as the filter lets
them through now, G fetches the feeds. An entry that is read stays listed
until g."
  (set-buffer-read-only! #t))

(define (%feeds-show-progress!)
  (when (member %feeds-buffer (buffer-list))
    (with-current-buffer %feeds-buffer
      (%feeds-set-mode-line!))))

(define (%feeds-set-mode-line!)
  (set-mode-line-text!
   (string-append (or %feeds-filter "")
                  "  " (number->string (length %feeds-rows)) " shown"
                  (if %feeds-updating
                      (string-append "  Fetching " (number->string %feeds-done) "/"
                                     (number->string %feeds-total))
                      ""))))

(define (%feeds-row entry title-width)
  ;; The line of an entry, and the faces on it, from position 0.
  (let* ((date (format-time (feed-entry-date entry) "%Y-%m-%d"))
         (title (string-pad-right (feed-entry-title entry) title-width))
         (feed (string-pad-right (feeds-feed-title (feed-entry-feed entry)) 20))
         (tags (string-append "("
                              (string-join (map symbol->string (feed-entry-tags entry)) " ")
                              ")"))
         (title-start 12)
         (feed-start (+ title-start title-width 2))
         (tags-start (+ feed-start 22)))
    (list (string-append date "  " title "  " feed "  " tags)
          (list (list 0 10 'feeds-date)
                (list title-start (+ title-start title-width)
                      (if (memq 'unread (feed-entry-tags entry)) 'feeds-unread 'feeds-title))
                (list feed-start (+ feed-start 20) 'feeds-feed)
                (list tags-start (+ tags-start (string-length tags)) 'feeds-tag)))))

(define (%feeds-draw! . keep)
  ;; Fills the current buffer with the entries that the filter lets
  ;; through. Point stays on the entry that it was on, if that is still
  ;; shown. With 'keep, the entries are those shown already, drawn again
  ;; with their tags as they are now: as in elfeed, an entry that is read
  ;; stays in the list of unread ones until it is drawn anew with g.
  (let* ((at (%feeds-entry-at-point))
         (column (- (point) (line-beginning-position)))
         (rows (if (null? keep) (feeds-search %feeds-filter) %feeds-rows))
         ;; The title takes what the date, the feed and the longest tags
         ;; leave of the window, from 20 to 70 columns.
         (tags-width (fold (lambda (entry widest)
                             (max widest (+ 2 (string-length
                                               (string-join (map symbol->string (feed-entry-tags entry)) " ")))))
                           0 rows))
         (title-width (max 20 (min 70 (- (window-width) 1 12 2 22 tags-width))))
         (made (map (lambda (entry) (%feeds-row entry title-width)) rows))
         (offsets (let loop ((made made) (at 0) (out '()))
                    (if (null? made)
                        (reverse out)
                        (loop (cdr made) (+ at 1 (string-length (car (car made)))) (cons at out))))))
    (set! %feeds-rows rows)
    (%replace-buffer-text! (apply string-append (map (lambda (row) (string-append (car row) "\n")) made)))
    (clear-faces!)
    (for-each (lambda (row offset)
                (for-each (lambda (face)
                            (add-face! (+ offset (car face)) (+ offset (cadr face)) (caddr face)))
                          (cadr row)))
              made offsets)
    (let ((index (and at (list-index (lambda (entry) (eq? entry at)) rows))))
      (goto-char (if index (+ (list-ref offsets index) column) 0)))
    (%feeds-set-mode-line!)))

(define (%feeds-redraw! . keep)
  (when (member %feeds-buffer (buffer-list))
    (with-current-buffer %feeds-buffer
      (apply %feeds-draw! keep))))

(define (%feeds-entry-at-point)
  (and (eq? (major-mode) 'feeds-search-mode)
       (let ((line (- (line-number-at-pos) 1)))
         (and (< line (length %feeds-rows)) (list-ref %feeds-rows line)))))

(define (%feeds-selected)
  ;; The entries of the lines of the region, if it is active, or the one
  ;; of the line of point.
  (if (region-active?)
      (let* ((start (min (mark) (point)))
             (end (max (mark) (point)))
             (first (- (line-number-at-pos start) 1))
             (last (- (line-number-at-pos end) 1
                      (if (and (> end start) (= end (line-beginning-position end))) 1 0))))
        (deactivate-mark!)
        (let loop ((line last) (out '()))
          (if (< line first)
              out
              (loop (- line 1)
                    (if (< line (length %feeds-rows)) (cons (list-ref %feeds-rows line) out) out)))))
      (let ((entry (%feeds-entry-at-point)))
        (if entry (list entry) '()))))

(define (%feeds-entry-or-error)
  (or (%feeds-entry-at-point) (error "No entry on this line.")))

(define-command (feeds)
  "Show the entries of the feeds, as far as the filter lets them through."
  (%feeds-load!)
  (unless (member (buffer-name) (list %feeds-buffer %feeds-entry-buffer))
    (set! %feeds-previous-buffer (buffer-name)))
  (switch-to-buffer %feeds-buffer)
  (unless (eq? (major-mode) 'feeds-search-mode)
    (feeds-search-mode))
  (unless %feeds-filter (set! %feeds-filter (option 'feeds-default-filter)))
  (%feeds-draw!))

(define-command (feeds-redraw)
  "Show the entries again, as the filter lets them through now."
  (%feeds-draw!))

(define-command (feeds-set-filter)
  "Change the filter of the list of entries: +tag, -tag, @6-months-ago,
=feed, !regexp, #20 and words that the title must match. The list follows
the filter as it is typed; C-g gives back the one before."
  (let ((old %feeds-filter) (done #f))
    (dynamic-wind
     (lambda () (set! %feeds-filtering #t))
     (lambda ()
       (set! %feeds-filter (string-trim (read-string "Filter: " (string-append (or old "") " "))))
       (set! done #t))
     (lambda ()
       (set! %feeds-filtering #f)
       (unless done (set! %feeds-filter old))
       (%feeds-redraw!)))))

;; Whether the prompt of feeds-set-filter is open. While it is, the list
;; follows the filter as it is typed. A filter that is not whole yet, such
;; as a regular expression without its closing bracket, leaves the list as
;; it was.
(define %feeds-filtering #f)

(define (%feeds-prompt-changed kind input)
  (when %feeds-filtering
    (let ((before %feeds-filter))
      (guard (e (#t (set! %feeds-filter before)))
        (set! %feeds-filter (string-trim input))
        (%feeds-redraw!)))))

(add-hook! 'prompt-change-hook %feeds-prompt-changed)

(define-command (feeds-clear-filter)
  "Set the filter of the list of entries to the default one again."
  (set! %feeds-filter (option 'feeds-default-filter))
  (%feeds-draw!))

(define (%feeds-change-selected! change)
  (let ((entries (%feeds-selected)))
    (when (null? entries) (error "No entry on this line."))
    (for-each change entries)
    (let ((line (line-number-at-pos)))
      (%feeds-draw! 'keep)
      (when (= (length entries) 1)
        (goto-line (min (+ line 1) (max 1 (length %feeds-rows))))))))

(define-command (feeds-mark-read)
  "Take the unread tag off the entry of this line, or off the entries of
the region, and go to the next line."
  (%feeds-change-selected! (lambda (entry) (feeds-untag! entry 'unread))))

(define-command (feeds-mark-unread)
  "Put the unread tag on the entry of this line, or on the entries of the
region, and go to the next line."
  (%feeds-change-selected! (lambda (entry) (feeds-tag! entry 'unread))))

(define (%feeds-read-tag prompt)
  (string->symbol
   (string-trim (completing-read prompt (map symbol->string (feeds-all-tags)) 'feeds-tag))))

(define-command (feeds-add-tag)
  "Put a tag on the entry of this line, or on the entries of the region."
  (let ((tag (%feeds-read-tag "Put on tag: ")))
    (%feeds-change-selected! (lambda (entry) (feeds-tag! entry tag)))))

(define-command (feeds-remove-tag)
  "Take a tag off the entry of this line, or off the entries of the region."
  (let ((tag (%feeds-read-tag "Take off tag: ")))
    (%feeds-change-selected! (lambda (entry) (feeds-untag! entry tag)))))

(define (%feeds-browse! entry)
  (unless (feed-entry-link entry) (error "The entry has no link."))
  (feeds-untag! entry 'unread)
  (browse-url (feed-entry-link entry)))

(define-command (feeds-browse)
  "Open the page of the entry of this line in the browser, and mark the
entry read."
  (let ((entries (%feeds-selected)))
    (when (null? entries) (error "No entry on this line."))
    (for-each %feeds-browse! entries)
    (%feeds-draw! 'keep)))

(define (%feeds-copy-link! entry)
  (unless (feed-entry-link entry) (error "The entry has no link."))
  (kill-new (feed-entry-link entry))
  (message "Copied " (feed-entry-link entry)))

(define-command (feeds-copy-link)
  "Copy the address of the entry of this line to the kill ring."
  (%feeds-copy-link! (%feeds-entry-or-error)))

(define-command (feeds-show-entry)
  "Show the entry of this line, and mark it read."
  (feeds-show (%feeds-entry-or-error)))

(define-command (feeds-quit)
  "Leave the list of entries and show what the window showed before. The
list stays as it is, for C-x w."
  (if (and %feeds-previous-buffer (member %feeds-previous-buffer (buffer-list)))
      (switch-to-buffer %feeds-previous-buffer)
      (previous-buffer)))

(for-each
 (lambda (binding) (define-key 'feeds-search-mode (car binding) (cadr binding)))
 '(("RET" feeds-show-entry) ("b" feeds-browse) ("y" feeds-copy-link)
   ("r" feeds-mark-read) ("u" feeds-mark-unread) ("+" feeds-add-tag) ("-" feeds-remove-tag)
   ("s" feeds-set-filter) ("c" feeds-clear-filter) ("g" feeds-redraw) ("G" feeds-fetch)
   ("n" next-line) ("p" previous-line) ("q" feeds-quit)))

(global-set-key "C-x w" 'feeds)

;;; An entry

(define %feeds-entry-buffer "*feed entry*")
;; The entry that the buffer shows, and its links as (start end target).
(define %feeds-shown #f)
(define %feeds-shown-links '())

(define-major-mode feeds-show-mode special-mode "Feed-Entry"
  "Major mode of an entry of a feed: its title, author, date, feed, link
and tags, then its content. RET on a link follows it and TAB goes to the
next. n and p show the next and the entry before in the list, b opens the
page in the browser, q goes back to the list."
  (set-buffer-read-only! #t))

(define (feeds-show entry)
  "Shows an entry in its buffer, and marks it read."
  (feeds-untag! entry 'unread)
  (%feeds-redraw! 'keep)
  (switch-to-buffer %feeds-entry-buffer)
  (unless (eq? (major-mode) 'feeds-show-mode)
    (feeds-show-mode))
  (set! %feeds-shown entry)
  (let* ((header
          (filter cadr
                  (list (list "Title" (feed-entry-title entry) 'feeds-entry-title)
                        (list "Author" (feed-entry-author entry) #f)
                        (list "Date" (format-time (feed-entry-date entry) "%Y-%m-%d %H:%M") #f)
                        (list "Feed" (feeds-feed-title (feed-entry-feed entry)) 'feeds-feed)
                        (list "Link" (feed-entry-link entry) 'feeds-link)
                        (list "Tags" (string-join (map symbol->string (feed-entry-tags entry)) " ")
                              'feeds-tag))))
         (faces '())
         (head
          (let loop ((rows header) (at 0) (out ""))
            (if (null? rows)
                out
                (let* ((row (car rows))
                       (name (string-append (car row) ": "))
                       (line (string-append name (cadr row) "\n")))
                  (set! faces (cons (list at (+ at (string-length name)) 'feeds-header) faces))
                  (when (caddr row)
                    (set! faces (cons (list (+ at (string-length name))
                                            (+ at (string-length line) -1) (caddr row))
                                      faces)))
                  (loop (cdr rows) (+ at (string-length line)) (string-append out line))))))
         (content (feed-entry-content entry))
         (body (html-render (or content "<p>(The entry has no content.)</p>")
                            (max 20 (min 100 (- (window-width) 2)))
                            (or (feed-entry-link entry) (feed-entry-feed entry))))
         (offset (+ (string-length head) 1)))
    (set! %feeds-shown-links
          (map (lambda (link) (list (+ offset (car link)) (+ offset (cadr link)) (caddr link)))
               (caddr body)))
    (%replace-buffer-text! (string-append head "\n" (car body)))
    (clear-faces!)
    (for-each (lambda (face) (add-face! (car face) (cadr face) (caddr face))) faces)
    (for-each (lambda (span) (add-face! (+ offset (car span)) (+ offset (cadr span)) (caddr span)))
              (cadr body))
    (goto-char 0)))

(define (%feeds-shown-or-error)
  (or %feeds-shown (error "No entry is shown.")))

(define (%feeds-link-at position)
  (let loop ((links %feeds-shown-links))
    (cond [(null? links) #f]
          [(and (>= position (car (car links))) (< position (cadr (car links))))
           (list (car (car links)) (cadr (car links)) (caddr (car links)) #f)]
          [else (loop (cdr links))])))

(define-links 'feeds-show-mode 'at-point %feeds-link-at)

(define-command (feeds-next-link)
  "Go to the next link of the entry."
  (let ((next (find (lambda (link) (> (car link) (point))) %feeds-shown-links)))
    (if next (goto-char (car next)) (message "No more links."))))

(define-command (feeds-previous-link)
  "Go to the link of the entry before point."
  (let ((before (filter (lambda (link) (< (car link) (point))) %feeds-shown-links)))
    (if (null? before) (message "No link before this.") (goto-char (car (last before))))))

(define (%feeds-step! offset)
  ;; Shows the entry that is offset lines from the shown one in the list.
  (let* ((entry (%feeds-shown-or-error))
         (index (list-index (lambda (row) (eq? row entry)) %feeds-rows))
         (next (and index (+ index offset))))
    (if (and next (>= next 0) (< next (length %feeds-rows)))
        (begin
          (with-current-buffer %feeds-buffer (goto-line (+ next 1)))
          (feeds-show (list-ref %feeds-rows next)))
        (message "No more entries."))))

(define-command (feeds-show-next)
  "Show the next entry of the list."
  (%feeds-step! 1))

(define-command (feeds-show-previous)
  "Show the entry before this one in the list."
  (%feeds-step! -1))

(define-command (feeds-show-browse)
  "Open the page of the entry in the browser."
  (%feeds-browse! (%feeds-shown-or-error)))

(define-command (feeds-show-copy-link)
  "Copy the address of the entry to the kill ring."
  (%feeds-copy-link! (%feeds-shown-or-error)))

(define-command (feeds-show-mark-unread)
  "Put the unread tag on the entry again."
  (feeds-tag! (%feeds-shown-or-error) 'unread)
  (%feeds-redraw! 'keep)
  (message "Marked unread."))

(define-command (feeds-show-add-tag)
  "Put a tag on the entry."
  (feeds-tag! (%feeds-shown-or-error) (%feeds-read-tag "Put on tag: "))
  (%feeds-redraw! 'keep))

(define-command (feeds-show-remove-tag)
  "Take a tag off the entry."
  (feeds-untag! (%feeds-shown-or-error) (%feeds-read-tag "Take off tag: "))
  (%feeds-redraw! 'keep))

(define-command (feeds-show-quit)
  "Go back to the list of entries."
  (kill-buffer %feeds-entry-buffer)
  (if (member %feeds-buffer (buffer-list))
      (switch-to-buffer %feeds-buffer)
      (feeds)))

(for-each
 (lambda (binding) (define-key 'feeds-show-mode (car binding) (cadr binding)))
 '(("n" feeds-show-next) ("p" feeds-show-previous) ("b" feeds-show-browse)
   ("y" feeds-show-copy-link) ("u" feeds-show-mark-unread) ("+" feeds-show-add-tag)
   ("-" feeds-show-remove-tag) ("RET" open-link-at-point) ("TAB" feeds-next-link)
   ("S-TAB" feeds-previous-link)
   ("SPC" scroll-up-command) ("DEL" scroll-down-command) ("q" feeds-show-quit)))
