//! The smaller commands of Emacs, written in Scheme in
//! `runtime/lib/simple.scm`.

mod common;

use common::Harness;

/// A harness with a text in the scratch buffer and point at its start.
fn with_text(text: &str) -> Harness {
    let mut h = Harness::new(60, 14);
    h.eval(&format!("(insert {text:?}) (goto-char 0)"));
    h
}

fn text(h: &mut Harness) -> String {
    let shown = h.eval("(buffer-string)");
    serde_json::from_str(&shown).unwrap_or(shown)
}

#[test]
fn a_file_is_inserted_at_point() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("part.txt");
    std::fs::write(&path, "middle\n").unwrap();
    let mut h = with_text("start\nend\n");
    h.keys("C-n C-x i");
    assert!(h.prompt().contains("Insert file"), "{}", h.prompt());
    h.keys("C-a C-k");
    h.type_text(&path.display().to_string()).keys("RET");
    assert_eq!(text(&mut h), "start\nmiddle\nend\n");
    // Point is before what came, and the mark after it.
    assert_eq!(h.eval("(list (point) (mark))"), "(6 13)");
    // A file that is not there is said to be so.
    h.keys("C-x i C-a C-k");
    h.type_text("/nonexistent/amacs-test/none").keys("RET");
    assert!(h.echo().contains("No such file"), "{}", h.echo());
}

#[test]
fn chars_words_and_lines_are_transposed() {
    let mut h = with_text("abc def\nsecond\nthird");
    h.keys("C-f C-t");
    assert_eq!(text(&mut h), "bac def\nsecond\nthird");
    // At the end of a line, the two before point.
    h.keys("C-e C-t");
    assert_eq!(text(&mut h), "bac dfe\nsecond\nthird");
    h.keys("C-a M-f M-t");
    assert_eq!(text(&mut h), "dfe bac\nsecond\nthird");
    h.keys("C-n C-x C-t");
    assert_eq!(text(&mut h), "second\ndfe bac\nthird");
    assert_eq!(h.eval("(line-number-at-pos)"), "3");
}

#[test]
fn lines_are_killed_copied_and_cleared() {
    let mut h = with_text("one\ntwo\nthree");
    h.keys("C-n C-f C-S-DEL");
    assert_eq!(text(&mut h), "one\nthree");
    assert_eq!(h.eval("(current-kill)"), "\"two\\n\"");
    h.eval("(duplicate-line)");
    assert_eq!(text(&mut h), "one\nthree\nthree");
    // Blank lines: many become one, one alone goes, and after a line with
    // text they all go.
    let mut h = with_text("a\n\n\n\nb\n\nc\n\n\nd");
    h.keys("C-n C-x C-o");
    assert_eq!(text(&mut h), "a\n\nb\n\nc\n\n\nd");
    h.keys("C-x C-o");
    assert_eq!(text(&mut h), "a\nb\n\nc\n\n\nd");
    h.eval("(goto-line 4)");
    h.keys("C-x C-o");
    assert_eq!(text(&mut h), "a\nb\n\nc\nd");
    let mut h = with_text("a  \nb\t \nc");
    h.keys("C-e");
    h.eval("(delete-trailing-whitespace)");
    assert_eq!(text(&mut h), "a\nb\nc");
    assert_eq!(h.eval("(point)"), "1");
}

#[test]
fn the_lines_of_the_region_are_sorted_and_sifted() {
    let mut h = with_text("pear\napple\nfig\napple\nkiwi");
    h.eval("(sort-lines)");
    assert_eq!(text(&mut h), "apple\napple\nfig\nkiwi\npear");
    h.eval("(delete-duplicate-lines)");
    assert_eq!(text(&mut h), "apple\nfig\nkiwi\npear");
    assert!(h.echo().contains("Deleted 1 lines"), "{}", h.echo());
    h.eval("(reverse-region)");
    assert_eq!(text(&mut h), "pear\nkiwi\nfig\napple");
    // Only the lines of the region, when there is one.
    h.keys("M-< C-SPC C-n C-n");
    h.eval("(sort-lines)");
    assert_eq!(text(&mut h), "kiwi\npear\nfig\napple");
    h.keys("M-x");
    h.type_text("flush-lines").keys("RET");
    h.type_text("i").keys("RET");
    assert_eq!(text(&mut h), "pear\napple");
    h.keys("M-x");
    h.type_text("keep-lines").keys("RET");
    h.type_text("pp").keys("RET");
    assert_eq!(text(&mut h), "apple");
}

