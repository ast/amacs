;;; Talking to a coding agent over the Agent Client Protocol.
;;;
;;; The Agent Client Protocol (agentclientprotocol.com) is JSON-RPC over the
;;; standard input and output of an agent, the way Zed and others talk to
;;; coding agents. Claude Code speaks it through the `claude-agent-acp`
;;; adapter. This client starts the agent, opens a session, sends prompts
;;; written in a compose buffer, answers the agent when it reads or writes a
;;; file or runs a program, and shows the user what the agent wants leave
;;; for, to be answered when it suits: nothing is asked in the minibuffer
;;; while something else is being typed.
;;;
;;; The conversation is a buffer of sections, as the git buffers are: a
;;; section for each turn, with the agent's thoughts folded away, its tool
;;; calls with their state and the changes they made, and its plan. What the
;;; agent sends changes a model of the conversation, and the buffer is drawn
;;; from the model, so that a tool call changes where it stands.
;;;
;;; There can be several sessions at once, each with an agent of its own, a
;;; conversation and a compose buffer. What is known of a session is in
;;; session variables, made with `define-acp-variable`: they hold the values
;;; of the session at hand, as the local variables of a buffer hold those of
;;; the current buffer. A command makes the session of the buffer it is
;;; given in the one at hand, and what comes from an agent is handled with
;;; the session of that agent at hand.

