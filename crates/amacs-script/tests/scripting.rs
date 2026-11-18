//! End-to-end tests of the Scheme integration: keys in, screen out, with
//! commands, hooks and modes defined in Scheme.

mod common;

use std::time::{Duration, Instant};

use amacs_proto::InputEvent;
use common::Harness;

#[test]
fn evaluates_expressions() {
    let mut h = Harness::new(70, 8);
    assert_eq!(h.eval("(+ 1 2)"), "3");
    assert_eq!(h.eval("(buffer-name)"), "\"*scratch*\"");
    assert_eq!(h.eval("(define x 5)"), "");
    assert_eq!(h.eval("(list x 'sym \"str\")"), "(5 sym \"str\")");
}

#[test]
fn a_scheme_command_bound_to_a_key_is_one_undo_step() {
    let mut h = Harness::new(70, 8);
    h.eval(
        r#"(define-command (insert-banner)
             "Insert a banner line."
             (call-command 'move-beginning-of-line)
             (insert "== ")
             (call-command 'move-end-of-line)
             (insert " =="))
           (global-set-key "C-c b" 'insert-banner)"#,
    );
    h.type_text("title").keys("C-c b");
    assert_eq!(h.text(), "== title ==");
    h.keys("C-/");
    assert_eq!(h.text(), "title");
    h.keys("M-x").type_text("insert-ban").keys("TAB RET");
    assert_eq!(h.text(), "== title ==");
}

#[test]
fn built_in_keys_can_be_rebound_to_scheme_commands() {
    let mut h = Harness::new(70, 8);
    h.eval(
        r#"(define-command (kill-region-or-backward-kill-word)
             "Kill the region if it is active, else the word before point."
             (if (region-active?)
                 (call-command 'kill-region)
                 (call-command 'backward-kill-word)))
           (global-set-key "C-w" 'kill-region-or-backward-kill-word)
           (global-set-key "C-h" 'delete-backward-char)"#,
    );
    h.type_text("one two three").keys("C-w");
    assert_eq!(h.text(), "one two ");
    h.keys("C-h C-h");
    assert_eq!(h.text(), "one tw");
    h.keys("C-a C-SPC M-f C-w");
    assert_eq!(h.text(), " tw");
}

#[test]
fn prefix_arguments_reach_scheme_commands() {
    let mut h = Harness::new(70, 8);
    h.eval(
        r#"(define-command (show-arg)
             "Show the prefix argument."
             (message "arg " (prefix-arg) " given " (prefix-arg-given?)))
           (global-set-key "C-c a" 'show-arg)"#,
    );
    h.keys("C-c a");
    assert!(h.echo().contains("arg 1 given #f"), "{}", h.echo());
    h.keys("C-u 7 C-c a");
    assert!(h.echo().contains("arg 7 given #t"), "{}", h.echo());
}

#[test]
fn a_command_can_wait_for_minibuffer_input() {
    let mut h = Harness::new(70, 8);
    h.eval(
        r#"(define-command (greet)
             "Ask for a name and a greeting, and insert them."
             (insert "[")
             (let* ((name (read-string "Name: "))
                    (greeting (read-string "Greeting: " "Hello")))
               (insert greeting ", " name "]")))
           (global-set-key "C-c g" 'greet)"#,
    );
    h.keys("C-c g");
    assert_eq!(h.echo(), "Name:");
    assert_eq!(
        h.text(),
        "",
        "the minibuffer is selected while the command waits"
    );
    h.type_text("Albin").keys("RET");
    assert_eq!(h.echo(), "Greeting: Hello");
    h.keys("RET");
    assert_eq!(h.text(), "[Hello, Albin]");
    assert_eq!(h.eval("(%waiting-count)"), "0");
}

#[test]
fn quitting_a_prompt_abandons_the_command() {
    let mut h = Harness::new(70, 8);
    h.eval(
        r#"(define-command (ask)
             "Ask and insert."
             (insert (read-string "Word: ")))
           (global-set-key "C-c a" 'ask)"#,
    );
    h.keys("C-c a").type_text("abc").keys("C-g");
    assert_eq!(h.echo(), "Quit");
    assert_eq!(h.text(), "");
    assert_eq!(
        h.eval("(%waiting-count)"),
        "0",
        "the abandoned command is not kept"
    );
    h.keys("C-c a").type_text("xyz").keys("RET");
    assert_eq!(h.text(), "xyz");
}

#[test]
fn yes_or_no_questions() {
    let mut h = Harness::new(70, 8);
    h.eval(
        r#"(define-command (confirm)
             "Ask and report."
             (message (if (y-or-n? "Really?") "yes" "no")))
           (global-set-key "C-c y" 'confirm)"#,
    );
    h.keys("C-c y");
    assert_eq!(h.echo(), "Really? (y or n)");
    h.type_text("y");
    assert_eq!(h.echo(), "yes");
    h.keys("C-c y").type_text("n");
    assert_eq!(h.echo(), "no");
    h.keys("C-c y C-g");
    assert_eq!(h.echo(), "Quit");
}

#[test]
fn errors_in_commands_are_reported_and_logged() {
    let mut h = Harness::new(70, 8);
    h.eval(
        r#"(define-command (broken)
             "Fail after editing."
             (insert "partial")
             (car 5))
           (global-set-key "C-c x" 'broken)"#,
    );
    h.keys("C-c x");
    assert!(
        h.echo().contains("car: expected a pair, got 5"),
        "{}",
        h.echo()
    );
    assert_eq!(h.text(), "partial");
    h.keys("C-/");
    assert_eq!(
        h.text(),
        "",
        "the failed command's edits are still one undo step"
    );
    let log = h.eval(r#"(buffer-string "*Messages*")"#);
    assert!(log.contains("car: expected a pair, got 5"), "{log}");
    h.type_text("ok");
    assert_eq!(h.text(), "ok");
}

#[test]
fn primitives_check_their_arguments() {
    let mut h = Harness::new(70, 8);
    let arity = h.session.eval("(goto-char)").unwrap_err();
    assert!(
        arity
            .summary
            .contains("goto-char: expected 1 argument, got 0"),
        "{}",
        arity.summary
    );
    let kind = h.session.eval("(goto-char \"x\")").unwrap_err();
    assert!(
        kind.summary.contains("argument 1 must be an integer"),
        "{}",
        kind.summary
    );
    let range = h.session.eval("(goto-char 99)").unwrap_err();
    assert!(
        range
            .summary
            .contains("position 99 is outside the buffer (0 to 0)"),
        "{}",
        range.summary
    );
}

#[test]
fn eval_expression_shows_the_result() {
    let mut h = Harness::new(70, 8);
    h.keys("M-:").type_text("(* 6 7)").keys("RET");
    assert_eq!(h.echo(), "42");
    h.keys("M-:").type_text("(undefined-function)").keys("RET");
    assert!(
        h.echo().contains("unbound variable: undefined-function"),
        "{}",
        h.echo()
    );
}

#[test]
fn eval_buffer_defines_commands_from_the_buffer() {
    let mut h = Harness::new(70, 8);
    h.session.handle_event(InputEvent::Paste(
        "(define-command (from-buffer) \"Say so.\" (message \"defined in a buffer\"))".to_owned(),
    ));
    h.keys("M-x").type_text("eval-buffer").keys("RET");
    h.keys("M-x").type_text("from-buffer").keys("RET");
    assert_eq!(h.echo(), "defined in a buffer");
}

#[test]
fn runaway_scheme_code_can_be_interrupted() {
    let mut h = Harness::new(70, 8);
    h.eval("(define (spin) (spin))");
    let interrupt = h.session.interrupt_handle();
    assert!(!interrupt.interrupt(), "nothing is running yet");

    let watchdog = std::thread::spawn(move || {
        let start = Instant::now();
        while !interrupt.interrupt() {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "the loop never started"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    });
    h.keys("M-:").type_text("(spin)").keys("RET");
    watchdog.join().unwrap();
    assert!(h.echo().contains("interrupted"), "{}", h.echo());

    assert_eq!(
        h.eval("(+ 1 1)"),
        "2",
        "the interpreter works after an interrupt"
    );
    h.type_text("still editing");
    assert_eq!(h.text(), "still editing");
}

#[test]
fn files_get_their_major_mode_from_their_name() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.scm");
    std::fs::write(&path, "(define\tx 1)\n").unwrap();

    let mut h = Harness::new(70, 8);
    h.eval(r#"(add-hook! 'scheme-mode-hook (lambda () (set-local! 'tab-width 4)))"#);
    h.eval(&format!("(find-file {:?})", path.to_str().unwrap()));
    assert_eq!(h.mode_line(), " --  config.scm   L1 C0  (Scheme)");
    assert_eq!(
        h.screen.row(0),
        "(define x 1)",
        "the tab stops come from the mode hook"
    );
    assert_eq!(h.eval("(option 'tab-width)"), "4");
    h.keys("C-x b RET");
    assert_eq!(h.eval("(option 'tab-width)"), "8");
}

#[test]
fn source_files_are_highlighted_and_indented_by_their_mode() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("main.rs");
    std::fs::write(&file, "fn main() {\n}\n").unwrap();
    let mut h = Harness::new(70, 8);
    h.eval(
        r#"(add-hook! 'prog-mode-hook (lambda () (set-local! 'line-numbers 'relative)))
           (enable-mode! 'auto-pair-mode)
           (global-set-key "C-c t" 'toggle-theme)"#,
    );
    h.eval(&format!("(find-file {:?})", file.display().to_string()));
    assert!(h.mode_line().ends_with("(Rust)"), "{}", h.mode_line());
    assert_eq!(h.screen.row(0), "1   fn main() {");
    let keyword = h.screen.face_at(0, 4);
    assert!(keyword.bold, "keywords are bold");
    assert_ne!(keyword.fg, h.screen.face_at(0, 7).fg);

    h.keys("C-e RET").type_text("vec![1");
    assert_eq!(h.text(), "fn main() {\n    vec![1]\n}\n");

    // A theme changes the colours of what is on the screen.
    h.keys("C-c t");
    assert_eq!(h.echo(), "Theme: modus-vivendi-tinted");
    assert_ne!(h.screen.face_at(0, 4).fg, keyword.fg);
    assert_ne!(h.screen.face_at(0, 4).bg, keyword.bg);

    // So does changing a face or the palette.
    let dark = h.screen.face_at(0, 4);
    h.eval("(set-face! 'keyword 'fg \"#123456\" 'bold #f)");
    let changed = h.screen.face_at(0, 4);
    assert!(!changed.bold);
    assert_ne!(changed.fg, dark.fg);
    h.eval("(set-face! 'keyword 'fg 'keyword 'bold #t)");
    h.eval("(set-palette-overrides! '((keyword red-intense)))");
    assert_ne!(h.screen.face_at(0, 4).fg, dark.fg);
    h.eval("(set-palette-overrides! '())");
    assert_eq!(h.screen.face_at(0, 4), dark);
}

#[test]
fn lisp_files_can_have_paredit_from_a_hook() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("init.scm");
    std::fs::write(&file, "").unwrap();
    let mut h = Harness::new(70, 8);
    h.eval("(add-hook! 'scheme-mode-hook (lambda () (enable-mode! 'paredit-mode)))");
    h.eval(&format!("(find-file {:?})", file.display().to_string()));
    assert!(
        h.mode_line().ends_with("(Scheme Paredit)"),
        "{}",
        h.mode_line()
    );
    h.type_text("(define (f x)").keys("RET").type_text("(* x x");
    assert_eq!(h.text(), "(define (f x)\n  (* x x))");
    h.keys("M-x").type_text("paredit-mode").keys("RET");
    assert_eq!(h.echo(), "Paredit mode disabled");
    h.keys("M->").type_text("(");
    assert_eq!(h.text(), "(define (f x)\n  (* x x))(");
}

#[test]
fn snippets_and_formatters_are_set_up_from_scheme() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes.txt");
    std::fs::write(&file, "").unwrap();
    let mut h = Harness::new(70, 8);
    h.eval(
        r#"(define-snippets 'text-mode
             `((sig "-- " (p "name") n ,(lambda () (string-append "sent " "today")) q)))
           (define-formatter 'upper "tr" '("a-z" "A-Z"))
           (format-on-save! 'text-mode 'upper)"#,
    );
    h.eval(&format!("(find-file {:?})", file.display().to_string()));
    assert_eq!(h.eval("(option 'formatter)"), "upper");

    // The procedure in the template is called when the snippet expands.
    h.type_text("sig").keys("M-+");
    assert_eq!(h.text(), "-- name\nsent today");
    h.type_text("me").keys("TAB").type_text("!");
    assert_eq!(h.text(), "-- me\nsent today!");

    h.keys("C-x C-s");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "-- ME\nSENT TODAY!"
    );
    assert_eq!(h.text(), "-- ME\nSENT TODAY!");
}

#[test]
fn minor_modes_show_in_the_mode_line() {
    let mut h = Harness::new(70, 8);
    h.eval(r#"(define-minor-mode ruler-mode "Ruler" "Pretend to show a ruler.")"#);
    h.keys("M-x").type_text("ruler-mode").keys("RET");
    assert_eq!(h.echo(), "Ruler mode enabled");
    assert_eq!(h.mode_line(), " --  *scratch*   L1 C0  (Fundamental Ruler)");
    h.keys("M-x").type_text("ruler-mode").keys("RET");
    assert_eq!(h.mode_line(), " --  *scratch*   L1 C0  (Fundamental)");
}

#[test]
fn post_command_hook_runs_once_per_command() {
    let mut h = Harness::new(70, 8);
    h.eval(
        r#"(define count 0)
           (add-hook! 'post-command-hook (lambda () (set! count (+ count 1))))"#,
    );
    h.type_text("abc").keys("C-b C-x 2");
    assert_eq!(h.eval("count"), "5");
    h.keys("C-/");
    assert_eq!(
        h.text(),
        "",
        "hook runs do not split the undo group of typed text"
    );
}

#[test]
fn after_save_hook_runs_after_saving() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("saved.txt");
    let mut h = Harness::new(70, 8);
    h.eval(r#"(add-hook! 'after-save-hook (lambda () (log-message (string-append "saved " (buffer-name)))))"#);
    h.eval(&format!("(find-file {:?})", path.to_str().unwrap()));
    assert_eq!(h.mode_line(), " --  saved.txt   L1 C0  (Text)");
    h.type_text("data").keys("C-x C-s");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "data");
    assert_eq!(
        h.eval(r#"(buffer-string "*Messages*")"#),
        "\"saved saved.txt\\n\""
    );
}

#[test]
fn before_save_hook_changes_what_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("saved.txt");
    let mut h = Harness::new(70, 8);
    h.eval(
        r#"(add-hook! 'before-save-hook
             (lambda () (save-excursion (goto-char (point-max)) (insert "\n"))))"#,
    );
    h.eval(&format!("(find-file {:?})", path.to_str().unwrap()));
    h.type_text("data").keys("C-x C-s");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "data\n");
    assert_eq!(h.mode_line(), " --  saved.txt   L1 C4  (Text)");
    assert!(h.echo().starts_with("Wrote "), "{}", h.echo());
}

#[test]
fn before_save_hook_that_fails_does_not_stop_the_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("saved.txt");
    let mut h = Harness::new(70, 8);
    h.eval(r#"(add-hook! 'before-save-hook (lambda () (error "no good")))"#);
    h.eval(&format!("(find-file {:?})", path.to_str().unwrap()));
    h.type_text("data").keys("C-x C-s");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "data");
    assert_eq!(h.echo(), "Saved, but before-save-hook failed: no good");
}

#[test]
fn save_hooks_run_in_the_buffer_that_is_saved() {
    let dir = tempfile::tempdir().unwrap();
    let (one, two) = (dir.path().join("one.txt"), dir.path().join("two.txt"));
    let mut h = Harness::new(70, 8);
    h.eval(
        r#"(add-hook! 'before-save-hook
             (lambda () (goto-char (point-max)) (insert " in " (buffer-name))))
           (add-hook! 'after-save-hook
             (lambda () (log-message (string-append "saved " (buffer-name)))))"#,
    );
    h.eval(&format!("(find-file {:?})", one.to_str().unwrap()));
    h.type_text("1");
    h.eval(&format!("(find-file {:?})", two.to_str().unwrap()));
    h.type_text("2");
    h.eval(r#"(switch-to-buffer "*scratch*")"#);
    h.keys("C-x s");
    assert_eq!(std::fs::read_to_string(&one).unwrap(), "1 in one.txt");
    assert_eq!(std::fs::read_to_string(&two).unwrap(), "2 in two.txt");
    assert_eq!(h.mode_line(), " --  *scratch*   L1 C0  (Fundamental)");
    let log = h.eval(r#"(buffer-string "*Messages*")"#);
    assert!(log.contains("saved one.txt"), "{log}");
    assert!(log.contains("saved two.txt"), "{log}");
}

#[test]
fn init_file_errors_are_reported_and_earlier_forms_take_effect() {
    let dir = tempfile::tempdir().unwrap();
    let init = dir.path().join("init.scm");
    std::fs::write(
        &init,
        "(global-set-key \"C-c i\" 'forward-char)\n(this-is-not-defined)\n",
    )
    .unwrap();

    let mut h = Harness::new(70, 8);
    let error = h.session.load_file(&init).unwrap_err();
    assert!(
        error.summary.contains("this-is-not-defined"),
        "{}",
        error.summary
    );
    h.redisplay();
    assert!(h.echo().starts_with("Error in "), "{}", h.echo());
    assert!(
        h.session
            .load_file(&dir.path().join("missing.scm"))
            .is_err()
    );
}

#[test]
fn the_scheme_test_suite_passes() {
    let tests = concat!(env!("CARGO_MANIFEST_DIR"), "/../../runtime/tests/core.scm");
    let mut h = Harness::new(70, 8);
    h.session
        .load_file(std::path::Path::new(tests))
        .unwrap_or_else(|e| panic!("{}", e.report));
    let result = h.eval("(run-tests)");
    assert_eq!(result, "(26)", "failures are listed after the pass count");
    assert_eq!(h.mode_line(), " --  *scratch*   L1 C0  (Fundamental)");
}

#[test]
fn failing_scheme_tests_are_reported_by_name() {
    let mut h = Harness::new(70, 8);
    h.eval(
        r#"(deftest passes (check-equal? 1 1))
           (deftest fails (insert "x") (check-equal? (buffer-string) "y"))
           (deftest raises (car 5))"#,
    );
    let result = h.eval("(run-tests)");
    assert!(result.starts_with("(1 (fails "), "{result}");
    assert!(result.contains("check-equal? failed"), "{result}");
    assert!(result.contains("(raises "), "{result}");
}

#[test]
fn the_example_init_file_loads_and_its_commands_work() {
    let init = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/init.scm");
    let mut h = Harness::new(70, 8);
    h.session
        .load_file(std::path::Path::new(init))
        .unwrap_or_else(|e| panic!("{}", e.report));
    h.redisplay();

    h.type_text("value 255")
        .keys("M-x")
        .type_text("decimal-to-hex")
        .keys("TAB RET");
    assert_eq!(h.text(), "value ff");
    h.keys("M-x")
        .type_text("hexadecimal-to-dec")
        .keys("TAB RET");
    assert_eq!(h.echo(), "Hex ff is 255");
    h.keys("C-w C-w");
    assert_eq!(
        h.text(),
        "",
        "C-w kills words backward when there is no region"
    );
    h.keys("C-u M-x").type_text("copyright-sm6wjm").keys("RET");
    assert!(
        h.text().starts_with("# Copyright (c) SM6WJM 20"),
        "{}",
        h.text()
    );
    // The line is 27 columns long, so with the init file's tab-width of 4
    // the next tab stop is column 28. With the default of 8 it would be 32.
    h.paste_tab();
    assert_eq!(h.screen.row(0).find('x'), Some(28));
}

#[test]
fn scheme_commands_can_offer_candidates() {
    let mut h = Harness::new(70, 8);
    h.eval(
        r#"(define-command (pick-colour)
             "Choose a colour and insert it."
             (insert (completing-read "Colour: "
                                      (list "red" (list "green" "like grass") "blue")
                                      'colour)))
           (global-set-key "C-c p" 'pick-colour)"#,
    );
    h.keys("C-c p");
    // The frame has eight rows, so five are left for candidates.
    assert_eq!(h.screen.row(2), "1/3 Colour:");
    assert_eq!(h.screen.row(4), "green  like grass");
    h.type_text("gr").keys("RET");
    assert_eq!(h.text(), "green");
    h.keys("C-c p").type_text("purple").keys("RET");
    assert_eq!(
        h.text(),
        "greenpurple",
        "text that matches nothing is returned as typed"
    );
}

/// A waiting command that is abandoned still does what it has promised to do
/// on its way out.
#[test]
fn quitting_a_prompt_runs_the_cleanup_of_the_waiting_command() {
    let mut h = Harness::new(70, 8);
    h.eval(
        r#"(define trail '())
           (define-command (careful)
             "Ask, and tidy up whatever happens."
             (unwind-protect
               (guard (e (#t (set! trail (cons 'caught-as-an-error trail))))
                 (insert (read-string "Word: ")))
               (set! trail (cons 'tidied trail))))
           (global-set-key "C-c a" 'careful)"#,
    );
    h.keys("C-c a").type_text("abc").keys("C-g");
    assert_eq!(h.echo(), "Quit");
    assert_eq!(h.eval("trail"), "(tidied)");
    assert_eq!(h.eval("(%waiting-count)"), "0");
    h.keys("C-c a").type_text("abc").keys("RET");
    assert_eq!(h.text(), "abc");
    assert_eq!(h.eval("trail"), "(tidied tidied)");
}

#[test]
fn evaluated_text_can_wait_and_goes_on_afterwards() {
    let mut h = Harness::new(70, 8);
    // As a program outside the editor would ask: the answer comes at once,
    // and the rest runs when the user has answered.
    let result = h
        .session
        .eval("(define word (read-string \"Word: \"))\n(insert word)\n(insert \"!\" (read-string \"More: \"))\n(insert \".\")")
        .unwrap();
    assert_eq!(result, "");
    h.redisplay();
    assert_eq!(h.echo(), "Word:");
    h.type_text("abc").keys("RET");
    // The next question is open already, with the first answer put in.
    assert_eq!(h.echo(), "More:");
    assert_eq!(h.screen.row(0), "abc");
    h.type_text("def").keys("RET");
    assert_eq!(h.text(), "abc!def.");
    assert_eq!(h.eval("(%waiting-count)"), "0");
    // An error in what follows is placed in the text as it was given.
    h.session
        .eval("(read-string \"Again: \")\n\n(car 5)")
        .unwrap();
    h.redisplay();
    h.keys("RET");
    assert_eq!(h.echo(), "car: expected a pair, got 5");
    assert!(
        h.eval("(buffer-string \"*Messages*\")")
            .contains("at eval:3:1")
    );
    // Quitting gives up the rest too.
    h.session
        .eval("(read-string \"Last: \") (insert \"never\")")
        .unwrap();
    h.redisplay();
    h.keys("C-g");
    assert_eq!(h.text(), "abc!def.");
}

#[test]
fn files_of_scheme_are_loaded_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lib.scm");
    std::fs::write(&path, "(define (triple x) (* 3 x))\n(triple 2)\n").unwrap();
    let mut h = Harness::new(70, 8);
    assert_eq!(
        h.eval(&format!("(load {:?})", path.display().to_string())),
        "6"
    );
    assert_eq!(
        h.eval("(procedure-location triple)"),
        format!("\"{}:1:1\"", path.display())
    );
    // A change to the file takes effect when it is loaded again.
    std::fs::write(&path, "(define (triple x) (list x x x))\n").unwrap();
    h.eval(&format!("(load {:?})", path.display().to_string()));
    assert_eq!(h.eval("(triple 2)"), "(2 2 2)");
    let error = h
        .session
        .eval("(load \"/nonexistent/none.scm\")")
        .unwrap_err();
    assert!(
        error
            .summary
            .contains("read-file: cannot read /nonexistent/none.scm"),
        "{}",
        error.summary
    );
}

#[test]
fn the_editor_can_be_asked_about_itself() {
    let mut h = Harness::new(70, 8);
    h.eval(
        r#"(define-command (shout)
             "Insert a loud word."
             (insert "HEY"))"#,
    );
    assert_eq!(
        h.eval("(describe 'shout)"),
        "\"shout is a procedure: (shout)\\nInsert a loud word.\\nDefined at eval:1:1\""
    );
    assert_eq!(
        h.eval("(describe 'point)"),
        "\"point is a procedure built in: (point)\\nPosition of point in the current buffer.\""
    );
    assert_eq!(
        h.eval("(procedure-location add-hook!)"),
        "\"runtime/core/hooks.scm:21:1\""
    );
    assert_eq!(
        h.eval("(describe 'read-string)"),
        "\"read-string is a procedure: (read-string prompt . initial)\\nReads a line in the minibuffer and returns it. The optional argument is\\nthe text the line starts with.\\nDefined at runtime/core/prompts.scm:9:1\""
    );
    assert!(
        h.eval("(describe 'define-command)")
            .starts_with("\"define-command is a macro.\\n(define-command (name)"),
        "{}",
        h.eval("(describe 'define-command)")
    );
    // Every procedure of the runtime that is meant to be used says what it
    // does.
    assert_eq!(
        h.eval(
            "(filter (lambda (name)
                       (and (procedure? (global-ref name))
                            (not (documentation name))
                            (not (char=? #\\% (string-ref (symbol->string name) 0)))))
                     (global-names))"
        ),
        "()"
    );
    // What code writes goes to the message log.
    h.eval("(display \"written by display\")");
    assert!(
        h.eval("(buffer-string \"*Messages*\")")
            .contains("written by display")
    );
    // A new definition of something in the runtime is used at once.
    h.eval("(define (message . parts) (insert \"<\" (apply string-append parts) \">\"))");
    h.eval("(define-command (say) \"Say.\" (message \"hi\")) (say)");
    assert_eq!(h.text(), "<hi>");
}

#[test]
fn the_cursor_comes_with_the_colour_that_the_theme_gives_it() {
    use amacs_proto::{Color, DrawOp, InputEvent, Key};
    let mut h = Harness::new(40, 6);
    let cursor_color = |h: &mut Harness| {
        h.session.handle_event(InputEvent::Key(Key::char('x')));
        h.session
            .redisplay()
            .into_iter()
            .find_map(|op| match op {
                DrawOp::Cursor { color, .. } => Some(color),
                _ => None,
            })
            .expect("typing moves the cursor")
    };
    assert_eq!(cursor_color(&mut h), Some(Color::rgb(0xd0, 0x00, 0x00)));
    h.eval("(load-theme 'modus-vivendi)");
    assert_eq!(cursor_color(&mut h), Some(Color::rgb(0xff, 0xff, 0xff)));
}
