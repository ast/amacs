//! Markup that is left out of what is drawn, in `hide-markup-mode`. The
//! rows are laid out in `crates/amacs-core/src/display.rs`; what is markup
//! is said by the language, for Org in `crates/amacs-treesit/src/org.rs`.

mod common;

use common::Harness;

const TEXT: &str = "* Head with ~code~\n\
Some *strong* and /stressed/ text, å =verbatim= and ~code~.\n\
See [[https://example.org/a/long/address][the site]] or [[*Head]].\n\
| a | *b* |\n\
#+begin_src sh\necho *not* this\n#+end_src\n\
_under_ and +struck+ stay, as a*b*c and 2 * 3 * 4 do.\n";

/// An editor with a buffer of Org that holds a text and hides markup,
/// point at the end of the text.
fn setup(width: u16, text: &str) -> Harness {
    let mut h = Harness::new(width, 16);
    h.eval(r#"(switch-to-buffer "notes.org") (org-mode)"#);
    h.paste(text);
    h.eval("(enable-mode! 'hide-markup-mode)");
    h
}

#[test]
fn markup_is_left_out_of_lines_that_point_is_not_on() {
    let mut h = setup(80, TEXT);
    assert_eq!(h.screen.row(0), "* Head with code");
    assert_eq!(
        h.screen.row(1),
        "Some strong and stressed text, å verbatim and code."
    );
    assert_eq!(h.screen.row(2), "See the site or *Head.");
    // What is hidden in a table would put its columns out of line, and a
    // block of code has no markup.
    assert_eq!(h.screen.row(3), "| a | *b* |");
    assert_eq!(h.screen.row(5), "echo *not* this");
    // Marks that mean nothing here are text.
    assert_eq!(
        h.screen.row(7),
        "_under_ and +struck+ stay, as a*b*c and 2 * 3 * 4 do."
    );
    // The text is as it was.
    assert_eq!(h.text(), TEXT);

    // What is shown has its colours, after chars that are left out and
    // chars of more than one byte too.
    assert_eq!(h.screen.face_at(0, 12), h.face("code"));
    assert_eq!(h.screen.face_at(1, 5), h.face("strong"));
    assert_eq!(h.screen.face_at(1, 16), h.face("emphasis"));
    assert_eq!(h.screen.face_at(1, 33), h.face("prose-verbatim"));
    assert_eq!(h.screen.face_at(1, 46), h.face("code"));
    assert_eq!(h.screen.face_at(1, 50), h.face("default"));
    assert_eq!(h.screen.face_at(2, 4), h.face("link"));
    assert_eq!(h.screen.face_at(2, 16), h.face("link"));
    assert_eq!(h.screen.face_at(2, 13), h.face("default"));

    h.eval("(disable-mode! 'hide-markup-mode)");
    assert_eq!(h.screen.row(0), "* Head with ~code~");
    assert_eq!(h.screen.row(2).len(), 66);
}

#[test]
fn the_line_that_point_is_on_is_shown_as_it_is_written() {
    let mut h = setup(80, TEXT);
    h.keys("M-< C-n");
    assert_eq!(h.screen.row(0), "* Head with code");
    assert_eq!(
        h.screen.row(1),
        "Some *strong* and /stressed/ text, å =verbatim= and ~code~."
    );
    // Point is where it is in the text as it is written.
    h.keys("C-e");
    assert_eq!(h.screen.cursor(), Some((1, 59)));
    h.keys("C-n");
    assert_eq!(
        h.screen.row(1),
        "Some strong and stressed text, å verbatim and code."
    );
    assert_eq!(
        h.screen.row(2),
        "See [[https://example.org/a/long/address][the site]] or [[*Head]]."
    );
    assert_eq!(h.screen.cursor(), Some((2, 59)));

    // Typing a mark shows at once what it does, in the next line too.
    h.keys("C-n C-n C-n C-n C-n C-a").type_text("*new* ");
    assert!(
        h.screen.row(7).starts_with("*new* _under_"),
        "{}",
        h.screen.row(7)
    );
    h.keys("C-p");
    assert!(
        h.screen.row(7).starts_with("new _under_"),
        "{}",
        h.screen.row(7)
    );
    assert_eq!(h.screen.face_at(7, 0), h.face("strong"));
}

#[test]
fn a_press_of_the_button_goes_to_the_char_that_is_shown_there() {
    let mut h = setup(80, TEXT);
    // "the site" starts at column 4 of what is shown, and after the address
    // in the text.
    h.press("<mouse-1>", 8, 2);
    let line_start: usize = h.eval("(line-beginning-position)").parse().unwrap();
    let point: usize = h.eval("(point)").parse().unwrap();
    assert_eq!(
        h.eval("(buffer-substring (point) (+ (point) 4))"),
        "\"site\""
    );
    assert_eq!(point - line_start, 46);
    // The line is now shown as it is written, with point on that char.
    assert_eq!(h.screen.cursor(), Some((2, 46)));
}

#[test]
fn rows_are_as_long_as_what_is_shown() {
    // The line is 66 chars as written and 22 as shown: in a narrow window
    // it is one row where point is not on it, and three where it is.
    let mut h = setup(
        30,
        "* Head\nSee [[https://example.org/a/long/address][the site]] or [[*Head]].\nend\n",
    );
    assert_eq!(h.screen.row(1), "See the site or *Head.");
    assert_eq!(h.screen.row(2), "end");
    h.keys("C-p C-p");
    assert_eq!(h.screen.row(1), "See [[https://example.org/a/l\\");
    assert_eq!(h.screen.row(4), "end");
    assert_eq!(h.screen.cursor(), Some((1, 0)));

    // Scrolling counts the rows that are shown: with point at the end of
    // many such lines, the window is full of them.
    let line = "See [[https://example.org/a/long/address][the site]] or [[*Head]].\n";
    let mut h = setup(30, &line.repeat(40));
    h.keys("M-< M->");
    for row in 0..13 {
        assert_eq!(h.screen.row(row), "See the site or *Head.", "row {row}");
    }
    assert_eq!(h.screen.cursor(), Some((13, 0)));
    h.keys("M-v");
    assert!(h.screen.cursor().is_some());
}

#[test]
fn a_row_breaks_before_a_word_that_starts_with_hidden_markup() {
    // Nineteen cells to a row: "some words go here " fills it, and the
    // next word starts with a mark that is left out.
    let mut h = setup(20, "* Head\nsome words go here *strong text* more\nend\n");
    h.eval("(enable-mode! 'visual-line-mode)");
    h.keys("M-<");
    assert_eq!(h.screen.row(1), "some words go here");
    assert_eq!(h.screen.row(2), "strong text more");
    assert_eq!(h.screen.face_at(2, 0), h.face("strong"));
    assert_eq!(h.screen.row(3), "end");
}
