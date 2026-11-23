;;; Talking to a coding agent over the Agent Client Protocol.
;;;
;;; The Agent Client Protocol (agentclientprotocol.com) is JSON-RPC over the
;;; standard input and output of an agent, the way Zed and others talk to
;;; coding agents. Claude Code speaks it through the `claude-agent-acp`
;;; adapter. This client starts the agent, opens a session, sends prompts
;;; written in a compose buffer, answers the agent when it reads or writes a
;;; file, and asks the user when the agent wants to do something that needs
;;; leave.
;;;
;;; The conversation is a buffer of sections, as the git buffers are: a
;;; section for each turn, with the agent's thoughts folded away, its tool
;;; calls with their state and the changes they made, and its plan. What the
;;; agent sends changes a model of the conversation, and the buffer is drawn
;;; from the model, so that a tool call changes where it stands.
;;;
;;; It is deliberately one connection and one buffer for now; several
;;; sessions and the backend seam for other agents come next.

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
;; Where the agent works, which its relative paths are taken from.
(define %acp-root #f)
;; The model and the mode of the session, as the agent names them.
(define %acp-model #f)
(define %acp-mode #f)
;; How much of the model's context the conversation fills, and how much
;; there is, in tokens; #f until the agent has said.
(define %acp-used #f)
(define %acp-size #f)

(set-face! 'agent-prompt 'fg 'blue 'bold #t)
(set-face! 'agent-tool 'bold #t)
(set-face! 'agent-pending 'fg 'fg-dim)
(set-face! 'agent-done 'fg 'fg-added)
(set-face! 'agent-failed 'fg 'fg-removed 'bold #t)

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
      [(equal? kind "available_commands_update") #f]
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

;;; The conversation, as it is known

;; A turn: its number, the prompt that started it (#f for what came before
;; the first prompt), and what came in answer, newest first. Each of those
;; is a vector of its kind and what it holds: the text of a 'message, a
;; 'thought or a 'note, the fields of a 'tool call, the entries of a 'plan.
;; A turn that is over keeps the sections it was last drawn as, so that a
;; long conversation is not worked out anew for each piece of an answer.
(define-record-type acp-turn
  (%make-acp-turn number prompt items drawn)
  %acp-turn?
  (number %acp-turn-number)
  (prompt %acp-turn-prompt)
  (items %acp-turn-items %acp-turn-set-items!)
  (drawn %acp-turn-drawn %acp-turn-set-drawn!))

;; The turns, newest first.
(define %acp-turns '())
;; Tool calls by their id, to find the one that an update is about.
(define %acp-tools (make-hash-table))

(define (%acp-turn)
  (when (null? %acp-turns)
    (set! %acp-turns (list (%make-acp-turn 0 #f '() #f))))
  (car %acp-turns))

(define (%acp-start-turn! prompt)
  (set! %acp-turns
    (cons (%make-acp-turn (+ 1 (%acp-turn-number (%acp-turn))) prompt '() #f)
          %acp-turns))
  (%acp-changed!))

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
    (%acp-add! item)))

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
         (diff? (find (lambda (part) (equal? (%acp-get part 'type) "diff")) content)))
    (make-section
     (list 'tool index)
     (list (cons title 'agent-tool) (cons "  " #f) (cons status (%acp-status-face status)))
     (append
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
     ;; A change and a failure are worth seeing without asking.
     (not (or diff? (equal? status "failed"))))))

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

(define (%acp-sections)
  (let ((sections (apply append (map %acp-turn-sections (reverse %acp-turns)))))
    ;; No empty line above the first turn when nothing came before it.
    (if (and (pair? sections) (eq? (car (section-key (car sections))) 'gap))
        (cdr sections)
        sections)))

;;; The buffer

;; What the user has folded and opened in the conversation.
(define %acp-view (make-section-view))
;; Whether a redraw has been asked for and not yet done.
(define %acp-redraw-waits #f)

(define-major-mode agent-mode #f "Agent"
  "Major mode of the conversation with a coding agent. TAB folds and opens
what is at point, RET goes to the file that a tool call or a line of a
change is about, M-n and M-p move by turn. Start with C-c c c.")

(define (%acp-ensure-buffer)
  (switch-to-buffer %acp-buffer)
  (unless (eq? (major-mode) 'agent-mode)
    (agent-mode)
    ;; What the agent writes is Markdown, and is coloured as such; the
    ;; faces of the sections lie over that.
    (set-buffer-language! "markdown")
    (set-buffer-read-only! #t)
    (%acp-redraw!))
  %acp-buffer)

(define (%acp-changed!)
  ;; Many changes may come at once, as the pieces of a text do: the buffer
  ;; is drawn once, after them.
  (unless %acp-redraw-waits
    (set! %acp-redraw-waits #t)
    (run-later %acp-redraw!)))

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
  (for-each
   (lambda (option)
     (let ((category (%acp-get option 'category))
           (value (%acp-get option 'currentValue)))
       (cond [(equal? category "model") (set! %acp-model value)]
             [(equal? category "mode") (set! %acp-mode value)])))
   (%acp-list options)))

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
                               (and %acp-busy "working")))))
      (with-current-buffer %acp-buffer
        (set-mode-line-text!
         (and (pair? parts) (string-append "[" (string-join parts ", ") "]")))))))

(define-command (agent-toggle-section)
  "Fold or open the section at point: a turn, a thought, a tool call, the
plan."
  (unless (section-view-toggle! %acp-view)
    (message "Nothing to fold here")))

(define-command (agent-visit)
  "Go to the file that the tool call or the line of a change at point is
about, at its line. Elsewhere, fold or open the section."
  (let ((place (find (lambda (value) (and (pair? value) (eq? (car value) 'location)))
                     (map section-value (section-chain-at-point %acp-view)))))
    (if (not place)
        (agent-toggle-section)
        (begin
          (find-file (expand-file-name (cadr place)
                                       (or %acp-root (project-root) (working-directory))))
          (when (caddr place) (goto-line (caddr place)))))))

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
  (let ((line (%acp-turn-line (- (line-number-at-pos) 1) 1)))
    (if line (goto-line line) (message "No turn after this"))))

(define-command (agent-previous-turn)
  "Move to the prompt of this turn, or of the one before."
  (let ((line (%acp-turn-line (- (line-number-at-pos) 1) -1)))
    (if line (goto-line line) (message "No turn before this"))))

(define-command (agent-redraw)
  "Draw the conversation again."
  (%acp-redraw!))

(for-each
 (lambda (binding) (define-key 'agent-mode (car binding) (cadr binding)))
 '(("TAB" agent-toggle-section) ("RET" agent-visit)
   ("n" next-line) ("p" previous-line)
   ("M-n" agent-next-turn) ("M-p" agent-previous-turn)
   ("g" agent-redraw)))

;;; Commands

(define (%acp-connected?)
  (and %acp-process (process-live? %acp-process)))

(define-command (claude)
  "Start Claude for this project over the Agent Client Protocol, showing
the conversation and a buffer to write prompts in; or show them if it is
already running. Write a prompt and send it with C-c C-c."
  (if (%acp-connected?)
      (%acp-show-ui)
      ;; The project is that of the buffer the command was given in, so it
      ;; is asked for before the conversation takes the window.
      (let ((root (or (project-root) (working-directory))))
        (%acp-ensure-buffer)
        (%acp-connect root)
        (%acp-show-ui))))

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

(define (%acp-connect root)
  (set! %acp-input "")
  (set! %acp-session #f)
  (set! %acp-next-id 0)
  (set! %acp-pending (make-hash-table))
  (set! %acp-root root)
  (with-current-buffer %acp-buffer (set-buffer-directory! root))
  (set! %acp-process
    (make-process "claude-acp" (option 'acp-command)
                  'filter %acp-filter
                  'stderr %acp-stderr
                  'sentinel %acp-sentinel
                  'directory root))
  (%acp-note "Starting Claude…")
  (%acp-request "initialize"
    (json-object 'protocolVersion 1
                 'clientCapabilities (json-object
                                      'fs (json-object 'readTextFile #t 'writeTextFile #t))
                 'clientInfo (json-object 'name "amacs" 'version "0.1.0"))
    (lambda (result error)
      (if error
          (%acp-note (string-append "[initialize failed: " (or (%acp-get error 'message) "?") "]"))
          (%acp-new-session root)))))

(define (%acp-new-session root)
  (%acp-request "session/new"
    (json-object 'cwd root 'mcpServers (json-array))
    (lambda (result error)
      (if error
          (%acp-note (string-append "[session/new failed: " (or (%acp-get error 'message) "?") "]"))
          (begin
            (set! %acp-session (%acp-get result 'sessionId))
            (set! %acp-mode (%acp-get (%acp-get result 'modes) 'currentModeId))
            (%acp-read-options! (%acp-get result 'configOptions))
            (%acp-show-state)
            (%acp-note "Ready. Write a prompt below and send it with C-c C-c."))))))

(define (%acp-stderr process text)
  ;; The adapter's own logs on standard error: kept apart from the protocol
  ;; on standard output, and dropped. Useful to log while debugging.
  #f)

(define (%acp-sentinel process status)
  (set! %acp-process #f)
  (set! %acp-session #f)
  (set! %acp-busy #f)
  (set! %acp-model #f)
  (set! %acp-mode #f)
  (set! %acp-used #f)
  (set! %acp-size #f)
  (%acp-show-state)
  (%acp-note (string-append "[Claude ended" (if status (string-append " (" (number->string status) ")") "") "]")))

(define-command (claude-send)
  "Ask the running Claude a one-line question from the minibuffer, a quick
alternative to the compose buffer."
  (unless %acp-session (error "Claude is not running; C-c c c first"))
  (when %acp-busy (error "Claude is still working; C-c c k to cancel"))
  (let ((text (read-string "Ask Claude: ")))
    (unless (equal? (string-trim text) "")
      (%acp-send-prompt text))))

;;; The compose buffer

(define %acp-prompt-buffer "*claude prompt*")
;; Prompts sent, oldest first, for M-p and M-n.
(define %acp-history '())
(define %acp-history-index #f)

(define-minor-mode claude-compose-mode "Compose"
  "Writing a prompt for Claude. C-c C-c sends it, C-c C-k clears it, C-c C-r
inserts a reference to the region in the other window, and M-p and M-n walk
earlier prompts.")

(define (%acp-open-compose)
  (switch-to-buffer %acp-prompt-buffer)
  (unless (eq? (major-mode) 'markdown-mode) (markdown-mode))
  (enable-mode! 'claude-compose-mode)
  (set! %acp-history-index #f)
  (goto-char (point-max)))

(define-command (claude-compose)
  "Show the conversation and the compose buffer, with point ready to type."
  (unless (%acp-connected?) (error "Claude is not running; C-c c c first"))
  (%acp-show-ui))

(define-command (claude-compose-send)
  "Send the prompt in the compose buffer to Claude, and clear it for the
next one."
  (let ((text (buffer-string)))
    (cond [(not %acp-session) (error "Claude is not running")]
          [%acp-busy (error "Claude is still working; C-c c k to cancel")]
          [(equal? (string-trim text) "") (error "The prompt is empty")]
          [else
           (%acp-send-prompt text)
           (set! %acp-history (append %acp-history (list text)))
           (set! %acp-history-index #f)
           (%replace-buffer-text! "")])))

(define-command (claude-compose-clear)
  "Clear the compose buffer."
  (%replace-buffer-text! "")
  (set! %acp-history-index #f))

(define-command (claude-compose-previous)
  "Put the previous sent prompt in the compose buffer."
  (let ((n (length %acp-history)))
    (when (> n 0)
      (let ((i (if %acp-history-index (max 0 (- %acp-history-index 1)) (- n 1))))
        (set! %acp-history-index i)
        (%replace-buffer-text! (list-ref %acp-history i))
        (goto-char (point-max))))))

(define-command (claude-compose-next)
  "Put the next sent prompt in the compose buffer, or empty it past the end."
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
    (%acp-ensure-compose)
    (with-current-buffer %acp-prompt-buffer
      (goto-char (point-max))
      (let ((end (point-max)))
        (when (and (> end 0) (not (equal? (buffer-substring (- end 1) end) "\n"))
                   (not (equal? (buffer-substring (- end 1) end) " ")))
          (insert " ")))
      (insert ref " "))
    (message "Added " ref " to the prompt")))

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
(define-key 'claude-compose-mode "M-p" 'claude-compose-previous)
(define-key 'claude-compose-mode "M-n" 'claude-compose-next)

(define (%acp-send-prompt text)
  (%acp-start-turn! text)
  (set! %acp-busy #t)
  (%acp-show-state)
  (%acp-request "session/prompt"
    (json-object 'sessionId %acp-session
                 'prompt (json-array (json-object 'type "text" 'text text)))
    (lambda (result error)
      (set! %acp-busy #f)
      (%acp-show-state)
      (%acp-note (if error
                     (string-append "[error: " (or (%acp-get error 'message) "?") "]")
                     (string-append "— " (or (%acp-get result 'stopReason) "done") " —"))))))

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
  (%acp-show-state)
  (message "Claude stopped."))

(global-set-key "C-c c c" 'claude)
(global-set-key "C-c c p" 'claude-compose)
(global-set-key "C-c c l" 'claude-send)
(global-set-key "C-c c r" 'claude-add-reference)
(global-set-key "C-c c k" 'claude-cancel)
