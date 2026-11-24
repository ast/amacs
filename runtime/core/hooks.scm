;;; Hooks: named lists of procedures that run when something happens.
;;;
;;; The editor runs these hooks itself:
;;;   find-file-hook     after a file is first visited, in its buffer
;;;   before-save-hook   before a buffer is written to its file, in it
;;;   after-save-hook    after a buffer is written to its file, in it
;;;   pre-command-hook   before every command
;;;   post-command-hook  after every command
;;;   kill-buffer-hook   after a buffer is killed, with its name
;;;   window-size-change-hook  after the text area of a window changed
;;;                      size, or the window came to show another buffer
;;;   term-directory-hook  after the program in a terminal changed
;;;                      directory, with the buffer's name and the directory
;;; Every mode also has a hook named after it, such as `rust-mode-hook`.

(define %hooks (make-hash-table))

(define (hook-functions name)
  "The procedures on a hook, in the order they run."
  (hash-table-ref %hooks name '()))

(define (%set-hook-functions! name functions)
  (hash-table-set! %hooks name functions)
  (%hook-active! name (not (null? functions))))

(define (add-hook! name f)
  "Adds a procedure to the end of a hook, unless it is already there."
  (unless (member f (hook-functions name))
    (%set-hook-functions! name (append (hook-functions name) (list f)))))

(define (remove-hook! name f)
  "Takes a procedure off a hook."
  (%set-hook-functions! name (filter (lambda (g) (not (equal? f g))) (hook-functions name))))

(define (run-hooks name . args)
  "Calls every procedure on a hook with the arguments."
  (for-each (lambda (f) (apply f args)) (hook-functions name)))

;; The editor's entry point.
(define (%run-hook name . args)
  (apply run-hooks name args))

;; The same for a hook of one buffer, which may no longer be the current
;; one, or be there at all.
(define (%run-hook-in buffer name)
  (when (member buffer (buffer-list))
    (%call-in-buffer buffer (lambda () (run-hooks name)))))

;; The editor's entry point for saving while before-save-hook has functions,
;; and for saving to another host. A function that fails is reported and
;; the buffer is saved all the same. A file on another host is written in
;; the background, and the buffer is the current one only until that starts.
(define (%save-buffer buffer path kill?)
  (let* ((problem #f)
         (sent (%call-in-buffer
                buffer
                (lambda ()
                  (set! problem
                        (guard (e (#t (if (error-object? e)
                                          (error-object->string e)
                                          (write->string e))))
                          (run-hooks 'before-save-hook)
                          #f))
                  (if (file-remote? path)
                      (begin (message "Writing " path "...")
                             (%send-buffer path))
                      (begin (%write-buffer path) #f))))))
    (when sent
      (let ((failed (await sent)))
        (when (string? failed) (error failed))))
    (when problem
      (message "Saved, but before-save-hook failed: " problem))
    (when kill? (kill-buffer buffer))))
