//! What Scheme code builds on to extend the editor: advice on commands,
//! hooks, other buffers and windows, keys, time and programs.

mod common;

use common::Harness;

#[test]
fn commands_take_advice_whoever_defined_them() {
    let mut h = Harness::new(70, 8);
    h.eval(
        r#"(define trail '())
           (define (note x) (set! trail (cons x trail)))
           (add-advice! 'forward-char 'before 'log (lambda () (note 'before)))
           (add-advice! 'forward-char 'after 'log-after (lambda () (note (point))))"#,
    );
    h.paste("abc");
    h.keys("M-< C-f");
    assert_eq!(h.eval("(reverse trail)"), "(before 1)");
    assert_eq!(h.screen.cursor(), Some((0, 1)));
    // Around advice decides whether and how often the command runs.
    h.eval("(add-advice! 'forward-char 'around 'twice (lambda (next) (next) (next)))");
    h.keys("M-< C-f");
    assert_eq!(h.screen.cursor(), Some((0, 2)));
    // By name too, and the prefix argument still reaches the command.
    h.keys("M-< C-u 1 M-x")
        .type_text("forward-char")
        .keys("RET");
    assert_eq!(h.screen.cursor(), Some((0, 2)));
    assert_eq!(
        h.eval("(map car (command-advice 'forward-char))"),
        "(log log-after twice)"
    );
    // Advice under an id replaces what was there; removing gives it back.
    h.eval("(add-advice! 'forward-char 'around 'twice (lambda (next) #f))");
    h.keys("M-< C-f");
    assert_eq!(h.screen.cursor(), Some((0, 0)));
    h.eval("(for-each (lambda (id) (remove-advice! 'forward-char id)) '(twice log log-after))");
    h.eval("(set! trail '())");
    h.keys("M-< C-f");
    assert_eq!(h.screen.cursor(), Some((0, 1)));
    assert_eq!(h.eval("trail"), "()");
    assert_eq!(
        h.eval("(%script-command 'forward-char)"),
        "#f",
        "it is the built-in command again"
    );

    let error = h
        .session
        .eval("(add-advice! 'no-such-command 'before 'x (lambda () #f))")
        .unwrap_err();
    assert_eq!(error.summary, "there is no command named no-such-command");
    let error = h
        .session
        .eval("(add-advice! 'forward-char 'sideways 'x (lambda () #f))")
        .unwrap_err();
    assert!(
        error.summary.starts_with("advice is one of"),
        "{}",
        error.summary
    );
}

#[test]
fn a_command_can_replace_a_built_in_one_and_still_call_it() {
    let mut h = Harness::new(70, 8);
    h.eval(
        r#"(define-command (newline)
             "Insert a newline and say so."
             (call-built-in-command 'newline)
             (insert "> "))
           (add-advice! 'newline 'after 'mark (lambda () (insert "!")))"#,
    );
    h.type_text("a").keys("RET");
    assert_eq!(h.text(), "a\n> !");
    // Defining it again keeps the advice.
    h.eval(r#"(define-command (newline) "Plain again." (call-built-in-command 'newline))"#);
    h.keys("RET");
    assert_eq!(h.text(), "a\n> !\n!");
    assert_eq!(
        h.eval("(command-documentation 'newline)"),
        "\"Plain again.\""
    );
}

#[test]
fn hooks_run_before_and_after_every_command() {
    let mut h = Harness::new(70, 8);
    h.eval(
        r#"(define trail '())
           (add-hook! 'pre-command-hook (lambda () (set! trail (cons (list 'pre (point)) trail))))
           (add-hook! 'post-command-hook (lambda () (set! trail (cons (list 'post (point)) trail))))"#,
    );
    h.eval("(set! trail '())");
    h.type_text("ab");
    h.keys("C-b");
    assert_eq!(
        h.eval("(reverse trail)"),
        "((pre 0) (post 1) (pre 1) (post 2) (pre 2) (post 1))"
    );
    assert_eq!(h.text(), "ab");
    // Typing is still one undo step, and commands defined in Scheme get
    // the hooks too.
    h.keys("C-/");
    assert_eq!(h.text(), "");
    h.eval(r#"(define-command (hello) "Greet." (insert "hello")) (global-set-key "C-c h" 'hello)"#);
    h.eval("(set! trail '())");
    h.keys("C-c h");
    assert_eq!(h.eval("(reverse trail)"), "((pre 0) (post 5))");
    // The minibuffer works with the hooks on.
    h.keys("M-x").type_text("hello").keys("RET");
    assert_eq!(h.text(), "hellohello");
}

