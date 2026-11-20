;;; Talking to a coding agent over the Agent Client Protocol.
;;;
;;; The Agent Client Protocol (agentclientprotocol.com) is JSON-RPC over the
;;; standard input and output of an agent, the way Zed and others talk to
;;; coding agents. Claude Code speaks it through the `claude-agent-acp`
;;; adapter. This is a first, small client: it starts the agent, opens a
;;; session, sends a prompt and shows the reply as it streams, answers the
;;; agent when it reads or writes a file, and asks the user when the agent
;;; wants to do something that needs leave.
;;;
;;; It is deliberately one connection and one buffer for now; the full
;;; agent layer (several sessions, a conversation buffer on `section.scm`, a
;;; compose buffer, the backend seam for other agents) comes next.

(define-option 'acp-command '("claude-agent-acp")
  "The command that speaks the Agent Client Protocol, a list of the program
and its arguments. The default is the Claude Code adapter.")

(define %acp-buffer "*claude*")
(define %acp-process #f)
(define %acp-session #f)
(define %acp-next-id 0)
;; Ids we are waiting on, to the procedure (result error) that handles the reply.
(define %acp-pending (make-hash-table))
;; Output from the agent not yet split into whole lines.
(define %acp-input "")
;; Whether a turn is in progress, for the mode line and to refuse overlap.
(define %acp-busy #f)

;;; The connection

(define (%acp-send message)
  (process-send-string %acp-process (string-append (json-encode message) "\n")))

(define (%acp-request method params callback)
  "Send a request and call `callback` with (result error) when the reply
comes."
  (let ((id %acp-next-id))
    (set! %acp-next-id (+ id 1))
    (when callback (hash-table-set! %acp-pending id callback))
    (%acp-send (json-object 'jsonrpc "2.0" 'id id 'method method 'params params))
    id))

(define (%acp-notify method params)
  (%acp-send (json-object 'jsonrpc "2.0" 'method method 'params params)))

(define (%acp-respond id result)
  (%acp-send (json-object 'jsonrpc "2.0" 'id id 'result result)))

(define (%acp-respond-error id code message)
  (%acp-send (json-object 'jsonrpc "2.0" 'id id
                          'error (json-object 'code code 'message message))))

(define (%acp-filter process text)
  ;; Whole lines of JSON are handled; a part line waits for its rest.
  (set! %acp-input (string-append %acp-input text))
  (let loop ()
    (let ((nl (string-search %acp-input "\n")))
      (when nl
        (let ((line (substring %acp-input 0 nl)))
          (set! %acp-input (substring %acp-input (+ nl 1) (string-length %acp-input)))
          (unless (equal? (string-trim line) "")
            (guard (e (#t (%acp-log (string-append "\n[amacs: bad message: "
                                                   (if (error-object? e) (error-object-message e) "?")
                                                   "]\n"))))
              (%acp-dispatch (json-decode line))))
          (loop))))))

(define (%acp-dispatch message)
  (let ((id (json-ref message 'id))
        (method (json-ref message 'method)))
    (cond [(and id method) (%acp-handle-request id method (json-ref message 'params))]
          [method (%acp-handle-notification method (json-ref message 'params))]
          [id (%acp-handle-response id message)])))

(define (%acp-handle-response id message)
  (let ((callback (hash-table-ref %acp-pending id #f)))
    (when callback
      (hash-table-delete! %acp-pending id)
      (callback (json-ref message 'result) (json-ref message 'error)))))

;;; Notifications from the agent: the reply as it streams

(define (%acp-handle-notification method params)
  (cond [(equal? method "session/update") (%acp-update (json-ref params 'update))]
        [else #f]))

(define (%acp-update update)
  (let ((kind (json-ref update 'sessionUpdate)))
    (cond
      [(equal? kind "agent_message_chunk")
       (%acp-log (%acp-content-text (json-ref update 'content)))]
      [(equal? kind "agent_thought_chunk")
       (%acp-log-face (%acp-content-text (json-ref update 'content)) 'shadow)]
      [(equal? kind "tool_call")
       (%acp-log (string-append "\n⚙ " (or (json-ref update 'title) "tool")
                                " [" (or (json-ref update 'status) "") "]\n"))]
      [(equal? kind "tool_call_update")
       (let ((status (json-ref update 'status)))
         (when (member status '("completed" "failed"))
           (%acp-log (string-append "  → " status "\n"))))]
      [(equal? kind "plan")
       (%acp-log (string-append "\n📋 plan: " (number->string (vector-length (or (json-ref update 'entries) #()))) " steps\n"))]
      [(equal? kind "available_commands_update") #f]
      [(equal? kind "current_mode_update")
       (%acp-log (string-append "\n[mode: " (or (json-ref update 'currentModeId) "") "]\n"))]
      [else #f])))

(define (%acp-content-text content)
  (cond [(not content) ""]
        [(equal? (json-ref content 'type) "text") (or (json-ref content 'text) "")]
        [else ""]))

;;; Requests from the agent: files, terminals, permission

(define (%acp-handle-request id method params)
  (cond [(equal? method "fs/read_text_file") (%acp-fs-read id params)]
        [(equal? method "fs/write_text_file") (%acp-fs-write id params)]
        [(equal? method "session/request_permission") (%acp-permission id params)]
        [else (%acp-respond-error id -32601 (string-append "amacs does not support " method))]))

(define (%acp-buffer-visiting path)
  (let loop ((buffers (buffer-list)))
    (cond [(null? buffers) #f]
          [(equal? (with-current-buffer (car buffers) (buffer-file-name)) path) (car buffers)]
          [else (loop (cdr buffers))])))

(define (%acp-fs-read id params)
  ;; Unsaved edits are seen: a buffer visiting the file wins over the disk.
  (let* ((path (json-ref params 'path))
         (buffer (%acp-buffer-visiting path))
         (content (cond [buffer (buffer-string buffer)]
                        [(file-exists? path) (read-file path)]
                        [else ""])))
    (%acp-respond id (json-object 'content content))))

(define (%acp-fs-write id params)
  (let* ((path (json-ref params 'path))
         (content (json-ref params 'content))
         (buffer (%acp-buffer-visiting path)))
    (write-file path content)
    ;; Keep an open buffer in step with what the agent wrote.
    (when buffer (with-current-buffer buffer (revert-buffer)))
    (%acp-respond id '())))

(define (%acp-permission id params)
  ;; The agent wants leave for a tool call. Ask, and pick an option.
  (let* ((tool (json-ref (json-ref params 'toolCall) 'title))
         (options (vector->list (or (json-ref params 'options) #()))))
    (run-later
     (lambda ()
       (let ((allow (y-or-n? (string-append "Claude: " (or tool "a tool call") " — allow? "))))
         (let ((option (%acp-choose-option options allow)))
           (if option
               (%acp-respond id (json-object 'outcome (json-object 'outcome "selected" 'optionId option)))
               (%acp-respond id (json-object 'outcome (json-object 'outcome "cancelled"))))))))))

(define (%acp-choose-option options allow)
  "The id of an option to answer with: an allow-once when allowing, a
reject-once when not."
  (let ((want (if allow '("allow_once" "allow_always") '("reject_once" "reject_always"))))
    (let loop ((rest options))
      (cond [(null? rest) (and (pair? options) (json-ref (car options) 'optionId))]
            [(member (json-ref (car rest) 'kind) want) (json-ref (car rest) 'optionId)]
            [else (loop (cdr rest))]))))

;;; The buffer

(define (%acp-ensure-buffer)
  (unless (member %acp-buffer (buffer-list))
    (switch-to-buffer %acp-buffer)
    (markdown-mode)
    (set-buffer-read-only! #t))
  %acp-buffer)

(define (%acp-log text)
  (when (and text (not (equal? text "")) (member %acp-buffer (buffer-list)))
    (with-current-buffer %acp-buffer
      (set-buffer-read-only! #f)
      (save-excursion (goto-char (point-max)) (insert text))
      (set-buffer-read-only! #t)
      (%acp-follow))))

(define (%acp-log-face text face)
  (when (and text (not (equal? text "")) (member %acp-buffer (buffer-list)))
    (with-current-buffer %acp-buffer
      (set-buffer-read-only! #f)
      (let ((start (point-max)))
        (save-excursion (goto-char start) (insert text))
        (add-face! start (+ start (string-length text)) face))
      (set-buffer-read-only! #t)
      (%acp-follow))))

(define (%acp-follow)
  ;; Keep the end in view in every window showing the buffer.
  (for-each (lambda (window)
              (when (equal? (window-buffer window) %acp-buffer)
                (with-selected-window window (goto-char (point-max)))))
            (window-list)))

;;; Commands

(define (%acp-connected?)
  (and %acp-process (process-live? %acp-process)))

(define-command (claude)
  "Start Claude for this project over the Agent Client Protocol, or show
its buffer if it is already running. Ask it something with M-x claude-send."
  (%acp-ensure-buffer)
  (switch-to-buffer %acp-buffer)
  (if (%acp-connected?)
      (message "Claude is running. M-x claude-send to ask something.")
      (%acp-connect)))

(define (%acp-connect)
  (set! %acp-input "")
  (set! %acp-session #f)
  (set! %acp-next-id 0)
  (set! %acp-pending (make-hash-table))
  (let ((root (or (project-root) (working-directory))))
    (set! %acp-process
      (make-process "claude-acp" (option 'acp-command)
                    'filter %acp-filter
                    'stderr %acp-stderr
                    'sentinel %acp-sentinel
                    'directory root))
    (%acp-log "Starting Claude…\n")
    (%acp-request "initialize"
      (json-object 'protocolVersion 1
                   'clientCapabilities (json-object
                                        'fs (json-object 'readTextFile #t 'writeTextFile #t))
                   'clientInfo (json-object 'name "amacs" 'version "0.1.0"))
      (lambda (result error)
        (if error
            (%acp-log (string-append "\n[initialize failed: " (or (json-ref error 'message) "?") "]\n"))
            (%acp-new-session root))))))

(define (%acp-new-session root)
  (%acp-request "session/new"
    (json-object 'cwd root 'mcpServers (json-array))
    (lambda (result error)
      (if error
          (%acp-log (string-append "\n[session/new failed: " (or (json-ref error 'message) "?") "]\n"))
          (begin
            (set! %acp-session (json-ref result 'sessionId))
            (%acp-log (string-append "Ready (" (or (json-ref (json-ref result 'models) 'currentModelId) "claude") "). Ask with M-x claude-send.\n")))))))

(define (%acp-stderr process text)
  ;; The adapter's own logs on standard error: kept apart from the protocol
  ;; on standard output, and dropped. Useful to log while debugging.
  #f)

(define (%acp-sentinel process status)
  (set! %acp-process #f)
  (set! %acp-session #f)
  (set! %acp-busy #f)
  (%acp-log (string-append "\n[Claude ended" (if status (string-append " (" (number->string status) ")") "") "]\n")))

(define-command (claude-send)
  "Ask the running Claude something: read a line and send it as a prompt."
  (unless %acp-session (error "Claude is not running; M-x claude first"))
  (when %acp-busy (error "Claude is still working; wait or M-x claude-cancel"))
  (let ((text (read-string "Ask Claude: ")))
    (unless (equal? (string-trim text) "")
      (%acp-send-prompt text))))

(define (%acp-send-prompt text)
  (%acp-log (string-append "\n\n# ▶ " text "\n\n"))
  (set! %acp-busy #t)
  (%acp-request "session/prompt"
    (json-object 'sessionId %acp-session
                 'prompt (json-array (json-object 'type "text" 'text text)))
    (lambda (result error)
      (set! %acp-busy #f)
      (cond [error (%acp-log (string-append "\n[error: " (or (json-ref error 'message) "?") "]\n"))]
            [else (%acp-log (string-append "\n\n— " (or (json-ref result 'stopReason) "done") " —\n"))]))))

(define-command (claude-cancel)
  "Cancel the current Claude turn."
  (when (and %acp-session %acp-busy)
    (%acp-notify "session/cancel" (json-object 'sessionId %acp-session))
    (message "Asked Claude to stop.")))

(define-command (claude-quit)
  "Stop the Claude process."
  (when (%acp-connected?)
    (delete-process %acp-process))
  (set! %acp-process #f)
  (set! %acp-session #f)
  (set! %acp-busy #f)
  (message "Claude stopped."))

(global-set-key "C-c c c" 'claude)
(global-set-key "C-c c p" 'claude-send)
(global-set-key "C-c c k" 'claude-cancel)
