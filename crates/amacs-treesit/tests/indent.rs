//! Indentation: each sample, stripped of its indentation and indented again
//! line by line, comes out as it was written.

use amacs_text::Text;
use amacs_treesit::{IndentParams, Syntax, language};

fn fixture(file: &str) -> String {
    let path = format!("{}/tests/fixtures/{file}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(path).expect("the fixture exists")
}

/// Removes all indentation and puts it back one line at a time, as
/// `indent-region` does.
fn reindent(name: &str, source: &str, offset: usize) -> String {
    let stripped: String = source
        .lines()
        .map(|line| format!("{}\n", line.trim_start()))
        .collect();
    let mut text = Text::from(stripped.as_str());
    let mut syntax = Syntax::new(language(name).expect("a known language")).expect("a grammar");
    let params = IndentParams {
        offset,
        tab_width: 8,
    };
    for line in 0..text.len_lines() {
        syntax.update(&text);
        let start = text.line_start(line);
        if start == text.line_end(line) {
            continue;
        }
        if let Some(column) = syntax.indent_column(&text, line, params) {
            let indentation = " ".repeat(column);
            syntax.before_insert(&text, start, &indentation);
            text.insert(start, &indentation);
        }
    }
    text.to_string()
}

fn assert_round_trip(name: &str, file: &str, offset: usize) {
    let source = fixture(file);
    let result = reindent(name, &source, offset);
    if result != source {
        let diff: Vec<String> = source
            .lines()
            .zip(result.lines())
            .filter(|(want, got)| want != got)
            .map(|(want, got)| format!("want: {want:?}\n got: {got:?}"))
            .collect();
        panic!("{file} is indented differently:\n{}", diff.join("\n"));
    }
}

macro_rules! round_trips {
    ($( $test:ident: $language:literal, $file:literal, $offset:literal; )*) => {
        $(
            #[test]
            fn $test() {
                assert_round_trip($language, $file, $offset);
            }
        )*
    };
}

round_trips! {
    bash: "bash", "sample.sh", 4;
    c: "c", "sample.c", 2;
    cmake: "cmake", "CMakeLists.txt", 2;
    cpp: "cpp", "sample.cpp", 2;
    css: "css", "sample.css", 2;
    devicetree: "devicetree", "sample.dts", 4;
    go: "go", "sample.go", 4;
    hcl: "hcl", "sample.tf", 2;
    javascript: "javascript", "sample.js", 2;
    json: "json", "sample.json", 2;
    nix: "nix", "sample.nix", 2;
    rust: "rust", "sample.rs", 4;
    toml: "toml", "sample.toml", 2;
    tsx: "tsx", "sample.tsx", 2;
    typescript: "typescript", "sample.ts", 2;
}

fn indent_of_last_line(name: &str, source: &str, offset: usize) -> Option<usize> {
    let text = Text::from(source);
    let mut syntax = Syntax::new(language(name).unwrap()).unwrap();
    syntax.update(&text);
    let params = IndentParams {
        offset,
        tab_width: 8,
    };
    syntax.indent_column(&text, text.len_lines() - 1, params)
}

#[test]
fn an_unfinished_block_indents_the_next_line() {
    assert_eq!(indent_of_last_line("rust", "fn main() {\n", 4), Some(4));
    assert_eq!(
        indent_of_last_line("rust", "fn main() {\n    let x = foo(\n", 4),
        Some(8)
    );
    assert_eq!(
        indent_of_last_line("c", "int main() {\n  if (x) {\n", 2),
        Some(4)
    );
    assert_eq!(
        indent_of_last_line("go", "func main() {\n    for {\n", 4),
        Some(8)
    );
    assert_eq!(
        indent_of_last_line("javascript", "const a = {\n", 2),
        Some(2)
    );
}

#[test]
fn a_blank_line_in_a_block_is_indented_like_the_block() {
    assert_eq!(
        indent_of_last_line("rust", "fn main() {\n    foo();\n", 4).map(|_| ()),
        Some(())
    );
    let source = "fn main() {\n    foo();\n\n}\n";
    let text = Text::from(source);
    let mut syntax = Syntax::new(language("rust").unwrap()).unwrap();
    syntax.update(&text);
    let params = IndentParams {
        offset: 4,
        tab_width: 8,
    };
    assert_eq!(syntax.indent_column(&text, 2, params), Some(4));
    assert_eq!(syntax.indent_column(&text, 3, params), Some(0));
}

#[test]
fn lines_inside_a_string_are_left_alone() {
    let source = "fn main() {\n    let s = \"one\ntwo\";\n}\n";
    let text = Text::from(source);
    let mut syntax = Syntax::new(language("rust").unwrap()).unwrap();
    syntax.update(&text);
    let params = IndentParams {
        offset: 4,
        tab_width: 8,
    };
    assert_eq!(syntax.indent_column(&text, 2, params), None);
}

#[test]
fn python_follows_the_previous_line() {
    assert_eq!(indent_of_last_line("python", "def f():\n", 4), Some(4));
    assert_eq!(
        indent_of_last_line("python", "def f():\n    x = 1\n", 4),
        Some(4)
    );
    assert_eq!(
        indent_of_last_line("python", "def f():\n    x = [\n", 4),
        Some(8)
    );
    assert_eq!(
        indent_of_last_line("yaml", "services:\n  web:\n", 2),
        Some(4)
    );
}

#[test]
#[ignore = "prints a syntax tree, for working on the indentation rules"]
fn print_tree() {
    let name = std::env::var("TREE_LANGUAGE").unwrap_or_else(|_| "css".to_owned());
    let file = std::env::var("TREE_FILE").unwrap_or_else(|_| "sample.css".to_owned());
    let text = Text::from(fixture(&file).as_str());
    let mut syntax = Syntax::new(language(&name).unwrap()).unwrap();
    syntax.update(&text);
    println!("{}", syntax.sexp().unwrap());
}
