//! Jumping by label, which is written in Scheme: `runtime/lib/avy.scm`.

mod common;

use common::Harness;

#[test]
fn a_place_on_screen_is_reached_by_its_label() {
    let mut h = Harness::new(60, 14);
    h.paste("one two\nthree two\nfour");
    h.keys("M-< M-g c");
    assert_eq!(h.echo(), "Jump to char:");
    h.type_text("t");
    // Every t but the one... there is none at point, so all four: the
    // nearest gets the first key of the home row.
    assert_eq!(h.screen.row(0), "one awo");
    assert_eq!(h.screen.row(1), "shree dwo");
    assert_eq!(h.screen.face_at(0, 4), h.face("jump-label"));
    assert_eq!(
        h.screen.face_at(0, 0).fg,
        h.face("jump-background").fg,
        "the rest is dimmed"
    );
    h.type_text("d");
    assert_eq!(h.screen.cursor(), Some((1, 6)));
    assert_eq!(h.screen.row(1), "three two", "the labels are gone");
    // The mark is left behind, to go back.
    h.keys("C-x C-x");
    assert_eq!(h.screen.cursor(), Some((0, 0)));

    // A single match needs no label.
    h.keys("C-g M-g c").type_text("f");
    assert_eq!(h.screen.cursor(), Some((2, 0)));
    h.keys("M-g c").type_text("z");
    assert_eq!(h.echo(), "Nothing on screen matches");
    // Quitting removes the labels.
    h.keys("M-g c").type_text("o").keys("C-g");
    assert_eq!(h.echo(), "Quit");
    assert_eq!(h.screen.row(0), "one two");
}

#[test]
fn labels_get_longer_when_there_are_many_places() {
    let mut h = Harness::new(60, 14);
    h.paste(&"x ".repeat(12));
    h.keys("M-g c").type_text("x");
    // Twelve places for nine keys: some keys lead on to a second.
    let row = h.screen.row(0);
    assert!(row.contains("ka") || row.contains("la"), "{row}");
    h.type_text("l");
    let narrowed = h.screen.row(0);
    assert!(narrowed.contains('x'), "{narrowed}");
    h.type_text("a");
    assert_eq!(h.screen.row(0), "x x x x x x x x x x x x");
    assert_ne!(h.screen.cursor(), Some((0, 24)));
}

#[test]
fn lines_words_and_pairs_of_chars_can_be_jumped_to() {
    let mut h = Harness::new(60, 14);
    h.paste("alpha beta\n\nbeta alpha");
    // Point is at the end, so the last line is the nearest.
    h.keys("M-g l");
    assert_eq!(h.screen.row(0), "dlpha beta");
    assert_eq!(h.screen.row(1), "s", "an empty line has a label too");
    assert_eq!(h.screen.row(2), "aeta alpha");
    h.type_text("s");
    assert_eq!(h.screen.cursor(), Some((1, 0)));

    // Words that start with a, not the a inside them.
    h.keys("M-g w").type_text("a");
    assert_eq!(h.screen.row(0), "slpha beta");
    assert_eq!(h.screen.row(2), "beta alpha");
    h.type_text("s");
    assert_eq!(h.screen.cursor(), Some((0, 0)));

    // "et" occurs twice; the nearer is on the first line.
    h.keys("M-x").type_text("avy-goto-char-2").keys("RET");
    h.type_text("e");
    assert_eq!(h.echo(), "Jump to chars: e");
    h.type_text("t");
    assert_eq!(h.screen.row(0), "alpha bata");
    h.type_text("a");
    assert_eq!(h.screen.cursor(), Some((0, 7)));
}

#[test]
fn zapping_kills_to_a_char_picked_on_screen() {
    let mut h = Harness::new(60, 14);
    h.paste("one, two, three");
    h.keys("M-< M-z").type_text(",");
    h.type_text("s");
    assert_eq!(h.text(), " three", "through the second comma");
    h.keys("C-/ M-< M-Z").type_text(",").type_text("a");
    assert_eq!(h.text(), ", two, three", "up to the first");
    h.keys("C-y");
    assert_eq!(h.text(), "one, two, three");
}

#[test]
fn windows_are_selected_by_label() {
    let mut h = Harness::new(80, 20);
    h.paste("text");
    // Three windows: two side by side above one.
    h.keys("C-x 2 C-x 3");
    let selected = |h: &Harness| h.screen.cursor().unwrap();
    assert_eq!(selected(&h), (0, 4));
    // By label: each other window shows a key in its corner.
    h.keys("M-o");
    assert_eq!(
        h.screen.row(0),
        "text                                    │aext"
    );
    assert_eq!(h.screen.row(10), "sext");
    h.type_text("s");
    assert_eq!(selected(&h).0, 10);
    assert_eq!(h.screen.row(10), "text");
    // With two windows there is nothing to pick.
    h.keys("C-x 0");
    assert_eq!(selected(&h).1, 45);
    h.keys("M-o");
    assert_eq!(selected(&h), (0, 4), "straight to the only other one");
    h.keys("C-x 1 M-o");
    assert_eq!(h.echo(), "There is no other window");
}

#[test]
fn labels_are_distinct_and_short_first() {
    let mut h = Harness::new(60, 14);
    assert_eq!(h.eval("(avy-labels 3)"), "(\"a\" \"s\" \"d\")");
    assert_eq!(
        h.eval("(avy-labels 10)"),
        "(\"a\" \"s\" \"d\" \"f\" \"g\" \"h\" \"j\" \"k\" \"la\" \"ls\")"
    );
    // No label is the start of another, so each can be typed.
    assert_eq!(
        h.eval(
            "(define (clash? labels)
               (any (lambda (label)
                      (any (lambda (other)
                             (and (not (eq? label other)) (string-prefix? label other)))
                           labels))
                    labels))
             (map (lambda (count)
                    (let ((labels (avy-labels count)))
                      (list (= (length labels) count) (clash? labels))))
                  '(9 10 17 18 50 81 82 300 1000))"
        ),
        "((#t #f) (#t #f) (#t #f) (#t #f) (#t #f) (#t #f) (#t #f) (#t #f) (#t #f))"
    );
    // The keys are an option.
    h.eval("(set-option! 'avy-keys \"12\")");
    assert_eq!(h.eval("(avy-labels 3)"), "(\"1\" \"21\" \"22\")");
}

#[test]
fn the_jump_commands_are_made_of_parts_that_others_can_use() {
    let mut h = Harness::new(60, 14);
    h.paste("alpha beta gamma");
    // A command of one's own: jump to the end of a word.
    h.eval(
        r#"(define-command (jump-to-word-end)
             "Jump to the end of a word on screen."
             (avy-jump
              (avy-candidates
               (lambda (chars index start)
                 (and (> index 0)
                      (char-alphabetic? (vector-ref chars (- index 1)))
                      (or (= index (vector-length chars))
                          (not (char-alphabetic? (vector-ref chars index)))))))))
           (global-set-key "C-c e" 'jump-to-word-end)"#,
    );
    h.keys("M-< C-c e");
    assert_eq!(h.screen.row(0), "alphaabetasgammad");
    h.type_text("s");
    assert_eq!(h.screen.cursor(), Some((0, 10)));
}
