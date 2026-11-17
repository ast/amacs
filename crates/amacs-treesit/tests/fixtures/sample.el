;;; sample.el --- A sample  -*- lexical-binding: t; -*-

(require 'cl-lib)

(defvar my/limit 10
  "How many.")

(defun my/square (x)
  "Return X squared."
  (interactive "nNumber: ")
  (let ((result (* x x)))
    (when (> result my/limit)
      (message "big: %d" result))
    result))

(use-package magit
  :bind ("C-x g" . magit-status))
