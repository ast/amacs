//! Syntax highlighting, line numbers, indentation, paired delimiters,
//! structural editing and themes, as a user sees them.

use amacs_core::Value;
use amacs_proto::FaceAttrs;
use amacs_testkit::Harness;

fn harness(language: &str) -> Harness {
    let mut h = Harness::new(60, 12);
    h.editor.set_language(Some(language)).unwrap();
    h
}

/// A harness in a language whose delimiters are inserted in pairs.
fn pairing(language: &str) -> Harness {
    let mut h = harness(language);
    h.editor
        .set_minor_mode("auto-pair-mode", true, true)
        .unwrap();
    h
}

fn paredit() -> Harness {
    let mut h = harness("scheme");
    h.editor
        .set_minor_mode("paredit-mode", false, true)
        .unwrap();
    h
}

/// The attributes that the named face has in the current theme.
fn face(h: &Harness, name: &str) -> FaceAttrs {
    let faces = h.editor.faces();
    faces.table()[faces.face_id(name).unwrap().0 as usize]
}

#[test]
fn source_text_is_drawn_in_syntax_faces() {
    let mut h = harness("rust");
    h.paste("fn main() {\n    let s = \"hi\"; // note\n}\n");
    assert_eq!(h.screen.face_at(0, 0), face(&h, "keyword"));
    assert!(h.screen.face_at(0, 0).bold);
    assert_eq!(h.screen.face_at(0, 3), face(&h, "function-name"));
    assert_eq!(h.screen.face_at(1, 12), face(&h, "string"));
    assert_eq!(h.screen.face_at(1, 18), face(&h, "comment"));
    assert!(h.screen.face_at(1, 18).italic);
    assert_eq!(h.screen.face_at(1, 8), face(&h, "default"));
    assert_ne!(face(&h, "keyword").fg, face(&h, "default").fg);
}

#[test]
fn highlighting_follows_edits_and_undo() {
    let mut h = harness("rust");
    h.paste("let x = 1; // c");
    assert_eq!(h.screen.face_at(0, 11), face(&h, "comment"));
    // Without its second slash the comment is code.
    h.keys("C-a M-f M-f M-f C-f C-f C-f C-d");
    assert_eq!(h.screen.row(0), "let x = 1; / c");
    assert_ne!(h.screen.face_at(0, 13), face(&h, "comment"));
    h.keys("C-/");
    assert_eq!(h.screen.face_at(0, 13), face(&h, "comment"));
    // The region shows over the syntax colours.
    h.keys("C-a C-SPC M-f");
    assert_eq!(h.screen.face_at(0, 1), face(&h, "region"));
}

#[test]
fn a_buffer_without_a_language_is_plain() {
    let mut h = Harness::new(40, 6);
    h.paste("fn main() {}");
    assert_eq!(h.screen.face_at(0, 0), face(&h, "default"));
}

#[test]
fn line_numbers_are_relative_to_point() {
    let mut h = harness("rust");
    h.editor
        .set_local_option("line-numbers", Value::sym("relative"))
        .unwrap();
    h.paste("a\nb\nc\nd\ne");
    h.keys("M-< C-n C-n");
    let rows: Vec<String> = (0..5).map(|row| h.screen.row(row)).collect();
    assert_eq!(rows, ["  2 a", "  1 b", "3   c", "  1 d", "  2 e"]);
    assert_eq!(
        h.screen.cursor(),
        Some((2, 4)),
        "the text starts after the numbers"
    );
    assert_eq!(h.screen.face_at(2, 0), face(&h, "line-number-current"));
    assert_eq!(h.screen.face_at(1, 0), face(&h, "line-number"));

    h.editor
        .set_local_option("line-numbers", Value::sym("absolute"))
        .unwrap();
    h.keys("C-n");
    assert_eq!(h.screen.row(0), "  1 a");
    assert_eq!(h.screen.row(3), "  4 d");
    assert_eq!(h.screen.face_at(3, 0), face(&h, "line-number-current"));
}

#[test]
fn a_wrapped_line_has_one_number() {
    let mut h = Harness::new(20, 8);
    h.editor
        .set_local_option("line-numbers", Value::Bool(true))
        .unwrap();
    h.paste(&"x".repeat(20));
    h.paste("\nyy");
    // Twenty columns less four for the numbers and one for the wrap mark.
    assert_eq!(h.screen.row(0), format!("  1 {}\\", "x".repeat(15)));
    assert_eq!(h.screen.row(1), format!("    {}", "x".repeat(5)));
    assert_eq!(h.screen.row(2), "  2 yy");
    h.keys("C-p C-e");
    assert_eq!(h.screen.cursor(), Some((1, 9)));
}

