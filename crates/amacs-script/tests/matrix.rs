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
    // The rooms of a space are under it, then the talks with one person,
    // then the other rooms, the one last written in first.
    assert_eq!(
        (0..11).map(|row| h.screen.row(row)).collect::<Vec<_>>(),
        [
            "Matrix  @me:example.org",
            "",
            "Garden club",
            "      Roses",
            "",
            "People",
            "      Ada",
            "",
            "Rooms",
            "  1@  Work",
            "      Tea",
        ]
    );
    assert!(h.mode_line().contains("Matrix"), "{}", h.mode_line());
}

#[test]
fn a_room_shows_its_messages_and_takes_new_ones() {
    let mut h = harness("@me:example.org");
    start(&mut h);
    // RET on the line of a room shows it.
    h.keys("M-> C-p RET");
    h.wait_until(|h| h.screen.text().contains("kettle on?"));
    assert!(h.mode_line().contains("*matrix: Tea*"), "{}", h.mode_line());
    let row = |h: &Harness, n: usize| h.screen.row(n)[6..].to_owned();
    assert_eq!(row(&h, 0), "Ada: Is the kettle on?");
    assert_eq!(row(&h, 1), "2 answers in a thread: t shows them");
    assert_eq!(row(&h, 2), "* Brook puts it on");
    assert!(
        row(&h, 3).starts_with("Ada (not decrypted"),
        "{}",
        row(&h, 3)
    );
    assert_eq!(row(&h, 4), "Brook: [image: kettle.jpg]");

    h.keys("RET");
    assert_eq!(h.prompt(), "To Tea:");
    h.type_text("It is now").keys("RET");
    h.wait_until(|h| h.screen.text().contains("Me: It is now"));
    assert_eq!(row(&h, 5), "Me: It is now");

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

    // The list has the room first among the rooms now.
    h.keys("l");
    h.wait_until(|h| h.screen.row(9).contains("Tea"));
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
    h.keys("s").type_text("Tea").keys("RET");
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
    assert_eq!(row(&h, 5), "      > Ada: Is the kettle on? …");
    assert!(row(&h, 6).ends_with("Me: Yes"), "{}", row(&h, 6));

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
    assert_eq!(h.screen.row(9), "  1@  Work");
    h.keys("s").type_text("Work").keys("RET");
    h.wait_until(|h| h.screen.text().contains("kettle on?"));
    h.keys("l");
    h.wait_until(|h| h.screen.row(9) == "      Work");
}

/// Writes a picture of 16 by 32 pixels.
fn picture(path: &std::path::Path) {
    let file = std::fs::File::create(path).unwrap();
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), 16, 32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .and_then(|mut writer| writer.write_image_data(&[90; 16 * 32 * 3]))
        .unwrap();
}

#[test]
fn a_picture_is_shown_under_its_message_and_files_are_opened_and_sent() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("kettle.png");
    picture(&file);
    let mut h = harness("@me:example.org");
    h.session.handle_event(amacs_proto::InputEvent::Resize {
        cols: 70,
        rows: 16,
        cell: Some((8, 16)),
    });
    h.eval(&format!(
        "(begin (set-option! 'matrix-program '(\"env\" {:?} {:?}))
                (define opened #f) (set! browse-url (lambda (url) (set! opened url))))",
        format!("FAKE_MATRIX_PICTURE={}", file.display()),
        amacs_testkit::example_path("fake-matrix")
            .display()
            .to_string()
    ));
    open_tea(&mut h);
    // Two rows of picture under the message that it is of.
    h.wait_until(|h| !h.screen.bands(5).is_empty());
    assert!(h.screen.row(4).ends_with("Brook: [image: kettle.jpg]"));
    assert_eq!(h.screen.bands(5)[0].bands, 2);
    assert!(h.screen.bands(4).is_empty() && h.screen.bands(7).is_empty());

    // o on it opens the file with the desktop's program.
    h.keys("M-> C-p o");
    h.wait_until(|h| h.eval("opened") != "#f");
    assert_eq!(
        h.eval("opened"),
        format!("{:?}", file.display().to_string())
    );
    // A message without a file has nothing to open.
    h.keys("M-< o");
    assert!(h.echo().contains("no file"), "{}", h.echo());

    // u sends a file.
    h.keys("u");
    assert!(h.prompt().contains("Send file:"), "{}", h.prompt());
    h.keys("C-a C-k")
        .type_text(&file.display().to_string())
        .keys("RET");
    h.wait_until(|h| h.screen.text().contains("Me: [file: kettle.png]"));
}

#[test]
fn a_thread_has_a_buffer_of_its_own() {
    let mut h = harness("@me:example.org");
    open_tea(&mut h);
    h.keys("M-< t");
    h.wait_until(|h| h.screen.text().contains("Which kettle?"));
    assert!(
        h.mode_line().contains("*matrix: Tea / thread $1*"),
        "{}",
        h.mode_line()
    );
    // In the thread, the message that it starts with says nothing of it.
    assert!(!h.screen.text().contains("answers in a thread"));
    h.keys("RET");
    assert_eq!(h.prompt(), "To Tea, in the thread:");
    h.type_text("The blue one").keys("RET");
    h.wait_until(|h| h.screen.text().contains("Me: The blue one"));
    // The room itself did not get the answer.
    h.keys("k");
    h.eval("(switch-to-buffer \"*matrix: Tea*\")");
    h.redisplay();
    assert!(
        !h.screen.text().contains("The blue one"),
        "{}",
        h.screen.text()
    );
}

