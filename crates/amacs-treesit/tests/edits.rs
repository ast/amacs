//! A tree kept up to date through edits is the tree a fresh parse gives.

use amacs_text::Text;
use amacs_treesit::{Syntax, language};
use proptest::prelude::*;

const SOURCE: &str = include_str!("fixtures/sample.rs");

#[derive(Debug, Clone)]
enum Op {
    Insert(usize, String),
    Delete(usize, usize),
}

fn op() -> impl Strategy<Value = Op> {
    let snippet = prop_oneof![
        Just("{".to_owned()),
        Just("}".to_owned()),
        Just("\n".to_owned()),
        Just("\"".to_owned()),
        Just("fn f() {}\n".to_owned()),
        Just("ö".to_owned()),
        Just("// x\n".to_owned()),
        "[a-z(); ]{1,6}",
    ];
    prop_oneof![
        (any::<usize>(), snippet).prop_map(|(pos, text)| Op::Insert(pos, text)),
        (any::<usize>(), 0usize..40).prop_map(|(pos, len)| Op::Delete(pos, len)),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn incremental_parsing_matches_a_fresh_parse(ops in prop::collection::vec(op(), 1..24)) {
        let rust = language("rust").unwrap();
        let mut text = Text::from(SOURCE);
        let mut syntax = Syntax::new(rust).unwrap();
        syntax.update(&text);
        for op in ops {
            match op {
                Op::Insert(pos, inserted) => {
                    let pos = pos % (text.len() + 1);
                    syntax.before_insert(&text, pos, &inserted);
                    text.insert(pos, &inserted);
                }
                Op::Delete(pos, len) => {
                    let start = pos % (text.len() + 1);
                    let end = (start + len).min(text.len());
                    syntax.before_delete(&text, start..end);
                    text.remove(start..end);
                }
            }
            syntax.update(&text);

            let mut fresh = Syntax::new(rust).unwrap();
            fresh.update(&text);
            let all = 0..text.len_bytes();
            prop_assert_eq!(
                syntax.highlights(&text, all.clone()),
                fresh.highlights(&text, all)
            );
        }
    }
}

fn org_op() -> impl Strategy<Value = Op> {
    let snippet = prop_oneof![
        Just("\n".to_owned()),
        Just("* ".to_owned()),
        Just("#+begin_src sh\n".to_owned()),
        Just("#+end_src\n".to_owned()),
        Just("#+begin_example\n".to_owned()),
        Just("[[".to_owned()),
        Just("]]".to_owned()),
        Just("ö".to_owned()),
        "[a-z*=~/ |:<>-]{1,6}",
    ];
    prop_oneof![
        (any::<usize>(), snippet).prop_map(|(pos, text)| Op::Insert(pos, text)),
        (any::<usize>(), 0usize..40).prop_map(|(pos, len)| Op::Delete(pos, len)),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn org_highlights_after_edits_are_those_of_the_text(
        ops in prop::collection::vec(org_op(), 1..24)
    ) {
        let org = language("org").unwrap();
        let mut text = Text::from(include_str!("fixtures/sample.org"));
        let mut syntax = Syntax::new(org).unwrap();
        syntax.update(&text);
        for op in ops {
            match op {
                Op::Insert(pos, inserted) => {
                    let pos = pos % (text.len() + 1);
                    syntax.before_insert(&text, pos, &inserted);
                    text.insert(pos, &inserted);
                }
                Op::Delete(pos, len) => {
                    let start = pos % (text.len() + 1);
                    let end = (start + len).min(text.len());
                    syntax.before_delete(&text, start..end);
                    text.remove(start..end);
                }
            }
            syntax.update(&text);

            let mut fresh = Syntax::new(org).unwrap();
            fresh.update(&text);
            let all = 0..text.len_bytes();
            prop_assert_eq!(
                syntax.highlights(&text, all.clone()),
                fresh.highlights(&text, all.clone())
            );
            // Any part of the text has the highlights that it has in the
            // whole.
            let whole = syntax.highlights(&text, all);
            let lines = text.len_lines();
            let (first, last) = (lines / 3, (2 * lines / 3).max(lines / 3));
            let (start, end) = (text.line_to_byte(first), text.line_to_byte(last));
            let part = syntax.highlights(&text, start..end);
            for byte in start..end {
                prop_assert_eq!(part.get(byte), whole.get(byte), "byte {}", byte);
            }
        }
    }
}

#[test]
#[ignore = "prints timings; run with --release --ignored --nocapture"]
fn timings_on_a_large_file() {
    use std::time::{Duration, Instant};
    let rust = language("rust").unwrap();
    let mut text = Text::from(SOURCE.repeat(2000).as_str());
    println!("{} bytes, {} lines", text.len_bytes(), text.len_lines());
    let mut syntax = Syntax::new(rust).unwrap();
    let start = Instant::now();
    syntax.update(&text);
    println!("first parse: {:?}", start.elapsed());

    let middle = text.len() / 2;
    let start = Instant::now();
    for i in 0..100 {
        syntax.before_insert(&text, middle + i, "x");
        text.insert(middle + i, "x");
        syntax.update(&text);
    }
    println!("edit and reparse: {:?} each", start.elapsed() / 100);

    let from = text.char_to_byte(middle);
    let start = Instant::now();
    for _ in 0..100 {
        let _ = syntax.highlights(&text, from..from + 4000);
    }
    println!("highlight a screenful: {:?} each", start.elapsed() / 100);

    let line = text.line_of(middle);
    let params = amacs_treesit::IndentParams {
        offset: 4,
        tab_width: 8,
    };
    let start = Instant::now();
    for i in 0..100 {
        let _ = syntax.indent_column(&text, line + i, params);
    }
    println!("indent a line: {:?} each", start.elapsed() / 100);

    // The markup of Markdown is found by a parse of each paragraph, which
    // is done again only for the paragraphs that changed.
    let paragraph = "Some **strong** and *stressed* text with `code` and [a link](http://example.org) in it,\nover two lines of the file.\n\n";
    let mut text = Text::from(paragraph.repeat(8000).as_str());
    println!("Markdown: {} bytes", text.len_bytes());
    let mut syntax = Syntax::new(language("markdown").unwrap()).unwrap();
    syntax.update(&text);
    let start = Instant::now();
    syntax.update_markup(&text);
    println!("first markup: {:?}", start.elapsed());
    let middle = text.len() / 2;
    let (mut parse, mut markup) = (Duration::ZERO, Duration::ZERO);
    for i in 0..100 {
        let start = Instant::now();
        syntax.before_insert(&text, middle + i, "x");
        text.insert(middle + i, "x");
        syntax.update(&text);
        parse += start.elapsed();
        let start = Instant::now();
        syntax.update_markup(&text);
        markup += start.elapsed();
    }
    println!("edit and reparse: {:?} each", parse / 100);
    println!("markup after an edit: {:?} each", markup / 100);
}

/// The highlights of a window are remembered between edits, and more is
/// painted than asked for when the window scrolls. What is given is what
/// painting just that range gives.
#[test]
fn remembered_highlights_are_those_of_the_text() {
    let rust = language("rust").unwrap();
    let mut text = Text::from(SOURCE);
    let mut syntax = Syntax::new(rust).unwrap();
    syntax.update(&text);
    let exact = |text: &Text, range: std::ops::Range<usize>| {
        let mut fresh = Syntax::new(rust).unwrap();
        fresh.update(text);
        fresh.highlights(text, range)
    };
    let window = 300.min(text.len_bytes() / 4);
    // Asked twice, scrolled down by parts of a window and back up, and
    // somewhere else altogether.
    let last = text.len_bytes() - window;
    let starts = [0, 0, 40, 90, 200, 520, 480, 100, last, last - 70, 3];
    for start in starts {
        let range = start..start + window;
        assert_eq!(
            syntax.highlights(&text, range.clone()),
            exact(&text, range.clone()),
            "at {range:?}"
        );
    }
    // After a change, nothing of before is given.
    syntax.before_insert(&text, 0, "/* ");
    text.insert(0, "/* ");
    syntax.update(&text);
    for start in [0, 40, 0] {
        let range = start..start + window;
        assert_eq!(
            syntax.highlights(&text, range.clone()),
            exact(&text, range.clone()),
            "after the change, at {range:?}"
        );
    }
}

const PYTHON: &str = "import os\nMAX_SIZE = 10\n\nclass Shape(object):\n    \"\"\"A shape.\"\"\"\n\n    def __init__(self, name):\n        self.name = name\n        print(len(name), MAX_SIZE)\n\n    @property\n    def area(self):\n        # nothing yet\n        return None if self.name else isinstance(self, Shape)\n\n\ndef main(argv):\n    for arg in argv:\n        shape = Shape(arg)\n        print(f\"{shape.name}: {shape.area}\")\n    return 0\n";

const JAVASCRIPT: &str = "const fs = require('fs');\nconst MAX = 3;\n\nclass Shape extends Base {\n  constructor(name) {\n    super();\n    this.name = name;\n  }\n\n  get area() {\n    // nothing yet\n    return undefined;\n  }\n}\n\nfunction main(argv) {\n  for (const arg of argv) {\n    const shape = new Shape(arg);\n    console.log(`${shape.name}: ${shape.area}`, MAX);\n  }\n  return fs.existsSync('x') ? 0 : 1;\n}\n\nmodule.exports = { main };\n";

const C: &str = "#include <stdio.h>\n#define MAX_SIZE 10\n\nstruct shape {\n    const char *name;\n    int sides;\n};\n\n/* The area of a shape. */\nstatic int area(const struct shape *s) {\n    return s->sides * MAX_SIZE;\n}\n\nint main(int argc, char **argv) {\n    struct shape square = {\"square\", 4};\n    for (int i = 0; i < argc; i++) {\n        printf(\"%s: %d\\n\", argv[i], area(&square));\n    }\n    return NULL == argv;\n}\n";

fn small_op() -> impl Strategy<Value = Op> {
    let snippet = prop_oneof![
        Just("{".to_owned()),
        Just("}".to_owned()),
        Just("(".to_owned()),
        Just("\n".to_owned()),
        Just("\"".to_owned()),
        Just("'".to_owned()),
        Just("#".to_owned()),
        Just("//".to_owned()),
        Just("/*".to_owned()),
        Just("X".to_owned()),
        Just("x".to_owned()),
        Just("_".to_owned()),
        Just(" ".to_owned()),
        Just(".".to_owned()),
        Just("ö".to_owned()),
        Just("self".to_owned()),
        "[a-zA-Z(); =:]{1,4}",
    ];
    prop_oneof![
        (any::<usize>(), snippet).prop_map(|(pos, text)| Op::Insert(pos, text)),
        (any::<usize>(), 1usize..4).prop_map(|(pos, len)| Op::Delete(pos, len)),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Windows on a text ask for their highlights after each change, as
    /// the editor does when it draws, and get what is remembered where
    /// the change left that standing. It is what painting anew gives.
    #[test]
    fn highlights_kept_across_a_change_are_those_of_the_text(
        which in 0usize..4,
        ops in prop::collection::vec((small_op(), any::<usize>(), any::<bool>()), 1..40),
    ) {
        let (name, source) =
            [("rust", SOURCE), ("python", PYTHON), ("javascript", JAVASCRIPT), ("c", C)][which];
        let language = language(name).unwrap();
        let mut text = Text::from(source);
        let mut syntax = Syntax::new(language).unwrap();
        syntax.update(&text);
        let window = 400;
        let mut top = 0;
        // The window as the editor asks for it: so many chars from a char.
        let shown = |text: &Text, top: usize| {
            let top = top.min(text.len());
            text.char_to_byte(top)..text.char_to_byte((top + window).min(text.len()))
        };
        syntax.highlights(&text, shown(&text, top));
        for (op, scroll, twice) in ops {
            let edits = if twice { 2 } else { 1 };
            for _ in 0..edits {
                match &op {
                    Op::Insert(pos, inserted) => {
                        let pos = pos % (text.len() + 1);
                        syntax.before_insert(&text, pos, inserted);
                        text.insert(pos, inserted);
                    }
                    Op::Delete(pos, len) => {
                        let start = pos % (text.len() + 1);
                        let end = (start + len).min(text.len());
                        syntax.before_delete(&text, start..end);
                        text.remove(start..end);
                    }
                }
            }
            syntax.update(&text);
            // The window stays, moves by a little or goes elsewhere.
            top = match scroll % 4 {
                0 => top,
                1 => top + scroll % 90,
                2 => top.saturating_sub(scroll % 90),
                _ => scroll % (text.len() + 1),
            };
            let range = shown(&text, top);
            let mut fresh = Syntax::new(language).unwrap();
            fresh.update(&text);
            prop_assert_eq!(
                syntax.highlights(&text, range.clone()),
                fresh.highlights(&text, range.clone()),
                "{} at {:?} after {:?}", name, range, op
            );
        }
    }
}

/// Brings a tree up to date as the editor does when it draws, and waits
/// for the thread that parses to be done.
fn settle(syntax: &mut Syntax, text: &Text) {
    let notify: amacs_treesit::Notify = std::sync::Arc::new(|| {});
    let started = std::time::Instant::now();
    loop {
        syntax.update_soon(text, &notify);
        if !syntax.is_parsing() {
            return;
        }
        assert!(started.elapsed().as_secs() < 20, "the parse never came");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[test]
fn text_with_errors_is_parsed_on_another_thread() {
    let rust = language("rust").unwrap();
    let mut text = Text::from(SOURCE);
    let mut syntax = Syntax::new(rust).unwrap();
    let woken = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let notify: amacs_treesit::Notify = {
        let woken = woken.clone();
        std::sync::Arc::new(move || {
            woken.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        })
    };
    // The first parse, and a change that leaves no error, are done at once.
    syntax.update_soon(&text, &notify);
    assert!(!syntax.is_parsing());
    let at = SOURCE.find("fn ").unwrap() + 3;
    syntax.before_insert(&text, at, "x");
    text.insert(at, "x");
    syntax.update_soon(&text, &notify);
    assert!(!syntax.is_parsing());

    // A bracket that is not closed is an error: the tree comes later, and
    // there are highlights to draw with until then.
    syntax.before_insert(&text, at, "(");
    text.insert(at, "(");
    syntax.update_soon(&text, &notify);
    assert!(syntax.is_parsing());
    let all = 0..text.len_bytes();
    let _ = syntax.highlights(&text, all.clone());
    let started = std::time::Instant::now();
    while woken.load(std::sync::atomic::Ordering::SeqCst) == 0 {
        assert!(started.elapsed().as_secs() < 20, "the parse never came");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    syntax.update_soon(&text, &notify);
    assert!(!syntax.is_parsing());
    let mut fresh = Syntax::new(rust).unwrap();
    fresh.update(&text);
    assert_eq!(
        syntax.highlights(&text, all.clone()),
        fresh.highlights(&text, all.clone())
    );

    // A tree that is asked for in earnest, as to indent a line, does not
    // wait for the thread.
    syntax.before_insert(&text, at, "{");
    text.insert(at, "{");
    syntax.update_soon(&text, &notify);
    assert!(syntax.is_parsing());
    syntax.update(&text);
    assert!(!syntax.is_parsing());
    let mut fresh = Syntax::new(rust).unwrap();
    fresh.update(&text);
    let all = 0..text.len_bytes();
    assert_eq!(
        syntax.highlights(&text, all.clone()),
        fresh.highlights(&text, all)
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// Text that is changed while another thread parses it is drawn from
    /// a tree of before, which must not fail, and comes to the highlights
    /// that the text has.
    #[test]
    fn drawing_while_parsing_comes_to_the_highlights_of_the_text(
        which in 0usize..4,
        ops in prop::collection::vec((small_op(), any::<usize>(), 0usize..3), 1..40),
    ) {
        let (name, source) =
            [("rust", SOURCE), ("python", PYTHON), ("javascript", JAVASCRIPT), ("c", C)][which];
        let language = language(name).unwrap();
        let notify: amacs_treesit::Notify = std::sync::Arc::new(|| {});
        let mut text = Text::from(source);
        let mut syntax = Syntax::new(language).unwrap();
        syntax.update_soon(&text, &notify);
        for (op, top, wait) in ops {
            match &op {
                Op::Insert(pos, inserted) => {
                    let pos = pos % (text.len() + 1);
                    syntax.before_insert(&text, pos, inserted);
                    text.insert(pos, inserted);
                }
                Op::Delete(pos, len) => {
                    let start = pos % (text.len() + 1);
                    let end = (start + len).min(text.len());
                    syntax.before_delete(&text, start..end);
                    text.remove(start..end);
                }
            }
            // Drawn at once, drawn after the parse, or asked in earnest.
            match wait {
                0 => syntax.update_soon(&text, &notify),
                1 => settle(&mut syntax, &text),
                _ => syntax.update(&text),
            }
            let top = top % (text.len() + 1);
            let range = text.char_to_byte(top)..text.char_to_byte((top + 400).min(text.len()));
            let drawn = syntax.highlights(&text, range.clone());
            if !syntax.is_parsing() {
                let mut fresh = Syntax::new(language).unwrap();
                fresh.update(&text);
                prop_assert_eq!(
                    drawn,
                    fresh.highlights(&text, range.clone()),
                    "{} at {:?} after {:?}", name, range, op
                );
            }
        }
        settle(&mut syntax, &text);
        let mut fresh = Syntax::new(language).unwrap();
        fresh.update(&text);
        let all = 0..text.len_bytes();
        prop_assert_eq!(syntax.highlights(&text, all.clone()), fresh.highlights(&text, all));
    }
}
