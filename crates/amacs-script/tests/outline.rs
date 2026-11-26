//! Headings and what is under them, which is written in Scheme:
//! `runtime/lib/outline.scm`. Markdown stands for the modes that have
//! headings.

mod common;

use common::Harness;

/// An editor with a Markdown buffer that holds a text, point at its start.
fn setup(text: &str) -> Harness {
    let mut h = Harness::new(80, 16);
    h.eval(r#"(switch-to-buffer "notes.md") (markdown-mode)"#);
    h.paste(text);
    h.keys("M-<");
    h
}

/// Puts point at the first occurrence of a text in the buffer.
fn goto(h: &mut Harness, text: &str) {
    h.eval(&format!(
        "(goto-char (string-search (buffer-string) {text:?}))"
    ));
}

/// The line that point is on.
fn line(h: &mut Harness) -> String {
    let quoted = h.eval("(buffer-substring (line-beginning-position) (line-end-position))");
    quoted.trim_matches('"').to_owned()
}

const TEXT: &str = "intro\n\n# One\n\none\n\n## One a\n\na\n\n## One b\n\nb\n\n# Two\n\ntwo\n";

#[test]
fn up_goes_to_the_heading_above_this_one() {
    let mut h = setup(TEXT);
    goto(&mut h, "b\n\n# Two");
    h.keys("C-c C-u");
    assert_eq!(line(&mut h), "## One b");
    h.keys("C-c C-u");
    assert_eq!(line(&mut h), "# One");
    h.keys("C-c C-u");
    assert_eq!(h.echo(), "No heading above this");
    h.keys("M-< C-c C-u");
    assert_eq!(h.echo(), "No heading above this");
}

#[test]
fn headings_of_a_level_are_gone_through_under_their_heading() {
    let mut h = setup(TEXT);
    goto(&mut h, "## One a");
    h.keys("C-c C-f");
    assert_eq!(line(&mut h), "## One b");
    h.keys("C-c C-f");
    assert_eq!(h.echo(), "No heading of this level after this");
    assert_eq!(line(&mut h), "## One b");
    h.keys("C-c C-b");
    assert_eq!(line(&mut h), "## One a");
    h.keys("C-c C-b");
    assert_eq!(h.echo(), "No heading of this level before this");

    // From text, back is to its own heading first.
    goto(&mut h, "two\n");
    h.keys("C-c C-b");
    assert_eq!(line(&mut h), "# Two");
    h.keys("C-c C-b");
    assert_eq!(line(&mut h), "# One");
    h.keys("C-c C-f");
    assert_eq!(line(&mut h), "# Two");
}

#[test]
fn a_subtree_changes_level_with_all_under_it() {
    let mut h = setup(TEXT);
    goto(&mut h, "# One");
    h.keys("M-S-<right>");
    assert_eq!(
        h.text(),
        "intro\n\n## One\n\none\n\n### One a\n\na\n\n### One b\n\nb\n\n# Two\n\ntwo\n"
    );
    h.keys("M-S-<left>");
    assert_eq!(h.text(), TEXT);
    // None changes if one of them cannot.
    h.keys("M-S-<left>");
    assert_eq!(h.echo(), "Headings have levels 1 to 6");
    assert_eq!(h.text(), TEXT);
    // The heading alone leaves those under it.
    h.keys("M-<right>");
    assert!(
        h.text().contains("\n## One\n\none\n\n## One a\n"),
        "{}",
        h.text()
    );
    h.keys("C-n M-<right>");
    assert_eq!(h.echo(), "Point is not on a heading");
}

#[test]
fn a_subtree_moves_past_the_one_beside_it() {
    let mut h = setup(TEXT);
    goto(&mut h, "one\n");
    h.keys("C-f M-<down>");
    assert_eq!(
        h.text(),
        "intro\n\n# Two\n\ntwo\n# One\n\none\n\n## One a\n\na\n\n## One b\n\nb\n\n"
    );
    // Point stays in the text that moved.
    assert_eq!(
        h.eval("(buffer-substring (- (point) 1) (+ (point) 2))"),
        "\"one\""
    );
    h.keys("M-<down>");
    assert_eq!(h.echo(), "No heading of this level after this");
    h.keys("M-<up>");
    assert_eq!(
        h.text(),
        "intro\n\n# One\n\none\n\n## One a\n\na\n\n## One b\n\nb\n\n# Two\n\ntwo\n"
    );
    h.keys("M-<up>");
    assert_eq!(h.echo(), "No heading of this level before this");

    // Under a heading, only among those under it.
    goto(&mut h, "## One b");
    h.keys("M-<down>");
    assert_eq!(h.echo(), "No heading of this level after this");
    h.keys("M-<up>");
    assert!(
        h.text()
            .contains("one\n\n## One b\n\nb\n\n## One a\n\na\n\n# Two"),
        "{}",
        h.text()
    );
    h.keys("C-/");
    assert_eq!(h.text(), TEXT);
}

#[test]
fn the_last_subtree_of_a_text_without_a_last_newline_moves_too() {
    let mut h = setup("# One\none\n# Two\ntwo");
    goto(&mut h, "# Two");
    h.keys("M-<up>");
    assert_eq!(h.text(), "# Two\ntwo\n# One\none");
    h.keys("M-<down>");
    assert_eq!(h.text(), "# One\none\n# Two\ntwo");
}

#[test]
fn a_mode_without_headings_says_so() {
    let mut h = Harness::new(60, 8);
    h.eval("(global-set-key \"<f5>\" 'outline-next-heading)");
    h.keys("<f5>");
    assert_eq!(h.echo(), "This mode has no headings");
}
