//! Completion from the buffer's own words, snippets and formatters.

use std::time::Duration;

use amacs_core::{Formatter, Value};
use amacs_testkit::Harness;

const LATER: Duration = Duration::from_secs(3600);

fn harness() -> Harness {
    let mut h = Harness::new(60, 12);
    h.editor
        .set_option("completion-delay", Value::Int(1_000_000))
        .unwrap();
    h
}

fn sym(name: &str) -> Value {
    Value::sym(name)
}

fn text(text: &str) -> Value {
    Value::str(text)
}

/// A function snippet in the manner of the example init file.
fn define_fn_snippet(h: &mut Harness) {
    h.editor.set_language(Some("rust")).unwrap();
    h.editor.define_snippet(
        "fundamental-mode",
        "fn",
        vec![
            text("fn "),
            sym("p"),
            text("("),
            Value::List(vec![sym("p"), text("args")]),
            text(") {"),
            sym("n>"),
            sym("r>"),
            sym("n"),
            text("}"),
        ],
    );
}

#[test]
fn words_of_the_buffer_complete_without_a_server() {
    let mut h = harness();
    h.paste("configuration config\nconsole\n");
    h.type_text("c");
    h.wait(LATER);
    assert_eq!(h.screen.row(3), "", "one char is too little to go on");
    h.type_text("o");
    h.wait(LATER);
    // Nearest first, each word once, and not the word being typed.
    assert_eq!(h.screen.row(3), " console");
    assert_eq!(h.screen.row(4), " config");
    assert_eq!(h.screen.row(5), " configuration");
    assert_eq!(h.screen.row(6), "");
    h.type_text("nf").keys("C-n RET");
    assert_eq!(
        h.buffer_text(),
        "configuration config\nconsole\nconfiguration"
    );

    // The option turns the popping up off; asking still works.
    h.editor
        .set_option("completion-delay", Value::Int(-1))
        .unwrap();
    h.keys("RET").type_text("cons");
    h.wait(LATER);
    assert_eq!(h.screen.row(4), "");
    h.keys("M-TAB");
    assert_eq!(h.screen.row(4), " console");
    h.keys("TAB");
    assert!(h.buffer_text().ends_with("\nconsole"));
    h.keys("RET").type_text("zzz").keys("C-M-i");
    assert_eq!(h.echo(), "No completions");
}

#[test]
fn the_list_goes_above_point_at_the_bottom_of_the_window() {
    let mut h = Harness::new(40, 8);
    h.paste("alpha\nalphabet\n\n\n\n");
    h.type_text("al").keys("C-M-i");
    // Point is on the last text row; the list is over the rows above.
    assert_eq!(h.screen.cursor(), Some((5, 2)));
    assert_eq!(h.screen.row(3), " alphabet");
    assert_eq!(h.screen.row(4), " alpha");
    assert_eq!(h.screen.row(5), "al");
    h.keys("C-g");
    assert_eq!(h.screen.row(3), "");
}

#[test]
fn a_snippet_is_filled_in_field_by_field() {
    let mut h = harness();
    define_fn_snippet(&mut h);
    h.type_text("mod m {\n}");
    h.keys("C-p C-e RET").type_text("fn").keys("M-+");
    assert_eq!(
        h.buffer_text(),
        "mod m {\n    fn (args) {\n        \n    }\n}"
    );
    assert_eq!(h.screen.cursor(), Some((1, 7)), "in the first field");
    h.type_text("main").keys("TAB");
    // The placeholder is selected, and typing replaces it.
    assert_eq!(h.screen.cursor(), Some((1, 16)));
    assert!(h.editor.buffer().mark_active);
    h.type_text("x: u8");
    assert_eq!(h.screen.row(1), "    fn main(x: u8) {");
    h.keys("S-TAB");
    assert_eq!(
        h.screen.cursor(),
        Some((1, 11)),
        "back at the end of the first"
    );
    assert!(
        !h.editor.buffer().mark_active,
        "what was typed is not a placeholder"
    );
    h.type_text("2").keys("TAB TAB");
    assert_eq!(h.screen.cursor(), Some((2, 8)), "the body, indented");
    h.type_text("body();").keys("TAB");
    assert_eq!(
        h.buffer_text(),
        "mod m {\n    fn main2(x: u8) {\n        body();\n    }\n}"
    );
    assert_eq!(h.screen.cursor(), Some((3, 5)), "the end of the snippet");
    // The snippet is done: TAB indents again.
    h.keys("C-a TAB");
    assert_eq!(h.screen.cursor(), Some((3, 4)));
}

