//! The scripting tests on amacs's own Scheme.

const ENGINE: amacs_script::Engine = amacs_script::Engine::Own;

include!("shared/scripting.rs");

/// Only this engine keeps what a waiting command has promised to do on its
/// way out.
#[test]
fn quitting_a_prompt_runs_the_cleanup_of_the_waiting_command() {
    let mut h = Harness::new();
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
fn the_editor_can_be_asked_about_itself() {
    let mut h = Harness::new();
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
        "\"runtime/core/hooks.scm:19:1\""
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
