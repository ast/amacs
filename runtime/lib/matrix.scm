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

(define-option 'matrix-store-secret "amacs/matrix"
  "The name of the secret that locks what the Matrix program keeps on disk:
its keys of encryption and your session. secret-get finds it by this name
as the host, so with the password store it is the entry of that name. If
there is no such secret, M-x matrix asks for a passphrase each time.")

(set-face! 'matrix-heading 'fg 'blue 'bold #t)
(set-face! 'matrix-unread 'bold #t)
(set-face! 'matrix-mention 'fg 'red 'bold #t)
(set-face! 'matrix-sender 'fg 'cyan 'bold #t)
(set-face! 'matrix-time 'fg 'fg-dim)
(set-face! 'matrix-note 'fg 'fg-dim 'italic #t)

;;; State

(define %matrix-process #f)
(define %matrix-input "")
(define %matrix-next-id 0)
(define %matrix-pending (make-hash-table))
;; What the program last said of itself: "starting", "login", "syncing",
;; "offline", "error", and #f while it does not run.
(define %matrix-state #f)
(define %matrix-state-note #f)
;; The rooms by their ids, each as the program told of it.
(define %matrix-rooms (make-hash-table))
;; The messages of the rooms that are open, by room id: lists in order.
(define %matrix-timelines (make-hash-table))
;; The room of each row of the list of rooms, from the first row: #f for
;; rows that are of no room.
(define %matrix-rows '())
;; Whether a redraw is on its way.
(define %matrix-redraw-due #f)

(define %matrix-rooms-buffer "*matrix*")

(define (%matrix-room-buffer room)
  (string-append "*matrix: " (%matrix-room-name room) "*"))

(define (%matrix-list value)
  (if (vector? value) (vector->list value) '()))

(define (%matrix-room-name room)
  (let ((name (json-ref room 'name)))
    (if (and (string? name) (not (string-null? name))) name (json-ref room 'id "?"))))

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

(define (%matrix-report error)
  ;; Says what went wrong with a request, if something did.
  (when error (message (string-append "Matrix: " (if (string? error) error "the request failed")))))

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

(define (%matrix-dispatch message)
  (let ((event (json-ref message 'event))
        (id (json-ref message 'id)))
    (cond [event (%matrix-event event message)]
          [id (let ((callback (hash-table-ref %matrix-pending id #f)))
                (when callback
                  (hash-table-delete! %matrix-pending id)
                  (callback (json-ref message 'result) (json-ref message 'error))))])))

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
    (set! %matrix-state-note (json-ref data 'message))
    (%matrix-changed!)]
   [(equal? event "login-url")
    (let ((url (json-ref data 'url)))
      (message "Matrix: finish the login in the browser.")
      (browse-url url))]
   [(equal? event "room")
    (let ((room (json-ref data 'room)))
      (hash-table-set! %matrix-rooms (json-ref room 'id) room)
      (%matrix-changed!))]
   [(equal? event "room-removed")
    (hash-table-delete! %matrix-rooms (json-ref data 'id))
    (%matrix-changed!)]
   [(equal? event "timeline")
    (%matrix-timeline-event (json-ref data 'room) (json-ref data 'op)
                            (%matrix-list (json-ref data 'items))
                            (%matrix-list (json-ref data 'ids)))
    (%matrix-changed!)]
   [else #f]))

(define (%matrix-timeline-event room op items ids)
  ;; Only the rooms that are open have their messages kept.
  (when (hash-table-contains? %matrix-timelines room)
    (let ((old (hash-table-ref %matrix-timelines room '()))
          (id-of (lambda (item) (json-ref item 'id))))
      (hash-table-set!
       %matrix-timelines room
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

;;; Drawing

(define (%matrix-changed!)
  ;; Draws everything again, once, after what is being handled now.
  (unless %matrix-redraw-due
    (set! %matrix-redraw-due #t)
    (run-later (lambda ()
                 (set! %matrix-redraw-due #f)
                 (%matrix-redraw!)))))

(define (%matrix-redraw!)
  (when (member %matrix-rooms-buffer (buffer-list))
    (%matrix-draw-rooms!))
  (for-each
   (lambda (id)
     (let ((room (hash-table-ref %matrix-rooms id #f)))
       (when (and room (member (%matrix-room-buffer room) (buffer-list)))
         (%matrix-draw-room! room))))
   (hash-table-keys %matrix-timelines)))

(define (%matrix-state-line)
  (cond [(equal? %matrix-state "syncing") ""]
        [(equal? %matrix-state "starting") "Starting…"]
        [(equal? %matrix-state "login") "Not logged in. Use M-x matrix-login."]
        [(equal? %matrix-state "offline")
         (string-append "No connection" (if %matrix-state-note
                                            (string-append ": " %matrix-state-note) "") ".")]
        [(equal? %matrix-state "error") (or %matrix-state-note "The Matrix program has a problem.")]
        [else (or %matrix-state-note "Matrix is not running. Use M-x matrix.")]))

;; A list of the text of a buffer and the faces to lay over it, built by
;; rows: `(emit! text face)` adds text, with a face or #f.
(define (%matrix-builder)
  (let ((parts '()) (faces '()) (length 0))
    (lambda (command . args)
      (cond
       [(eq? command 'emit)
        (let ((text (car args)) (face (cadr args)))
          (when face
            (set! faces (cons (list length (+ length (string-length text)) face) faces)))
          (set! parts (cons text parts))
          (set! length (+ length (string-length text))))]
       [(eq? command 'length) length]
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

(define (%matrix-sorted-rooms)
  ;; The rooms that are no spaces, the one last written in first.
  (sort (filter (lambda (room) (not (eq? (json-ref room 'space) #t)))
                (map (lambda (id) (hash-table-ref %matrix-rooms id #f))
                     (hash-table-keys %matrix-rooms)))
        (lambda (a b)
          (let ((ta (json-ref a 'activity 0)) (tb (json-ref b 'activity 0)))
            (if (= ta tb)
                (string<? (%matrix-room-name a) (%matrix-room-name b))
                (> ta tb))))))

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
     (%matrix-sorted-rooms))
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

(define (%matrix-draw-room! room)
  (let ((builder (%matrix-builder))
        (items (hash-table-ref %matrix-timelines (json-ref room 'id) '())))
    (for-each
     (lambda (item)
       (let ((kind (json-ref item 'kind "text"))
             (sender (or (json-ref item 'sender_name) (json-ref item 'sender "?"))))
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
         (builder 'emit "\n" #f)))
     items)
    (with-current-buffer (%matrix-room-buffer room)
      (%matrix-fill-buffer! builder #t)
      (set-mode-line-text!
       (let ((state (%matrix-state-line)))
         (if (string-null? state) #f state))))))

;;; Modes

(define-major-mode matrix-rooms-mode special-mode "Matrix"
  "Major mode of the list of Matrix rooms, the one last written in first,
each with the number of messages that are not read; a number with @ is of
messages that name you. RET shows the room of the line.")

(define-major-mode matrix-room-mode special-mode "Matrix-Room"
  "Major mode of a Matrix room: its messages, the newest last. RET writes
a message, and p shows earlier messages.")

(define (%matrix-row-room)
  ;; The id of the room of the line that point is on in the list of rooms.
  (let* ((before (buffer-substring 0 (point)))
         (row (length (filter (lambda (c) (char=? c #\newline)) (string->list before)))))
    (and (< row (length %matrix-rows)) (list-ref %matrix-rows row))))

(define (%matrix-current-room)
  ;; The room that the current buffer shows, or the one of the line that
  ;; point is on in the list of rooms; an error if there is none.
  (let* ((name (buffer-name))
         (id (if (equal? name %matrix-rooms-buffer)
                 (%matrix-row-room)
                 (let loop ((ids (hash-table-keys %matrix-rooms)))
                   (cond [(null? ids) #f]
                         [(equal? (%matrix-room-buffer (hash-table-ref %matrix-rooms (car ids) #f))
                                  name)
                          (car ids)]
                         [else (loop (cdr ids))])))))
    (or (and id (hash-table-ref %matrix-rooms id #f))
        (error "There is no Matrix room here."))))

;;; Commands

(define (%matrix-passphrase)
  ;; The passphrase of the store: the secret of the option, or one typed.
  (or (guard (e (#t #f)) (secret-get 'host (option 'matrix-store-secret)))
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
      (set! %matrix-state "starting")
      (set! %matrix-state-note #f)
      (set! %matrix-process
            (make-process "matrix" (option 'matrix-program)
                          'filter %matrix-filter
                          'stderr %matrix-stderr
                          'sentinel %matrix-sentinel))
      (%matrix-request "start"
                       (json-object 'user user
                                    'homeserver (or (option 'matrix-homeserver) 'null)
                                    'passphrase passphrase)
                       (lambda (result error) (%matrix-report error))))))

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
  (%matrix-request "login" (json-object) (lambda (result error) (%matrix-report error))))

(define-command (matrix-quit)
  "Stop the program that speaks Matrix. The buffers of the rooms stay."
  (when %matrix-process
    (let ((process %matrix-process))
      (process-send-eof process))))

(define-command (matrix-open-room)
  "Show the messages of the room on this line."
  (let* ((room (%matrix-current-room))
         (id (json-ref room 'id)))
    (unless (hash-table-contains? %matrix-timelines id)
      (hash-table-set! %matrix-timelines id '())
      (%matrix-request "timeline-open" (json-object 'room id)
                       (lambda (result error) (%matrix-report error))))
    (switch-to-buffer (%matrix-room-buffer room))
    (unless (eq? (major-mode) 'matrix-room-mode) (matrix-room-mode))
    (%matrix-draw-room! room)
    (goto-char (string-length (buffer-string)))))

(define-command (matrix-send)
  "Write a message to the room of this buffer."
  (let* ((room (%matrix-current-room))
         (text (read-string (string-append "To " (%matrix-room-name room) ": "))))
    (unless (string-null? (string-trim text))
      (%matrix-request "send" (json-object 'room (json-ref room 'id) 'body text)
                       (lambda (result error) (%matrix-report error))))))

(define-command (matrix-load-earlier)
  "Show earlier messages of the room of this buffer."
  (let ((room (%matrix-current-room)))
    (%matrix-request "paginate-back" (json-object 'room (json-ref room 'id) 'count 30)
                     (lambda (result error)
                       (%matrix-report error)
                       (when (eq? result #f)
                         (message "There are no earlier messages."))))))

(define-command (matrix-close-room)
  "Close the room of this buffer: its messages are no longer kept up to
date here."
  (let* ((room (%matrix-current-room))
         (id (json-ref room 'id)))
    (hash-table-delete! %matrix-timelines id)
    (when %matrix-process
      (%matrix-request "timeline-close" (json-object 'room id) #f))
    (kill-buffer (%matrix-room-buffer room))))

(for-each
 (lambda (binding) (define-key 'matrix-rooms-mode (car binding) (cadr binding)))
 '(("RET" matrix-open-room) ("g" matrix)))

(for-each
 (lambda (binding) (define-key 'matrix-room-mode (car binding) (cadr binding)))
 '(("RET" matrix-send) ("p" matrix-load-earlier) ("l" matrix) ("k" matrix-close-room)))