(define-option 'acp-command '("claude-agent-acp")
  "The command that speaks the Agent Client Protocol, a list of the program
and its arguments. The default is the Claude Code adapter.")

(define-option 'acp-mcp-servers '()
  "The Model Context Protocol servers that an agent is given with each
session, besides those of its own configuration. Each is a list of a name
and a command with its arguments, such as (\"amacs\" \"amacs-mcp\"), or of a
name and the URL of a server that speaks HTTP.")

(define-option 'acp-additional-directories '()
  "Directories besides that of the project that an agent may work in, for
an agent that takes such: a list of absolute paths, given with each session
that is started.")

;;; Sessions

;; The session variables, in the order they were defined: each a vector of
;; a procedure that makes the value a new session starts with, one that
;; reads the variable and one that sets it.
(define %acp-variables '())

(define-syntax define-acp-variable
  (syntax-rules ()
    [(_ name init)
     (begin
       (define name init)
       (set! %acp-variables
         (append %acp-variables
                 (list (vector (lambda () init)
                               (lambda () name)
                               (lambda (value) (set! name value)))))))]))

(set-documentation! 'define-acp-variable
  "(define-acp-variable name init)
Defines a variable that has a value of its own in each session with an
agent: it holds the value of the session at hand. The expression is
evaluated anew for each session that is made.")

;; A session is a vector of the values of its variables, as they were when
;; it was last at hand, and its number, which its buffers are named by. The
;; variables themselves hold the values of the session at hand.
(define %acp-here (vector '() 1))
;; Every session, the oldest first.
(define %acp-sessions (list %acp-here))

(define (%acp-name base number)
  ;; The name of a buffer of a session: the first has the plain name.
  (if (= number 1)
      base
      (string-append base "<" (number->string number) ">")))

(define (%acp-enter! session)
  ;; Makes a session the one at hand: the variables are given its values,
  ;; and those of the one that was at hand are kept with it.
  (unless (eq? session %acp-here)
    (vector-set! %acp-here 0
                 (map (lambda (variable) ((vector-ref variable 1))) %acp-variables))
    (set! %acp-here session)
    (for-each (lambda (variable value) ((vector-ref variable 2) value))
              %acp-variables
              (vector-ref session 0))))

(define (%acp-in session thunk)
  ;; Calls a procedure with a session at hand, and has the one that was at
  ;; hand so again afterwards.
  (let ((before %acp-here))
    (dynamic-wind
      (lambda () (%acp-enter! session))
      thunk
      (lambda () (%acp-enter! before)))))

(define (%acp-bound proc)
  ;; A procedure that calls another with the session that is at hand now,
  ;; for what an agent or a program does later.
  (let ((session %acp-here))
    (lambda arguments
      (%acp-in session (lambda () (apply proc arguments))))))

(define (%acp-session-showing name)
  ;; The session that a buffer is the conversation or the compose buffer of.
  (find (lambda (session)
          (let ((number (vector-ref session 1)))
            (or (equal? name (%acp-name "*claude*" number))
                (equal? name (%acp-name "*claude prompt*" number)))))
        %acp-sessions))

(define (%acp-new-session!)
  ;; A session of which nothing is known yet, which becomes the one at
  ;; hand. Those whose agent has ended and whose conversation was killed
  ;; are forgotten, and their numbers are free again.
  (set! %acp-sessions
    (filter (lambda (session)
              (or (eq? session %acp-here)
                  (%acp-in session
                    (lambda () (or (%acp-connected?) (member %acp-buffer (buffer-list)))))))
            %acp-sessions))
  (let* ((taken (map (lambda (session) (vector-ref session 1)) %acp-sessions))
         (number (let loop ((n 1)) (if (memv n taken) (loop (+ n 1)) n)))
         (session (vector '() number)))
    (vector-set! %acp-here 0
                 (map (lambda (variable) ((vector-ref variable 1))) %acp-variables))
    (set! %acp-sessions (append %acp-sessions (list session)))
    (set! %acp-here session)
    (for-each (lambda (variable) ((vector-ref variable 2) ((vector-ref variable 0))))
              %acp-variables)
    session))

(define (%acp-find-session wanted?)
  ;; The first session of which a procedure, called with it at hand, says
  ;; yes: the one at hand before the others.
  (find (lambda (session) (%acp-in session wanted?))
        (cons %acp-here %acp-sessions)))

(define (%acp-select!)
  ;; Makes the session that a command is about the one at hand: that of the
  ;; conversation or compose buffer it is given in, or one whose agent
  ;; works in the project of the buffer, or else the one already at hand.
  (let ((root (or (project-root) (working-directory))))
    (%acp-enter!
     (or (%acp-session-showing (buffer-name))
         (%acp-find-session (lambda () (and (%acp-connected?) (equal? %acp-root root))))
         %acp-here))))

(define-acp-variable %acp-buffer (%acp-name "*claude*" (vector-ref %acp-here 1)))
(define-acp-variable %acp-prompt-buffer
  (%acp-name "*claude prompt*" (vector-ref %acp-here 1)))
(define-acp-variable %acp-process #f)
(define-acp-variable %acp-session #f)
(define-acp-variable %acp-next-id 0)
;; Ids we are waiting on, to the procedure (result error) that handles the reply.
(define-acp-variable %acp-pending (make-hash-table))
;; Output from the agent not yet split into whole lines.
(define-acp-variable %acp-input "")
;; What the agent said of itself when the connection was made: what it can
;; do is in it.
(define-acp-variable %acp-agent #f)
;; Whether a turn is in progress, for the mode line and to hold back the
;; prompts written meanwhile.
(define-acp-variable %acp-busy #f)
;; Prompts written while the agent was at work, oldest first: each a list
;; of the text and, if it has more than text, what is sent for it.
(define-acp-variable %acp-queue '())
;; Where the agent works, which its relative paths are taken from.
(define-acp-variable %acp-root #f)
;; The title that the agent has given the conversation, or #f.
(define-acp-variable %acp-title #f)
;; The session that the agent of this conversation had when it ended, to
;; take it up again where it was.
(define-acp-variable %acp-resumable #f)
;; Whether the agent is telling of a conversation that was, and the id of
;; the last message of the user in it.
(define-acp-variable %acp-replaying #f)
(define-acp-variable %acp-replayed-id #f)
;; The model and the mode of the session, as the agent names them.
(define-acp-variable %acp-model #f)
(define-acp-variable %acp-mode #f)
;; How much of the model's context the conversation fills, and how much
;; there is, in tokens; #f until the agent has said.
(define-acp-variable %acp-used #f)
(define-acp-variable %acp-size #f)
;; What of the session can be set, as the agent tells of it: the mode, the
;; model and whatever else, each an object with its values and the one now.
(define-acp-variable %acp-options '())
;; The modes as agents told of them before there were options, kept for an
;; agent that has no option for the mode.
(define-acp-variable %acp-modes #f)
;; The slash commands that the agent knows, objects with a name, a
;; description and perhaps a hint of what to write after the name.
(define-acp-variable %acp-commands '())
;; What the agent has asked leave for and has no answer to yet, oldest
;; first. Each is a vector of the id of the request, the id of the tool
;; call, and the options: lists of a key, a label, the id to answer with and
;; the kind.
(define-acp-variable %acp-permissions '())
;; The programs run for the agent, by the ids they were given. Each is a
;; vector of the process, what it has written, whether the start of that
;; was dropped, how it ended (#f while it runs, then a pair of the exit
;; status and the signal), the requests that wait for it to end, how many
;; bytes of output to keep, how many are kept, whether the editor ended it,
;; and whether the agent is done with it.
(define-acp-variable %acp-terminals (make-hash-table))
;; The ids are of every session, since the programs are named by them.
(define %acp-next-terminal 0)
;; The tool call that shows a terminal, by the id of the terminal.
(define-acp-variable %acp-terminal-tools (make-hash-table))

(set-face! 'agent-prompt 'fg 'blue 'bold #t)
(set-face! 'agent-tool 'bold #t)
(set-face! 'agent-pending 'fg 'fg-dim)
(set-face! 'agent-done 'fg 'fg-added)
(set-face! 'agent-failed 'fg 'fg-removed 'bold #t)
(set-face! 'agent-asks 'fg 'yellow-warmer 'bold #t)

(define (%acp-get object key)
  ;; A field of an object of JSON, with null and a missing field both #f.
  (let ((value (and (pair? object) (json-ref object key))))
    (and (not (eq? value 'null)) value)))

(define (%acp-list value)
  ;; An array of JSON, or the lack of one, as a list.
  (if (vector? value) (vector->list value) '()))

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
            (guard (e (#t (%acp-note (string-append "[amacs: bad message: "
                                                    (if (error-object? e) (error-object-message e) "?")
                                                    "]"))))
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
  (let ((kind (%acp-get update 'sessionUpdate)))
    (cond
      [(equal? kind "agent_message_chunk")
       (%acp-add-text! 'message (%acp-content-text (%acp-get update 'content)))]
      [(equal? kind "agent_thought_chunk")
       (%acp-add-text! 'thought (%acp-content-text (%acp-get update 'content)))]
      [(equal? kind "tool_call") (%acp-tool-call! update)]
      [(equal? kind "tool_call_update") (%acp-tool-update! update)]
      [(equal? kind "plan") (%acp-plan! (%acp-list (%acp-get update 'entries)))]
      [(equal? kind "available_commands_update")
       (set! %acp-commands (%acp-list (%acp-get update 'availableCommands)))]
      [(equal? kind "current_mode_update")
       (set! %acp-mode (%acp-get update 'currentModeId))
       (%acp-show-state)]
      [(equal? kind "usage_update")
       (set! %acp-used (%acp-get update 'used))
       (set! %acp-size (%acp-get update 'size))
       (%acp-show-state)]
      [(equal? kind "config_option_update")
       (%acp-read-options! (%acp-get update 'configOptions))
       (%acp-show-state)]
      [(equal? kind "session_info_update")
       (when (%acp-get update 'title) (set! %acp-title (%acp-get update 'title)))]
      [(equal? kind "user_message_chunk")
       ;; What the user wrote is only news in a conversation that was.
       (when %acp-replaying
         (%acp-replay-prompt! (%acp-content-text (%acp-get update 'content))
                              (%acp-get update 'messageId)))]
      [else #f])))

(define (%acp-content-text content)
  (if (equal? (%acp-get content 'type) "text")
      (or (%acp-get content 'text) "")
      ""))

;;; Requests from the agent: files, terminals, permission

(define (%acp-handle-request id method params)
  (cond [(equal? method "fs/read_text_file") (%acp-fs-read id params)]
        [(equal? method "fs/write_text_file") (%acp-fs-write id params)]
        [(equal? method "session/request_permission") (%acp-permission id params)]
        [(equal? method "terminal/create") (%acp-terminal-create id params)]
        [(equal? method "terminal/output")
         (%acp-with-terminal id params
           (lambda (terminal)
             (%acp-respond id (json-object 'output (vector-ref terminal 1)
                                           'truncated (vector-ref terminal 2)
                                           'exitStatus (if (vector-ref terminal 3)
                                                           (%acp-terminal-exit terminal)
                                                           'null)))))]
        [(equal? method "terminal/wait_for_exit")
         (%acp-with-terminal id params
           (lambda (terminal)
             (if (vector-ref terminal 3)
                 (%acp-respond id (%acp-terminal-exit terminal))
                 (vector-set! terminal 4 (cons id (vector-ref terminal 4))))))]
        [(equal? method "terminal/kill")
         (%acp-with-terminal id params
           (lambda (terminal)
             (%acp-terminal-end! terminal)
             (%acp-respond id '())))]
        [(equal? method "terminal/release")
         (%acp-with-terminal id params
           (lambda (terminal)
             (%acp-terminal-end! terminal)
             (vector-set! terminal 8 #t)
             (%acp-respond id '())))]
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
    (%acp-respond id (json-object 'content (%acp-slice content
                                                       (%acp-get params 'line)
                                                       (%acp-get params 'limit))))))

(define (%acp-slice text line limit)
  ;; The lines of a text from a line, counted from 1, and no more of them
  ;; than a limit; all of it when neither is given.
  (if (not (or line limit))
      text
      (let* ((lines (string-split text #\newline))
             (skip (min (length lines) (max 0 (- (or line 1) 1))))
             (rest (list-tail lines skip)))
        (string-join (if (and limit (< limit (length rest))) (list-head rest limit) rest)
                     "\n"))))

(define (%acp-fs-write id params)
  (let* ((path (json-ref params 'path))
         (content (json-ref params 'content))
         (buffer (%acp-buffer-visiting path)))
    (write-file path content)
    ;; Keep an open buffer in step with what the agent wrote.
    (when buffer (with-current-buffer buffer (revert-buffer)))
    (%acp-respond id '())))

;;; Leave for a tool call
;;;
;;; The agent asks, and waits. The request is shown with the tool call it
;;; is about, with a key for each answer the agent offers, and is answered
;;; by a command; the user is not stopped in what they are doing.

(define (%acp-option-keys options)
  ;; A key for each option: y to allow once, a to allow always, n to refuse
  ;; once, r to refuse always, and a digit for one whose key is taken or
  ;; whose kind is not known.
  (let loop ((rest options) (taken '()) (digit 1) (out '()))
    (if (null? rest)
        (reverse out)
        (let* ((option (car rest))
               (kind (%acp-get option 'kind))
               (wanted (cond [(equal? kind "allow_once") "y"]
                             [(equal? kind "allow_always") "a"]
                             [(equal? kind "reject_once") "n"]
                             [(equal? kind "reject_always") "r"]
                             [else #f]))
               (free? (and wanted (not (member wanted taken))))
               (key (if free? wanted (number->string digit))))
          (loop (cdr rest)
                (cons key taken)
                (if free? digit (+ digit 1))
                (cons (list key
                            (or (%acp-get option 'name) kind "?")
                            (%acp-get option 'optionId)
                            kind)
                      out))))))

(define (%acp-permission id params)
  (let* ((tool (%acp-get params 'toolCall))
         (call (%acp-get tool 'toolCallId))
         (request (vector id call
                          (%acp-option-keys (%acp-list (%acp-get params 'options))))))
    ;; What the request says of the tool call is news of it, and makes it
    ;; known if it was not.
    (if call
        (%acp-tool-update! tool)
        (%acp-note "[The agent asks leave for something it does not name]"))
    (set! %acp-permissions (append %acp-permissions (list request)))
    (%acp-changed!)
    (%acp-show-state)
    (message "Claude asks leave"
             (if (null? (cdr %acp-sessions)) "" (string-append " in " %acp-buffer))
             ": " (%acp-request-title request)
             " (C-c c y allows, C-c c n refuses, C-c c o chooses)")))

(define (%acp-request-title request)
  (let ((item (hash-table-ref %acp-tools (vector-ref request 1) #f)))
    (or (and item (%acp-get (vector-ref item 1) 'title)) "a tool call")))

(define (%acp-request-for call)
  ;; The request that waits for an answer about a tool call, or #f.
  (and call
       (find (lambda (request) (equal? (vector-ref request 1) call)) %acp-permissions)))

(define (%acp-answer! request option)
  ;; Answers a request with the id of an option, or with #f to say that
  ;; nothing was chosen. A request that is no longer waiting is left alone.
  (when (memq request %acp-permissions)
    (set! %acp-permissions
      (filter (lambda (other) (not (eq? other request))) %acp-permissions))
    (%acp-respond (vector-ref request 0)
                  (json-object 'outcome
                               (if option
                                   (json-object 'outcome "selected" 'optionId option)
                                   (json-object 'outcome "cancelled"))))
    (let ((item (hash-table-ref %acp-tools (vector-ref request 1) #f)))
      (when item (%acp-turn-set-drawn! (vector-ref item 2) #f)))
    (%acp-changed!)
    (%acp-show-state)))

(define (%acp-option-of request kinds)
  ;; The first option of the first of the kinds that the request has one of.
  (let loop ((kinds kinds))
    (and (pair? kinds)
         (or (find (lambda (option) (equal? (list-ref option 3) (car kinds)))
                   (vector-ref request 2))
             (loop (cdr kinds))))))

(define (%acp-asked)
  ;; The request to answer: the oldest of the session of this buffer if it
  ;; asks, or of the one at hand, or of any other that asks. That session
  ;; becomes the one at hand.
  (let* ((asks? (lambda () (pair? %acp-permissions)))
         (shown (%acp-session-showing (buffer-name)))
         (session (or (and shown (%acp-in shown asks?) shown)
                      (%acp-find-session asks?))))
    (unless session
      (error "Claude is not asking leave for anything"))
    (%acp-enter! session)
    (car %acp-permissions)))

(define-command (claude-allow)
  "Allow, this once, what Claude has asked leave for the longest. Works from
any buffer. C-c c o shows the other answers, such as allowing it always."
  (let* ((request (%acp-asked))
         (title (%acp-request-title request))
         (option (%acp-option-of request '("allow_once"))))
    (unless option
      (error "Nothing here allows it just this once; C-c c o shows the choices"))
    (%acp-answer! request (caddr option))
    (message "Allowed: " title)))

(define-command (claude-refuse)
  "Refuse what Claude has asked leave for the longest. Works from any
buffer."
  (let* ((request (%acp-asked))
         (title (%acp-request-title request))
         (option (%acp-option-of request '("reject_once" "reject_always"))))
    (%acp-answer! request (and option (caddr option)))
    (message "Refused: " title)))

(define-command (claude-answer)
  "Show the answers that Claude offers to what it has asked leave for the
longest, each with its key, and give the one whose key is pressed."
  (let* ((request (%acp-asked))
         (title (%acp-request-title request))
         (choice (car (key-menu (string-append "Claude asks leave: " title)
                                '()
                                (map (lambda (option) (list-head option 3))
                                     (vector-ref request 2))))))
    (%acp-answer! request choice)
    (message "Answered: " title)))

;;; Programs run for the agent
;;;
;;; An agent that has no shell of its own asks the editor to run a program,
;;; and then for what it wrote and how it ended. Claude Code runs its own
;;; and does not ask.

;; How much of the output of a program is kept when the agent names no
;; limit, in bytes.
(define %acp-terminal-limit 1000000)

(define (%acp-bytes text)
  ;; How many bytes a text is as UTF-8, which the agent's limit is in.
  (let loop ((chars (string->list text)) (count 0))
    (if (null? chars)
        count
        (loop (cdr chars)
              (+ count (let ((code (char->integer (car chars))))
                         (cond [(< code #x80) 1]
                               [(< code #x800) 2]
                               [(< code #x10000) 3]
                               [else 4])))))))

(define (%acp-with-terminal id params proc)
  (let ((terminal (hash-table-ref %acp-terminals (%acp-get params 'terminalId) #f)))
    (if (and terminal (not (vector-ref terminal 8)))
        (proc terminal)
        (%acp-respond-error id -32602 "amacs has no such terminal"))))

(define (%acp-terminal-exit terminal)
  (let ((exit (vector-ref terminal 3)))
    (json-object 'exitCode (or (car exit) 'null) 'signal (or (cdr exit) 'null))))

(define (%acp-terminal-end! terminal)
  ;; Ends the program, if it runs. Its sentinel says so to who waits.
  (let ((process (vector-ref terminal 0)))
    (when (and process (not (vector-ref terminal 3)) (process-live? process))
      (vector-set! terminal 7 #t)
      (kill-process process))))

(define (%acp-terminal-changed! name)
  ;; The tool call that shows the terminal is drawn again.
  (let ((item (hash-table-ref %acp-terminal-tools name #f)))
    (when item
      (%acp-turn-set-drawn! (vector-ref item 2) #f)
      (%acp-changed!))))

(define (%acp-terminal-create id params)
  (let* ((command (%acp-get params 'command))
         (arguments (%acp-list (%acp-get params 'args)))
         (name (string-append "term-" (number->string %acp-next-terminal)))
         (terminal (vector #f "" #f #f '()
                           (or (%acp-get params 'outputByteLimit) %acp-terminal-limit)
                           0 #f #f)))
    (set! %acp-next-terminal (+ %acp-next-terminal 1))
    (hash-table-set! %acp-terminals name terminal)
    (guard (e (#t (hash-table-delete! %acp-terminals name)
                  (%acp-respond-error id -32603
                                      (if (error-object? e)
                                          (error-object-message e)
                                          "the program could not be started"))))
      (vector-set! terminal 0
        (make-process
         (string-append "claude-" name)
         ;; A command with no arguments and a space in it is a line for a
         ;; shell, as some agents send.
         (if (and (null? arguments) (string-search command " "))
             (list "sh" "-c" command)
             (cons command arguments))
         'filter (%acp-bound
                  (lambda (process text) (%acp-terminal-output! name terminal text)))
         'sentinel (%acp-bound
                    (lambda (process status)
                      (vector-set! terminal 3
                        (cons status
                              (and (not status)
                                   (if (vector-ref terminal 7) "SIGKILL" "unknown"))))
                      (for-each (lambda (waiting)
                                  (%acp-respond waiting (%acp-terminal-exit terminal)))
                                (vector-ref terminal 4))
                      (vector-set! terminal 4 '())
                      (%acp-terminal-changed! name)))
         'directory (or (%acp-get params 'cwd) %acp-root (working-directory))
         'environment (map (lambda (variable)
                             (list (%acp-get variable 'name) (%acp-get variable 'value)))
                           (%acp-list (%acp-get params 'env)))))
      (%acp-respond id (json-object 'terminalId name)))))

(define (%acp-terminal-output! name terminal text)
  ;; More of what the program writes. Past the limit the oldest goes: a
  ;; char for each byte over, which is never too few.
  (vector-set! terminal 1 (string-append (vector-ref terminal 1) text))
  (vector-set! terminal 6 (+ (vector-ref terminal 6) (%acp-bytes text)))
  (let ((over (- (vector-ref terminal 6) (vector-ref terminal 5))))
    (when (> over 0)
      (let* ((output (vector-ref terminal 1))
             (cut (min over (string-length output))))
        (vector-set! terminal 6 (- (vector-ref terminal 6)
                                   (%acp-bytes (substring output 0 cut))))
        (vector-set! terminal 1 (substring output cut (string-length output)))
        (vector-set! terminal 2 #t))))
  (%acp-terminal-changed! name))

(define (%acp-end-terminals!)
  ;; When the agent is gone, so are the programs it had the editor run.
  (hash-table-walk %acp-terminals
    (lambda (name terminal)
      (let ((process (vector-ref terminal 0)))
        (when (and process (not (vector-ref terminal 3)))
          (delete-process process)))))
  (set! %acp-terminals (make-hash-table))
  (set! %acp-terminal-tools (make-hash-table)))

;;; The conversation, as it is known

;; A turn: its number, the prompt that started it (#f for what came before
;; the first prompt), and what came in answer, newest first. Each of those
;; is a vector of its kind and what it holds: the text of a 'message, a
;; 'thought or a 'note, the fields of a 'tool call, the entries of a 'plan,
;; the text of what the user 'said into the turn while it went on.
;; A turn that is over keeps the sections it was last drawn as, so that a
;; long conversation is not worked out anew for each piece of an answer.
(define-record-type acp-turn
  (%make-acp-turn number prompt items drawn)
  %acp-turn?
  (number %acp-turn-number)
  (prompt %acp-turn-prompt %acp-turn-set-prompt!)
  (items %acp-turn-items %acp-turn-set-items!)
  (drawn %acp-turn-drawn %acp-turn-set-drawn!))

;; The turns, newest first.
(define-acp-variable %acp-turns '())
;; Tool calls by their id, to find the one that an update is about.
(define-acp-variable %acp-tools (make-hash-table))

(define (%acp-turn)
  (when (null? %acp-turns)
    (set! %acp-turns (list (%make-acp-turn 0 #f '() #f))))
  (car %acp-turns))

(define (%acp-start-turn! prompt)
  (set! %acp-turns
    (cons (%make-acp-turn (+ 1 (%acp-turn-number (%acp-turn))) prompt '() #f)
          %acp-turns))
  (%acp-changed!))

(define (%acp-replay-prompt! text id)
  ;; A prompt of a conversation that was, as the agent tells of it again:
  ;; it heads a turn, and the pieces of one message join.
  (unless (equal? text "")
    (let ((turn (%acp-turn)))
      (if (and id (equal? id %acp-replayed-id)
               (%acp-turn-prompt turn)
               (null? (%acp-turn-items turn)))
          (begin
            (%acp-turn-set-prompt! turn (string-append (%acp-turn-prompt turn) text))
            (%acp-changed!))
          (%acp-start-turn! text)))
    (set! %acp-replayed-id id)))

(define (%acp-add! item)
  (let ((turn (%acp-turn)))
    (%acp-turn-set-items! turn (cons item (%acp-turn-items turn)))
    (%acp-changed!)))

(define (%acp-add-text! kind text)
  ;; Text comes in pieces: a piece joins what came just before it, if that
  ;; is of its kind.
  (unless (equal? text "")
    (let ((items (%acp-turn-items (%acp-turn))))
      (if (and (pair? items) (eq? (vector-ref (car items) 0) kind))
          (begin
            (vector-set! (car items) 1 (string-append (vector-ref (car items) 1) text))
            (%acp-changed!))
          (%acp-add! (vector kind text))))))

(define (%acp-note text)
  ;; Something the editor has to say in the conversation.
  (%acp-add! (vector 'note text)))

(define (%acp-tool-call! fields)
  ;; With the turn it is of, since an update may come when that is over.
  (let ((item (vector 'tool fields (%acp-turn))))
    (hash-table-set! %acp-tools (%acp-get fields 'toolCallId) item)
    (%acp-note-terminals! item)
    (%acp-add! item)))

(define (%acp-note-terminals! item)
  ;; Which terminals a tool call shows, to draw it again as they write.
  (for-each
   (lambda (part)
     (when (equal? (%acp-get part 'type) "terminal")
       (hash-table-set! %acp-terminal-tools (%acp-get part 'terminalId) item)))
   (%acp-list (%acp-get (vector-ref item 1) 'content))))

(define (%acp-tool-update! update)
  ;; An update carries only the fields that changed.
  (let ((item (hash-table-ref %acp-tools (%acp-get update 'toolCallId) #f)))
    (if (not item)
        (%acp-tool-call! update)
        (begin
          (vector-set! item 1
            (let loop ((rest update) (fields (vector-ref item 1)))
              (cond [(null? rest) fields]
                    [(eq? (cdar rest) 'null) (loop (cdr rest) fields)]
                    [else
                     (loop (cdr rest)
                           (cons (car rest)
                                 (filter (lambda (field) (not (eq? (car field) (caar rest))))
                                         fields)))])))
          (%acp-note-terminals! item)
          (%acp-turn-set-drawn! (vector-ref item 2) #f)
          (%acp-changed!)))))

(define (%acp-plan! entries)
  ;; A plan comes whole each time, and replaces the one of the turn.
  (let ((old (find (lambda (item) (eq? (vector-ref item 0) 'plan))
                   (%acp-turn-items (%acp-turn)))))
    (if old
        (begin (vector-set! old 1 entries) (%acp-changed!))
        (%acp-add! (vector 'plan entries)))))

;;; The change that a tool made to a file

;; The agent sends the text before and after, not a diff. Above this many
;; pairs of lines to compare, the change is shown as all of the old lines
;; and then all of the new.
(define %acp-diff-bound 40000)

(define (%acp-lines text)
  (if (string? text)
      (let ((lines (string-split text #\newline)))
        (if (and (pair? lines) (equal? (last lines) ""))
            (reverse (cdr (reverse lines)))
            lines))
      '()))

;; The steps from one vector of lines to another, between the lines that
;; they start and end with alike: pairs of 'same, 'removed or 'added and the
;; line, by the longest run of lines that both have.
(define (%acp-diff-middle a b head rows cols)
  (let* ((width (+ cols 1))
         (table (make-vector (* (+ rows 1) width) 0)))
    (define (at i j) (vector-ref table (+ (* i width) j)))
    (define (alike? i j) (equal? (vector-ref a (+ head i)) (vector-ref b (+ head j))))
    (let down ((i (- rows 1)))
      (when (>= i 0)
        (let left ((j (- cols 1)))
          (when (>= j 0)
            (vector-set! table (+ (* i width) j)
                         (if (alike? i j)
                             (+ 1 (at (+ i 1) (+ j 1)))
                             (max (at (+ i 1) j) (at i (+ j 1)))))
            (left (- j 1))))
        (down (- i 1))))
    (let loop ((i 0) (j 0) (steps '()))
      (cond
        [(and (< i rows) (< j cols) (alike? i j))
         (loop (+ i 1) (+ j 1) (cons (cons 'same (vector-ref a (+ head i))) steps))]
        [(and (< i rows) (or (= j cols) (>= (at (+ i 1) j) (at i (+ j 1)))))
         (loop (+ i 1) j (cons (cons 'removed (vector-ref a (+ head i))) steps))]
        [(< j cols)
         (loop i (+ j 1) (cons (cons 'added (vector-ref b (+ head j))) steps))]
        [else (reverse steps)]))))

(define (%acp-diff-steps old new)
  (let* ((a (list->vector old))
         (b (list->vector new))
         (n (vector-length a))
         (m (vector-length b))
         (head (let loop ((i 0))
                 (if (and (< i n) (< i m) (equal? (vector-ref a i) (vector-ref b i)))
                     (loop (+ i 1))
                     i)))
         (tail (let loop ((k 0))
                 (if (and (< (+ head k) n) (< (+ head k) m)
                          (equal? (vector-ref a (- n k 1)) (vector-ref b (- m k 1))))
                     (loop (+ k 1))
                     k)))
         (rows (- n head tail))
         (cols (- m head tail))
         (from (lambda (kind lines start count)
                 (map (lambda (i) (cons kind (vector-ref lines i))) (iota count start)))))
    (append (from 'same a 0 head)
            (if (> (* rows cols) %acp-diff-bound)
                (append (from 'removed a head rows) (from 'added b head cols))
                (%acp-diff-middle a b head rows cols))
            (from 'same a (- n tail) tail))))

(define (%acp-line-diff old new)
  "The change from one text to another as the lines of a diff: lines that
start with a space, - or +, under the @@ line of their hunk, with three
lines around each change. Each is paired with the number of the line of the
new text that it is at. Empty when the texts are the same; the old text may
be #f for a file that is new."
  (let* ((steps (list->vector (%acp-diff-steps (%acp-lines old) (%acp-lines new))))
         (count (vector-length steps))
         (near (make-vector count #f)))
    (define (kind index) (car (vector-ref steps index)))
    (let loop ((i 0))
      (when (< i count)
        (unless (eq? (kind i) 'same)
          (let mark ((k (max 0 (- i 3))))
            (when (and (< k count) (<= k (+ i 3)))
              (vector-set! near k #t)
              (mark (+ k 1)))))
        (loop (+ i 1))))
    (let loop ((i 0) (old-line 1) (new-line 1) (out '()))
      (cond
        [(>= i count) (reverse out)]
        [(not (vector-ref near i)) (loop (+ i 1) (+ old-line 1) (+ new-line 1) out)]
        [else
         (let* ((end (let find-end ((k i))
                       (if (and (< k count) (vector-ref near k)) (find-end (+ k 1)) k)))
                (kinds (map kind (iota (- end i) i)))
                (olds (length (filter (lambda (kind) (not (eq? kind 'added))) kinds)))
                (news (length (filter (lambda (kind) (not (eq? kind 'removed))) kinds)))
                (heading (string-append
                          "@@ -" (number->string (if (= olds 0) (- old-line 1) old-line))
                          "," (number->string olds)
                          " +" (number->string (if (= news 0) (- new-line 1) new-line))
                          "," (number->string news) " @@")))
           (let hunk ((k i) (old-line old-line) (new-line new-line)
                      (out (cons (cons heading new-line) out)))
             (if (= k end)
                 (loop end old-line new-line out)
                 (let ((step (vector-ref steps k)))
                   (hunk (+ k 1)
                         (if (eq? (car step) 'added) old-line (+ old-line 1))
                         (if (eq? (car step) 'removed) new-line (+ new-line 1))
                         (cons (cons (string-append
                                      (case (car step)
                                        [(added) "+"]
                                        [(removed) "-"]
                                        [else " "])
                                      (cdr step))
                                     new-line)
                               out))))))]))))

;;; The conversation as sections

(define (%acp-text-lines text)
  ;; The lines of a text, without the empty ones at its start and end.
  (define (trim lines)
    (if (and (pair? lines) (equal? (string-trim (car lines)) ""))
        (trim (cdr lines))
        lines))
  (reverse (trim (reverse (trim (string-split text #\newline))))))

(define (%acp-line-sections key lines face indent)
  (map (lambda (line n)
         (make-section (list key n) (list (cons (string-append indent line) face)) '() #f))
       lines
       (iota (length lines))))

(define (%acp-short text width)
  (if (> (string-length text) width)
      (string-append (substring text 0 (- width 1)) "…")
      text))

(define (%acp-thought-section text index)
  (let ((lines (%acp-text-lines text)))
    (make-section (list 'thought index)
                  (list (cons (string-append "Thinking: " (%acp-short (car lines) 60)) 'shadow))
                  (%acp-line-sections 'line lines 'shadow "  ")
                  #f
                  #t)))

(define (%acp-status-face status)
  (cond [(equal? status "completed") 'agent-done]
        [(equal? status "failed") 'agent-failed]
        [else 'agent-pending]))

(define (%acp-shown-path path)
  ;; A path as it is shown: from where the agent works, if it is under it.
  (if (and %acp-root (string? path))
      (%acp-relative %acp-root path)
      (or path "?")))

;; How many lines of what a tool wrote are shown.
(define %acp-output-lines 30)

;; The sections for one part of what a tool call holds: text, or a change
;; to a file.
(define (%acp-content-sections content index)
  (let ((type (%acp-get content 'type)))
    (cond
      [(equal? type "diff")
       (let* ((path (or (%acp-get content 'path) "?"))
              (diff (%acp-line-diff (%acp-get content 'oldText) (%acp-get content 'newText))))
         (cons
          (make-section (list index 'path)
                        (list (cons (string-append "  " (%acp-shown-path path)) 'git-file))
                        '() (list 'location path #f))
          (map (lambda (entry n)
                 (let ((line (car entry)))
                   (make-section (list index n)
                                 (list (cons "  " #f)
                                       (cons line
                                             (cond [(string-prefix? "@@" line) 'git-hunk]
                                                   [(string-prefix? "+" line) 'git-added]
                                                   [(string-prefix? "-" line) 'git-removed]
                                                   [else #f])))
                                 '()
                                 (list 'location path (cdr entry)))))
               diff
               (iota (length diff)))))]
      [(equal? type "content")
       (let* ((lines (%acp-text-lines (%acp-content-text (%acp-get content 'content))))
              (count (length lines))
              (shown (if (> count %acp-output-lines)
                         (append (list-head lines %acp-output-lines)
                                 (list (string-append "… " (number->string (- count %acp-output-lines))
                                                      " more lines")))
                         lines)))
         (%acp-line-sections index shown #f "  "))]
      [(equal? type "terminal")
       ;; The end of what the program wrote, which is where the news is.
       (let* ((terminal (hash-table-ref %acp-terminals (%acp-get content 'terminalId) #f))
              (lines (if terminal (%acp-text-lines (vector-ref terminal 1)) '()))
              (count (length lines))
              (shown (if (> count %acp-output-lines)
                         (cons (string-append "… " (number->string (- count %acp-output-lines))
                                              " lines before these")
                               (list-tail lines (- count %acp-output-lines)))
                         lines))
              (exit (and terminal (vector-ref terminal 3))))
         (%acp-line-sections
          index
          (append shown
                  (cond [(not exit) '()]
                        [(not (car exit)) (list "[ended by a signal]")]
                        [(= (car exit) 0) '()]
                        [else (list (string-append "[exit " (number->string (car exit)) "]"))]))
          #f "  "))]
      [else '()])))

(define (%acp-tool-section fields index)
  (let* ((status (or (%acp-get fields 'status) "pending"))
         (title (car (append (%acp-text-lines (or (%acp-get fields 'title) "")) '("Tool"))))
         (locations (filter-map
                     (lambda (location)
                       (and (string? (%acp-get location 'path))
                            (list 'location (%acp-get location 'path) (%acp-get location 'line))))
                     (%acp-list (%acp-get fields 'locations))))
         (content (%acp-list (%acp-get fields 'content)))
         (diff? (find (lambda (part) (equal? (%acp-get part 'type) "diff")) content))
         (request (%acp-request-for (%acp-get fields 'toolCallId))))
    (make-section
     (list 'tool index)
     (list (cons title 'agent-tool)
           (cons "  " #f)
           (if request
               (cons "asks leave" 'agent-asks)
               (cons status (%acp-status-face status))))
     (append
      ;; The answers to give, each with its key; RET on one gives it.
      (if request
          (map (lambda (option n)
                 (make-section (list 'leave n)
                               (list (cons (string-append "  " (car option) "  " (cadr option))
                                           'agent-asks))
                               '()
                               (list 'leave request (caddr option))))
               (vector-ref request 2)
               (iota (length (vector-ref request 2))))
          '())
      (apply append (map %acp-content-sections content (iota (length content))))
      ;; Where it worked, unless a change says so already.
      (if diff?
          '()
          (map (lambda (location n)
                 (make-section (list 'location n)
                               (list (cons (string-append
                                            "  " (%acp-shown-path (cadr location))
                                            (if (caddr location)
                                                (string-append ":" (number->string (caddr location)))
                                                ""))
                                           'shadow))
                               '() location))
               locations
               (iota (length locations)))))
     (and (pair? locations) (car locations))
     ;; A change, a failure and a question are worth seeing without asking.
     (not (or diff? request (equal? status "failed"))))))

(define (%acp-plan-section entries index)
  (let ((done (length (filter (lambda (entry) (equal? (%acp-get entry 'status) "completed"))
                              entries))))
    (make-section
     (list 'plan index)
     (list (cons "Plan" 'git-heading)
           (cons (string-append " (" (number->string done) "/"
                                (number->string (length entries)) ")")
                 #f))
     (map (lambda (entry n)
            (let ((status (%acp-get entry 'status)))
              (make-section n
                            (list (cons (string-append
                                         (cond [(equal? status "completed") "  [x] "]
                                               [(equal? status "in_progress") "  [~] "]
                                               [else "  [ ] "])
                                         (or (%acp-get entry 'content) ""))
                                        (and (equal? status "completed") 'shadow)))
                            '() #f)))
          entries
          (iota (length entries)))
     #f)))

(define (%acp-item-sections item index)
  (let ((kind (vector-ref item 0))
        (what (vector-ref item 1)))
    (case kind
      [(message) (%acp-line-sections index (%acp-text-lines what) #f "")]
      [(note) (%acp-line-sections index (%acp-text-lines what) 'shadow "")]
      [(said) (%acp-line-sections index (%acp-text-lines what) 'agent-prompt "› ")]
      [(thought) (if (null? (%acp-text-lines what)) '() (list (%acp-thought-section what index)))]
      [(tool) (list (%acp-tool-section what index))]
      [(plan) (list (%acp-plan-section what index))]
      [else '()])))

(define (%acp-turn-sections turn)
  (or (%acp-turn-drawn turn)
      (let ((sections (%acp-draw-turn turn)))
        ;; Only the newest turn gets more; the others change with a tool
        ;; call of theirs, which forgets what is kept here.
        (unless (eq? turn (car %acp-turns))
          (%acp-turn-set-drawn! turn sections))
        sections)))

(define (%acp-draw-turn turn)
  (let* ((items (reverse (%acp-turn-items turn)))
         (answer (apply append (map %acp-item-sections items (iota (length items)))))
         (prompt (%acp-turn-prompt turn))
         (number (%acp-turn-number turn)))
    (if (not prompt)
        answer
        (let ((lines (%acp-text-lines prompt)))
          (list (make-section (list 'gap number) "" '() #f)
                (make-section
                 (list 'turn number)
                 (list (cons (if (pair? lines) (car lines) "") 'agent-prompt))
                 (append (if (pair? lines)
                             (%acp-line-sections 'prompt (cdr lines) 'agent-prompt "")
                             '())
                         answer)
                 (list 'turn number)))))))

(define (%acp-waiting-sections)
  ;; The prompts that wait for the turn at work to end, a line for each.
  (if (null? %acp-queue)
      '()
      (cons (make-section (list 'gap 'waiting) "" '() #f)
            (map (lambda (entry n)
                   (let ((lines (%acp-text-lines (car entry))))
                     (make-section
                      (list 'waiting n)
                      (list (cons (string-append
                                   "Waits its turn: "
                                   (%acp-short (if (pair? lines) (car lines) "") 60))
                                  'shadow))
                      '() #f)))
                 %acp-queue
                 (iota (length %acp-queue))))))

(define (%acp-sections)
  (let ((sections (append (apply append (map %acp-turn-sections (reverse %acp-turns)))
                          (%acp-waiting-sections))))
    ;; No empty line above the first turn when nothing came before it.
    (if (and (pair? sections) (eq? (car (section-key (car sections))) 'gap))
        (cdr sections)
        sections)))

;;; The buffer

;; What the user has folded and opened in the conversation.
(define-acp-variable %acp-view (make-section-view))
;; Whether a redraw has been asked for and not yet done.
(define-acp-variable %acp-redraw-waits #f)

(define-major-mode agent-mode #f "Agent"
  "Major mode of the conversation with a coding agent. TAB folds and opens
what is at point, RET goes to the file that a tool call or a line of a
change is about, M-n and M-p move by turn. When the agent asks leave, y
allows it, o shows every answer, and RET on an answer gives it. m sets the
mode or the model, / sends a slash command, and b goes to another session.
Start with C-c c c.")

(define (%acp-ensure-buffer)
  ;; A conversation that no window showed is shown at its end, which the
  ;; window then follows.
  (let ((shown? (and (member %acp-buffer (buffer-list))
                     (pair? (buffer-windows %acp-buffer)))))
    (switch-to-buffer %acp-buffer)
    (unless (eq? (major-mode) 'agent-mode)
      (agent-mode)
      ;; What the agent writes is Markdown, and is coloured as such; the
      ;; faces of the sections lie over that.
      (set-buffer-language! "markdown")
      (set-buffer-read-only! #t)
      (%acp-redraw!))
    (unless shown? (goto-char (point-max))))
  %acp-buffer)

(define (%acp-changed!)
  ;; Many changes may come at once, as the pieces of a text do: the buffer
  ;; is drawn once, after them.
  (unless %acp-redraw-waits
    (set! %acp-redraw-waits #t)
    (run-later (%acp-bound %acp-redraw!))))

(define (%acp-at-end?)
  ;; Whether point is at the end, where the redraws leave it until the
  ;; reader moves it.
  (>= (point) (point-max)))

(define (%acp-redraw!)
  (set! %acp-redraw-waits #f)
  (when (member %acp-buffer (buffer-list))
    ;; The windows of every frame: an answer comes when it comes, and the
    ;; frame that is current then need not be the one that shows it.
    (let* ((windows (buffer-windows %acp-buffer))
           ;; A window whose point is at the end goes on showing the end;
           ;; one that is further up stays where its reader is.
           (places (map (lambda (window)
                          (with-selected-window window
                            (list window (%acp-at-end?) (line-number-at-pos))))
                        windows))
           (sections (%acp-sections)))
      (if (null? windows)
          (with-current-buffer %acp-buffer
            (section-view-render! %acp-view sections)
            (goto-char (point-max)))
          (begin
            ;; The first keeps the section it was on, as the view sees to.
            (with-selected-window (car windows)
              (section-view-render! %acp-view sections))
            (for-each
             (lambda (place)
               (with-selected-window (car place)
                 (cond [(cadr place) (goto-char (point-max))]
                       [(not (eqv? (car place) (car windows))) (goto-line (caddr place))])))
             places))))))

(define (%acp-read-options! options)
  ;; The model and the mode are among the options of the session that the
  ;; agent tells of, each with its category and its value now.
  (set! %acp-options (%acp-list options))
  (for-each
   (lambda (option)
     (let ((category (%acp-get option 'category))
           (value (%acp-get option 'currentValue)))
       (cond [(equal? category "model") (set! %acp-model value)]
             [(equal? category "mode") (set! %acp-mode value)])))
   %acp-options))

(define (%acp-tokens count)
  ;; A count of tokens in few letters: 28k, 1M.
  (cond [(>= count 1000000)
         (string-append (number->string (quotient (+ count 500000) 1000000)) "M")]
        [(>= count 1000)
         (string-append (number->string (quotient (+ count 500) 1000)) "k")]
        [else (number->string count)]))

(define (%acp-show-state)
  ;; The model, the mode, how full the context is and whether the agent is
  ;; at work, on the mode line.
  (when (member %acp-buffer (buffer-list))
    (let ((parts (filter (lambda (part) part)
                         (list %acp-model
                               %acp-mode
                               (and %acp-used %acp-size
                                    (string-append (%acp-tokens %acp-used) "/"
                                                   (%acp-tokens %acp-size)))
                               (and (pair? %acp-permissions) "asks leave")
                               (and %acp-busy "working")
                               (and (pair? %acp-queue)
                                    (string-append (number->string (length %acp-queue))
                                                   " waiting"))))))
      (with-current-buffer %acp-buffer
        (set-mode-line-text!
         (and (pair? parts) (string-append "[" (string-join parts ", ") "]")))))))

(define-command (agent-toggle-section)
  "Fold or open the section at point: a turn, a thought, a tool call, the
plan."
  (%acp-select!)
  (unless (section-view-toggle! %acp-view)
    (message "Nothing to fold here")))

(define-command (agent-visit)
  "Go to the file that the tool call or the line of a change at point is
about, at its line. On an answer to what the agent asks leave for, give
that answer. Elsewhere, fold or open the section."
  (%acp-select!)
  (let* ((values (map section-value (section-chain-at-point %acp-view)))
         (leave (and (pair? values) (pair? (car values)) (eq? (caar values) 'leave)
                     (car values)))
         (place (find (lambda (value) (and (pair? value) (eq? (car value) 'location)))
                      values)))
    (cond [leave (%acp-answer! (cadr leave) (caddr leave))]
          [place
           (find-file (expand-file-name (cadr place)
                                        (or %acp-root (project-root) (working-directory))))
           (when (caddr place) (goto-line (caddr place)))]
          [else (agent-toggle-section)])))

(define (%acp-turn-line from step)
  ;; The line of the nearest turn from a line, upwards or downwards, or #f.
  (let ((chains (section-view-chains %acp-view)))
    (let loop ((index (+ from step)))
      (and (>= index 0)
           (< index (vector-length chains))
           (let* ((chain (vector-ref chains index))
                  (value (and (pair? chain) (section-value (car chain)))))
             (if (and (pair? value) (eq? (car value) 'turn))
                 (+ index 1)
                 (loop (+ index step))))))))

(define-command (agent-next-turn)
  "Move to the prompt of the next turn."
  (%acp-select!)
  (let ((line (%acp-turn-line (- (line-number-at-pos) 1) 1)))
    (if line (goto-line line) (message "No turn after this"))))

(define-command (agent-previous-turn)
  "Move to the prompt of this turn, or of the one before."
  (%acp-select!)
  (let ((line (%acp-turn-line (- (line-number-at-pos) 1) -1)))
    (if line (goto-line line) (message "No turn before this"))))

(define-command (agent-redraw)
  "Draw the conversation again."
  (%acp-select!)
  (%acp-redraw!))

(for-each
 (lambda (binding) (define-key 'agent-mode (car binding) (cadr binding)))
 '(("TAB" agent-toggle-section) ("RET" agent-visit)
   ("n" next-line) ("p" previous-line)
   ("M-n" agent-next-turn) ("M-p" agent-previous-turn)
   ("g" agent-redraw)
   ("y" claude-allow) ("o" claude-answer) ("m" claude-set-option)
   ("/" claude-slash-command) ("b" claude-switch)))

;;; Commands

(define (%acp-connected?)
  (and %acp-process (process-live? %acp-process)))

(define (%acp-capability . path)
  ;; Something that the agent said it can do when the connection was made,
  ;; by the names that lead to it; #f if it did not say so.
  (let loop ((value (%acp-get %acp-agent 'agentCapabilities)) (path path))
    (cond [(null? path) (if (null? value) #t value)]
          [(pair? value) (loop (%acp-get value (car path)) (cdr path))]
          [else #f])))

(define (%acp-claim! root fresh?)
  ;; Makes the session to start an agent in the one at hand: one whose
  ;; agent has ended, the one of this buffer before any other of the
  ;; project, or else a new one. With `fresh?`, only one with no
  ;; conversation in it will do.
  (let* ((free? (lambda ()
                  (and (not (%acp-connected?))
                       (or (not %acp-root) (equal? %acp-root root))
                       (or (not fresh?) (null? %acp-turns)))))
         (shown (%acp-session-showing (buffer-name)))
         (session (or (and shown (%acp-in shown free?) shown)
                      (%acp-find-session free?))))
    (if session
        (%acp-enter! session)
        (%acp-new-session!))))

(define (%acp-start! root fresh? open)
  ;; Starts an agent in a directory and shows its conversation. `open` is
  ;; called, with the session at hand, when the connection is made.
  (%acp-claim! root fresh?)
  (%acp-ensure-buffer)
  (%acp-connect root open)
  (%acp-show-ui))

(define-command (claude)
  "Start Claude for this project over the Agent Client Protocol, showing
the conversation and a buffer to write prompts in; or show them if it is
already running. Write a prompt and send it with C-c C-c. With a prefix
argument, start another session beside those that run. In a conversation
whose agent has ended, the agent is started again and takes the
conversation up where it was."
  ;; The project is that of the buffer the command was given in, so it is
  ;; asked for before the conversation takes the window.
  (let* ((root (or (project-root) (working-directory)))
         (shown (%acp-session-showing (buffer-name)))
         (running? (lambda () (and (%acp-connected?) (equal? %acp-root root))))
         (running (and (= (prefix-arg) 1)
                       (or (and shown (%acp-in shown %acp-connected?) shown)
                           (%acp-find-session running?)))))
    (if running
        (begin (%acp-enter! running) (%acp-show-ui))
        (%acp-start! root (not (= (prefix-arg) 1))
          (lambda ()
            (if (and %acp-resumable (%acp-capability 'sessionCapabilities 'resume))
                (%acp-open-session "session/resume" %acp-resumable)
                (%acp-open-session "session/new" #f)))))))

;;; The window layout: the conversation above, a compose buffer below.

(define-option 'claude-compose-height 8
  "How many rows the compose buffer gets below the conversation.")

(define (%acp-show-ui)
  (%acp-ensure-buffer)
  (call-command 'delete-other-windows)
  (call-command 'split-window-below)
  (call-command 'other-window)
  (%acp-open-compose)
  (%acp-resize-to (option 'claude-compose-height)))

(define (%acp-resize-to rows)
  ;; The selected window to about `rows` tall, a row at a time.
  (let loop ((height (window-height)) (guard 200))
    (when (> guard 0)
      (cond [(> height rows)
             (call-command 'shrink-window)
             (let ((now (window-height)))
               (when (< now height) (loop now (- guard 1))))]
            [(< height rows)
             (call-command 'enlarge-window)
             (let ((now (window-height)))
               (when (> now height) (loop now (- guard 1))))]))))

(define (%acp-connect root open)
  ;; Starts the agent of the session at hand in a directory. `open` is
  ;; called, with the session at hand, when the agent has said what it is.
  (set! %acp-input "")
  (set! %acp-next-id 0)
  (set! %acp-pending (make-hash-table))
  (%acp-forget-session!)
  (set! %acp-root root)
  (with-current-buffer %acp-buffer (set-buffer-directory! root))
  (set! %acp-process
    (make-process (%acp-name "claude-acp" (vector-ref %acp-here 1))
                  (option 'acp-command)
                  'filter (%acp-bound %acp-filter)
                  'stderr %acp-stderr
                  'sentinel (%acp-bound %acp-sentinel)
                  'directory root))
  (%acp-note "Starting Claude…")
  (%acp-request "initialize"
    (json-object 'protocolVersion 1
                 'clientCapabilities (json-object
                                      'fs (json-object 'readTextFile #t 'writeTextFile #t)
                                      'terminal #t)
                 'clientInfo (json-object 'name "amacs" 'version "0.1.0"))
    (lambda (result error)
      (cond [error
             (%acp-note (string-append "[initialize failed: "
                                       (or (%acp-get error 'message) "?") "]"))]
            [else
             (set! %acp-agent result)
             (open)]))))

(define (%acp-mcp-servers)
  ;; The servers of the `acp-mcp-servers` option, as the protocol has them.
  (list->vector
   (map (lambda (server)
          (let ((name (car server))
                (what (cadr server)))
            (if (or (string-prefix? "http://" what) (string-prefix? "https://" what))
                (json-object 'type "http" 'name name 'url what 'headers (json-array))
                (json-object 'name name 'command what
                             'args (list->vector (cddr server))
                             'env (json-array)))))
        (option 'acp-mcp-servers))))

(define (%acp-open-session method id)
  ;; Opens the session of the agent: a new one with "session/new", or with
  ;; the id of one that was, "session/load" to be told of it again from its
  ;; start, "session/resume" to go on without that, "session/fork" for
  ;; another that starts as it is.
  (let ((directories (option 'acp-additional-directories)))
    (when (equal? method "session/load")
      (set! %acp-replaying #t)
      (set! %acp-replayed-id #f))
    (%acp-request method
      (append (if id (json-object 'sessionId id) '())
              (json-object 'cwd %acp-root 'mcpServers (%acp-mcp-servers))
              (if (and (pair? directories)
                       (%acp-capability 'sessionCapabilities 'additionalDirectories))
                  (json-object 'additionalDirectories (list->vector directories))
                  '()))
      (lambda (result error)
        (set! %acp-replaying #f)
        (cond
          [error
           (%acp-note (string-append "[" method " failed: "
                                     (or (%acp-get error 'message) "?") "]"))
           ;; A conversation that cannot be taken up again gives way to a
           ;; new one.
           (when (equal? method "session/resume")
             (set! %acp-resumable #f)
             (%acp-open-session "session/new" #f))]
          [else
           (set! %acp-session (or (%acp-get result 'sessionId) id))
           (set! %acp-modes (%acp-get result 'modes))
           (set! %acp-mode (%acp-get %acp-modes 'currentModeId))
           (%acp-read-options! (%acp-get result 'configOptions))
           (%acp-show-state)
           (%acp-note
            (cond [(equal? method "session/new")
                   "Ready. Write a prompt below and send it with C-c C-c."]
                  [(equal? method "session/fork")
                   "Ready, in a session of its own that starts as the other was."]
                  [else "Ready, with the conversation as it was."]))])))))

(define (%acp-stderr process text)
  ;; The adapter's own logs on standard error: kept apart from the protocol
  ;; on standard output, and dropped. Useful to log while debugging.
  #f)

(define (%acp-forget-session!)
  ;; What was known of a session that is over, or of the one before a new.
  (set! %acp-session #f)
  (set! %acp-agent #f)
  (set! %acp-busy #f)
  (set! %acp-queue '())
  (set! %acp-replaying #f)
  (set! %acp-model #f)
  (set! %acp-mode #f)
  (set! %acp-used #f)
  (set! %acp-size #f)
  (set! %acp-options '())
  (set! %acp-modes #f)
  (set! %acp-commands '())
  ;; Nobody is left to hear the answers.
  (set! %acp-permissions '())
  (%acp-end-terminals!))

(define (%acp-sentinel process status)
  ;; An agent that was ended to start another in its place has nothing to
  ;; say of the one that runs now.
  (when (or (not %acp-process) (equal? process %acp-process))
    (set! %acp-process #f)
    (when %acp-session (set! %acp-resumable %acp-session))
    (%acp-forget-session!)
    (for-each (lambda (turn) (%acp-turn-set-drawn! turn #f)) %acp-turns)
    (%acp-show-state)
    (%acp-note (string-append "[Claude ended"
                              (if status (string-append " (" (number->string status) ")") "")
                              "]"))))

(define-command (claude-send)
  "Ask the running Claude a one-line question from the minibuffer, a quick
alternative to the compose buffer. While Claude is at work, the question
waits its turn."
  (%acp-select!)
  (unless %acp-session (error "Claude is not running; C-c c c first"))
  (let ((session %acp-here)
        (text (read-string "Ask Claude: ")))
    (%acp-enter! session)
    (unless (equal? (string-trim text) "")
      (%acp-submit text))))

;;; The compose buffer

;; Prompts sent, oldest first, for M-p and M-n: those of every session.
(define %acp-history '())
(define-acp-variable %acp-history-index #f)

(define-minor-mode claude-compose-mode "Compose"
  "Writing a prompt for Claude. C-c C-c sends it, or has it wait its turn
while Claude is at work; C-c C-s sends it into the turn that is going on.
C-c C-k clears it, C-c C-r inserts a reference to the region in the other
window, C-c C-a one to an image, and M-p and M-n walk earlier prompts.")

(define (%acp-open-compose)
  (switch-to-buffer %acp-prompt-buffer)
  (unless (eq? (major-mode) 'markdown-mode) (markdown-mode))
  (enable-mode! 'claude-compose-mode)
  (set! %acp-history-index #f)
  (goto-char (point-max)))

(define-command (claude-compose)
  "Show the conversation and the compose buffer, with point ready to type."
  (%acp-select!)
  (unless (%acp-connected?) (error "Claude is not running; C-c c c first"))
  (%acp-show-ui))

(define-command (claude-compose-send)
  "Send the prompt in the compose buffer to Claude, and clear it for the
next one. While Claude is at work the prompt waits its turn, and is sent
when the turn is over. An image named in it as @path is sent with it."
  (%acp-select!)
  (let ((text (buffer-string)))
    (cond [(not %acp-session) (error "Claude is not running")]
          [(equal? (string-trim text) "") (error "The prompt is empty")]
          [else
           (%acp-submit text)
           (%acp-sent! text)])))

(define (%acp-sent! text)
  ;; A prompt written in the compose buffer has gone: it is kept for M-p,
  ;; and the buffer is cleared for the next.
  (set! %acp-history (append %acp-history (list text)))
  (set! %acp-history-index #f)
  (when (member %acp-prompt-buffer (buffer-list))
    (with-current-buffer %acp-prompt-buffer (%replace-buffer-text! ""))))

(define-command (claude-compose-steer)
  "Send the prompt in the compose buffer into the turn that Claude is at
work on, to change its course, where C-c C-c would have it wait for the
turn to end. When no turn is going on it is sent as any prompt."
  (%acp-select!)
  (let ((text (buffer-string)))
    (cond [(not %acp-session) (error "Claude is not running")]
          [(equal? (string-trim text) "") (error "The prompt is empty")]
          [(not %acp-busy) (%acp-submit text) (%acp-sent! text)]
          [(not (%acp-get (%acp-get (%acp-get %acp-agent '_meta) 'steering) 'supported))
           (error "This agent takes nothing into a turn; C-c C-c has the prompt wait")]
          [else
           (let ((session %acp-here)
                 (blocks (%acp-prompt-blocks text)))
             (%acp-enter! session)
             (%acp-request "_session/steering"
               (json-object 'sessionId %acp-session 'prompt blocks)
               (lambda (result error)
                 (when error
                   (%acp-note (string-append "[Claude did not take it: "
                                             (or (%acp-get error 'message) "?") "]")))))
             (%acp-add! (vector 'said text))
             (%acp-sent! text))])))

(define-command (claude-compose-clear)
  "Clear the compose buffer."
  (%acp-select!)
  (%replace-buffer-text! "")
  (set! %acp-history-index #f))

(define-command (claude-compose-previous)
  "Put the previous sent prompt in the compose buffer."
  (%acp-select!)
  (let ((n (length %acp-history)))
    (when (> n 0)
      (let ((i (if %acp-history-index (max 0 (- %acp-history-index 1)) (- n 1))))
        (set! %acp-history-index i)
        (%replace-buffer-text! (list-ref %acp-history i))
        (goto-char (point-max))))))

(define-command (claude-compose-next)
  "Put the next sent prompt in the compose buffer, or empty it past the end."
  (%acp-select!)
  (when %acp-history-index
    (let ((i (+ %acp-history-index 1)))
      (if (< i (length %acp-history))
          (begin (set! %acp-history-index i)
                 (%replace-buffer-text! (list-ref %acp-history i)))
          (begin (set! %acp-history-index #f)
                 (%replace-buffer-text! "")))
      (goto-char (point-max)))))

(define (%acp-strip-slash s)
  (let ((n (string-length s)))
    (if (and (> n 1) (equal? (substring s (- n 1) n) "/"))
        (substring s 0 (- n 1))
        s)))

(define (%acp-relative root path)
  (let ((root (%acp-strip-slash root)))
    (if (string-prefix? root path)
        (let ((rel (substring path (string-length root) (string-length path))))
          (if (string-prefix? "/" rel) (substring rel 1 (string-length rel)) rel))
        path)))

(define (%acp-reference)
  "A reference to the region, or the line at point, of the current buffer:
@relpath#Lstart-end, or #f when the buffer has no file."
  (let ((path (buffer-file-name)))
    (and path
         (let* ((root (or (project-root) (working-directory)))
                (rel (%acp-relative root path))
                (a (if (region-active?) (min (mark) (point)) (point)))
                (b (if (region-active?) (max (mark) (point)) (point)))
                (start (line-number-at-pos a))
                (end (line-number-at-pos b)))
           (if (= start end)
               (string-append "@" rel "#L" (number->string start))
               (string-append "@" rel "#L" (number->string start) "-" (number->string end)))))))

(define (%acp-insert-in-prompt! text)
  ;; Adds something to the end of the prompt that is being written, apart
  ;; from what is there.
  (%acp-ensure-compose)
  (with-current-buffer %acp-prompt-buffer
    (goto-char (point-max))
    (let ((end (point-max)))
      (when (and (> end 0) (not (equal? (buffer-substring (- end 1) end) "\n"))
                 (not (equal? (buffer-substring (- end 1) end) " ")))
        (insert " ")))
    (insert text " ")))

(define (%acp-ensure-compose)
  (unless (member %acp-prompt-buffer (buffer-list))
    (save-excursion
      (switch-to-buffer %acp-prompt-buffer)
      (markdown-mode)
      (enable-mode! 'claude-compose-mode))))

(define-command (claude-add-reference)
  "From a file buffer, add a reference to the region, or the line at point,
to the compose buffer: @file#L3-9. Select the code, then this, then keep
writing the prompt."
  (let ((ref (%acp-reference)))
    (unless ref (error "This buffer has no file to point at"))
    (%acp-select!)
    (%acp-insert-in-prompt! ref)
    (message "Added " ref " to the prompt")))

;;; Images
;;;
;;; An image is named in a prompt as a file is, @path, and is sent with the
;;; prompt for the model to see, if the agent takes images.

(define %acp-image-types
  '((".png" . "image/png") (".jpg" . "image/jpeg") (".jpeg" . "image/jpeg")
    (".gif" . "image/gif") (".webp" . "image/webp")))

(define (%acp-image-type path)
  ;; The media type of an image by the end of its name, or #f.
  (let ((name (string-downcase path)))
    (let loop ((types %acp-image-types))
      (cond [(null? types) #f]
            [(string-suffix? (caar types) name) (cdar types)]
            [else (loop (cdr types))]))))

(define (%acp-words text)
  ;; The words of a text: what its spaces and line breaks are between.
  (let loop ((chars (string->list text)) (word '()) (words '()))
    (define (ended)
      (if (null? word) words (cons (list->string (reverse word)) words)))
    (cond [(null? chars) (reverse (ended))]
          [(char-whitespace? (car chars)) (loop (cdr chars) '() (ended))]
          [else (loop (cdr chars) (cons (car chars) word) words)])))

(define (%acp-images text)
  ;; The images that a prompt names as @path and that are there: pairs of
  ;; the path in full and the media type.
  (filter-map
   (lambda (word)
     (and (string-prefix? "@" word)
          (let* ((path (expand-file-name (substring word 1 (string-length word))
                                         (or %acp-root (working-directory))))
                 (type (%acp-image-type path)))
            (and type (file-exists? path) (cons path type)))))
   (%acp-words text)))

(define (%acp-prompt-blocks text)
  ;; What is sent for a prompt: its text, and after it the images that it
  ;; names, if the agent takes images. This waits for a program, so it is
  ;; for commands.
  (let ((session %acp-here)
        (images (if (%acp-capability 'promptCapabilities 'image) (%acp-images text) '())))
    (list->vector
     (cons (json-object 'type "text" 'text text)
           (map (lambda (image)
                  (let ((result (run-process "base64" (list "-w0" (car image)))))
                    (%acp-enter! session)
                    (unless (eqv? (car result) 0)
                      (error (string-append "Could not read " (car image))))
                    (json-object 'type "image"
                                 'mimeType (cdr image)
                                 'data (string-trim (cadr result)))))
                images)))))

(define-command (claude-attach-image)
  "Add an image to the prompt that is being written: choose the file, and
a reference to it, @path, goes into the compose buffer. The image is sent
with the prompt."
  (%acp-select!)
  (let* ((session %acp-here)
         (path (read-file-name "Image: ")))
    (%acp-enter! session)
    (unless (%acp-image-type path)
      (error "Not an image that can be sent: png, jpg, gif and webp are"))
    (unless (file-exists? path) (error (string-append "No such file: " path)))
    (let ((ref (string-append "@" (%acp-relative (or %acp-root (working-directory)) path))))
      (%acp-insert-in-prompt! ref)
      (message "Added " ref " to the prompt"))))

(define-command (claude-compose-insert-reference)
  "In the compose buffer, insert a reference to the region in another
window that shows a file."
  (let* ((windows (window-list))
         (other (and (> (length windows) 1) (cadr windows)))
         (ref (and other (with-selected-window other (%acp-reference)))))
    (if ref (insert ref " ") (message "No other window with a file to point at"))))

(define-key 'claude-compose-mode "C-c C-c" 'claude-compose-send)
(define-key 'claude-compose-mode "C-c C-k" 'claude-compose-clear)
(define-key 'claude-compose-mode "C-c C-r" 'claude-compose-insert-reference)
(define-key 'claude-compose-mode "C-c C-s" 'claude-compose-steer)
(define-key 'claude-compose-mode "C-c C-a" 'claude-attach-image)
(define-key 'claude-compose-mode "M-p" 'claude-compose-previous)
(define-key 'claude-compose-mode "M-n" 'claude-compose-next)

(define (%acp-submit text)
  ;; Sends a prompt, or has it wait until the turn at work is over. For
  ;; commands: an image that the prompt names is read first.
  (let ((entry (if (null? (%acp-images text))
                   (list text)
                   (list text (%acp-prompt-blocks text)))))
    (set! %acp-queue (append %acp-queue (list entry)))
    (cond [%acp-busy
           (%acp-changed!)
           (%acp-show-state)
           (message "Claude is at work; the prompt waits its turn")]
          [else (%acp-send-waiting!)])))

(define (%acp-send-waiting!)
  ;; Sends the prompt that has waited the longest, if one waits.
  (when (and %acp-session (pair? %acp-queue))
    (let ((entry (car %acp-queue)))
      (set! %acp-queue (cdr %acp-queue))
      (apply %acp-send-prompt entry))))

(define (%acp-send-prompt text . blocks)
  ;; Starts a turn with a prompt. What is sent for it is its text, unless
  ;; it is given: the text with the images that it names.
  (%acp-start-turn! text)
  (set! %acp-busy #t)
  (%acp-show-state)
  (%acp-request "session/prompt"
    (json-object 'sessionId %acp-session
                 'prompt (if (pair? blocks)
                             (car blocks)
                             (json-array (json-object 'type "text" 'text text))))
    (lambda (result error)
      (set! %acp-busy #f)
      (%acp-show-state)
      (%acp-note (if error
                     (string-append "[error: " (or (%acp-get error 'message) "?") "]")
                     (string-append "— " (or (%acp-get result 'stopReason) "done") " —")))
      ;; A turn that failed leaves what waits for the user to send again.
      (if error (%acp-changed!) (%acp-send-waiting!)))))

;;; The mode, the model and the other options of the session

(define (%acp-option-choices option)
  ;; The values an option can have, as lists of the value, its name and its
  ;; description or #f. They may come in groups, which are taken apart.
  (define (choice entry)
    (list (%acp-get entry 'value)
          (or (%acp-get entry 'name) (%acp-get entry 'value))
          (%acp-get entry 'description)))
  (apply append
         (map (lambda (entry)
                (if (%acp-get entry 'group)
                    (map choice (%acp-list (%acp-get entry 'options)))
                    (list (choice entry))))
              (%acp-list (%acp-get option 'options)))))

(define (%acp-settable-options)
  ;; The options with values to choose among. An agent that tells of modes
  ;; but has no option for the mode gets one made of them.
  (let ((options (filter (lambda (option) (pair? (%acp-option-choices option)))
                         %acp-options))
        (modes (%acp-list (%acp-get %acp-modes 'availableModes))))
    (if (or (null? modes)
            (find (lambda (option) (equal? (%acp-get option 'category) "mode")) options))
        options
        (cons (json-object 'name "Mode" 'category "mode" 'modes #t
                           'currentValue %acp-mode
                           'options (list->vector
                                     (map (lambda (mode)
                                            (json-object 'value (%acp-get mode 'id)
                                                         'name (%acp-get mode 'name)
                                                         'description (%acp-get mode 'description)))
                                          modes)))
              options))))

(define (%acp-option-name option)
  (or (%acp-get option 'name) (%acp-get option 'id) "?"))

(define (%acp-candidate name note)
  ;; Something to choose, with what is said beside it if anything is.
  (if (and (string? note) (not (equal? note ""))) (list name note) name))

(define-command (claude-set-option)
  "Set the mode, the model or another option of the session with Claude:
choose which, among those the agent tells of, and then its value. The mode
line of the conversation shows the mode and the model in force."
  (%acp-select!)
  (unless %acp-session (error "Claude is not running; C-c c c first"))
  (let ((options (%acp-settable-options))
        (session %acp-here))
    (when (null? options) (error "The agent has told of nothing to set"))
    (let* ((name (if (null? (cdr options))
                     (%acp-option-name (car options))
                     (completing-read
                      "Set: "
                      (map (lambda (option)
                             (let ((now (find (lambda (choice)
                                                (equal? (car choice) (%acp-get option 'currentValue)))
                                              (%acp-option-choices option))))
                               (%acp-candidate (%acp-option-name option) (and now (cadr now)))))
                           options)
                      'agent-option)))
           (option (find (lambda (option) (equal? (%acp-option-name option) name)) options)))
      (unless option (error (string-append "Nothing to set is called " name)))
      (let* ((choices (%acp-option-choices option))
             (picked (completing-read
                      (string-append name ": ")
                      (map (lambda (choice) (%acp-candidate (cadr choice) (caddr choice)))
                           choices)
                      'agent-value))
             (choice (find (lambda (choice) (equal? (cadr choice) picked)) choices)))
        (unless choice (error (string-append name " cannot be " picked)))
        (%acp-enter! session)
        (%acp-set-option! option choice)))))

(define (%acp-set-option! option choice)
  (let ((name (%acp-option-name option))
        (said (lambda (error)
                (message "Claude would not set it: " (or (%acp-get error 'message) "?")))))
    (if (%acp-get option 'modes)
        (%acp-request "session/set_mode"
          (json-object 'sessionId %acp-session 'modeId (car choice))
          (lambda (result error)
            (if error
                (said error)
                (begin
                  (set! %acp-mode (car choice))
                  (%acp-show-state)
                  (message name ": " (cadr choice))))))
        (%acp-request "session/set_config_option"
          (json-object 'sessionId %acp-session
                       'configId (%acp-get option 'id)
                       'value (car choice))
          (lambda (result error)
            (if error
                (said error)
                (begin
                  ;; The answer has every option, since one may change others.
                  (when (%acp-get result 'configOptions)
                    (%acp-read-options! (%acp-get result 'configOptions)))
                  (%acp-show-state)
                  (message name ": " (cadr choice)))))))))

;;; Slash commands

(define-command (claude-slash-command)
  "Send one of the slash commands that Claude knows: choose it by name, and
write what it takes after its name if it takes something. While Claude is
at work, the command waits its turn."
  (%acp-select!)
  (unless %acp-session (error "Claude is not running; C-c c c first"))
  (when (null? %acp-commands) (error "The agent has told of no commands"))
  (let* ((session %acp-here)
         (picked (completing-read
                  "Command: "
                  (map (lambda (command)
                         (%acp-candidate (string-append "/" (%acp-get command 'name))
                                         (%acp-get command 'description)))
                       %acp-commands)
                  'agent-command))
         (command (find (lambda (command)
                          (equal? (string-append "/" (%acp-get command 'name)) picked))
                        %acp-commands)))
    (unless command (error (string-append "Claude has no command " picked)))
    (let* ((hint (%acp-get (%acp-get command 'input) 'hint))
           (more (if hint
                     (string-trim (read-string (string-append picked " (" hint "): ")))
                     ""))
           (text (if (equal? more "") picked (string-append picked " " more))))
      (%acp-enter! session)
      (unless %acp-session (error "Claude is not running; C-c c c first"))
      (%acp-submit text)
      (set! %acp-history (append %acp-history (list text))))))

(define-command (claude-cancel)
  "Cancel the current Claude turn. What Claude was asking leave for goes
unanswered, and the prompts that waited for the turn to end are not sent:
M-p in the compose buffer brings them back."
  (%acp-select!)
  (when (and %acp-session %acp-busy)
    (let ((waiting (length %acp-queue)))
      (set! %acp-queue '())
      (%acp-notify "session/cancel" (json-object 'sessionId %acp-session))
      ;; The protocol wants every question that waits answered so.
      (for-each (lambda (request) (%acp-answer! request #f)) %acp-permissions)
      (%acp-changed!)
      (%acp-show-state)
      (message "Asked Claude to stop."
               (if (> waiting 0)
                   (string-append " " (number->string waiting)
                                  (if (= waiting 1) " prompt" " prompts")
                                  " that waited will not be sent.")
                   "")))))

(define-command (claude-quit)
  "Stop the Claude of this session. C-c c c in its conversation starts it
again, and it takes the conversation up where it was."
  (%acp-select!)
  (when (%acp-connected?)
    (delete-process %acp-process))
  (set! %acp-process #f)
  (when %acp-session (set! %acp-resumable %acp-session))
  (%acp-forget-session!)
  (for-each (lambda (turn) (%acp-turn-set-drawn! turn #f)) %acp-turns)
  (%acp-changed!)
  (%acp-show-state)
  (message "Claude stopped."))

;;; Several sessions, and those that were

(define (%acp-ask method params)
  ;; For a command: sends a request and waits for the answer, which is
  ;; returned. An answer that is an error is an error here.
  (let ((session %acp-here)
        (answer #f))
    (%acp-request method params
                  (lambda (result error) (set! answer (list result error))))
    (let loop ((left 600))
      (cond [answer
             (when (cadr answer)
               (error (string-append "Claude: "
                                     (or (%acp-get (cadr answer) 'message) "it failed"))))
             (car answer)]
            [(not (%acp-connected?)) (error "Claude has ended")]
            [(= left 0) (error "Claude does not answer")]
            [else
             (sleep-for 50)
             (%acp-enter! session)
             (loop (- left 1))]))))

(define (%acp-await-agent)
  ;; For a command: waits until the agent that was started has said what
  ;; it is.
  (let ((session %acp-here))
    (let loop ((left 600))
      (cond [%acp-agent #t]
            [(not (%acp-connected?)) (error "Claude has ended")]
            [(= left 0) (error "Claude does not answer")]
            [else
             (sleep-for 50)
             (%acp-enter! session)
             (loop (- left 1))]))))

(define (%acp-state)
  ;; What the session at hand is doing, in a word or two.
  (cond [(not (%acp-connected?)) "ended"]
        [(pair? %acp-permissions) "asks leave"]
        [%acp-busy "working"]
        [else "ready"]))

(define-command (claude-switch)
  "Go to another of the sessions with Claude: choose it by the name of its
conversation, beside which is where it works, what it is doing and the
title of the conversation."
  (let* ((sessions (filter (lambda (session) (%acp-in session (lambda () %acp-root)))
                           %acp-sessions))
         (name (lambda (session) (%acp-name "*claude*" (vector-ref session 1)))))
    (when (null? sessions) (error "No session with Claude; C-c c c starts one"))
    (let* ((picked (completing-read
                    "Session: "
                    (map (lambda (session)
                           (list (name session)
                                 (%acp-in session
                                   (lambda ()
                                     (string-join
                                      (filter (lambda (part) part)
                                              (list %acp-root (%acp-state) %acp-title))
                                      "  ")))))
                         sessions)
                    'agent-session))
           (session (find (lambda (session) (equal? (name session) picked)) sessions)))
      (unless session (error (string-append "No session is called " picked)))
      (%acp-enter! session)
      (%acp-show-ui))))

(define (%acp-session-candidates sessions)
  ;; What to choose among for the conversations that were: lists of a name
  ;; that no other has, when it was last written to, and its id.
  (let loop ((rest sessions) (seen '()) (out '()))
    (if (null? rest)
        (reverse out)
        (let* ((session (car rest))
               (title (or (%acp-get session 'title) "(no title)"))
               (count (length (filter (lambda (other) (equal? other title)) seen)))
               (name (if (= count 0)
                         title
                         (string-append title " (" (number->string (+ count 1)) ")")))
               (when (or (%acp-get session 'updatedAt) "")))
          (loop (cdr rest)
                (cons title seen)
                (cons (list name
                            (if (>= (string-length when) 16)
                                (string-append (substring when 0 10) " " (substring when 11 16))
                                when)
                            (%acp-get session 'sessionId))
                      out))))))

(define-command (claude-resume)
  "Take up a conversation with Claude that was had in this project before:
choose it by its title, the latest first, and it is shown again from its
start in a session of its own, ready to go on. If none is chosen, the
session has a new conversation."
  (let ((root (or (project-root) (working-directory))))
    (%acp-start! root #t (lambda () #t))
    (let ((session %acp-here)
          (opened #f))
      (dynamic-wind
        (lambda () #f)
        (lambda ()
          (%acp-await-agent)
          (unless (and (%acp-capability 'loadSession)
                       (%acp-capability 'sessionCapabilities 'list))
            (error "This agent cannot take up a conversation that was; this is a new one"))
          (let* ((sessions (%acp-list
                            (%acp-get (%acp-ask "session/list" (json-object 'cwd root))
                                      'sessions)))
                 (candidates (%acp-session-candidates sessions)))
            (when (null? candidates)
              (error "No conversation was had here before; this is a new one"))
            (let* ((picked (completing-read "Take up: "
                                            (map (lambda (candidate) (list-head candidate 2))
                                                 candidates)
                                            'agent-conversation))
                   (candidate (find (lambda (candidate) (equal? (car candidate) picked))
                                    candidates)))
              (unless candidate
                (error (string-append "No conversation is called " picked
                                      "; this is a new one")))
              (%acp-enter! session)
              (set! %acp-title (car candidate))
              (set! opened #t)
              (%acp-open-session "session/load" (caddr candidate)))))
        ;; Whatever came between, the agent that was started is not left
        ;; without a conversation.
        (lambda ()
          (unless opened
            (%acp-in session
              (lambda ()
                (when (and (%acp-connected?) %acp-agent (not %acp-session))
                  (%acp-open-session "session/new" #f))))))))))

(define-command (claude-fork)
  "Start another session that takes the conversation up as it is now, and
leaves this one as it is: a way to try something else from here."
  (%acp-select!)
  (unless %acp-session (error "Claude is not running; C-c c c first"))
  (when %acp-busy (error "Claude is still working; C-c c k to cancel"))
  (unless (%acp-capability 'sessionCapabilities 'fork)
    (error "This agent cannot start a session from another"))
  (let ((id %acp-session)
        (root %acp-root)
        (title %acp-title)
        (turns (map (lambda (turn)
                      (%make-acp-turn (%acp-turn-number turn) (%acp-turn-prompt turn)
                                      (%acp-turn-items turn) #f))
                    %acp-turns)))
    (%acp-new-session!)
    (set! %acp-turns turns)
    (set! %acp-title title)
    (%acp-ensure-buffer)
    (%acp-connect root (lambda () (%acp-open-session "session/fork" id)))
    (%acp-show-ui)))

(global-set-key "C-c c c" 'claude)
(global-set-key "C-c c p" 'claude-compose)
(global-set-key "C-c c l" 'claude-send)
(global-set-key "C-c c r" 'claude-add-reference)
(global-set-key "C-c c k" 'claude-cancel)
(global-set-key "C-c c y" 'claude-allow)
(global-set-key "C-c c n" 'claude-refuse)
(global-set-key "C-c c o" 'claude-answer)
(global-set-key "C-c c m" 'claude-set-option)
(global-set-key "C-c c /" 'claude-slash-command)
(global-set-key "C-c c b" 'claude-switch)
(global-set-key "C-c c R" 'claude-resume)
(global-set-key "C-c c f" 'claude-fork)
(global-set-key "C-c c i" 'claude-attach-image)
