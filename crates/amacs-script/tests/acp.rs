//! The agent workflow: the compose buffer, prompt history, code references
//! and the window layout. The connection and sending are stubbed, so no
//! real agent runs; the protocol client is tried by hand against the real
//! adapter.

mod common;

use common::Harness;

/// A harness with Claude's connection and sending stubbed out: it looks
/// connected, and sent prompts are recorded in `sent` instead of going to a
/// process.
fn connected() -> Harness {
    let mut h = Harness::new(80, 24);
    h.eval(
        r#"(set! %acp-connected? (lambda () #t))
           (set! %acp-session "stub")
           (define sent '())
           (set! %acp-send-prompt (lambda (text) (set! sent (cons text sent))))"#,
    );
    h
}

#[test]
fn starting_lays_out_the_conversation_and_a_compose_buffer() {
    let mut h = connected();
    h.keys("C-c c c");
    // Two windows: the conversation above, the compose buffer below.
    assert_eq!(h.eval("(length (window-list))"), "2");
    assert_eq!(h.eval("(buffer-name)"), "\"*claude prompt*\"");
    // The compose buffer is the smaller one.
    let compose_h: usize = h.eval("(window-height)").parse().unwrap();
    assert!(compose_h <= 10, "compose window is {compose_h} rows");
    // Its minor mode is on.
    assert!(
        h.eval("(minor-mode-enabled? 'claude-compose-mode)")
            .starts_with("#t")
    );
}

#[test]
fn a_prompt_is_written_and_sent_with_c_c_c_c() {
    let mut h = connected();
    h.keys("C-c c c");
    h.type_text("first line\nsecond line");
    h.keys("C-c C-c");
    // It was sent and the buffer cleared for the next.
    assert_eq!(h.eval("sent"), "(\"first line\\nsecond line\")");
    assert_eq!(h.eval("(buffer-string)"), "\"\"");
    // Another prompt.
    h.type_text("again").keys("C-c C-c");
    assert_eq!(h.eval("(car sent)"), "\"again\"");
    assert_eq!(h.eval("(length sent)"), "2");
}

#[test]
fn an_empty_prompt_is_refused() {
    let mut h = connected();
    h.keys("C-c c c");
    h.keys("C-c C-c");
    assert!(h.echo().contains("empty"), "{}", h.echo());
    assert_eq!(h.eval("sent"), "()");
}

#[test]
fn earlier_prompts_come_back_with_m_p_and_m_n() {
    let mut h = connected();
    h.keys("C-c c c");
    h.type_text("one").keys("C-c C-c");
    h.type_text("two").keys("C-c C-c");
    // M-p walks back from the newest.
    h.keys("M-p");
    assert_eq!(h.eval("(buffer-string)"), "\"two\"");
    h.keys("M-p");
    assert_eq!(h.eval("(buffer-string)"), "\"one\"");
    // M-n walks forward, then empties past the end.
    h.keys("M-n");
    assert_eq!(h.eval("(buffer-string)"), "\"two\"");
    h.keys("M-n");
    assert_eq!(h.eval("(buffer-string)"), "\"\"");
}

#[test]
fn a_reference_to_the_region_is_added_from_a_file_buffer() {
    let mut h = Harness::new(80, 24);
    h.eval(
        r#"(set! %acp-connected? (lambda () #t))
           (set! %acp-session "stub")
           (set! %acp-send-prompt (lambda (text) #f))
           (find-file "/nonexistent/amacs-test/sub/code.rs")
           (insert "line1\nline2\nline3\nline4\n")"#,
    );
    // In the code buffer, select lines 2 to 3, then add the reference.
    h.keys("M-< C-n C-SPC C-n C-e");
    h.keys("C-c c r");
    assert!(h.echo().contains("@code.rs#L2-3"), "{}", h.echo());
    assert_eq!(
        h.eval(r#"(buffer-string "*claude prompt*")"#),
        r#""@code.rs#L2-3 ""#
    );
}