#[test]
fn a_text_is_replaced_from_point_on() {
    let mut h = with_text("a cat, a cat\nand a cat");
    h.keys("M-f M-f M-x");
    h.type_text("replace-string").keys("RET");
    h.type_text("cat").keys("RET");
    h.type_text("dog").keys("RET");
    assert_eq!(text(&mut h), "a cat, a dog\nand a dog");
    assert!(h.echo().contains("Replaced 2 occurrences"), "{}", h.echo());
}

#[test]
fn words_and_paragraphs_are_marked_and_counted() {
    let mut h = with_text("one two\nthree\n\nfour five six");
    h.keys("M-=");
    assert!(
        h.echo()
            .contains("Buffer has 4 lines, 6 words and 28 chars"),
        "{}",
        h.echo()
    );
    h.keys("M-@");
    assert_eq!(h.eval("(list (point) (mark) (region-active?))"), "(0 3 #t)");
    h.keys("C-g C-n M-h");
    assert_eq!(h.eval("(region-active?)"), "#t");
    h.keys("M-=");
    assert!(
        h.echo().contains("Region has 2 lines, 3 words"),
        "{}",
        h.echo()
    );
    h.keys("C-g M-< C-x =");
    assert!(
        h.echo().contains("Char: o (111)") && h.echo().contains("point 0 of 28"),
        "{}",
        h.echo()
    );
}

#[test]
fn a_paragraph_is_filled_to_the_fill_column() {
    let mut h = with_text(
        "before\n\nthe quick brown fox jumps over the lazy dog and runs away\nfast\n\nafter",
    );
    h.eval("(set-option! 'fill-column 20)");
    h.eval("(goto-line 3)");
    h.keys("M-q");
    assert_eq!(
        text(&mut h),
        "before\n\nthe quick brown fox\njumps over the lazy\ndog and runs away\nfast\n\nafter"
    );
    // The marks of a comment are kept on every line.
    let mut h = with_text("  ;; a comment that is too long for its line");
    h.eval("(set-option! 'fill-column 24)");
    h.keys("M-q");
    assert_eq!(
        text(&mut h),
        "  ;; a comment that is\n  ;; too long for its\n  ;; line"
    );
}

#[test]
fn the_expression_before_point_is_evaluated() {
    let mut h = with_text("(+ 1 2) (string-append \"a\" \"b\")");
    h.eval("(goto-char 7)");
    h.keys("C-x C-e");
    assert_eq!(h.echo(), "3");
    h.keys("C-e C-x C-e");
    assert_eq!(h.echo(), "ab");
}

#[test]
fn places_and_texts_are_kept_in_registers() {
    let mut h = with_text("first second");
    h.keys("M-f C-x r SPC a");
    h.keys("C-SPC M-f C-x r s t");
    h.keys("M-< C-x r j a");
    assert_eq!(h.eval("(point)"), "5");
    h.keys("M-< C-x r i t");
    assert_eq!(text(&mut h), " secondfirst second");
    h.keys("C-x r j z");
    assert!(h.echo().contains("Register z is empty"), "{}", h.echo());
}

#[test]
fn a_key_is_inserted_as_the_char_it_stands_for() {
    let mut h = with_text("");
    h.keys("C-q TAB C-q C-j C-q a");
    assert_eq!(text(&mut h), "\t\na");
    // In a terminal, C-q is still the key that leads to its commands.
    h.eval(r#"(term-start "*term*" '("cat"))"#);
    assert_eq!(h.eval("(key-binding \"C-q C-k\")"), "term-char-mode");
    h.eval("(kill-buffer)");
}

#[test]
fn buffers_are_gone_through_in_the_order_of_their_names() {
    let mut h = with_text("");
    h.eval(r#"(switch-to-buffer "b") (switch-to-buffer "a")"#);
    h.keys("C-x <right>");
    assert_eq!(h.eval("(buffer-name)"), "\"b\"");
    h.keys("C-x <left> C-x <left>");
    assert_ne!(h.eval("(buffer-name)"), "\"b\"");
    h.keys("C-h e");
    assert_eq!(h.eval("(buffer-name)"), "\"*Messages*\"");
}

#[test]
fn the_other_window_is_scrolled_from_this_one() {
    let lines: String = (1..=60).map(|n| format!("line {n}\n")).collect();
    let mut h = with_text(&lines);
    h.keys("C-x 2");
    let before = h.eval("(window-start (cadr (window-list)))");
    h.keys("C-M-v");
    assert_ne!(h.eval("(window-start (cadr (window-list)))"), before);
    assert_eq!(h.eval("(point)"), "0");
    h.keys("C-x 1 C-M-v");
    assert!(h.echo().contains("no other window"), "{}", h.echo());
}
