//! Matrix in the editor, against a program that speaks to no server.

mod common;

use common::Harness;

/// An editor that is set up for a user and the program of the tests, with
/// a passphrase that needs no asking.
fn harness(user: &str) -> Harness {
    let mut h = Harness::new(70, 16);
    let program = amacs_testkit::example_path("fake-matrix");
    h.eval(&format!(
        "(begin (set-option! 'matrix-user-id {user:?})
                (set-option! 'matrix-program '({:?}))
                (set! %matrix-passphrase (lambda () \"secret\")))",
        program.display().to_string()
    ));
    h
}

fn start(h: &mut Harness) {
    h.keys("M-x").type_text("matrix").keys("RET");
    h.wait_until(|h| h.screen.text().contains("Tea"));
}

#[test]
fn the_rooms_are_listed_with_what_is_not_read() {
    let mut h = harness("@me:example.org");
    start(&mut h);
    assert_eq!(h.screen.row(0), "Matrix  @me:example.org");
    // The room last written in is first, and a space is no room.
    assert_eq!(h.screen.row(2), "  1@  Work");
    assert_eq!(h.screen.row(3), "      Tea");
    assert!(!h.screen.text().contains("A space"));
    assert!(h.mode_line().contains("Matrix"), "{}", h.mode_line());
}

#[test]
fn a_room_shows_its_messages_and_takes_new_ones() {
    let mut h = harness("@me:example.org");
    start(&mut h);
    h.keys("C-n C-n C-n RET");
    h.wait_until(|h| h.screen.text().contains("kettle on?"));
    assert!(h.mode_line().contains("*matrix: Tea*"), "{}", h.mode_line());
    let row = |h: &Harness, n: usize| h.screen.row(n)[6..].to_owned();
    assert_eq!(row(&h, 0), "Ada: Is the kettle on?");
    assert_eq!(row(&h, 1), "* Brook puts it on");
    assert!(
        row(&h, 2).starts_with("Ada (not decrypted"),
        "{}",
        row(&h, 2)
    );
    assert_eq!(row(&h, 3), "Brook: [image: kettle.jpg]");

    h.keys("RET");
    assert_eq!(h.prompt(), "To Tea:");
    h.type_text("It is now").keys("RET");
    h.wait_until(|h| h.screen.text().contains("Me: It is now"));
    assert_eq!(row(&h, 4), "Me: It is now");

    // An edit and a deletion that the server tells of.
    h.keys("RET").type_text("edit that").keys("RET");
    h.wait_until(|h| h.screen.text().contains("(edited)"));
    assert_eq!(row(&h, 0), "Ada: Is the kettle on yet? (edited)");
    assert!(!h.screen.text().contains("puts it on"));

    // Earlier messages come above, and then there are none.
    h.keys("p");
    h.wait_until(|h| h.screen.text().contains("Good morning"));
    assert_eq!(row(&h, 0), "Brook: Good morning");
    h.keys("p");
    h.wait_until(|h| h.echo().contains("no earlier messages"));

    // The list has the room first now.
    h.keys("l");
    h.wait_until(|h| h.screen.row(2).contains("Tea"));
}

#[test]
fn login_goes_through_the_browser() {
    let mut h = harness("@nobody:example.org");
    h.eval("(define opened #f) (set! browse-url (lambda (url) (set! opened url)))");
    h.keys("M-x").type_text("matrix").keys("RET");
    h.wait_until(|h| h.screen.text().contains("Not logged in"));
    h.keys("M-x").type_text("matrix-login").keys("RET");
    h.wait_until(|h| h.screen.text().contains("Tea"));
    assert_eq!(h.eval("opened"), "\"https://example.org/login\"");
}

#[test]
fn a_program_that_stops_is_said_to_have_stopped() {
    let mut h = harness("@me:example.org");
    start(&mut h);
    h.keys("M-x").type_text("matrix-quit").keys("RET");
    h.wait_until(|h| h.screen.text().contains("The Matrix program stopped"));
    // And it starts again.
    h.keys("g");
    h.wait_until(|h| !h.screen.text().contains("stopped") && h.screen.text().contains("Tea"));
}

/// Opens the room "Tea" and waits for its messages.
fn open_tea(h: &mut Harness) {
    start(h);
    h.keys("C-n C-n C-n RET");
    h.wait_until(|h| h.screen.text().contains("kettle on?"));
}

#[test]
fn messages_are_answered_changed_reacted_to_and_deleted() {
    let mut h = harness("@me:example.org");
    open_tea(&mut h);
    let row = |h: &Harness, n: usize| h.screen.row(n).to_owned();

    // An answer shows what it answers above it, in one line.
    h.keys("M-< r");
    assert_eq!(h.prompt(), "Answer to Ada:");
    h.type_text("Yes").keys("RET");
    h.wait_until(|h| h.screen.text().contains("Me: Yes"));
    assert_eq!(row(&h, 4), "      > Ada: Is the kettle on? …");
    assert!(row(&h, 5).ends_with("Me: Yes"), "{}", row(&h, 5));

    // A reaction is under its message.
    h.keys("M-< a").type_text("+1").keys("RET");
    h.wait_until(|h| h.screen.text().contains("+1 2"));
    assert_eq!(row(&h, 1), "      +1 2  tea");

    // Only a message of one's own is changed.
    h.keys("M-< e");
    assert!(h.echo().contains("only your own"), "{}", h.echo());
    h.keys("M-> C-p e");
    assert_eq!(h.prompt(), "Change to: Yes");
    h.type_text(", it is").keys("RET");
    h.wait_until(|h| h.screen.text().contains("Yes, it is (edited)"));

    // A message is deleted after a question.
    h.keys("M-< d");
    h.type_text("y");
    h.wait_until(|h| h.screen.text().contains("Ada (deleted)"));

    // Under the last message there is none.
    h.eval("(%replace-buffer-text! \"\") (hash-table-set! %matrix-item-starts (buffer-name) '())");
    h.keys("r");
    assert!(h.echo().contains("no message here"), "{}", h.echo());
}

#[test]
fn a_message_that_names_you_is_told_of() {
    let dir = tempfile::tempdir().unwrap();
    let told = dir.path().join("told");
    let mut h = harness("@me:example.org");
    h.eval(&format!(
        "(set-option! 'matrix-notify-program '(\"sh\" \"-c\" \"printf '%s|%s' \\\"$0\\\" \\\"$1\\\" > {}\"))",
        told.display()
    ));
    open_tea(&mut h);
    h.keys("RET").type_text("ping me").keys("RET");
    h.wait_until(|h| h.echo().contains("the tea is ready"));
    assert_eq!(h.echo(), "Ada in Tea: me: the tea is ready");
    h.wait_until(|_| told.exists() && !std::fs::read_to_string(&told).unwrap().is_empty());
    assert_eq!(
        std::fs::read_to_string(&told).unwrap(),
        "Ada in Tea|me: the tea is ready"
    );
}

#[test]
fn a_room_that_is_looked_at_is_read() {
    let mut h = harness("@me:example.org");
    start(&mut h);
    assert_eq!(h.screen.row(2), "  1@  Work");
    h.keys("C-n C-n RET");
    h.wait_until(|h| h.screen.text().contains("kettle on?"));
    h.keys("l");
    h.wait_until(|h| h.screen.row(2) == "      Work");
}
