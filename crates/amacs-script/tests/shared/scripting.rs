// End-to-end tests of the Scheme integration: keys in, screen out, with
// commands, hooks and modes defined in Scheme. Included once for each Scheme
// engine, by a file that says which in `ENGINE`.

use std::time::{Duration, Instant};

use amacs_proto::{InputEvent, Key, parse_keys};
use amacs_script::Session;
use amacs_testkit::Screen;

struct Harness {
    session: Session,
    screen: Screen,
}

impl Harness {
    fn new() -> Self {
        let mut session = Session::with_engine(70, 8, ENGINE).unwrap_or_else(|e| panic!("{}", e.report));
        session
            .editor_mut()
            .set_default_directory("/nonexistent/amacs-test");
        let mut harness = Self {
            session,
            screen: Screen::default(),
        };
        harness.redisplay();
        harness
    }

    fn redisplay(&mut self) {
        // Wait for the matcher's worker threads, so that what a test sees
        // does not depend on timing.
        let start = Instant::now();
        loop {
            self.session.poll();
            if !self.session.editor().is_matching() || start.elapsed() > Duration::from_secs(5) {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let ops = self.session.redisplay();
        self.screen.apply(&ops);
    }

    fn keys(&mut self, keys: &str) -> &mut Self {
        for key in parse_keys(keys).unwrap() {
            self.session.handle_event(InputEvent::Key(key));
            self.redisplay();
        }
        self
    }

    fn type_text(&mut self, text: &str) -> &mut Self {
        for c in text.chars() {
            self.session.handle_event(InputEvent::Key(Key::char(c)));
        }
        self.redisplay();
        self
    }

    /// Evaluates Scheme that is expected to succeed.
    fn eval(&mut self, source: &str) -> String {
        let result = self
            .session
            .eval(source)
            .unwrap_or_else(|e| panic!("{}", e.report));
        self.redisplay();
        result
    }

    /// Inserts a tab and an `x`, to see where the next tab stop is.
    fn paste_tab(&mut self) {
        self.session
            .handle_event(InputEvent::Paste("\tx".to_owned()));
        self.redisplay();
    }

    fn echo(&self) -> String {
        self.screen.row(7)
    }

    fn mode_line(&self) -> String {
        self.screen.row(6)
    }

    fn text(&self) -> String {
        self.session.editor().text().to_string()
    }
}

/// Whether a text has the wording of either Scheme engine for something,
/// for as long as there are two.
fn either(text: &str, steel: &str, own: &str) -> bool {
    text.contains(steel) || text.contains(own)
}

#[test]
fn evaluates_expressions() {
    let mut h = Harness::new();
    assert_eq!(h.eval("(+ 1 2)"), "3");
    assert_eq!(h.eval("(buffer-name)"), "\"*scratch*\"");
    assert_eq!(h.eval("(define x 5)"), "");
    assert_eq!(h.eval("(list x 'sym \"str\")"), "(5 sym \"str\")");
}

#[test]
fn a_scheme_command_bound_to_a_key_is_one_undo_step() {
    let mut h = Harness::new();
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
    let mut h = Harness::new();
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
    let mut h = Harness::new();
    h.eval(
        r#"(define-command (show-arg)
             "Show the prefix argument."
             (message "arg " (prefix-arg) " given " (prefix-arg-given?)))
           (global-set-key "C-c a" 'show-arg)"#,
    );
    h.keys("C-c a");
    assert!(either(&h.echo(), "arg 1 given #false", "arg 1 given #f"), "{}", h.echo());
    h.keys("C-u 7 C-c a");
    assert!(either(&h.echo(), "arg 7 given #true", "arg 7 given #t"), "{}", h.echo());
}

#[test]
fn a_command_can_wait_for_minibuffer_input() {
    let mut h = Harness::new();
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
    let mut h = Harness::new();
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
    let mut h = Harness::new();
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
    let mut h = Harness::new();
    h.eval(
        r#"(define-command (broken)
             "Fail after editing."
             (insert "partial")
             (car 5))
           (global-set-key "C-c x" 'broken)"#,
    );
    h.keys("C-c x");
    assert!(
        either(&h.echo(), "car expected a list or pair", "car: expected a pair, got 5"),
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
    assert!(
        either(&log, "car expected a list or pair", "car: expected a pair, got 5"),
        "{log}"
    );
    h.type_text("ok");
    assert_eq!(h.text(), "ok");
}

#[test]
fn primitives_check_their_arguments() {
    let mut h = Harness::new();
    let arity = h.session.eval("(goto-char)").unwrap_err();
    assert!(
        either(
            &arity.summary,
            "goto-char takes 1 arguments, got 0",
            "goto-char: expected 1 argument, got 0"
        ),
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
    let mut h = Harness::new();
    h.keys("M-:").type_text("(* 6 7)").keys("RET");
    assert_eq!(h.echo(), "42");
    h.keys("M-:").type_text("(undefined-function)").keys("RET");
    assert!(
        either(&h.echo(), "FreeIdentifier", "unbound variable: undefined-function"),
        "{}",
        h.echo()
    );
}

#[test]
fn eval_buffer_defines_commands_from_the_buffer() {
    let mut h = Harness::new();
    h.session.handle_event(InputEvent::Paste(
        "(define-command (from-buffer) \"Say so.\" (message \"defined in a buffer\"))".to_owned(),
    ));
    h.keys("M-x").type_text("eval-buffer").keys("RET");
    h.keys("M-x").type_text("from-buffer").keys("RET");
    assert_eq!(h.echo(), "defined in a buffer");
}

#[test]
fn runaway_scheme_code_can_be_interrupted() {
    let mut h = Harness::new();
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
    assert!(either(&h.echo(), "Interrupted", "interrupted"), "{}", h.echo());

    assert_eq!(
        h.eval("(+ 1 1)"),
        "2",
        "the engine works after an interrupt"
    );
    h.type_text("still editing");
    assert_eq!(h.text(), "still editing");
}

#[test]
fn files_get_their_major_mode_from_their_name() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.scm");
    std::fs::write(&path, "(define\tx 1)\n").unwrap();

    let mut h = Harness::new();
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
    let mut h = Harness::new();
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
    let mut h = Harness::new();
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
    let mut h = Harness::new();
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
    let mut h = Harness::new();
    h.eval(r#"(define-minor-mode ruler-mode "Ruler" "Pretend to show a ruler.")"#);
    h.keys("M-x").type_text("ruler-mode").keys("RET");
    assert_eq!(h.echo(), "Ruler mode enabled");
    assert_eq!(h.mode_line(), " --  *scratch*   L1 C0  (Fundamental Ruler)");
    h.keys("M-x").type_text("ruler-mode").keys("RET");
    assert_eq!(h.mode_line(), " --  *scratch*   L1 C0  (Fundamental)");
}

#[test]
fn post_command_hook_runs_once_per_command() {
    let mut h = Harness::new();
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
    let mut h = Harness::new();
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
fn init_file_errors_are_reported_and_earlier_forms_take_effect() {
    let dir = tempfile::tempdir().unwrap();
    let init = dir.path().join("init.scm");
    std::fs::write(
        &init,
        "(global-set-key \"C-c i\" 'forward-char)\n(this-is-not-defined)\n",
    )
    .unwrap();

    let mut h = Harness::new();
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
    let mut h = Harness::new();
    h.session
        .load_file(std::path::Path::new(tests))
        .unwrap_or_else(|e| panic!("{}", e.report));
    let result = h.eval("(run-tests)");
    assert_eq!(result, "(27)", "failures are listed after the pass count");
    assert_eq!(h.mode_line(), " --  *scratch*   L1 C0  (Fundamental)");
}

#[test]
fn failing_scheme_tests_are_reported_by_name() {
    let mut h = Harness::new();
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
    let mut h = Harness::new();
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
    let mut h = Harness::new();
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
