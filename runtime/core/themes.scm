;;; Themes.
;;;
;;; A theme is a palette: names for colours. Faces refer to palette names,
;;; so loading a theme recolours everything. The Modus themes are built in:
;;; modus-operandi and modus-vivendi, and their tinted variants.
;;;
;;;   (load-theme 'modus-vivendi-tinted)
;;;   (set-palette-overrides! '((comment yellow-faint) (string green-warmer)))
;;;   (set-face! 'keyword 'fg 'magenta 'bold #f)
;;;   (define-theme 'mine 'modus-vivendi '((bg-main "#101418")))

(define-option 'themes-to-toggle '(modus-operandi-tinted modus-vivendi-tinted)
  "The themes that `toggle-theme` switches between.")

(define-command (toggle-theme)
  "Switch to the next of the themes in the `themes-to-toggle` option."
  (let* ((themes (option 'themes-to-toggle))
         (rest (member (current-theme) themes))
         (next (if (and rest (not (null? (cdr rest))))
                   (car (cdr rest))
                   (car themes))))
    (load-theme next)
    (message "Theme: " next)))

(define-command (choose-theme)
  "Choose a theme by name and switch to it."
  (load-theme (completing-read "Theme: " (theme-names) 'theme)))
