//! Lines that are hidden: what code can hide and show, how the hidden lines
//! are drawn, and how point and motion keep out of them. The lines are
//! kept in `crates/amacs-core/src/fold.rs`.

mod common;

use common::Harness;

/// An editor that shows ten lines, `line 0` to `line 9`, point at the
/// start.
fn setup() -> Harness {
    let mut h = Harness::new(40, 14);
    let text: String = (0..10).map(|n| format!("line {n}\n")).collect();
    h.paste(&text);
    h.keys("M-<");
    h
}

/// The start of a line, counted from 0, as Scheme.
fn line(n: usize) -> usize {
    n * 7
}

/// Hides lines `first` to `last`.
fn hide(h: &mut Harness, first: usize, last: usize) {
    h.eval(&format!("(hide-lines! {} {})", line(first), line(last + 1)));
}

/// The rows of the window that hold text.
fn rows(h: &Harness) -> Vec<String> {
    (0..12)
        .map(|row| h.screen.row(row))
        .take_while(|row| !row.is_empty())
        .collect()
}

#[test]
fn hidden_lines_are_not_drawn_and_the_line_before_ends_in_a_mark() {
    let mut h = setup();
    hide(&mut h, 2, 4);
    assert_eq!(
        rows(&h),
        [
            "line 0",
            "line 1…",
            "line 5",
            "line 6",
            "line 7",
            "line 8",
            "line 9"
        ]
    );
    assert_eq!(h.screen.face_at(1, 6), h.face("fold"));
    assert_eq!(h.screen.face_at(1, 5), h.face("default"));
    assert_eq!(h.eval("(hidden-ranges)"), "((14 35))");
    assert_eq!(h.eval("(map hidden? '(13 14 34 35))"), "(#f #t #t #f)");
    assert_eq!(h.text().lines().count(), 10, "the text is as it was");
    assert_eq!(h.eval("(buffer-modified?)"), "#t");

    h.eval("(show-lines!)");
    assert_eq!(rows(&h).len(), 10);
    assert_eq!(h.eval("(hidden-ranges)"), "()");

    // The first line is never hidden, and to the end of the text nothing
    // is left to show after the mark.
    h.eval("(hide-lines! 0 70)");
    assert_eq!(rows(&h), ["line 0…"]);
}

#[test]
fn hidden_lines_within_hidden_lines_stay_so_when_the_outer_are_shown() {
    let mut h = setup();
    hide(&mut h, 3, 4);
    hide(&mut h, 1, 6);
    assert_eq!(rows(&h), ["line 0…", "line 7", "line 8", "line 9"]);
    assert_eq!(h.eval("(hidden-ranges)"), "((7 49) (21 35))");
    h.eval("(show-lines! 7 8)");
    assert_eq!(
        rows(&h),
        [
            "line 0",
            "line 1",
            "line 2…",
            "line 5",
            "line 6",
            "line 7",
            "line 8",
            "line 9"
        ]
    );
    // What hidden-ranges gives is what hide-lines! takes.
    h.eval("(show-lines!) (hide-lines! 21 35)");
    assert_eq!(h.eval("(hidden-ranges)"), "((21 35))");
    // Showing a range shows what starts in it.
    hide(&mut h, 7, 8);
    h.eval("(show-lines! 0 40)");
    assert_eq!(h.eval("(hidden-ranges)"), "((49 63))");
}

#[test]
fn hidden_lines_stay_hidden_as_the_text_around_them_changes() {
    let mut h = setup();
    hide(&mut h, 2, 4);
    h.type_text("new\n");
    assert_eq!(rows(&h)[..4], ["new", "line 0", "line 1…", "line 5"]);
    // At the end of the line before them, typing stays in view.
    h.keys("C-n C-e").type_text("!");
    assert_eq!(rows(&h)[2..4], ["line 1!…", "line 5"]);
    // Undo leaves them hidden too.
    h.keys("C-/ C-/");
    assert_eq!(rows(&h)[..3], ["line 0", "line 1…", "line 5"]);
    assert_eq!(h.eval("(hidden-ranges)"), "((14 35))");
}

#[test]
fn killing_the_line_end_before_hidden_lines_shows_them() {
    let mut h = setup();
    hide(&mut h, 2, 4);
    h.keys("C-n C-e C-d");
    assert_eq!(rows(&h)[..3], ["line 0", "line 1line 2", "line 3"]);
    assert_eq!(h.eval("(hidden-ranges)"), "()");

    // And so does joining the line after them to them.
    let mut h = setup();
    hide(&mut h, 2, 4);
    h.keys("C-n C-n DEL");
    assert_eq!(
        rows(&h)[..5],
        ["line 0", "line 1", "line 2", "line 3", "line 4line 5"]
    );
}