#[test]
fn a_new_device_is_verified_by_emojis_or_by_the_recovery_key() {
    let mut h = harness("@new:example.org");
    start(&mut h);
    assert!(
        h.screen.row(1).starts_with("This device is not verified."),
        "{}",
        h.screen.row(1)
    );
    h.keys("M-x").type_text("matrix-verify").keys("RET");
    h.wait_until(|h| h.prompt().contains("D Dog  K Key"));
    h.type_text("n");
    h.wait_until(|h| h.echo().contains("the emojis are not the same"));
    assert!(h.screen.row(1).starts_with("This device is not verified."));
    h.keys("M-x").type_text("matrix-verify").keys("RET");
    h.wait_until(|h| h.prompt().contains("D Dog  K Key"));
    h.type_text("y");
    h.wait_until(|h| h.screen.row(1).is_empty());

    // Or by the key, which is not shown as it is typed.
    let mut h = harness("@new:example.org");
    start(&mut h);
    h.keys("M-x").type_text("matrix-recover").keys("RET");
    h.type_text("wrong");
    assert!(!h.screen.text().contains("wrong"));
    h.keys("RET");
    h.wait_until(|h| h.echo().contains("not correct"));
    h.keys("M-x").type_text("matrix-recover").keys("RET");
    h.type_text("EsTc 1234").keys("RET");
    h.wait_until(|h| h.screen.row(1).is_empty());
}

#[test]
fn a_message_of_more_lines_is_written_in_a_buffer() {
    let mut h = harness("@me:example.org");
    open_tea(&mut h);
    h.keys("c");
    assert!(
        h.mode_line().contains("*matrix compose: Tea*"),
        "{}",
        h.mode_line()
    );
    h.type_text("First line")
        .keys("RET")
        .type_text("second line");
    h.keys("C-c C-c");
    h.wait_until(|h| h.screen.text().contains("Me: First line"));
    assert!(h.mode_line().contains("*matrix: Tea*"), "{}", h.mode_line());
    assert_eq!(h.screen.row(6), "second line");

    // A message that is given up is not sent, and an empty one is not either.
    h.keys("c").type_text("never mind").keys("C-c C-k");
    assert!(h.mode_line().contains("*matrix: Tea*"), "{}", h.mode_line());
    h.keys("c C-c C-c");
    assert!(h.echo().contains("empty"), "{}", h.echo());
    h.keys("C-c C-k");
    assert!(!h.screen.text().contains("never mind"));

    // One's own message is changed with its text to start from.
    h.keys("M-> C-p C-p E");
    assert!(h.screen.text().contains("First line\nsecond line"));
    h.keys("M->").type_text(", changed").keys("C-c C-c");
    h.wait_until(|h| h.screen.text().contains("second line, changed (edited)"));
}

#[test]
fn what_comes_while_a_message_is_typed_does_not_touch_it() {
    let mut h = harness("@me:example.org");
    open_tea(&mut h);
    h.keys("RET").type_text("half");
    // Everything is drawn again, as when a message comes.
    h.eval("(%matrix-redraw!)");
    h.redisplay();
    assert_eq!(h.prompt(), "To Tea: half");
    h.type_text(" and whole").keys("RET");
    h.wait_until(|h| h.screen.text().contains("Me: half and whole"));
}

#[test]
fn the_passphrase_is_the_secret_of_the_address_or_is_asked_for() {
    let mut h = Harness::new(70, 16);
    let program = amacs_testkit::example_path("fake-matrix");
    // A source of secrets that has one for the host of the option, and
    // remembers what it was asked.
    h.eval(&format!(
        "(begin (set-option! 'matrix-user-id \"@me:example.org\")
                (set-option! 'matrix-program '({:?}))
                (define asked #f)
                (define-secret-source 'test
                  (lambda (host user port)
                    (set! asked (list host user))
                    (if (equal? host \"amacs-matrix\")
                        (list (list \"entry\" host user (lambda () \"from the store\")))
                        '())))
                (set-option! 'secret-sources '(test)))",
        program.display().to_string()
    ));
    start(&mut h);
    assert_eq!(h.eval("asked"), "(\"amacs-matrix\" \"@me:example.org\")");
    assert_eq!(h.prompt(), "");
    // The password store has such a secret under either of these names.
    for entry in [
        "amacs-matrix/@me:example.org",
        "amacs-matrix",
        "chat/amacs-matrix",
    ] {
        let found = h.eval(&format!(
            "(and (%pass-match {entry:?} \"amacs-matrix\" \"@me:example.org\" #f) #t)"
        ));
        assert_eq!(found, "#t", "{entry}");
    }
    assert_eq!(
        h.eval("(%pass-match \"amacs-matrix/@other:example.org\" \"amacs-matrix\" \"@me:example.org\" #f)"),
        "#f"
    );

    // Without a secret, the passphrase is typed, and is not shown.
    h.keys("M-x").type_text("matrix-quit").keys("RET");
    h.wait_until(|h| h.screen.text().contains("The Matrix program stopped"));
    h.eval("(set-option! 'matrix-store-secret \"elsewhere\")");
    h.keys("g");
    assert_eq!(h.prompt(), "Passphrase of the Matrix store:");
    h.type_text("typed");
    assert!(!h.screen.text().contains("typed"));
    h.keys("RET");
    h.wait_until(|h| !h.screen.text().contains("stopped") && h.screen.text().contains("Tea"));
}
