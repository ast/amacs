//! Processes that go on running: `make-process`, filters, sentinels, input
//! and signals, with real programs.

mod common;

use common::Harness;

/// A buffer that collects what a filter and a sentinel are given.
fn setup() -> Harness {
    let mut h = Harness::new(70, 10);
    h.eval(
        r#"(define log "")
           (define ended 'not-yet)
           (define (note! process text) (set! log (string-append log text)))
           (define (end! process status) (set! ended status))"#,
    );
    h
}

#[test]
fn what_a_program_writes_goes_to_its_filter_and_its_end_to_its_sentinel() {
    let mut h = setup();
    h.eval(r#"(define p (make-process "cat" '("cat") 'filter note! 'sentinel end!))"#);
    assert_eq!(h.eval("(process-live? p)"), "#t");
    assert_eq!(h.eval("(process-exit-status p)"), "running");
    assert_eq!(h.eval("(process-name p)"), "\"cat\"");
    h.eval(r#"(process-send-string p "hello ")"#);
    h.eval(r#"(process-send-string p "world\n")"#);
    h.wait_until(|h| h.eval("log") == "\"hello world\\n\"");
    // Typing is not held while the program runs.
    h.type_text("typed");
    assert_eq!(h.text(), "typed");
    assert_eq!(h.eval("ended"), "not-yet");
    h.eval("(process-send-eof p)");
    h.wait_until(|h| h.eval("ended") == "0");
    assert_eq!(h.eval("(process-live? p)"), "#f");
    assert_eq!(h.eval("(process-exit-status p)"), "0");
    // The sentinel came after the last of the output.
    assert_eq!(h.eval("log"), "\"hello world\\n\"");
}

#[test]
fn the_exit_status_and_standard_error_are_reported() {
    let mut h = setup();
    h.eval(
        r#"(define p (make-process "failing" '("sh" "-c" "echo oops >&2; exit 3")
                                  'filter note! 'sentinel end!))"#,
    );
    h.wait_until(|h| h.eval("ended") == "3");
    assert_eq!(h.eval("log"), "\"oops\\n\"");
}

#[test]
fn a_character_cut_between_two_writes_arrives_whole() {
    let mut h = setup();
    h.eval(
        r#"(define p (make-process "cut" '("sh" "-c" "printf 'a\\303'; sleep 0.2; printf '\\251b'")
                                  'filter note! 'sentinel end!))"#,
    );
    h.wait_until(|h| h.eval("ended") == "0");
    assert_eq!(h.eval("log"), "\"a\u{e9}b\"");
}

#[test]
fn a_signal_ends_a_program_and_the_sentinel_says_so() {
    let mut h = setup();
    h.eval(r#"(define p (make-process "sleeper" '("sleep" "30") 'sentinel end!))"#);
    assert_eq!(h.eval("(process-live? p)"), "#t");
    h.eval("(signal-process p 'term)");
    h.wait_until(|h| h.eval("ended") == "#f");
    assert_eq!(h.eval("(process-exit-status p)"), "#f");
    assert!(h.session.eval("(process-send-string p \"x\")").is_err());
}

#[test]
fn deleting_a_process_ends_it_quietly() {
    let mut h = setup();
    h.eval(r#"(define p (make-process "sleeper" '("sleep" "30") 'sentinel end!))"#);
    assert_ne!(h.eval("(memv p (process-list))"), "#f");
    h.eval("(delete-process p)");
    assert_eq!(h.eval("(memv p (process-list))"), "#f");
    assert!(h.session.eval("(process-live? p)").is_err());
    assert_eq!(h.eval("ended"), "not-yet");
}

#[test]
fn a_program_that_cannot_be_started_is_an_error() {
    let mut h = setup();
    let error = h
        .session
        .eval(r#"(make-process "nothing" '("/nonexistent/program"))"#)
        .unwrap_err();
    assert!(
        error.summary.contains("cannot run /nonexistent/program"),
        "{}",
        error.summary
    );
}

#[test]
fn a_process_with_a_buffer_writes_into_it_and_dies_with_it() {
    let mut h = setup();
    h.eval(
        r#"(switch-to-buffer "*out*")
           (define p (make-process "echo" '("sh" "-c" "echo one; sleep 30") 'buffer "*out*" 'sentinel end!))"#,
    );
    h.wait_until(|h| h.text() == "one\n");
    assert_eq!(h.eval("(process-buffer p)"), "\"*out*\"");
    assert_eq!(h.eval("(get-buffer-process \"*out*\")"), h.eval("p"));
    h.eval(r#"(kill-buffer "*out*")"#);
    h.wait_until(|h| h.eval("(memv p (process-list))") == "#f");
    assert_eq!(h.eval("(get-buffer-process \"*out*\")"), "#f");
}

#[test]
fn the_process_list_shows_them() {
    let mut h = setup();
    h.eval(r#"(define p (make-process "sleeper" '("sleep" "30") 'buffer "*s*"))"#);
    h.keys("M-x").type_text("list-processes").keys("RET");
    assert!(h.mode_line().contains("Processes"), "{}", h.mode_line());
    let row = h.screen.row(1);
    assert!(row.starts_with("sleeper"), "{row}");
    assert!(row.contains("run"), "{row}");
    assert!(row.contains("*s*"), "{row}");
    assert!(row.contains("sleep"), "{row}");
    h.keys("k");
    h.wait_until(|h| h.eval("(process-live? p)") == "#f");
    h.keys("g");
    assert!(h.screen.row(1).contains("signal"), "{}", h.screen.row(1));
    h.keys("d");
    assert_eq!(h.eval("(process-list)"), "()");
    h.keys("q");
    assert_ne!(h.eval("(buffer-name)"), "\"*Process List*\"");
}