#[test]
fn newlines_and_closing_brackets_indent_code() {
    let mut h = harness("rust");
    h.type_text("fn main() {\nlet x = foo(\n1,\n);\nif x {\ny();\n}\n}");
    assert_eq!(
        h.buffer_text(),
        "fn main() {\n    let x = foo(\n        1,\n    );\n    if x {\n        y();\n    }\n}"
    );
}

#[test]
fn tab_indents_the_line_or_the_region() {
    let mut h = harness("rust");
    h.paste("fn main() {\nlet x = 1;\n        let y = 2;\n}");
    h.keys("M-< C-n TAB");
    assert_eq!(h.screen.row(1), "    let x = 1;");
    assert_eq!(h.screen.cursor(), Some((1, 4)), "point moves to the text");
    h.keys("C-e TAB");
    assert_eq!(h.screen.cursor(), Some((1, 14)), "point stays on the text");

    h.keys("C-x h TAB");
    assert_eq!(
        h.buffer_text(),
        "fn main() {\n    let x = 1;\n    let y = 2;\n}"
    );
    h.keys("C-/");
    assert_eq!(
        h.buffer_text(),
        "fn main() {\n    let x = 1;\n        let y = 2;\n}",
        "indenting a region is undone as one change"
    );
}

#[test]
fn indentation_uses_tabs_where_the_buffer_says_so() {
    let mut h = harness("go");
    h.editor
        .set_local_option("indent-tabs-mode", Value::Bool(true))
        .unwrap();
    h.editor
        .set_local_option("indent-offset", Value::Int(8))
        .unwrap();
    h.type_text("func main() {\nfor {\nx()\n}\n}");
    assert_eq!(h.buffer_text(), "func main() {\n\tfor {\n\t\tx()\n\t}\n}");
}

#[test]
fn a_new_line_starts_where_the_last_one_did_without_a_language() {
    let mut h = Harness::new(40, 8);
    h.type_text("  item  \nnext");
    assert_eq!(
        h.buffer_text(),
        "  item\n  next",
        "trailing blanks go and the indentation carries over"
    );
    h.keys("C-e TAB");
    assert_eq!(
        h.buffer_text(),
        "  item\n  next  ",
        "TAB moves to a tab stop"
    );
}

#[test]
fn python_lines_follow_the_line_above() {
    let mut h = harness("python");
    h.type_text("def f(x):\nif x:\nreturn 1");
    assert_eq!(h.buffer_text(), "def f(x):\n    if x:\n        return 1");
    h.keys("RET");
    assert_eq!(h.screen.cursor(), Some((3, 8)));
    // TAB again steps out a level at a time, then back in.
    h.keys("TAB");
    assert_eq!(h.screen.cursor(), Some((3, 8)));
    h.keys("TAB");
    assert_eq!(h.screen.cursor(), Some((3, 4)));
    h.keys("TAB TAB");
    assert_eq!(h.screen.cursor(), Some((3, 8)));
    h.keys("C-x h C-M-\\");
    assert!(h.echo().starts_with("Indentation is part of the syntax"));
}

#[test]
fn lisp_lines_are_indented_by_their_form() {
    let mut h = harness("scheme");
    h.type_text("(define (f x)\n(let ((a 1)\n(b 2))\n(list a\nb)))");
    assert_eq!(
        h.buffer_text(),
        "(define (f x)\n  (let ((a 1)\n        (b 2))\n    (list a\n          b)))"
    );
}

#[test]
fn delimiters_are_inserted_in_pairs() {
    let mut h = pairing("rust");
    h.type_text("foo(");
    assert_eq!(h.buffer_text(), "foo()");
    assert_eq!(h.screen.cursor(), Some((0, 4)));
    h.type_text("\"a");
    assert_eq!(h.buffer_text(), "foo(\"a\")");
    // Typing the closing delimiters steps over them.
    h.type_text("\")");
    assert_eq!(h.buffer_text(), "foo(\"a\")");
    assert_eq!(h.screen.cursor(), Some((0, 8)));

    // Deleting an opening delimiter takes an empty pair with it.
    h.type_text(" [");
    assert_eq!(h.buffer_text(), "foo(\"a\") []");
    h.keys("DEL");
    assert_eq!(h.buffer_text(), "foo(\"a\") ");

    // In front of text, and for a lifetime's quote, nothing is added.
    h.keys("C-a").type_text("(");
    assert_eq!(h.buffer_text(), "(foo(\"a\") ");
    h.keys("C-e").type_text("&'a");
    assert_eq!(h.buffer_text(), "(foo(\"a\") &'a");
}

