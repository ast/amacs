;;; Tests for the editor primitives and the Scheme runtime.

(deftest insert-moves-point
  (insert "abc" "def")
  (check-equal? (point) 6)
  (check-equal? (buffer-string) "abcdef"))

(deftest positions-are-zero-based
  (insert "one\ntwo")
  (check-equal? (point-min) 0)
  (check-equal? (point-max) 7)
  (goto-char 5)
  (check-equal? (line-beginning-position) 4)
  (check-equal? (line-end-position) 7)
  (check-equal? (line-number-at-pos) 2)
  (check-equal? (current-column) 1)
  (check-equal? (char-after) "w")
  (check-equal? (char-after 7) #f)
  (check-error (lambda () (goto-char 8)))
  (check-error (lambda () (goto-char -1))))

(deftest regions-take-positions-in-either-order
  (insert "hello world")
  (check-equal? (buffer-substring 6 0) "hello ")
  (delete-region 11 5)
  (check-equal? (buffer-string) "hello"))

(deftest kill-ring
  (insert "keep kill")
  (kill-region 4 9)
  (check-equal? (buffer-string) "keep")
  (check-equal? (current-kill) " kill")
  (kill-new "newer")
  (call-command 'yank)
  (check-equal? (buffer-string) "keepnewer"))

(deftest mark-and-region
  (insert "abc")
  (check-false (mark))
  (check-false (region-active?))
  (set-mark! 1)
  (check-equal? (mark) 1)
  (check-false (region-active?))
  (activate-mark!)
  (check-true (region-active?)))

(deftest native-commands-are-callable
  (insert "one two")
  (call-command 'backward-word)
  (check-equal? (point) 4)
  (call-command 'move-beginning-of-line)
  (check-equal? (point) 0)
  (check-error (lambda () (call-command 'no-such-command))))

(define-command (test-insert-x)
  "Insert an x."
  (insert "x"))

(deftest scheme-commands-are-procedures-and-commands
  (test-insert-x)
  (call-command 'test-insert-x)
  (check-equal? (buffer-string) "xx")
  (check-true (command? 'test-insert-x))
  (check-equal? (command-documentation 'test-insert-x) "Insert an x.")
  (check-false (command? 'no-such-command)))

(deftest key-bindings
  (global-set-key "C-c t x" 'test-insert-x)
  (check-equal? (key-binding "C-c t x") 'test-insert-x)
  (check-equal? (key-binding "C-f") 'forward-char)
  (check-false (key-binding "C-c t"))
  (global-unset-key "C-c t x")
  (check-false (key-binding "C-c t x"))
  (check-error (lambda () (global-set-key "not a key" 'test-insert-x))))

(deftest pressed-keys-run-built-in-commands-at-once
  (press-keys "a b C-b")
  (check-equal? (buffer-string) "ab")
  (check-equal? (point) 1))

(deftest read-only-buffers-raise-errors
  (insert "text")
  (set-buffer-read-only! #t)
  (check-true (buffer-read-only?))
  (check-error (lambda () (insert "more")))
  (set-buffer-read-only! #f)
  (insert "!")
  (check-equal? (buffer-string) "text!"))

(deftest options-have-defaults-globals-and-locals
  (define-option 'test-width 80 "A width.")
  (check-equal? (option 'test-width) 80)
  (set-option! 'test-width 100)
  (check-equal? (option 'test-width) 100)
  (set-local! 'test-width 72)
  (check-equal? (option 'test-width) 72)
  (fundamental-mode)
  (check-equal? (option 'test-width) 100)
  (check-error (lambda () (option 'no-such-option)))
  (check-error (lambda () (set-option! 'tab-width 0))))

(deftest hooks-run-in-order-without-duplicates
  (define calls '())
  (define (first) (set! calls (cons 'first calls)))
  (define (second arg) (set! calls (cons arg calls)))
  (add-hook! 'test-hook-a first)
  (add-hook! 'test-hook-a first)
  (run-hooks 'test-hook-a)
  (check-equal? calls '(first))
  (add-hook! 'test-hook-b second)
  (run-hooks 'test-hook-b 'argument)
  (check-equal? calls '(argument first))
  (remove-hook! 'test-hook-a first)
  (run-hooks 'test-hook-a)
  (check-equal? calls '(argument first)))

(define-major-mode test-parent-mode #f "Parent"
  "A mode to derive from.")

(define-major-mode test-child-mode test-parent-mode "Child"
  "A derived mode."
  (set-local! 'tab-width 2))

(deftest major-modes-run-parent-hooks-first
  (define order '())
  (define (parent-hook) (set! order (cons 'parent order)))
  (define (child-hook) (set! order (cons 'child order)))
  (add-hook! 'test-parent-mode-hook parent-hook)
  (add-hook! 'test-child-mode-hook child-hook)
  (test-child-mode)
  (remove-hook! 'test-parent-mode-hook parent-hook)
  (remove-hook! 'test-child-mode-hook child-hook)
  (check-equal? (major-mode) 'test-child-mode)
  (check-equal? (reverse order) '(parent child))
  (check-equal? (option 'tab-width) 2))

(deftest mode-keymaps-are-inherited
  (define-key 'test-parent-mode "C-c t p" 'test-insert-x)
  (check-false (key-binding "C-c t p"))
  (test-child-mode)
  (check-equal? (key-binding "C-c t p") 'test-insert-x)
  (define-key 'test-child-mode "C-c t p" 'forward-char)
  (check-equal? (key-binding "C-c t p") 'forward-char)
  (undefine-key 'test-child-mode "C-c t p")
  (undefine-key 'test-parent-mode "C-c t p"))

(define-minor-mode test-minor-mode "Minor"
  "A minor mode for the tests.")

(deftest minor-modes-toggle-and-take-precedence
  (define-key 'test-minor-mode "C-f" 'test-insert-x)
  (check-false (minor-mode-enabled? 'test-minor-mode))
  (test-minor-mode)
  (check-true (minor-mode-enabled? 'test-minor-mode))
  (check-equal? (key-binding "C-f") 'test-insert-x)
  (test-minor-mode)
  (check-false (minor-mode-enabled? 'test-minor-mode))
  (check-equal? (key-binding "C-f") 'forward-char))

(deftest await-needs-a-command
  ;; Tests run at top level, where there is nothing to return to.
  (check-error (lambda () (read-string "Prompt: "))))

(deftest buffers
  (check-equal? (buffer-name) "*test*")
  (check-false (buffer-file-name))
  (check-false (buffer-modified? "*test*"))
  (check-true (if (member "*scratch*" (buffer-list)) #t #f))
  (check-error (lambda () (buffer-string "no such buffer"))))
