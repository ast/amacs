//! Keymaps with a default binding, the key that ran a command, pasting
//! through a binding, and the sizes of windows.

mod common;

use common::Harness;

/// A mode whose default binding writes down each key it gets.
fn setup() -> Harness {
    let mut h = Harness::new(60, 10);
    h.eval(
        r#"(define got '())
           (define-command (note-key)
             "Write down the key that ran this."
             (set! got (append got (list (last-key)))))
           (define-command (note-paste)
             "Write down what was pasted."
             (set! got (append got (list (list 'paste (last-paste))))))
           (define-major-mode catch-mode #f "Catch" "Catches keys.")
           (define-key 'catch-mode #t 'note-key)
           (define-key 'catch-mode "<paste>" 'note-paste)
           (define-key 'catch-mode "C-c C-c" 'note-key)
           (switch-to-buffer "*catch*")
           (catch-mode)"#,
    );
    h
}

#[test]
fn a_default_binding_gets_every_key_that_nothing_binds() {
    let mut h = setup();
    // Keys that the global map binds, such as C-d and <up>, run their
    // commands as anywhere; the default gets the rest.
    h.type_text("ab").keys("<f7> C-M-7 M-q s-q");
    assert_eq!(h.text(), "");
    assert_eq!(h.eval("got"), r#"("a" "b" "<f7>" "C-M-7" "M-q" "s-q")"#);
    h.keys("C-d");
    assert_eq!(h.eval("got"), r#"("a" "b" "<f7>" "C-M-7" "M-q" "s-q")"#);
}

#[test]
fn bound_keys_and_prefixes_still_work() {
    let mut h = setup();
    h.keys("M-x").type_text("note-key").keys("RET");
    assert_eq!(h.eval("got"), r#"("RET")"#);
    h.keys("C-c C-c");
    assert_eq!(h.eval("got"), r#"("RET" "C-c")"#);
    // C-x is a prefix in the global map, so it is not caught, and what it
    // leads to runs.
    h.keys("C-x b");
    assert!(h.prompt().contains("Switch to buffer"), "{}", h.prompt());
    h.keys("C-g");
    assert_eq!(h.eval("got"), r#"("RET" "C-c")"#);
}

#[test]
fn a_paste_goes_to_its_binding_instead_of_the_buffer() {
    let mut h = setup();
    h.paste("hello\nworld");
    assert_eq!(h.text(), "");
    assert_eq!(h.eval("got"), r#"((paste "hello\nworld"))"#);
    // Elsewhere a paste is inserted as before.
    h.keys("C-x b").type_text("*scratch*").keys("RET");
    h.paste("plain");
    assert_eq!(h.text(), "plain");
}

#[test]
fn removing_the_default_restores_ordinary_typing() {
    let mut h = setup();
    h.eval("(undefine-key 'catch-mode #t)");
    h.type_text("ab");
    assert_eq!(h.text(), "ab");
    assert_eq!(h.eval("got"), "()");
}

#[test]
fn windows_know_their_size_and_say_when_it_changes() {
    let mut h = Harness::new(60, 12);
    h.eval(
        r#"(define changes 0)
           (add-hook! 'window-size-change-hook (lambda () (set! changes (+ changes 1))))"#,
    );
    // The frame is 60 by 12: a mode line and an echo area leave 10 rows.
    assert_eq!(h.eval("(window-width)"), "60");
    assert_eq!(h.eval("(window-height)"), "10");
    assert_eq!(h.eval("changes"), "0");
    h.keys("C-x 2");
    h.wait_until(|h| h.eval("changes") != "0");
    let height: usize = h.eval("(window-height)").parse().unwrap();
    assert!(height < 10, "{height}");
    assert_eq!(h.eval("(window-width)"), "60");
    let seen = h.eval("changes");
    h.keys("C-x 3");
    h.wait_until(|h| h.eval("changes") != seen);
    let width: usize = h.eval("(window-width)").parse().unwrap();
    assert!(width < 60, "{width}");
    // Line numbers take columns from the text.
    let seen = h.eval("changes");
    h.eval("(set-local! 'line-numbers 'absolute)");
    h.wait_until(|h| h.eval("changes") != seen);
    let narrower: usize = h.eval("(window-width)").parse().unwrap();
    assert!(narrower < width, "{narrower} < {width}");
    // A size given explicitly is that of the window asked about.
    assert_eq!(
        h.eval("(window-height (car (window-list)))"),
        h.eval("(window-height)")
    );
}