#[test]
fn motion_steps_over_hidden_lines() {
    let mut h = setup();
    hide(&mut h, 2, 4);
    let at = |h: &mut Harness| h.screen.cursor();
    h.keys("C-f C-f C-n");
    assert_eq!(at(&mut h), Some((1, 2)));
    h.keys("C-n");
    assert_eq!(at(&mut h), Some((2, 2)), "the column is kept");
    assert_eq!(h.eval("(line-number-at-pos)"), "6");
    h.keys("C-p");
    assert_eq!(at(&mut h), Some((1, 2)));

    // The end of the line before them is a place for point; one char on is
    // the line after them.
    h.keys("C-e");
    assert_eq!(at(&mut h), Some((1, 6)));
    assert_eq!(h.eval("(point)"), "13");
    h.keys("C-f");
    assert_eq!(at(&mut h), Some((2, 0)));
    h.keys("C-b");
    assert_eq!(at(&mut h), Some((1, 6)));
    // A word that is hidden is stepped over with its lines.
    h.keys("M-f");
    assert_eq!(at(&mut h), Some((2, 0)));
    h.keys("M-b M-b");
    assert_eq!(at(&mut h), Some((1, 5)));
    assert_eq!(h.eval("(hidden-ranges)"), "((14 35))", "nothing was shown");

    // With all lines to the end hidden, the end of the buffer is the end
    // of the line before them.
    h.eval("(show-lines!) (delete-region 69 70) (hide-lines! 49 69)");
    h.keys("M->");
    assert_eq!(h.eval("(point)"), "48");
    h.keys("C-f");
    assert_eq!(h.echo(), "End of buffer");
    assert_eq!(h.eval("(point)"), "48");
    h.keys("C-n");
    assert_eq!(h.echo(), "End of buffer");
    assert_eq!(h.eval("(hidden-ranges)"), "((49 69))");
}

#[test]
fn a_jump_into_hidden_lines_shows_them() {
    let mut h = setup();
    hide(&mut h, 3, 3);
    hide(&mut h, 6, 7);
    hide(&mut h, 2, 8);
    assert_eq!(rows(&h), ["line 0", "line 1…", "line 9"]);
    h.eval("(goto-line 5)");
    // The lines around line 5 are shown; those hidden beside it stay.
    assert_eq!(
        rows(&h),
        [
            "line 0",
            "line 1",
            "line 2…",
            "line 4",
            "line 5…",
            "line 8",
            "line 9"
        ]
    );
    assert_eq!(h.screen.cursor(), Some((3, 0)));
    h.eval("(goto-char 45)");
    assert_eq!(h.eval("(hidden-ranges)"), "((21 28))");

    // reveal! does the same for a position that point is not at.
    h.eval("(reveal! 22)");
    assert_eq!(h.eval("(hidden-ranges)"), "()");
}

#[test]
fn hiding_the_lines_that_point_is_on_moves_point_before_them() {
    let mut h = setup();
    h.keys("C-x 2");
    h.eval("(goto-line 4)");
    h.keys("C-x o");
    h.eval("(goto-line 5)");
    hide(&mut h, 2, 6);
    assert_eq!(h.eval("(point)"), "13");
    assert_eq!(h.eval("(hidden-ranges)"), "((14 49))");
    // The other window shows the same lines, with its point moved too.
    h.keys("C-x o");
    assert_eq!(h.eval("(point)"), "13");
    assert_eq!(h.eval("(hidden-ranges)"), "((14 49))");
}

#[test]
fn rows_are_counted_without_hidden_lines() {
    let mut h = Harness::new(40, 8);
    let text: String = (0..40).map(|n| format!("line {n}\n")).collect();
    h.paste(&text);
    h.keys("M-<");
    h.eval("(set-option! 'line-numbers 'relative)");
    // Lines 2 to 20 are hidden: the window shows line 21 on its third row.
    h.eval(&format!("(hide-lines! 14 {})", 7 * 10 + 8 * 11));
    assert_eq!(h.screen.row(1).trim_start(), "1 line 1…");
    assert_eq!(h.screen.row(2).trim_start(), "2 line 21");
    // A page down goes by rows that are shown.
    h.keys("C-v");
    assert!(h.screen.row(0).ends_with("line 23"), "{}", h.screen.text());
    h.keys("M-v");
    assert!(h.screen.row(0).ends_with("line 0"), "{}", h.screen.text());
    // A click on the mark puts point at the end of its line.
    h.press("<mouse-1>", 14, 1);
    assert_eq!(h.eval("(point)"), "13");
}

#[test]
fn a_full_row_has_its_mark_on_its_last_cell() {
    let mut h = Harness::new(12, 6);
    h.paste("abcdefghijk\nhidden\nshown\n");
    h.keys("M-<");
    h.eval("(hide-lines! 12 19)");
    assert_eq!(h.screen.row(0), "abcdefghij…");
    assert_eq!(h.screen.row(1), "shown");
    // A line that wraps has it at the end of its last row.
    let mut h = Harness::new(8, 6);
    h.paste("abcdefghijk\nhidden\nshown\n");
    h.keys("M-<");
    h.eval("(hide-lines! 12 19)");
    assert_eq!(h.screen.row(0), "abcdefg\\");
    assert_eq!(h.screen.row(1), "hijk…");
    assert_eq!(h.screen.row(2), "shown");
}

#[test]
fn text_after_many_hidden_lines_keeps_its_colours() {
    let mut h = Harness::new(40, 6);
    h.eval(r#"(switch-to-buffer "notes.org") (org-mode)"#);
    let body: String = (0..200).map(|n| format!("text {n}\n")).collect();
    h.paste(&format!("* One\n{body}* TODO Two\n"));
    h.keys("M-<");
    h.eval(&format!("(hide-lines! 6 {})", 6 + body.chars().count()));
    assert_eq!(h.screen.row(0), "* One…");
    assert_eq!(h.screen.row(1), "* TODO Two");
    assert_eq!(h.screen.face_at(1, 2), h.face("prose-todo"));
}
