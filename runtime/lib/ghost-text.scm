;;; GhostText: edit the text fields of a web browser in the editor.
;;;
;;; The GhostText extension of Firefox and Chrome hands a text field of a
;;; page to an editor that listens on a port of this machine, and keeps
;;; the two the same while either is typed in. This is the editor's side:
;;; `M-x ghost-text-start` listens; a field that the extension is turned on
;;; for comes up in a buffer of its own; what is typed there is in the page
;;; a moment later; `C-c C-c` hands the field back.
;;;
;;; The protocol is the extension's: it asks the port with a plain GET for
;;; the port of the WebSocket, connects, and sends the field as JSON, with
;;; `text` and `selections`, again each time the field changes in the
;;; page. The editor answers with the same two each time the buffer
;;; changes. Selections count in the code units of UTF-16.

(define-option 'ghost-text-port 4001
  "The port that the editor listens on for the GhostText extension of a web
browser. 4001 is what the extension asks unless its options say otherwise.")

(define-option 'ghost-text-origins
  '("moz-extension://" "chrome-extension://" "safari-web-extension://")
  "What the origin of a program that connects for GhostText must start
with. These are the extensions of browsers. A web page has the origin of
its site, and is refused: any page can ask a browser to connect to a port
of this machine.")

(define-option 'ghost-text-modes
  '(("(^|\\.)github\\.com$" markdown-mode)
    ("(^|\\.)gitlab\\.com$" markdown-mode)
    ("(^|\\.)reddit\\.com$" markdown-mode)
    ("(^|\\.)stackoverflow\\.com$" markdown-mode)
    ("(^|\\.)stackexchange\\.com$" markdown-mode))
  "The major modes for text fields of web sites: lists of a regular
expression and a mode. The first whose expression matches the host of the
page gives the mode; `ghost-text-default-mode` is for the rest.")

(define-option 'ghost-text-default-mode 'text-mode
  "The major mode for a text field of a web site that `ghost-text-modes`
does not name.")

(define-option 'ghost-text-new-window #f
  "Whether a text field of a web browser gets a window of its own, which
closes when the field is done. Otherwise the field is shown in the frame
that was last used.")

(define-option 'ghost-text-delay 50
  "How long, in milliseconds, the editor collects changes to a text field
of a web browser before it sends them.")

(define-minor-mode ghost-text-mode "Ghost"
  "Editing a text field of a web browser. What is typed goes to the page as
it is typed. C-c C-c hands the field back, and C-c C-k does so with the
text that the field had at the start.")

;; A field that is being edited. `sent` is the texts last sent to the
;; browser, newest first, and `tick` the buffer's tick when it and the
;; browser last had the same text.
(define-record-type %ghost-field
  (%make-ghost-field connection buffer title original sent tick pending window)
  %ghost-field?
  (connection %ghost-field-connection)
  (buffer %ghost-field-buffer)
  (title %ghost-field-title)
  (original %ghost-field-original)
  (sent %ghost-field-sent %ghost-set-sent!)
  (tick %ghost-field-tick %ghost-set-tick!)
  (pending %ghost-field-pending? %ghost-set-pending!)
  ;; The window that was opened for the field, if one was.
  (window %ghost-field-window %ghost-set-window!))

(define %ghost-server #f)
;; The fields by the names of their buffers and by their connections.
(define %ghost-by-buffer (make-hash-table))
(define %ghost-by-connection (make-hash-table))
;; The buffers of the fields that wait for a window to open, oldest first.
(define %ghost-awaiting '())
;; How many of the texts sent are remembered.
(define %ghost-memory 4)

;;; From the browser

(define (%ghost-host url)
  ;; The extension sends the host; a whole address is taken apart.
  (let ((found (and (string? url)
                    (regexp-search "^(?:[a-zA-Z][a-zA-Z0-9+.-]*://)?([^/?#]*)" url))))
    (if found
        (substring url (car (cadr found)) (cadr (cadr found)))
        "")))

(define (%ghost-mode-for host)
  (let loop ((entries (option 'ghost-text-modes)))
    (cond [(null? entries) (option 'ghost-text-default-mode)]
          [(regexp-search (car (car entries)) host) (cadr (car entries))]
          [else (loop (cdr entries))])))

(define (%ghost-position text units)
  (if (and (integer? units) (>= units 0))
      (utf16->char-index text units)
      0))

(define (%ghost-select! text selections)
  ;; Puts point, and the region if there is one, where the browser has them.
  (let ((first (and (vector? selections)
                    (> (vector-length selections) 0)
                    (vector-ref selections 0))))
    (when (pair? first)
      (let ((start (%ghost-position text (json-ref first 'start 0)))
            (end (%ghost-position text (json-ref first 'end 0))))
        (goto-char end)
        (unless (= start end)
          (set-mark! start)
          (activate-mark!))))))

(define (%ghost-open! connection data)
  (let* ((text (json-ref data 'text ""))
         (host (%ghost-host (json-ref data 'url "")))
         (title (let ((title (json-ref data 'title "")))
                  (if (string? title) title "")))
         (label (cond [(not (string-null? host)) host]
                      [(not (string-null? title)) title]
                      [else "field"]))
         (name (make-buffer (string-append "*ghost: " label "*")))
         (field (%make-ghost-field connection name title text (list text) 0 #f #f)))
    (with-current-buffer name
      (%replace-buffer-text! text)
      (call-command (%ghost-mode-for host))
      (enable-mode! 'ghost-text-mode)
      (%ghost-select! text (json-ref data 'selections #f))
      (%ghost-set-tick! field (buffer-tick)))
    (hash-table-set! %ghost-by-buffer name field)
    (hash-table-set! %ghost-by-connection connection field)
    (if (option 'ghost-text-new-window)
        (%ghost-open-window! name)
        (%ghost-show! name))
    (message "This buffer is a text field of " label ". Type C-c C-c to finish.")))

(define (%ghost-update! field text)
  ;; The field changed in the page. A text that was sent is the browser
  ;; saying it back, and by now the buffer may be ahead of it.
  (unless (member text (%ghost-field-sent field))
    (with-current-buffer (%ghost-field-buffer field)
      (let ((old (buffer-string)))
        (unless (equal? old text)
          ;; Only what differs is replaced, and point stays on the text
          ;; that it is on.
          (let* ((same (string-difference old text))
                 (start (car same))
                 (end (- (string-length old) (cadr same)))
                 (point (point)))
            (delete-region start end)
            (goto-char start)
            (insert (substring text start (- (string-length text) (cadr same))))
            (goto-char (cond [(<= point start) point]
                             [(>= point end)
                              (+ point (- (string-length text) (string-length old)))]
                             [else start])))))
      (%ghost-set-tick! field (buffer-tick)))
    (%ghost-set-sent! field (list text))))

(define (%ghost-message connection message)
  (let* ((data (guard (e (#t #f)) (json-decode message)))
         (text (json-ref data 'text #f))
         (field (hash-table-ref %ghost-by-connection connection)))
    (cond [(not (string? text)) #f]
          [field (%ghost-update! field text)]
          [else (%ghost-open! connection data)])))

(define (%ghost-forget! field)
  (hash-table-delete! %ghost-by-buffer (%ghost-field-buffer field))
  (hash-table-delete! %ghost-by-connection (%ghost-field-connection field))
  (set! %ghost-awaiting (delete (%ghost-field-buffer field) %ghost-awaiting)))

(define (%ghost-changed? field)
  (not (= (buffer-tick (%ghost-field-buffer field)) (%ghost-field-tick field))))

(define (%ghost-closed connection)
  ;; The page went away, or the extension was turned off for the field.
  (let ((field (hash-table-ref %ghost-by-connection connection)))
    (when field
      (let ((name (%ghost-field-buffer field)))
        (%ghost-forget! field)
        (cond [(%ghost-changed? field)
               ;; The browser does not have the last of the text: keep it.
               (with-current-buffer name (disable-mode! 'ghost-text-mode))
               (message "The browser closed the text field. The text is in " name ".")]
              [else
               (%ghost-close-window! field)
               (kill-buffer name)
               (message "The browser closed the text field.")])))))

;;; Showing a field

(define (%ghost-show! name)
  ;; A prompt in the frame is not to be put aside for a field.
  (if (in-minibuffer?)
      (message "A text field of the browser is in the buffer " name ".")
      (switch-to-buffer name)))

(define (%ghost-open-window! name)
  ;; The window is a client of the editor like any other. What it is to
  ;; show is said when it is there: see `%ghost-take-frame`.
  (set! %ghost-awaiting (append %ghost-awaiting (list name)))
  (guard (e (#t (%ghost-no-window! name)))
    (start-process (editor-program "amacsclient") '("-c" "-n")))
  (run-later
   (lambda ()
     (sleep-for 3000)
     (%ghost-no-window! name))))

(define (%ghost-no-window! name)
  ;; No window came for the field: show it where the user is.
  (when (member name %ghost-awaiting)
    (set! %ghost-awaiting (delete name %ghost-awaiting))
    (when (hash-table-ref %ghost-by-buffer name)
      (%ghost-show! name))))

(define (%ghost-take-frame)
  (unless (null? %ghost-awaiting)
    (let* ((name (car %ghost-awaiting))
           (field (hash-table-ref %ghost-by-buffer name)))
      (set! %ghost-awaiting (cdr %ghost-awaiting))
      (when field
        (switch-to-buffer name)
        (%ghost-set-window! field (selected-window))))))

(define (%ghost-close-window! field)
  (let ((window (%ghost-field-window field)))
    (when (and window (window-live? window))
      (guard (e (#t #f))
        (with-selected-window window (call-command 'delete-frame))))))

;;; To the browser

(define (%ghost-send! field text start end)
  (%ghost-set-sent! field
                    (let ((sent (cons text (%ghost-field-sent field))))
                      (if (> (length sent) %ghost-memory)
                          (list-head sent %ghost-memory)
                          sent)))
  (websocket-send
   (%ghost-field-connection field)
   (json-encode
    (json-object 'text text
                 'selections (vector (json-object 'start (char-index->utf16 text start)
                                                  'end (char-index->utf16 text end)))))))

(define (%ghost-sync! field)
  ;; Sends the buffer as it is now, if the browser does not have it.
  (when (%ghost-changed? field)
    (with-current-buffer (%ghost-field-buffer field)
      (let* ((point (point))
             (other (if (and (region-active?) (mark)) (mark) point)))
        (%ghost-set-tick! field (buffer-tick))
        (%ghost-send! field (buffer-string) (min point other) (max point other))))))

(define (%ghost-current-field)
  (or (hash-table-ref %ghost-by-buffer (buffer-name))
      (error "This buffer is not a text field of a web browser")))

(define (%ghost-live? field)
  (eq? (hash-table-ref %ghost-by-buffer (%ghost-field-buffer field)) field))

(define (%ghost-after-command)
  ;; Changes are collected for a moment: a key held down is many commands.
  (let ((field (hash-table-ref %ghost-by-buffer (buffer-name))))
    (when (and field
               (not (%ghost-field-pending? field))
               (%ghost-changed? field))
      (%ghost-set-pending! field #t)
      (run-later
       (lambda ()
         (unwind-protect
          (sleep-for (option 'ghost-text-delay))
          (%ghost-set-pending! field #f))
         (when (%ghost-live? field)
           (%ghost-sync! field)))))))

(define (%ghost-killed name)
  ;; A field whose buffer is killed is handed back as it was last sent.
  (let ((field (hash-table-ref %ghost-by-buffer name)))
    (when field
      (%ghost-forget! field)
      (websocket-close (%ghost-field-connection field))
      (%ghost-close-window! field))))

(define (%ghost-hand-back! field)
  (let ((name (%ghost-field-buffer field)))
    (%ghost-forget! field)
    ;; The connection is closed after what was sent on it has gone out.
    (websocket-close (%ghost-field-connection field))
    (%ghost-close-window! field)
    (kill-buffer name)))

(define-command (ghost-text-finish)
  "Hand the text field of this buffer back to the web browser."
  (let ((field (%ghost-current-field)))
    (%ghost-sync! field)
    (%ghost-hand-back! field)
    (message "The text is in the browser.")))

(define-command (ghost-text-cancel)
  "Hand the text field of this buffer back to the web browser with the
text that it had when the editing began."
  (let* ((field (%ghost-current-field))
         (original (%ghost-field-original field))
         (end (string-length original)))
    (%ghost-send! field original end end)
    (%ghost-hand-back! field)
    (message "The text field is as it was.")))

(define-key 'ghost-text-mode "C-c C-c" 'ghost-text-finish)
(define-key 'ghost-text-mode "C-x #" 'ghost-text-finish)
(define-key 'ghost-text-mode "C-c C-k" 'ghost-text-cancel)

(add-hook! 'post-command-hook %ghost-after-command)
(add-hook! 'kill-buffer-hook %ghost-killed)
(add-hook! 'after-make-frame-hook %ghost-take-frame)

;;; The server

(define (ghost-text-running?)
  "Whether the editor listens for the GhostText extension of a web browser."
  (and %ghost-server
       (memv %ghost-server (websocket-server-list))
       #t))

(define (ghost-text-fields)
  "The names of the buffers that are text fields of a web browser."
  (hash-table-keys %ghost-by-buffer))

(define-command (ghost-text-start)
  "Listen for the GhostText extension of a web browser, which hands the
text fields of pages to the editor. In the browser, the extension's button
or its key, Control-Shift-H unless changed, turns it on for a field. The
port is the `ghost-text-port` option. For an init file:
(ghost-text-start)."
  (if (ghost-text-running?)
      (message "The editor already listens for GhostText on port "
               (websocket-server-port %ghost-server) ".")
      (let* ((asked (option 'ghost-text-port))
             (server
              (guard (e (#t (error (string-append
                                    "Port " (number->string asked)
                                    " is not available for GhostText. Stop the"
                                    " program that uses it, or change the"
                                    " ghost-text-port option."))))
                ;; The extension first asks which port the WebSocket is on:
                ;; this one, whatever number it turns out to have.
                (make-websocket-server asked
                                       'origins (option 'ghost-text-origins)
                                       'message %ghost-message
                                       'close %ghost-closed))))
        (set! %ghost-server server)
        (%ghost-answer! server)
        (message "The editor listens for GhostText on port "
                 (websocket-server-port server) "."))))

(define (%ghost-answer! server)
  (set-websocket-server-reply!
   server
   (json-encode (json-object 'ProtocolVersion 1
                             'WebSocketPort (websocket-server-port server)))))

(define-command (ghost-text-stop)
  "Stop listening for the GhostText extension. The text fields that are
being edited are handed back as they are."
  (when (ghost-text-running?)
    (for-each (lambda (name)
                (let ((field (hash-table-ref %ghost-by-buffer name)))
                  (%ghost-sync! field)
                  (%ghost-hand-back! field)))
              (ghost-text-fields))
    (delete-websocket-server %ghost-server))
  (set! %ghost-server #f)
  (message "The editor does not listen for GhostText."))
