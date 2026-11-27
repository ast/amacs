//! What ran and how long it took: `runtime/lib/telemetry.scm` on top of the
//! counting that the core does.

mod common;

use common::Harness;

/// The count of an entry, or `#f` if nothing of that kind and name ran.
fn count(h: &mut Harness, kind: &str, name: &str, all: bool) -> String {
    h.eval(&format!(
        "(let ((found (filter (lambda (entry) (equal? (telemetry-entry-name entry) {name:?}))
                              (telemetry-of '{kind} {}))))
           (and (pair? found) (telemetry-entry-count (car found))))",
        if all { "#t" } else { "#f" }
    ))
}

#[test]
fn commands_hooks_and_events_are_counted_by_name() {
    let mut h = Harness::new(80, 10);
    h.eval(
        "(define-command (greet) \"Insert a greeting.\" (insert \"hello\"))
         (global-set-key \"C-c g\" 'greet)
         (add-hook! 'post-command-hook (lambda () #t))",
    );
    h.keys("a b C-c g C-a");
    // A command that is built in, one written in Scheme, a hook, and the
    // keys: a key that is a prefix is an event without a command.
    assert_eq!(count(&mut h, "command", "self-insert-command", false), "2");
    assert_eq!(count(&mut h, "command", "greet", false), "1");
    assert_eq!(
        count(&mut h, "command", "move-beginning-of-line", false),
        "1"
    );
    assert_eq!(count(&mut h, "hook", "post-command-hook", false), "4");
    assert_eq!(count(&mut h, "event", "key", false), "5");
    assert_eq!(count(&mut h, "load", "runtime", false), "1");
    assert_ne!(count(&mut h, "redisplay", "frame", false), "#f");
    assert_ne!(count(&mut h, "scheme", "run", false), "#f");
    // The times are of the right order: no command here takes a second.
    assert_eq!(
        h.eval(
            "(let ((entries (telemetry-of 'command)))
               (list (pair? entries)
                     (every (lambda (entry)
                              (and (<= 0 (telemetry-entry-mean entry) (telemetry-entry-max entry))
                                   (<= (telemetry-entry-max entry) (telemetry-entry-total entry))
                                   (< (telemetry-entry-total entry) 1000)))
                            entries)))"
        ),
        "(#t #t)"
    );

    let report = h.eval("(telemetry-report)");
    assert!(report.contains("command "), "{report}");
    assert!(report.contains("  greet "), "{report}");
    assert!(report.contains("total ms"), "{report}");

    h.keys("M-x s h o w - t e l e m e t r y RET");
    assert!(h.mode_line().contains("*Telemetry*"), "{}", h.mode_line());
    assert!(h.text().contains("self-insert-command"), "{}", h.text());
}

#[test]
fn the_option_stops_the_counting() {
    let mut h = Harness::new(80, 10);
    h.eval("(set-option! 'telemetry #f)");
    h.keys("C-f");
    assert_eq!(count(&mut h, "command", "forward-char", false), "#f");
    h.eval("(set-option! 'telemetry #t)");
    h.keys("C-f");
    assert_eq!(count(&mut h, "command", "forward-char", false), "1");
}

#[test]
fn sessions_are_summed_in_the_state_directory() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    for _ in 0..2 {
        let mut h = Harness::new(80, 10);
        h.session
            .editor_mut()
            .set_state_directory(Some(state.clone()));
        h.keys("C-f C-f");
        h.session.editor_mut().save_state();
    }
    let mut h = Harness::new(80, 10);
    h.session
        .editor_mut()
        .set_state_directory(Some(state.clone()));
    h.keys("C-f");
    assert_eq!(count(&mut h, "command", "forward-char", false), "1");
    assert_eq!(count(&mut h, "command", "forward-char", true), "5");

    h.keys("M-x c l e a r - t e l e m e t r y RET");
    assert_eq!(count(&mut h, "command", "forward-char", true), "#f");
    assert!(!state.join("telemetry").exists());
}