#[test]
fn a_typed_delimiter_wraps_the_region() {
    let mut h = pairing("python");
    h.type_text("one two");
    h.keys("C-a C-SPC M-f").type_text("'");
    assert_eq!(h.buffer_text(), "'one' two");
    h.keys("C-e M-b C-SPC C-e").type_text("[");
    assert_eq!(h.buffer_text(), "'one' [two]");
}

#[test]
fn return_between_brackets_opens_a_block() {
    let mut h = pairing("rust");
    h.type_text("fn main() {\n");
    assert_eq!(h.buffer_text(), "fn main() {\n    \n}");
    assert_eq!(h.screen.cursor(), Some((1, 4)));
    h.type_text("call(\n");
    assert_eq!(
        h.buffer_text(),
        "fn main() {\n    call(\n        \n    )\n}"
    );
    assert_eq!(h.screen.cursor(), Some((2, 8)));
}

#[test]
fn pairs_stay_out_of_the_minibuffer() {
    let mut h = pairing("rust");
    h.keys("M-x").type_text("(");
    assert_eq!(h.prompt(), "*/0 M-x (");
    h.keys("C-g");
}

#[test]
fn expressions_are_moved_over_and_killed_whole() {
    let mut h = harness("scheme");
    h.paste("(a (b \"c )\") d) e");
    h.keys("M-< C-M-f");
    assert_eq!(h.screen.cursor(), Some((0, 15)));
    h.keys("C-M-b");
    assert_eq!(h.screen.cursor(), Some((0, 0)));
    h.keys("C-M-d C-M-f C-M-f");
    assert_eq!(h.screen.cursor(), Some((0, 12)), "after the inner list");
    h.keys("C-M-b C-M-d C-M-f C-M-f");
    assert_eq!(h.screen.cursor(), Some((0, 11)), "after the string");
    h.keys("C-M-f");
    assert_eq!(h.echo(), "No expression follows");
    h.keys("C-M-u");
    assert_eq!(h.screen.cursor(), Some((0, 3)));
    h.keys("C-M-k");
    assert_eq!(h.buffer_text(), "(a  d) e");
    h.keys("C-M-u C-M-SPC C-w");
    assert_eq!(h.buffer_text(), " e");
    h.keys("C-y C-M-DEL");
    assert_eq!(h.buffer_text(), " e");
}

#[test]
fn expressions_work_in_other_languages_too() {
    let mut h = harness("rust");
    h.paste("call(a, [1, 2], \"x)\") + y");
    h.keys("M-< C-M-f C-M-f");
    assert_eq!(h.screen.cursor(), Some((0, 21)));
    h.keys("C-M-b C-M-d C-M-f C-M-f");
    assert_eq!(h.screen.cursor(), Some((0, 14)));
}

#[test]
fn paredit_inserts_and_closes_brackets() {
    let mut h = paredit();
    h.type_text("(define (f x");
    assert_eq!(h.buffer_text(), "(define (f x))");
    h.type_text(")");
    assert_eq!(h.screen.cursor(), Some((0, 13)));
    h.type_text("\n(g \"ab");
    assert_eq!(h.buffer_text(), "(define (f x)\n  (g \"ab\"))");
    // Inside a string a quote is escaped and brackets are text.
    h.keys("C-b").type_text("\"(");
    assert_eq!(h.buffer_text(), "(define (f x)\n  (g \"a\\\"(b\"))");
    // At the closing quote, typing a quote steps over it.
    h.keys("C-f").type_text("\"");
    assert_eq!(h.screen.cursor(), Some((1, 12)));
    // A closing bracket leaves the list and drops the space before it.
    h.type_text("   )");
    assert_eq!(h.buffer_text(), "(define (f x)\n  (g \"a\\\"(b\"))");
    assert_eq!(h.screen.cursor(), Some((1, 13)));
    // A new expression is set apart by a space.
    h.type_text("(");
    assert_eq!(h.buffer_text(), "(define (f x)\n  (g \"a\\\"(b\") ())");
}

