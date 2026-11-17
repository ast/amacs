;;; Menus of keys: a few lines under the echo area that say what each key
;;; does, as the commands of git are offered.

(define (%key-menu-lines title switches enabled actions)
  (append
   (list title)
   (map (lambda (switch)
          (string-append " " (car switch) "  "
                         (if (member (car switch) enabled) "[x] " "[ ] ")
                         (cadr switch)))
        switches)
   (map (lambda (action)
          (string-append " " (car action) "   " (cadr action)))
        actions)))

(define (key-menu title switches actions)
  "Shows a menu and waits for a key. Actions are lists of a key, a label and
a value; pressing an action's key ends the menu. Switches are lists of two
keys as one string, the first of them -, a label and a value; pressing them
turns the switch on or off. Returns a pair of the action's value and the
values of the switches that are on. Another key is an error, and C-g quits."
  (let loop ((enabled '()))
    (show-hint! (%key-menu-lines title switches enabled actions))
    (let* ((first (read-key (string-append title ":")))
           (key (if (equal? first "-")
                    (begin
                      (show-hint! (%key-menu-lines title switches enabled actions))
                      (string-append first (read-key (string-append title ": -"))))
                    first)))
      (cond
        [(assoc key actions)
         => (lambda (action)
              (cons (caddr action)
                    (filter-map (lambda (switch)
                                  (and (member (car switch) enabled) (caddr switch)))
                                switches)))]
        [(assoc key switches)
         (loop (if (member key enabled) (delete key enabled) (cons key enabled)))]
        [else (error (string-append key " is not in the menu"))]))))