#[test]
fn code_works_in_other_buffers_and_windows() {
    let mut h = Harness::new(70, 12);
    h.paste("here");
    h.eval(r#"(switch-to-buffer "other") (insert "one two") (goto-char 3) (switch-to-buffer "*scratch*")"#);
    assert_eq!(
        h.eval(r#"(with-current-buffer "other" (list (buffer-name) (point) (buffer-string)))"#),
        "(\"other\" 3 \"one two\")"
    );
    assert_eq!(h.eval("(list (buffer-name) (point))"), "(\"*scratch*\" 4)");
    // Edits go to that buffer, and an error on the way does not strand us.
    h.eval(r#"(with-current-buffer "other" (goto-char (point-max)) (insert " three"))"#);
    assert!(
        h.session
            .eval(r#"(with-current-buffer "other" (car 1))"#)
            .is_err()
    );
    assert_eq!(h.eval("(buffer-name)"), "\"*scratch*\"");
    assert_eq!(h.eval(r#"(buffer-string "other")"#), "\"one two three\"");
    assert_eq!(h.text(), "here");
    assert_eq!(h.screen.row(0), "here", "the window shows what it showed");

    assert_eq!(
        h.eval("(save-excursion (goto-char 0) (insert \">\") (point))"),
        "1"
    );
    assert_eq!(h.eval("(point)"), "4", "where it was, as a position");

    // Windows are numbers; the selected one comes first.
    h.keys("C-x 2");
    h.eval(r#"(switch-to-buffer "other")"#);
    assert_eq!(h.eval("(length (window-list))"), "2");
    assert_eq!(
        h.eval("(map window-buffer (window-list))"),
        "(\"other\" \"*scratch*\")"
    );
    assert_eq!(
        h.eval("(with-selected-window (cadr (window-list)) (list (buffer-name) (window-start) (window-end)))"),
        "(\"*scratch*\" 0 5)"
    );
    assert_eq!(h.eval("(buffer-name)"), "\"other\"");
    h.eval("(select-window (cadr (window-list)))");
    assert_eq!(h.eval("(buffer-name)"), "\"*scratch*\"");
    let error = h.session.eval("(select-window 12345)").unwrap_err();
    assert_eq!(error.summary, "select-window: There is no window 12345");
}

#[test]
fn commands_wait_for_keys_time_and_programs() {
    let mut h = Harness::new(70, 8);
    h.eval(
        r#"(define-command (ask-key)
             "Insert the name of the next key."
             (insert "[" (read-key "Key: ") "]"))
           (global-set-key "C-c k" 'ask-key)"#,
    );
    h.keys("C-c k");
    assert_eq!(h.echo(), "Key:");
    h.keys("C-x");
    assert_eq!(h.text(), "[C-x]");
    h.keys("C-c k RET");
    assert_eq!(h.text(), "[C-x][RET]");
    h.keys("C-c k C-g");
    assert_eq!(h.echo(), "Quit");
    assert_eq!(h.eval("(%waiting-count)"), "0");

    // Time passes without the editor standing still.
    h.eval(
        r#"(define-command (later)
             "Insert after a moment."
             (sleep-for 30)
             (insert "late"))
           (global-set-key "C-c l" 'later)"#,
    );
    h.keys("C-c l");
    h.type_text("x");
    assert_eq!(h.text(), "[C-x][RET]x", "typing goes on meanwhile");
    h.wait_until(|h| h.text().ends_with("late"));

    // Programs run beside the editor and hand back what they wrote.
    h.eval(
        r#"(define result #f)
           (define-command (run)
             "Run some programs."
             (set! result
                   (list (run-process "sh" '("-c" "cat; echo err >&2; exit 3") "in")
                         (process-lines "printf" "a\nb\n")
                         (car (run-process "/nonexistent/program" '()))
                         (guard (e (#t (error-object-message e))) (process-lines "false")))))
           (global-set-key "C-c r" 'run)"#,
    );
    h.keys("C-c r");
    h.wait_until(|h| h.eval("result") != "#f");
    assert_eq!(
        h.eval("result"),
        "((3 \"in\" \"err\\n\") (\"a\" \"b\") #f \"false failed: \")"
    );
}

#[test]
fn buttons_of_the_pointer_are_keys_that_scheme_binds() {
    let mut h = Harness::new(60, 10);
    h.paste("one two three\nfour five six\n");
    h.eval(
        r#"(define seen '())
           (define (note what)
             (set! seen (cons (list what (mouse-press-position) (mouse-position)
                                    (mouse-modifiers))
                              seen)))
           (define-command (my-menu) "Note a press." (note 'press))
           (define-command (my-drag) "Note a movement." (note 'motion))
           (define-command (my-drop) "Note a letting go." (note 'drop))
           (define-major-mode pointer-mode #f "Pointer" "A mode with keys for the pointer.")
           (define-key 'pointer-mode "<mouse-3>" 'my-menu)
           (define-key 'pointer-mode "<motion-mouse-3>" 'my-drag)
           (define-key 'pointer-mode "<drag-mouse-3>" 'my-drop)
           (pointer-mode)"#,
    );
    h.press("M-<mouse-3>", 4, 0);
    // With modifiers that have no binding of their own, it is the press.
    let w = h.eval("(selected-window)");
    assert_eq!(h.eval("seen"), format!("((press ({w} 4) ({w} 4) (meta)))"));
    h.drag_to(5, 1)
        .release(5, 1, amacs_proto::Mods::CTRL | amacs_proto::Mods::SHIFT);
    assert_eq!(
        h.eval("(reverse seen)"),
        format!(
            "((press ({w} 4) ({w} 4) (meta)) (motion ({w} 4) ({w} 19) ()) \
             (drop ({w} 4) ({w} 19) (control shift)))"
        )
    );
    // A button let go where it was pressed is no drag, and the main
    // button still does what it does everywhere.
    h.eval("(set! seen '())");
    h.press("<mouse-3>", 4, 0)
        .release(4, 0, amacs_proto::Mods::empty());
    assert_eq!(h.eval("(length seen)"), "1");
    h.press("<double-mouse-1>", 5, 1);
    assert_eq!(h.eval("(buffer-substring (mark) (point))"), "\"five\"");
    assert_eq!(h.eval("(key-binding \"<mouse-1>\")"), "mouse-set-point");
}
