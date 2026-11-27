;;; Matrix: rooms and their messages in buffers.
;;;
;;; The editor does not speak Matrix itself. A program beside it does,
;;; `amacs-matrix`, which has the Matrix SDK with its encryption and keeps
;;; what it knows on disk. The two talk in lines of JSON on the program's
;;; standard input and output:
;;;
;;;   {"id": 1, "method": "send", "params": {...}}   a request
;;;   {"id": 1, "result": ...}                       its answer, or
;;;   {"id": 1, "error": "what went wrong"}
;;;   {"event": "room", ...}                         what happened meanwhile
;;;
;;; M-x matrix starts the program and lists the rooms; RET on a room shows
;;; its messages, and RET there writes one.
;;;
;;; The words that the user sees are in Simplified Technical English.

(define-option 'matrix-user-id #f
  "Your Matrix address, as \"@name:example.org\". M-x matrix needs it.")

(define-option 'matrix-homeserver #f
  "The address of your Matrix server, as \"https://matrix.example.org\". #f
is to find it from the server name of matrix-user-id.")

(define-option 'matrix-program '("amacs-matrix")
  "The program that speaks Matrix for the editor, and its arguments.")

(define-option 'matrix-store-secret "amacs-matrix"
  "The host that the secret is kept under which locks what the Matrix
program has on disk: its keys of encryption and your session. secret-get is
asked for this host and your Matrix address as the user, so with the
password store the entry is amacs-matrix/@name:example.org, or amacs-matrix
alone. If there is no such secret, M-x matrix asks for a passphrase each
time.")

(define-option 'matrix-notify #t
  "Whether a message that your Matrix account's rules say to tell you of is
shown by the desktop too, and not only in the echo area.")

(define-option 'matrix-notify-program '("notify-send" "--app-name=amacs")
  "The program that shows a Matrix message on the desktop, and its first
arguments. The name of who wrote and the message are given after them.")

(define-option 'matrix-image-rows 10
  "The rows that a picture in a Matrix room gets at most, in a frame that
draws pictures. 0 is to show no pictures, only their names.")

(set-face! 'matrix-heading 'fg 'blue 'bold #t)
(set-face! 'matrix-unread 'bold #t)
(set-face! 'matrix-mention 'fg 'red 'bold #t)
(set-face! 'matrix-sender 'fg 'cyan 'bold #t)
(set-face! 'matrix-time 'fg 'fg-dim)
(set-face! 'matrix-note 'fg 'fg-dim 'italic #t)
(set-face! 'matrix-quote 'fg 'fg-dim)
(set-face! 'matrix-reaction 'fg 'yellow)

;;; State

(define %matrix-process #f)
(define %matrix-input "")
(define %matrix-next-id 0)
(define %matrix-pending (make-hash-table))
;; What the program last said of itself: "starting", "login", "syncing",
;; "offline", "error", and #f while it does not run.
(define %matrix-state #f)
(define %matrix-state-note #f)
;; Whether this device is one that the account trusts: without that, old
;; messages of encrypted rooms cannot be read. #t until the program says.
(define %matrix-verified #t)
;; The rooms and spaces by their ids, each as the program told of it.
(define %matrix-rooms (make-hash-table))
;; The messages that are shown, by room, or by room and thread: lists in
;; order. See %matrix-key.
(define %matrix-timelines (make-hash-table))
;; What each buffer of messages shows, by its name: a pair of the id of a
;; room and the id of the message that a thread starts with, or #f.
(define %matrix-views (make-hash-table))
;; The room of each row of the list of rooms, from the first row: #f for
;; rows that are of no room.
(define %matrix-rows '())
;; Where each message starts in a buffer of messages, by the buffer's
;; name: a list of pairs of a position and the message, the last first.
(define %matrix-item-starts (make-hash-table))
;; Whether a redraw is on its way.
(define %matrix-redraw-due #f)

(define %matrix-rooms-buffer "*matrix*")

(define (%matrix-get object key)
  ;; The value of a key, with #f for one that is not there or is null.
  (let ((value (json-ref object key)))
    (if (eq? value 'null) #f value)))

(define (%matrix-list value)
  (if (vector? value) (vector->list value) '()))

(define (%matrix-room-name room)
  (let ((name (%matrix-get room 'name)))
    (if (and (string? name) (not (string-null? name))) name (json-ref room 'id "?"))))

(define (%matrix-key room thread)
  ;; The name that the messages of a room, or of a thread in it, are
  ;; kept under.
  (if thread (string-append room " " thread) room))

(define (%matrix-view-buffer room thread)
  ;; The name of the buffer of a room, or of a thread in it.
  (string-append "*matrix: " (%matrix-room-name room)
                 (if thread
                     (string-append " / thread "
                                    (substring thread 0 (min 9 (string-length thread))))
                     "")
                 "*"))

;;; The connection

(define (%matrix-request method params callback)
  ;; Sends a request. The callback, if there is one, is called with the
  ;; result and the error, of which one is #f.
  (unless %matrix-process (error "Matrix is not running. Use M-x matrix."))
  (let ((id %matrix-next-id))
    (set! %matrix-next-id (+ id 1))
    (when callback (hash-table-set! %matrix-pending id callback))
    (process-send-string
     %matrix-process
     (string-append (json-encode (json-object 'id id 'method method 'params params)) "\n"))
    id))

(define (%matrix-report problem)
  ;; Says what went wrong with a request, if something did.
  (when (and problem (not (eq? problem 'null)))
    (message (string-append "Matrix: " (if (string? problem) problem "the request failed")))))

(define (%matrix-reporting result problem)
  ;; A callback for requests whose answer says nothing when all is well.
  (%matrix-report problem))

(define (%matrix-filter process text)
  ;; Whole lines of JSON are handled; a part line waits for its rest.
  (set! %matrix-input (string-append %matrix-input text))
  (let loop ()
    (let ((nl (string-search %matrix-input "\n")))
      (when nl
        (let ((line (substring %matrix-input 0 nl)))
          (set! %matrix-input
                (substring %matrix-input (+ nl 1) (string-length %matrix-input)))
          (unless (equal? (string-trim line) "")
            (guard (e (#t (log-message
                           (string-append "matrix: bad message: "
                                          (if (error-object? e) (error-object-message e) "?")))))
              (%matrix-dispatch (json-decode line))))
          (loop))))))

(define (%matrix-dispatch data)
  (let ((event (json-ref data 'event))
        (id (json-ref data 'id)))
    (cond [event (%matrix-event event data)]
          [id (let ((callback (hash-table-ref %matrix-pending id #f)))
                (when callback
                  (hash-table-delete! %matrix-pending id)
                  (callback (json-ref data 'result) (%matrix-get data 'error))))])))

(define (%matrix-stderr process text)
  (log-message (string-append "matrix: " (string-trim text))))

(define (%matrix-sentinel process status)
  (when (equal? process %matrix-process)
    (set! %matrix-process #f)
    (set! %matrix-state #f)
    (set! %matrix-state-note
          (string-append "The Matrix program stopped"
                         (if status (string-append " (" (number->string status) ")") "")
                         ". Use M-x matrix to start it again."))
    (set! %matrix-pending (make-hash-table))
    (%matrix-changed!)))

;;; What the program tells

(define (%matrix-event event data)
  (cond
   [(equal? event "state")
    (set! %matrix-state (json-ref data 'state))
    (set! %matrix-state-note (%matrix-get data 'message))
    (set! %matrix-verified (not (eq? (json-ref data 'verified #t) #f)))
    (%matrix-changed!)]
   [(equal? event "login-url")
    (message "Matrix: finish the login in the browser.")
    (browse-url (json-ref data 'url))]
   [(equal? event "room")
    (let ((room (json-ref data 'room)))
      (hash-table-set! %matrix-rooms (json-ref room 'id) room)
      (%matrix-changed!))]
   [(equal? event "room-removed")
    (hash-table-delete! %matrix-rooms (json-ref data 'id))
    (%matrix-changed!)]
   [(equal? event "timeline")
    (%matrix-timeline-event (%matrix-key (json-ref data 'room) (%matrix-get data 'thread))
                            (json-ref data 'op)
                            (%matrix-list (json-ref data 'items))
                            (%matrix-list (json-ref data 'ids)))
    (%matrix-changed!)]
   [(equal? event "notify") (%matrix-notify data)]
   [(equal? event "verify-request") (%matrix-verify-request data)]
   [(equal? event "verify-emojis") (%matrix-verify-emojis data)]
   [(equal? event "verify-done")
    (message (if (eq? (json-ref data 'ok) #t)
                 "Matrix: this device is verified."
                 (string-append "Matrix: the verification did not finish"
                                (let ((why (%matrix-get data 'message)))
                                  (if why (string-append ": " why) ".")))))]
   [else #f]))

(define (%matrix-notify data)
  ;; A message that the rules of the account say to tell of.
  (let* ((room (hash-table-ref %matrix-rooms (json-ref data 'room) #f))
         (who (string-append (or (%matrix-get data 'sender_name) "Somebody")
                             (if room (string-append " in " (%matrix-room-name room)) "")))
         (body (json-ref data 'body "")))
    (message (string-append who ": " body))
    (when (option 'matrix-notify)
      (let ((program (option 'matrix-notify-program)))
        ;; A desktop without the program is no reason to stop.
        (guard (e (#t #f))
          (start-process (car program) (append (cdr program) (list who body))))))))

(define (%matrix-timeline-event key op items ids)
  ;; Only what is shown has its messages kept.
  (when (hash-table-contains? %matrix-timelines key)
    (let ((old (hash-table-ref %matrix-timelines key '()))
          (id-of (lambda (item) (json-ref item 'id))))
      (hash-table-set!
       %matrix-timelines key
       (cond
        [(equal? op "reset") items]
        [(equal? op "append") (append old items)]
        [(equal? op "prepend") (append items old)]
        [(equal? op "update")
         (map (lambda (item)
                (let loop ((new items))
                  (cond [(null? new) item]
                        [(equal? (id-of (car new)) (id-of item)) (car new)]
                        [else (loop (cdr new))])))
              old)]
        [(equal? op "remove")
         (filter (lambda (item) (not (member (id-of item) ids))) old)]
        [else old])))))

;;; Verifying this device

(define (%matrix-verify-request data)
  ;; Another device of the account asks to verify this one.
  (let ((accept (y-or-n? "Matrix: another of your devices asks to verify this one. Start? ")))
    (%matrix-request (if accept "verify-accept" "verify-cancel") (json-object)
                     %matrix-reporting)))

(define (%matrix-verify-emojis data)
  ;; Both devices show the same emojis if nobody is between them.
  (let* ((emojis (map (lambda (emoji)
                        (string-append (json-ref emoji 'symbol "?") " "
                                       (json-ref emoji 'description "")))
                      (%matrix-list (json-ref data 'emojis))))
         (same (y-or-n? (string-append (string-join emojis "  ")
                                       "   Does the other device show these? "))))
    (%matrix-request "verify-confirm" (json-object 'match same) %matrix-reporting)))

;;; Drawing

(define (%matrix-changed!)
  ;; Draws everything again, once, after what is being handled now.
  (unless %matrix-redraw-due
    (set! %matrix-redraw-due #t)
    (run-later (lambda ()
                 (set! %matrix-redraw-due #f)
                 (%matrix-redraw!)))))

(define (%matrix-redraw!)
  (let ((buffers (buffer-list)))
    (when (member %matrix-rooms-buffer buffers)
      (%matrix-draw-rooms!))
    (for-each
     (lambda (name)
       (if (member name buffers)
           (%matrix-draw-view! name)
           ;; A buffer that was killed shows nothing any more.
           (%matrix-forget-view! name)))
     (hash-table-keys %matrix-views)))
  ;; Messages that come into the room that is being looked at are read.
  (let ((shown (window-buffer)))
    (when (hash-table-contains? %matrix-views shown)
      (with-current-buffer shown (%matrix-mark-read)))))

(define (%matrix-forget-view! name)
  (let ((view (hash-table-ref %matrix-views name #f)))
    (when view
      (hash-table-delete! %matrix-views name)
      (hash-table-delete! %matrix-item-starts name)
      (hash-table-delete! %matrix-timelines (%matrix-key (car view) (cdr view)))
      (when %matrix-process
        (%matrix-request "timeline-close" (%matrix-view-params view) #f)))))

(define (%matrix-state-line)
  (cond [(equal? %matrix-state "syncing")
         (if %matrix-verified
             ""
             "This device is not verified. Use M-x matrix-verify or M-x matrix-recover.")]
        [(equal? %matrix-state "starting") "Starting…"]
        [(equal? %matrix-state "login") "Not logged in. Use M-x matrix-login."]
        [(equal? %matrix-state "offline")
         (string-append "No connection" (if %matrix-state-note
                                            (string-append ": " %matrix-state-note) "") ".")]
        [(equal? %matrix-state "error") (or %matrix-state-note "The Matrix program has a problem.")]
        [else (or %matrix-state-note "Matrix is not running. Use M-x matrix.")]))

;; The text of a buffer and the faces to lay over it, built piece by
;; piece: (builder 'emit text face) adds text, with a face or #f.
(define (%matrix-builder)
  (let ((parts '()) (faces '()) (size 0))
    (lambda (command . args)
      (cond
       [(eq? command 'emit)
        (let ((text (car args)) (face (cadr args)))
          (when face
            (set! faces (cons (list size (+ size (string-length text)) face) faces)))
          (set! parts (cons text parts))
          (set! size (+ size (string-length text))))]
       [(eq? command 'length) size]
       [(eq? command 'text) (apply string-append (reverse parts))]
       [(eq? command 'faces) faces]))))

(define (%matrix-fill-buffer! builder keep-end?)
  ;; Makes what a builder holds the text of the current buffer. Point
  ;; stays where it is, or at the end if it was there and is to stay there.
  (let* ((old (point))
         (at-end (and keep-end? (= old (string-length (buffer-string)))))
         (text (builder 'text)))
    (%replace-buffer-text! text)
    (for-each (lambda (face) (add-face! (car face) (cadr face) (caddr face)))
              (builder 'faces))
    (goto-char (if at-end (string-length text) (min old (string-length text))))))

(define (%matrix-by-activity rooms)
  ;; The one last written in first.
  (sort rooms
        (lambda (a b)
          (let ((ta (json-ref a 'activity 0)) (tb (json-ref b 'activity 0)))
            (if (= ta tb)
                (string<? (%matrix-room-name a) (%matrix-room-name b))
                (> ta tb))))))

(define (%matrix-space? room)
  (eq? (json-ref room 'space) #t))

(define (%matrix-room-space room)
  ;; The space that a room is listed under: the first of those that it
  ;; is in and that are known, or #f.
  (let loop ((parents (%matrix-list (json-ref room 'parents))))
    (if (null? parents)
        #f
        (let ((space (hash-table-ref %matrix-rooms (car parents) #f)))
          (if (and space (%matrix-space? space))
              space
              (loop (cdr parents)))))))

(define (%matrix-groups)
  ;; The rooms in the groups that the list shows them in: a list of lists
  ;; of a heading and its rooms. First the spaces by name, then the talks
  ;; with one person, then the other rooms. A group without rooms is left
  ;; out.
  (let* ((all (map (lambda (id) (hash-table-ref %matrix-rooms id #f))
                   (hash-table-keys %matrix-rooms)))
         (rooms (filter (lambda (room) (not (%matrix-space? room))) all))
         (spaces (sort (filter %matrix-space? all)
                       (lambda (a b) (string<? (%matrix-room-name a) (%matrix-room-name b)))))
         (in-space (lambda (space)
                     (filter (lambda (room)
                               (let ((of (%matrix-room-space room)))
                                 (and of (equal? (json-ref of 'id) (json-ref space 'id)))))
                             rooms)))
         (loose (filter (lambda (room) (not (%matrix-room-space room))) rooms))
         (direct? (lambda (room) (eq? (json-ref room 'direct) #t))))
    (filter (lambda (group) (not (null? (cadr group))))
            (append
             (map (lambda (space) (list (%matrix-room-name space) (in-space space))) spaces)
             (list (list "People" (filter direct? loose))
                   (list "Rooms" (filter (lambda (room) (not (direct? room))) loose)))))))

(define (%matrix-draw-rooms!)
  (let ((builder (%matrix-builder))
        (rows '()))
    (define (row! id) (set! rows (cons id rows)))
    (builder 'emit (string-append "Matrix  " (or (option 'matrix-user-id) "")) 'matrix-heading)
    (builder 'emit "\n" #f)
    (row! #f)
    (let ((state (%matrix-state-line)))
      (unless (string-null? state)
        (builder 'emit (string-append state "\n") 'matrix-note)
        (row! #f)))
    (for-each
     (lambda (group)
       (builder 'emit "\n" #f)
       (row! #f)
       (builder 'emit (car group) 'matrix-heading)
       (builder 'emit "\n" #f)
       (row! #f)
       (for-each
        (lambda (room)
          (let* ((unread (json-ref room 'unread 0))
                 (mentions (json-ref room 'mentions 0))
                 (count (cond [(> mentions 0) (string-append (number->string mentions) "@")]
                              [(> unread 0) (number->string unread)]
                              [else ""])))
            (builder 'emit (string-pad-left count 4)
                     (if (> mentions 0) 'matrix-mention 'matrix-unread))
            (builder 'emit "  " #f)
            (builder 'emit (%matrix-room-name room)
                     (and (or (> unread 0) (> mentions 0)) 'matrix-unread))
            (builder 'emit "\n" #f)
            (row! (json-ref room 'id))))
        (%matrix-by-activity (cadr group))))
     (%matrix-groups))
    (set! %matrix-rows (reverse rows))
    (with-current-buffer %matrix-rooms-buffer
      (%matrix-fill-buffer! builder #f))))

(define (%matrix-clock seconds)
  (if (number? seconds) (format-time seconds "%H:%M") "     "))

(define (%matrix-body item)
  ;; What a message says, by its kind.
  (let ((kind (json-ref item 'kind "text"))
        (body (json-ref item 'body "")))
    (cond [(equal? kind "redacted") "(deleted)"]
          [(equal? kind "undecryptable") "(not decrypted: this device does not have the key)"]
          [(member kind '("image" "file")) (string-append "[" kind ": " body "]")]
          [else body])))

(define (%matrix-first-line text)
  (let ((nl (string-search text "\n")))
    (if nl (string-append (substring text 0 nl) " …") text)))

(define (%matrix-thumbnail item)
  ;; The file of the picture of a message, if the program has fetched one
  ;; and pictures are shown.
  (let ((path (%matrix-get (%matrix-get item 'file) 'thumbnail)))
    (and (string? path)
         (> (option 'matrix-image-rows) 0)
         (file-exists? path)
         path)))

(define (%matrix-draw-view! name)
  ;; Draws the messages of the buffer of a room or of a thread.
  (let* ((view (hash-table-ref %matrix-views name #f))
         (builder (%matrix-builder))
         (starts '())
         (pictures '())
         (items (hash-table-ref %matrix-timelines (%matrix-key (car view) (cdr view)) '())))
    (for-each
     (lambda (item)
       (let ((kind (json-ref item 'kind "text"))
             (sender (or (%matrix-get item 'sender_name) (json-ref item 'sender "?")))
             (reply (%matrix-get item 'reply_to))
             (thread (%matrix-get item 'thread))
             (reactions (%matrix-list (json-ref item 'reactions))))
         (set! starts (cons (cons (builder 'length) item) starts))
         ;; What a reply answers, above it.
         (when reply
           (builder 'emit
                    (string-append "      > " (or (%matrix-get reply 'sender_name) "?") ": "
                                   (%matrix-first-line (json-ref reply 'body "")) "\n")
                    'matrix-quote))
         (let ((picture (%matrix-thumbnail item)))
           (when picture
             (set! pictures (cons (cons (builder 'length) picture) pictures))))
         (builder 'emit (%matrix-clock (json-ref item 'time)) 'matrix-time)
         (builder 'emit " " #f)
         (cond
          [(member kind '("membership" "other" "redacted" "undecryptable"))
           (builder 'emit (string-append sender " " (%matrix-body item)) 'matrix-note)]
          [(equal? kind "emote")
           (builder 'emit (string-append "* " sender " " (%matrix-body item)) #f)]
          [else
           (builder 'emit sender 'matrix-sender)
           (builder 'emit (string-append ": " (%matrix-body item)) #f)])
         (when (eq? (json-ref item 'edited) #t)
           (builder 'emit " (edited)" 'matrix-note))
         (builder 'emit "\n" #f)
         (unless (null? reactions)
           (builder 'emit
                    (string-append
                     "      "
                     (string-join
                      (map (lambda (reaction)
                             (let ((count (json-ref reaction 'count 1)))
                               (string-append (json-ref reaction 'key "?")
                                              (if (> count 1)
                                                  (string-append " " (number->string count))
                                                  ""))))
                           reactions)
                      "  ")
                     "\n")
                    'matrix-reaction))
         ;; The answers that a message has in a thread of its own, in the
         ;; room: not in the thread itself.
         (when (and thread (not (cdr view)))
           (let ((count (json-ref thread 'count 0)))
             (builder 'emit
                      (string-append "      " (number->string count)
                                     (if (= count 1) " answer" " answers")
                                     " in a thread: t shows them\n")
                      'matrix-note)))))
     items)
    (hash-table-set! %matrix-item-starts name starts)
    (with-current-buffer name
      (%matrix-fill-buffer! builder #t)
      ;; The text was replaced, and its pictures went with it.
      (for-each (lambda (picture)
                  (add-image! (car picture) (cdr picture) (option 'matrix-image-rows)))
                pictures)
      (set-mode-line-text!
       (let ((state (%matrix-state-line)))
         (if (string-null? state) #f state))))))

;;; Modes

(define-major-mode matrix-rooms-mode special-mode "Matrix"
  "Major mode of the list of Matrix rooms: under each space its rooms, then
the talks with one person and the other rooms, the one last written in
first. Each has the number of messages that are not read; a number with @
is of messages that name you. RET shows the room of the line.")

(define-major-mode matrix-room-mode special-mode "Matrix-Room"
  "Major mode of a Matrix room or of a thread in one: its messages, the
newest last. RET writes a message of a line and c one of more lines, p
shows earlier messages and u sends a file. R and E answer and change in a
buffer too. On a message, r answers it, e changes it if it is yours, a puts a
reaction on it or takes yours off, d deletes it, o opens its file and t
shows its thread.")

(define (%matrix-row-room)
  ;; The id of the room of the line that point is on in the list of rooms.
  (let* ((before (buffer-substring 0 (point)))
         (row (length (filter (lambda (c) (char=? c #\newline)) (string->list before)))))
    (and (< row (length %matrix-rows)) (list-ref %matrix-rows row))))

(define (%matrix-current-view)
  ;; What the current buffer shows: a pair of the id of a room and of a
  ;; thread or #f. An error in a buffer that shows no messages.
  (or (hash-table-ref %matrix-views (buffer-name) #f)
      (error "There is no Matrix room here.")))

(define (%matrix-current-room)
  ;; The room that the current buffer shows, or the one of the line that
  ;; point is on in the list of rooms; an error if there is none.
  (let ((id (if (equal? (buffer-name) %matrix-rooms-buffer)
                (%matrix-row-room)
                (car (%matrix-current-view)))))
    (or (and id (hash-table-ref %matrix-rooms id #f))
        (error "There is no Matrix room here."))))

(define (%matrix-view-params view . more)
  ;; What a request about a room, or about a thread in it, is sent with:
  ;; `more` is further keys and values.
  (apply json-object 'room (car view)
         (append (if (cdr view) (list 'thread (cdr view)) '()) more)))

;;; Commands

(define (%matrix-passphrase)
  ;; The passphrase of the store: the secret of the option, or one typed.
  (or (guard (e (#t #f))
        (secret-get 'host (option 'matrix-store-secret) 'user (option 'matrix-user-id)))
      (read-passwd "Passphrase of the Matrix store: ")))

(define (%matrix-start!)
  (let ((user (option 'matrix-user-id)))
    (unless (string? user)
      (error "Set the matrix-user-id option to your Matrix address first."))
    (let ((passphrase (%matrix-passphrase)))
      (set! %matrix-input "")
      (set! %matrix-next-id 0)
      (set! %matrix-pending (make-hash-table))
      (set! %matrix-rooms (make-hash-table))
      (set! %matrix-timelines (make-hash-table))
      (set! %matrix-views (make-hash-table))
      (set! %matrix-item-starts (make-hash-table))
      (set! %matrix-state "starting")
      (set! %matrix-state-note #f)
      (set! %matrix-verified #t)
      (set! %matrix-process
            (make-process "matrix" (option 'matrix-program)
                          'filter %matrix-filter
                          'stderr %matrix-stderr
                          'sentinel %matrix-sentinel))
      (%matrix-request "start"
                       (json-object 'user user
                                    'homeserver (or (option 'matrix-homeserver) 'null)
                                    'passphrase passphrase)
                       %matrix-reporting))))

(define-command (matrix)
  "Show the list of Matrix rooms, and start the program that speaks Matrix
if it does not run. The matrix-user-id option says who you are."
  (unless %matrix-process (%matrix-start!))
  (switch-to-buffer %matrix-rooms-buffer)
  (unless (eq? (major-mode) 'matrix-rooms-mode) (matrix-rooms-mode))
  (%matrix-draw-rooms!))

(define-command (matrix-login)
  "Log in to your Matrix server. The server's page for that opens in the
browser. When you finish there, the rooms come."
  (%matrix-request "login" (json-object) %matrix-reporting))

(define-command (matrix-quit)
  "Stop the program that speaks Matrix. The buffers of the rooms stay."
  (when %matrix-process
    (process-send-eof %matrix-process)))

(define-command (matrix-verify)
  "Verify this device with another device of your account, such as
Element on a telephone: accept the request there, then compare the emojis
that both show. A verified device gets the keys of your encrypted rooms."
  (%matrix-request "verify-start" (json-object)
                   (lambda (result problem)
                     (if problem
                         (%matrix-report problem)
                         (message "Matrix: accept the verification on your other device.")))))

(define-command (matrix-recover)
  "Verify this device with the recovery key of your account, and get the
keys of your encrypted rooms from the server's backup."
  (let ((key (read-passwd "Recovery key: ")))
    (unless (string-null? (string-trim key))
      (%matrix-request "recover" (json-object 'key key)
                       (lambda (result problem)
                         (if problem
                             (%matrix-report problem)
                             (message "Matrix: this device is verified.")))))))

(define (%matrix-show-view! room thread)
  ;; Shows the buffer of a room, or of a thread in it, and asks the
  ;; program for its messages if they are not kept yet.
  (let* ((id (json-ref room 'id))
         (view (cons id thread))
         (name (%matrix-view-buffer room thread))
         (key (%matrix-key id thread)))
    (hash-table-set! %matrix-views name view)
    (unless (hash-table-contains? %matrix-timelines key)
      (hash-table-set! %matrix-timelines key '())
      (%matrix-request "timeline-open" (%matrix-view-params view) %matrix-reporting))
    (switch-to-buffer name)
    (unless (eq? (major-mode) 'matrix-room-mode) (matrix-room-mode))
    (%matrix-draw-view! name)
    (goto-char (string-length (buffer-string)))))

(define-command (matrix-open-room)
  "Show the messages of the room on this line."
  (%matrix-show-view! (%matrix-current-room) #f))

(define-command (matrix-switch-room)
  "Show the messages of a room that you choose by its name."
  (let* ((rooms (apply append (map cadr (%matrix-groups))))
         (name (completing-read "Room: " (map %matrix-room-name rooms)))
         (room (let loop ((rooms rooms))
                 (cond [(null? rooms) (error "There is no room of that name.")]
                       [(equal? (%matrix-room-name (car rooms)) name) (car rooms)]
                       [else (loop (cdr rooms))]))))
    (%matrix-show-view! room #f)))

(define-command (matrix-send)
  "Write a message to the room, or the thread, of this buffer."
  (let* ((view (%matrix-current-view))
         (room (%matrix-current-room))
         (text (read-string (string-append "To " (%matrix-room-name room)
                                           (if (cdr view) ", in the thread" "") ": "))))
    (unless (string-null? (string-trim text))
      (%matrix-request "send" (%matrix-view-params view 'body text) %matrix-reporting))))

(define (%matrix-item-at-point)
  ;; The message that point is in, in a buffer of messages; an error if
  ;; there is none.
  (let loop ((starts (hash-table-ref %matrix-item-starts (buffer-name) '())))
    (cond [(null? starts) (error "There is no message here.")]
          [(<= (car (car starts)) (point)) (cdr (car starts))]
          [else (loop (cdr starts))])))

(define (%matrix-on-message method callback . more)
  ;; Asks the program to do something with the message at point: `more`
  ;; is keys and values that go with the room and the message.
  (let ((view (%matrix-current-view))
        (item (%matrix-item-at-point)))
    (%matrix-request method
                     (apply %matrix-view-params view 'id (json-ref item 'id) more)
                     callback)))

(define-command (matrix-reply)
  "Answer the message at point."
  (let* ((item (%matrix-item-at-point))
         (who (or (%matrix-get item 'sender_name) (json-ref item 'sender "?")))
         (text (read-string (string-append "Answer to " who ": "))))
    (unless (string-null? (string-trim text))
      (%matrix-on-message "reply" %matrix-reporting 'body text))))

(define-command (matrix-edit)
  "Change the text of the message at point, which must be yours."
  (let ((item (%matrix-item-at-point)))
    (unless (eq? (json-ref item 'own) #t)
      (error "You can change only your own messages."))
    (let ((text (read-string "Change to: " (json-ref item 'body ""))))
      (unless (or (string-null? (string-trim text)) (equal? text (json-ref item 'body "")))
        (%matrix-on-message "edit" %matrix-reporting 'body text)))))

(define-command (matrix-react)
  "Put a reaction on the message at point: an emoji, or other short text.
A reaction that you put there before is taken off."
  (%matrix-item-at-point)
  (let ((key (string-trim (read-string "Reaction: "))))
    (unless (string-null? key)
      (%matrix-on-message "react" %matrix-reporting 'key key))))

(define-command (matrix-delete)
  "Delete the message at point for everybody in the room."
  (%matrix-item-at-point)
  (when (y-or-n? "Delete this message for everybody? ")
    (%matrix-on-message "redact" %matrix-reporting)))

(define-command (matrix-open-attachment)
  "Open the file or the picture of the message at point with the program
that the desktop has for it. The file is fetched first."
  (let ((item (%matrix-item-at-point)))
    (unless (%matrix-get item 'file)
      (error "This message has no file."))
    (message "Matrix: getting the file…")
    (%matrix-on-message "download"
                        (lambda (result problem)
                          (if (string? result)
                              (begin (message (string-append "Matrix: " result))
                                     (browse-url result))
                              (%matrix-report (or problem "the file did not come")))))))

(define-command (matrix-upload)
  "Send a file to the room, or the thread, of this buffer."
  (let* ((view (%matrix-current-view))
         (file (expand-file-name (read-file-name "Send file: "))))
    (unless (file-exists? file)
      (error "There is no such file."))
    (message "Matrix: sending the file…")
    (%matrix-request "upload" (%matrix-view-params view 'path file)
                     (lambda (result problem)
                       (if problem
                           (%matrix-report problem)
                           (message "Matrix: the file is sent."))))))

(define-command (matrix-open-thread)
  "Show the thread of the message at point in a buffer of its own: the
message that the thread starts with and the answers to it."
  (let* ((item (%matrix-item-at-point))
         (root (or (%matrix-get item 'thread_root) (json-ref item 'id))))
    (%matrix-show-view! (%matrix-current-room) root)))

;;; Messages of more than one line

(define-minor-mode matrix-compose-mode "Compose"
  "Writing a Matrix message of as many lines as it takes. C-c C-c sends it
and C-c C-k gives it up.")

;; What each buffer of a message that is being written is for, by its
;; name: a list of the buffer to go back to, the method and its parameters
;; without the text.
(define %matrix-composing (make-hash-table))

(define (%matrix-compose! method params initial)
  ;; Opens a buffer to write the text of a request in.
  (let* ((back (buffer-name))
         (name (string-append "*matrix compose: " (%matrix-room-name (%matrix-current-room)) "*")))
    (when (member name (buffer-list)) (kill-buffer name))
    (switch-to-buffer name)
    (insert initial)
    (enable-mode! 'matrix-compose-mode)
    (hash-table-set! %matrix-composing name (list back method params))
    (message "C-c C-c sends the message, C-c C-k gives it up.")))

(define (%matrix-compose-leave!)
  ;; Closes the buffer of the message and goes back to where it came from.
  (let* ((name (buffer-name))
         (pending (or (hash-table-ref %matrix-composing name #f)
                      (error "This buffer is not a Matrix message."))))
    (hash-table-delete! %matrix-composing name)
    (kill-buffer name)
    (when (member (car pending) (buffer-list))
      (switch-to-buffer (car pending)))
    pending))

(define-command (matrix-compose)
  "Write a message to the room, or the thread, of this buffer in a buffer
of its own, for a message of more than one line."
  (%matrix-compose! "send" (%matrix-view-params (%matrix-current-view)) ""))

(define-command (matrix-compose-reply)
  "Answer the message at point in a buffer of its own."
  (let ((item (%matrix-item-at-point)))
    (%matrix-compose! "reply"
                      (%matrix-view-params (%matrix-current-view) 'id (json-ref item 'id))
                      "")))

(define-command (matrix-compose-edit)
  "Change the message at point, which must be yours, in a buffer of its own."
  (let ((item (%matrix-item-at-point)))
    (unless (eq? (json-ref item 'own) #t)
      (error "You can change only your own messages."))
    (%matrix-compose! "edit"
                      (%matrix-view-params (%matrix-current-view) 'id (json-ref item 'id))
                      (json-ref item 'body ""))))

(define-command (matrix-compose-send)
  "Send the message that this buffer holds."
  (let ((text (string-trim (buffer-string))))
    (when (string-null? text)
      (error "The message is empty."))
    (let ((pending (%matrix-compose-leave!)))
      (%matrix-request (cadr pending)
                       (append (caddr pending) (json-object 'body text))
                       %matrix-reporting))))

(define-command (matrix-compose-cancel)
  "Give up the message that this buffer holds."
  (%matrix-compose-leave!)
  (message "The message was not sent."))

(define-key 'matrix-compose-mode "C-c C-c" 'matrix-compose-send)
(define-key 'matrix-compose-mode "C-c C-k" 'matrix-compose-cancel)

(define (%matrix-mark-read)
  ;; A room is read when its buffer is the one in use and point is at its
  ;; end: the program is told, once, while messages are not read.
  (when (and %matrix-process (hash-table-contains? %matrix-views (buffer-name))
             (= (point) (string-length (buffer-string))))
    (let ((room (hash-table-ref %matrix-rooms (car (%matrix-current-view)) #f)))
      (when (and room (or (> (json-ref room 'unread 0) 0) (> (json-ref room 'mentions 0) 0)))
        ;; Not asked for again before the program tells of the room anew.
        (hash-table-set! %matrix-rooms (json-ref room 'id)
                         (map (lambda (field)
                                (if (memq (car field) '(unread mentions)) (cons (car field) 0) field))
                              room))
        (%matrix-request "mark-read" (json-object 'room (json-ref room 'id)) #f)))))

(add-hook! 'post-command-hook %matrix-mark-read)

(define-command (matrix-load-earlier)
  "Show earlier messages of the room, or the thread, of this buffer."
  (%matrix-request "paginate-back" (%matrix-view-params (%matrix-current-view) 'count 30)
                   (lambda (result problem)
                     (%matrix-report problem)
                     (when (eq? result #f)
                       (message "There are no earlier messages.")))))

(define-command (matrix-close-room)
  "Close the room, or the thread, of this buffer: its messages are no
longer kept up to date here."
  (let ((name (buffer-name)))
    (%matrix-current-view)
    (%matrix-forget-view! name)
    (kill-buffer name)))

(for-each
 (lambda (binding) (define-key 'matrix-rooms-mode (car binding) (cadr binding)))
 '(("RET" matrix-open-room) ("g" matrix) ("s" matrix-switch-room)))

(for-each
 (lambda (binding) (define-key 'matrix-room-mode (car binding) (cadr binding)))
 '(("RET" matrix-send) ("p" matrix-load-earlier) ("l" matrix) ("k" matrix-close-room)
   ("r" matrix-reply) ("e" matrix-edit) ("a" matrix-react) ("d" matrix-delete)
   ("o" matrix-open-attachment) ("u" matrix-upload) ("t" matrix-open-thread)
   ("s" matrix-switch-room) ("c" matrix-compose) ("R" matrix-compose-reply)
   ("E" matrix-compose-edit)))
