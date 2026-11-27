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
