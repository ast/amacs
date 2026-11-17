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
