//! Highlighting of a sample of each language, as snapshots, and the tree's
//! behaviour under edits.

use amacs_text::Text;
use amacs_treesit::{Highlight, Syntax, language};

fn fixture(file: &str) -> String {
    let path = format!("{}/tests/fixtures/{file}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(path).expect("the fixture exists")
}

/// The text with each highlighted run written as `«name|text»`.
fn marked(syntax: &Syntax, text: &Text) -> String {
    let highlights = syntax.highlights(text, 0..text.len_bytes());
    let source = text.to_string();
    let mut out = String::new();
    let mut open: Option<Highlight> = None;
    for (byte, c) in source.char_indices() {
        let highlight = if c == '\n' {
            None
        } else {
            highlights.get(byte)
        };
        if highlight != open {
            if open.is_some() {
                out.push('»');
            }
            if let Some(highlight) = highlight {
                out.push('«');
                out.push_str(highlight.name());
                out.push('|');
            }
            open = highlight;
        }
        out.push(c);
    }
    if open.is_some() {
        out.push('»');
    }
    out
}

fn highlighted(name: &str, file: &str) -> String {
    let mut syntax = Syntax::new(language(name).expect("a known language")).expect("a grammar");
    let text = Text::from(fixture(file).as_str());
    syntax.update(&text);
    marked(&syntax, &text)
}

macro_rules! snapshots {
    ($( $test:ident: $language:literal, $file:literal; )*) => {
        $(
            #[test]
            fn $test() {
                insta::assert_snapshot!(highlighted($language, $file));
            }
        )*
    };
}

snapshots! {
    bash: "bash", "sample.sh";
    c: "c", "sample.c";
    cmake: "cmake", "CMakeLists.txt";
    cpp: "cpp", "sample.cpp";
    css: "css", "sample.css";
    devicetree: "devicetree", "sample.dts";
    elisp: "elisp", "sample.el";
    go: "go", "sample.go";
    hcl: "hcl", "sample.tf";
    html: "html", "sample.html";
    javascript: "javascript", "sample.js";
    json: "json", "sample.json";
    make: "make", "Makefile";
    markdown: "markdown", "sample.md";
    nix: "nix", "sample.nix";
    org: "org", "sample.org";
    python: "python", "sample.py";
    rust: "rust", "sample.rs";
    scheme: "scheme", "sample.scm";
    toml: "toml", "sample.toml";
    tsx: "tsx", "sample.tsx";
    typescript: "typescript", "sample.ts";
    yaml: "yaml", "sample.yaml";
}

#[test]
fn highlights_of_a_range_match_those_of_the_whole() {
    let mut syntax = Syntax::new(language("rust").unwrap()).unwrap();
    let text = Text::from(fixture("sample.rs").as_str());
    syntax.update(&text);
    let whole = syntax.highlights(&text, 0..text.len_bytes());
    let (start, end) = (text.line_to_byte(15), text.line_to_byte(25));
    let part = syntax.highlights(&text, start..end);
    for byte in start..end {
        assert_eq!(part.get(byte), whole.get(byte), "byte {byte}");
    }
    assert_eq!(part.get(end), None);
    assert_eq!(part.get(start - 1), None);
}

#[test]
fn highlights_of_any_lines_of_text_in_other_languages_match_those_of_the_whole() {
    any_lines_match_the_whole("markdown", "sample.md");
}

#[test]
fn highlights_of_any_lines_of_org_match_those_of_the_whole() {
    any_lines_match_the_whole("org", "sample.org");
}

#[test]
fn the_words_for_the_states_of_a_heading_can_be_set() {
    let mut syntax = Syntax::new(language("org").unwrap()).unwrap();
    let text = Text::from("* TODO one\n* FEEDBACK two\n* DELEGATED three\n* DONE four\n");
    syntax.update(&text);
    assert_eq!(
        marked(&syntax, &text),
        "«heading|* »«prose-todo|TODO»«heading| one»\n«heading|* FEEDBACK two»\n\
         «heading|* DELEGATED three»\n«heading|* »«prose-done|DONE»«heading| four»\n"
    );
    syntax.set_keywords(
        vec!["TODO".into(), "FEEDBACK".into()],
        vec!["DELEGATED".into()],
    );
    syntax.update(&text);
    assert_eq!(
        marked(&syntax, &text),
        "«heading|* »«prose-todo|TODO»«heading| one»\n«heading|* »«prose-todo|FEEDBACK»«heading| two»\n\
         «heading|* »«prose-done|DELEGATED»«heading| three»\n«heading|* DONE four»\n"
    );
}

fn any_lines_match_the_whole(name: &str, file: &str) {
    let mut syntax = Syntax::new(language(name).unwrap()).unwrap();
    let text = Text::from(fixture(file).as_str());
    syntax.update(&text);
    let whole = syntax.highlights(&text, 0..text.len_bytes());
    let lines = text.len_lines();
    let start_of = |line: usize| {
        if line >= lines {
            text.len_bytes()
        } else {
            text.line_to_byte(line)
        }
    };
    for first in 0..lines {
        for last in first..=lines {
            let (start, end) = (start_of(first), start_of(last));
            let part = syntax.highlights(&text, start..end);
            for byte in start..end {
                assert_eq!(
                    part.get(byte),
                    whole.get(byte),
                    "byte {byte} of lines {first} to {last}"
                );
            }
        }
    }
}

#[test]
fn the_markup_of_org_is_the_marks_of_emphasis_and_what_a_link_has_beside_its_text() {
    let mut syntax = Syntax::new(language("org").unwrap()).unwrap();
    let source = "* å ~c~\nö *b* [[target][text]] [[t]]\n| *cell* |\n#+begin_src sh\n*x*\n#+end_src\n_u_ a*b*c\n";
    let text = Text::from(source);
    syntax.update(&text);
    assert!(syntax.markup().is_empty(), "not asked for yet");
    syntax.update_markup(&text);
    let chars: Vec<char> = source.chars().collect();
    let found: Vec<String> = syntax
        .markup()
        .iter()
        .map(|range| chars[range.clone()].iter().collect())
        .collect();
    assert_eq!(found, ["~", "~", "*", "*", "[[target][", "]]", "[[", "]]"]);
    // In order and by chars, not bytes.
    assert_eq!(syntax.markup()[0], 4..5);
    assert!(
        syntax
            .markup()
            .windows(2)
            .all(|pair| pair[0].end <= pair[1].start)
    );
}