#[test]
fn a_placeholder_can_be_kept_and_a_snippet_left() {
    let mut h = harness();
    define_fn_snippet(&mut h);
    h.paste("// top\n");
    h.keys("M-*");
    assert_eq!(h.candidates(), ["fn"]);
    h.keys("RET").type_text("f").keys("TAB");
    // Moving on without typing keeps the placeholder.
    h.keys("TAB");
    assert_eq!(h.screen.row(1), "fn f(args) {");
    // Leaving the snippet ends it: TAB is for indenting again.
    h.keys("M-< TAB");
    assert_eq!(h.screen.cursor(), Some((0, 0)));
    assert_eq!(h.buffer_text(), "// top\nfn f(args) {\n    \n}");

    // C-g ends it too.
    h.keys("M-> RET M-*").keys("RET C-g TAB");
    assert!(h.buffer_text().ends_with("}\nfn (args) {\n    \n}"));
}

#[test]
fn mirrored_fields_repeat_what_is_typed() {
    let mut h = harness();
    let field = || Value::List(vec![sym("s"), sym("name")]);
    h.editor.define_snippet(
        "fundamental-mode",
        "guard",
        vec![
            text("#ifndef "),
            field(),
            sym("n"),
            text("#define "),
            field(),
            sym("n"),
            sym("q"),
            sym("n"),
            text("#endif"),
        ],
    );
    h.type_text("guard").keys("M-+");
    h.type_text("FOO_H").keys("TAB");
    assert_eq!(h.buffer_text(), "#ifndef FOO_H\n#define FOO_H\n\n#endif");
    assert_eq!(h.screen.cursor(), Some((2, 0)), "where the snippet says");
}

#[test]
fn snippet_names_are_among_the_completions() {
    let mut h = harness();
    define_fn_snippet(&mut h);
    h.editor
        .define_snippet("fundamental-mode", "fnord", vec![text("FNORD")]);
    h.paste("fnatic\n");
    h.type_text("fn");
    h.wait(LATER);
    assert_eq!(h.screen.row(2), " fnatic");
    assert_eq!(h.screen.row(3), " fn      snippet");
    assert_eq!(h.screen.row(4), " fnord   snippet");
    h.keys("C-n C-n RET");
    assert_eq!(h.buffer_text(), "fnatic\nFNORD");

    // M-+ offers only snippets, and expands an exact name at once.
    h.keys("RET").type_text("f").keys("M-+");
    assert_eq!(h.screen.row(3), " fn     snippet");
    assert_eq!(h.screen.row(4), " fnord  snippet");
    h.keys("C-g").type_text("x").keys("M-+");
    assert_eq!(h.echo(), "No snippet starts with fx");
}

