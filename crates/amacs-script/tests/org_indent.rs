//! Text of Org that is shown indented under its headings, in
//! `org-indent-mode`. The rows are laid out in
//! `crates/amacs-core/src/display.rs`.

mod common;

use common::Harness;

const TEXT: &str = "intro\n* One\nbody of one\n\n** Two\n- an item\n  more of it\n*** Three\n\tdeep\n* Four\nlast\n";

/// An editor with a buffer of Org that holds a text and indents it, point
/// at the start.
fn setup(width: u16, text: &str) -> Harness {
    let mut h = Harness::new(width, 16);
    h.eval(r#"(switch-to-buffer "notes.org") (org-mode)"#);
    h.paste(text);
    h.eval("(enable-mode! 'org-indent-mode)");
    h.keys("M-<");
    h
}

/// The rows of the window that hold text.
fn rows(h: &Harness) -> Vec<String> {
    (0..11).map(|row| h.screen.row(row)).collect()
}

#[test]
fn text_is_shown_indented_under_its_heading() {
    let mut h = setup(60, TEXT);
    assert_eq!(
        rows(&h),
        [
            "intro",
            "* One",
            "  body of one",
            "",
            " ** Two",
            "    - an item",
            "      more of it",
            "  *** Three",
            "              deep",
            "* Four",
            "  last",
        ]
    );
    // The text is as it was, and so are the columns of point in it.
    assert_eq!(h.text(), TEXT);
    h.keys("C-n C-n C-e");
    assert_eq!(h.screen.cursor(), Some((2, 13)));
    assert_eq!(h.eval("(current-column)"), "11");
    // Point on an empty line is where its text would start.
    h.keys("C-n");
    assert_eq!(h.screen.cursor(), Some((3, 2)));
    // The colours are those of the text, moved with it.
    assert_eq!(h.screen.face_at(4, 1), h.face("heading-2"));
    assert_eq!(h.screen.face_at(4, 0), h.face("default"));
    assert_eq!(h.screen.face_at(5, 4), h.face("delimiter"));

    h.eval("(disable-mode! 'org-indent-mode)");
    assert_eq!(h.screen.row(2), "body of one");
}

#[test]
fn the_indentation_follows_the_headings_as_they_change() {
    let mut h = setup(60, TEXT);
    h.keys("C-n M-<right>");
    assert_eq!(rows(&h)[1..4], [" ** One", "    body of one", ""]);
    h.keys("M-<left> C-n C-n C-n");
    // A heading that is typed takes what is under it.
    h.type_text("*");
    assert_eq!(h.screen.row(4), "  *** Two");
    assert_eq!(h.screen.row(5), "      - an item");
    // Folding goes with it.
    h.keys("TAB");
    assert_eq!(h.screen.row(4), "  *** Two…");
    assert_eq!(h.screen.row(5), "  *** Three");
}

#[test]
fn rows_wrap_and_the_pointer_goes_by_what_is_shown() {
    let mut h = setup(20, "* One\n** Two\nsome words that go on and on\nend\n");
    // Rows are 19 cells of which 4 are indentation.
    assert_eq!(h.screen.row(2), "    some words that\\");
    assert_eq!(h.screen.row(3), "     go on and on");
    assert_eq!(h.screen.row(4), "    end");
    // A press on a char goes to it; one on the indentation goes to the
    // start of the row.
    h.press("<mouse-1>", 9, 2);
    assert_eq!(
        h.eval("(buffer-substring (point) (+ (point) 5))"),
        "\"words\""
    );
    h.press("<mouse-1>", 1, 4);
    assert_eq!(
        h.eval("(buffer-substring (point) (+ (point) 3))"),
        "\"end\""
    );
    assert_eq!(h.screen.cursor(), Some((4, 4)));

    // With markup hidden as well, both are done.
    h.eval("(enable-mode! 'hide-markup-mode)");
    h.keys("M-<");
    h.eval("(begin (goto-char (point-max)) (insert \"a *b* c\\n\") (goto-char 0))");
    assert_eq!(h.screen.row(5), "    a b c");
}
