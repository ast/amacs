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
    // By a regular expression: the lines that end in i or g go.
    h.type_text("flush-lines").keys("RET");
    h.type_text("[ig]$").keys("RET");
    assert_eq!(text(&mut h), "pear\napple");
    h.keys("M-x");
    h.type_text("keep-lines").keys("RET");
    h.type_text("^a.*e$").keys("RET");
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

// Regular expressions

#[test]
fn matches_of_a_regular_expression_are_replaced_one_by_one() {
    let mut h = with_text("a1 b22 c333");
    h.keys("C-M-%");
    assert!(
        h.prompt().contains("Query replace regexp"),
        "{}",
        h.prompt()
    );
    h.type_text(r"(\w)(\d+)").keys("RET");
    h.type_text(r"\2\1").keys("RET");
    // The question shows what this match becomes.
    assert!(h.echo().contains("Replace with 1a?"), "{}", h.echo());
    h.keys("y");
    assert!(h.echo().contains("Replace with 22b?"), "{}", h.echo());
    h.keys("n y");
    assert_eq!(text(&mut h), "1a b22 333c");
    assert!(h.echo().contains("Replaced 2 occurrences"), "{}", h.echo());
    // An expression that matches no text is replaced at each place once.
    let mut h = with_text("one\ntwo\nthree");
    h.keys("C-M-%");
    h.type_text("^").keys("RET");
    h.type_text("> ").keys("RET !");
    assert_eq!(text(&mut h), "> one\n> two\n> three");
    // One that is wrong is said to be so, and nothing is asked.
    h.keys("C-M-%");
    h.type_text("a(b").keys("RET");
    assert!(h.echo().contains("Bad regular expression"), "{}", h.echo());
}

#[test]
fn regular_expressions_replace_count_and_search() {
    let mut h = with_text("x=1, y=22\nz=333");
    h.keys("M-x");
    h.type_text("count-matches").keys("RET");
    h.type_text(r"\d+").keys("RET");
    assert_eq!(h.echo(), "3 matches");
    h.keys("M-x");
    h.type_text("replace-regexp").keys("RET");
    h.type_text(r"(\w)=(\d+)").keys("RET");
    h.type_text(r"\2 is \1 (\&)").keys("RET");
    assert_eq!(
        text(&mut h),
        "1 is x (x=1), 22 is y (y=22)\n333 is z (z=333)"
    );
    assert!(h.echo().contains("Replaced 3 occurrences"), "{}", h.echo());
    // For code: where the match and its groups are, counted in chars.
    assert_eq!(
        h.eval(r#"(regexp-search "(é+)(x)?" "aåéé!" 1)"#),
        "((2 4) (2 4) #f)"
    );
    assert_eq!(h.eval(r#"(regexp-search "z" "abc")"#), "#f");
    assert_eq!(
        h.eval(r#"(regexp-replace "(a)(b)" "abab" "\\2\\1")"#),
        "(\"baba\" 2)"
    );
}

// Keyboard macros

#[test]
fn the_keys_of_a_macro_are_typed_again() {
    let mut h = with_text("one\ntwo\nthree\nfour\nfive");
    h.keys("<f3>");
    assert!(h.mode_line().contains("Def"), "{}", h.mode_line());
    h.keys("C-a").type_text("- ").keys("C-n <f4>");
    assert!(!h.mode_line().contains("Def"), "{}", h.mode_line());
    assert_eq!(text(&mut h), "- one\ntwo\nthree\nfour\nfive");
    // F4 types them again; with a prefix argument, that many times.
    h.keys("<f4>");
    assert_eq!(text(&mut h), "- one\n- two\nthree\nfour\nfive");
    h.keys("C-u 2 <f4>");
    assert_eq!(text(&mut h), "- one\n- two\n- three\n- four\nfive");
    assert_eq!(h.eval("(last-kbd-macro)"), "\"C-a - SPC C-n\"");
    // C-x ( and C-x ) define one as well, and C-x e runs it.
    h.keys("M-< C-x (").type_text("!").keys("C-x ) C-x e");
    assert!(text(&mut h).starts_with("!!- one"), "{}", text(&mut h));
}

#[test]
fn a_macro_runs_until_a_command_fails() {
    let mut h = with_text("a\nb\nc\nd");
    // On the last line C-n has nowhere to go, and that ends it.
    h.keys("<f3> C-e").type_text(";").keys("C-n <f4>");
    h.keys("C-u 0 <f4>");
    assert_eq!(text(&mut h), "a;\nb;\nc;\nd;");
    // Nothing is left to run after the failure.
    h.type_text("x");
    assert_eq!(text(&mut h), "a;\nb;\nc;\nd;x");
}

#[test]
fn commands_in_scheme_and_the_minibuffer_take_their_turn_in_a_macro() {
    // C-t is written in Scheme and the letter is inserted by the core:
    // they are done in the order they were typed.
    let mut h = with_text("abcd");
    h.keys("C-f <f3> C-t").type_text("x").keys("<f4>");
    assert_eq!(text(&mut h), "baxcd");
    h.keys("<f4>");
    assert_eq!(text(&mut h), "bacxxd");
    // What is typed in the minibuffer is part of the macro.
    let mut h = with_text("cat cat cat");
    h.keys("<f3> M-x");
    h.type_text("replace-string").keys("RET");
    h.type_text("cat").keys("RET");
    h.type_text("dog").keys("RET <f4>");
    assert_eq!(text(&mut h), "dog dog dog");
    h.eval("(erase-buffer) (insert \"a cat\") (goto-char 0)");
    h.keys("<f4>");
    assert_eq!(text(&mut h), "a dog");
}

#[test]
fn a_macro_is_kept_under_a_name() {
    let mut h = with_text("");
    h.keys("<f3>").type_text("ab").keys("<f4> M-x");
    h.type_text("name-last-kbd-macro").keys("RET");
    h.type_text("insert-ab").keys("RET");
    // Another macro takes its place as the last, and the name still has it.
    h.keys("<f3>").type_text("z").keys("<f4>");
    h.keys("C-u 2 M-x");
    h.type_text("insert-ab").keys("RET");
    assert_eq!(text(&mut h), "abzabab");
    // From code, a macro is its keys.
    h.eval(r#"(execute-kbd-macro "C-a ! RET")"#);
    assert_eq!(text(&mut h), "!\nabzabab");
    h.keys("<f4>");
    h.keys("C-x )");
    assert!(h.echo().contains("Not defining"), "{}", h.echo());
}
