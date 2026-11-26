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
    use std::time::Instant;
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
}