#[test]
fn snippets_of_a_mode_are_for_that_mode_and_its_children() {
    let mut h = harness();
    h.editor.define_mode("prog-mode", None, "Prog");
    h.editor.define_mode("rust-mode", Some("prog-mode"), "Rust");
    h.editor.define_mode("text-mode", None, "Text");
    h.editor
        .define_snippet("fundamental-mode", "everywhere", vec![text("1")]);
    h.editor
        .define_snippet("prog-mode", "todo", vec![text("// TODO")]);
    h.editor
        .define_snippet("rust-mode", "todo", vec![text("// TODO (rust)")]);
    let names = |h: &Harness| -> Vec<String> {
        h.editor
            .snippets_here()
            .into_iter()
            .map(|(name, _)| name.to_owned())
            .collect()
    };
    assert_eq!(names(&h), ["everywhere"]);
    h.editor.set_major_mode("rust-mode").unwrap();
    assert_eq!(names(&h), ["todo", "everywhere"]);
    h.type_text("todo").keys("M-+");
    assert_eq!(h.buffer_text(), "// TODO (rust)", "the nearer mode wins");
    h.editor.set_major_mode("text-mode").unwrap();
    assert_eq!(names(&h), ["everywhere"]);
}

fn formatter(program: &str, args: &[&str]) -> Formatter {
    Formatter {
        program: program.to_owned(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
    }
}

#[test]
fn a_formatter_rewrites_the_lines_that_differ() {
    let mut h = harness();
    // "Formats" by upper-casing the letters a to c.
    h.editor
        .define_formatter("upper", formatter("tr", &["abc", "ABC"]));
    h.keys("C-c C-f");
    assert_eq!(h.echo(), "No formatter is set for this buffer");
    h.editor
        .set_local_option("formatter", Value::sym("upper"))
        .unwrap();
    h.paste("xyz\nabc def\nxyz\ncab\nend");
    h.keys("M-< C-n M-f M-f");
    h.keys("C-c C-f");
    assert_eq!(h.echo(), "Formatted");
    assert_eq!(h.buffer_text(), "xyz\nABC def\nxyz\nCAB\nend");
    assert_eq!(h.screen.cursor(), Some((1, 7)), "point keeps its place");
    h.keys("C-c C-f");
    assert_eq!(h.echo(), "Already formatted");
    // One undo step.
    h.keys("C-/");
    assert_eq!(h.buffer_text(), "xyz\nabc def\nxyz\ncab\nend");

    h.editor
        .set_local_option("formatter", Value::sym("missing"))
        .unwrap();
    h.keys("C-c C-f");
    assert_eq!(h.echo(), "No formatter named missing is defined");
}

#[test]
fn saving_formats_when_asked_and_survives_a_formatter_that_fails() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes.txt");
    std::fs::write(&file, "abc\n").unwrap();
    let mut h = Harness::new(160, 12);
    h.editor
        .define_formatter("upper", formatter("tr", &["abc", "ABC"]));
    h.editor.define_formatter(
        "broken",
        formatter("sh", &["-c", "echo 'bad syntax' >&2; exit 2"]),
    );
    h.editor
        .define_formatter("absent", formatter("/nonexistent/formatter", &[]));
    h.editor.find_file(&file).unwrap();
    h.sync();

    h.editor
        .set_local_option("formatter", Value::sym("upper"))
        .unwrap();
    h.type_text("cab ").keys("C-x C-s");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "cab abc\n",
        "not unless asked"
    );
    h.editor
        .set_local_option("format-on-save", Value::Bool(true))
        .unwrap();
    h.type_text("b").keys("C-x C-s");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "CAB BABC\n");
    assert!(h.echo().starts_with("Wrote "));
    assert!(!h.editor.buffer().is_modified());
    assert_eq!(h.screen.cursor(), Some((0, 5)));

    // The text is saved as it is when the formatter fails, and says why.
    for (name, reason) in [
        ("broken", "not formatted: sh: bad syntax"),
        ("absent", "not formatted: cannot run /nonexistent/formatter"),
    ] {
        h.editor
            .set_local_option("formatter", Value::sym(name))
            .unwrap();
        h.type_text("c").keys("C-x C-s");
        assert!(h.echo().contains(reason), "{}", h.echo());
        assert!(!h.editor.buffer().is_modified());
    }
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "CAB BccABC\n");
}