#[test]
fn paredit_deletes_only_what_keeps_the_text_balanced() {
    let mut h = paredit();
    h.paste("(a (b) \"\" \"s\")");
    // Backward over a closing bracket: step in, then delete inside.
    h.keys("DEL");
    assert_eq!(h.buffer_text(), "(a (b) \"\" \"s\")");
    assert_eq!(h.screen.cursor(), Some((0, 13)));
    h.keys("DEL DEL");
    assert_eq!(
        h.buffer_text(),
        "(a (b) \"\" \"\")",
        "into the string, then its char"
    );
    h.keys("DEL");
    assert_eq!(
        h.buffer_text(),
        "(a (b) \"\" )",
        "an empty string goes whole"
    );
    h.keys("DEL DEL DEL");
    assert_eq!(h.buffer_text(), "(a (b) )");
    // An opening bracket with something after it stays.
    h.keys("C-b C-b C-b").keys("DEL");
    assert_eq!(h.buffer_text(), "(a (b) )");
    assert_eq!(h.screen.cursor(), Some((0, 4)));
    // Forward: step into the list, delete in it, then the empty pair.
    h.keys("C-a C-f C-f C-f C-d");
    assert_eq!(h.screen.cursor(), Some((0, 4)));
    h.keys("C-d C-d");
    assert_eq!(h.buffer_text(), "(a  )");
    // With a prefix argument the plain command runs.
    h.keys("C-a C-u 1 C-d");
    assert_eq!(h.buffer_text(), "a  )");
}

#[test]
fn paredit_kills_whole_expressions() {
    let mut h = paredit();
    h.paste("(let ((a 1)\n      (b 2))\n  (f a \"x y\" b)) ; end\n");
    // The binding list starts on the line and goes whole.
    h.keys("M-< C-f C-f C-f C-f C-f C-k");
    assert_eq!(
        h.buffer_text(),
        "(let \n  (f a \"x y\" b)) ; end\n",
        "an expression that starts on the line is killed whole"
    );
    // Inside a list, up to its closing bracket.
    h.keys("C-n C-a C-f C-f C-f C-f C-f C-k");
    assert_eq!(h.buffer_text(), "(let \n  (f )) ; end\n");
    // In a string, up to its closing quote.
    h.keys("C-/");
    h.keys("C-s").type_text("x y").keys("RET");
    h.keys("C-a M-f M-f M-f C-k");
    assert_eq!(h.buffer_text(), "(let \n  (f a \"x\" b)) ; end\n");
    // At the end of the text of a line, the newline goes as usual.
    h.keys("M-< C-e C-k");
    assert_eq!(h.buffer_text(), "(let   (f a \"x\" b)) ; end\n");
}

#[test]
fn paredit_slurps_barfs_splices_raises_and_wraps() {
    let mut h = paredit();
    h.paste("(a b) c d");
    h.keys("M-< C-f C-<right>");
    assert_eq!(h.buffer_text(), "(a b c) d");
    h.keys("C-)");
    assert_eq!(h.buffer_text(), "(a b c d)");
    h.keys("C-<left> C-}");
    assert_eq!(h.buffer_text(), "(a b) c d");
    h.keys("C-<right>");
    assert_eq!(h.buffer_text(), "(a b c) d");

    h.keys("C-M-<right>");
    assert_eq!(
        h.buffer_text(),
        "a (b c) d",
        "the first expression is put out"
    );
    h.keys("C-M-<left>");
    assert_eq!(h.buffer_text(), "(a b c) d", "and taken back in");

    h.keys("M-s");
    assert_eq!(h.buffer_text(), "a b c d", "the brackets are removed");
    h.keys("M-< M-(");
    assert_eq!(h.buffer_text(), "(a) b c d");
    h.keys("C-<right> C-<right>");
    assert_eq!(h.buffer_text(), "(a b c) d");
    h.keys("M-f M-f C-f M-r");
    assert_eq!(h.buffer_text(), "c d", "the expression replaces its list");

    // An empty list takes what follows right after its bracket.
    h.keys("M-<").type_text("(");
    assert_eq!(h.buffer_text(), "() c d");
    h.keys("C-<right>");
    assert_eq!(h.buffer_text(), "(c) d");
}

#[test]
fn themes_recolour_the_whole_frame() {
    let mut h = harness("rust");
    h.paste("fn main() {}");
    let light = h.screen.face_at(0, 0);
    let light_bg = h.screen.face_at(5, 0).bg;
    h.editor
        .faces_mut()
        .load_theme("modus-vivendi-tinted")
        .unwrap();
    h.keys("C-f");
    assert_eq!(h.screen.rows_drawn, 12, "everything is drawn again");
    assert_ne!(h.screen.face_at(0, 0).fg, light.fg);
    assert_ne!(h.screen.face_at(5, 0).bg, light_bg);
    assert_eq!(h.screen.face_at(0, 0), face(&h, "keyword"));
    h.keys("C-f");
    assert!(h.screen.rows_drawn <= 1, "and after that only what changes");
}
