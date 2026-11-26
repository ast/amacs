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

/// The rows of the window that hold text.
fn rows(h: &Harness) -> Vec<String> {
    (0..14)
        .map(|row| h.screen.row(row))
        .take_while(|row| !row.is_empty())
        .collect()
}

const FOLDED: &str = "# One\none\n## One a\na\n### Deep\ndeep\n## One b\nb\n# Two\ntwo\n";

#[test]
fn tab_folds_a_heading_then_shows_its_headings_then_all() {
    let mut h = setup(FOLDED);
    h.keys("TAB");
    assert_eq!(rows(&h), ["# One…", "# Two", "two"]);
    assert_eq!(h.echo(), "Folded");
    h.keys("TAB");
    assert_eq!(
        rows(&h),
        ["# One", "one", "## One a…", "## One b…", "# Two", "two"]
    );
    assert_eq!(h.echo(), "Headings shown");
    h.keys("TAB");
    assert_eq!(rows(&h).len(), 10);
    assert_eq!(h.echo(), "All shown");

    // A heading with text alone under it has two states.
    goto(&mut h, "# Two");
    h.keys("TAB");
    assert_eq!(rows(&h).last().unwrap(), "# Two…");
    h.keys("TAB");
    assert_eq!(h.echo(), "All shown");
    assert_eq!(rows(&h).len(), 10);
    h.keys("TAB");
    assert_eq!(h.echo(), "Folded");
    h.keys("TAB");

    // The next step is known by what is shown, so moving about between
    // the steps changes nothing; folding something else does.
    h.keys("M-< TAB TAB C-f C-b TAB");
    assert_eq!(h.echo(), "All shown");
    h.keys("TAB TAB C-n C-n TAB TAB M-< TAB");
    assert_eq!(h.echo(), "Folded");
    h.keys("TAB TAB");
    assert_eq!(rows(&h).len(), 10);

    // What was folded under a heading stays so when the heading is folded
    // and shown again.
    goto(&mut h, "### Deep");
    h.keys("TAB");
    goto(&mut h, "## One a");
    h.keys("TAB TAB");
    assert_eq!(rows(&h)[2..5], ["## One a", "a", "### Deep…"]);

    // Text is typed at the end of a folded heading, and RET there opens a
    // line under it.
    h.keys("M-< TAB C-e").type_text("!");
    assert_eq!(rows(&h)[..2], ["# One!…", "# Two"]);
}

#[test]
fn shift_tab_goes_from_the_top_headings_to_all_headings_to_all() {
    let mut h = setup(&format!("intro\n{FOLDED}"));
    h.keys("S-TAB");
    assert_eq!(rows(&h), ["intro", "# One…", "# Two…"]);
    assert_eq!(h.echo(), "Headings of the highest level");
    h.keys("S-TAB");
    assert_eq!(
        rows(&h),
        [
            "intro",
            "# One…",
            "## One a…",
            "### Deep…",
            "## One b…",
            "# Two…"
        ]
    );
    assert_eq!(h.echo(), "All headings");
    h.keys("S-TAB");
    assert_eq!(rows(&h).len(), 11);
    assert_eq!(h.echo(), "All shown");

    // Point in text that is folded away goes to its heading.
    goto(&mut h, "deep");
    h.keys("S-TAB");
    assert_eq!(line(&mut h), "# One");
    // Going to a heading by its name shows it.
    h.keys("C-c C-j").type_text("Deep");
    h.keys("RET");
    assert_eq!(line(&mut h), "### Deep");
    assert_eq!(
        rows(&h),
        [
            "intro", "# One", "one", "## One a", "a", "### Deep", "deep", "## One b", "b", "# Two…"
        ]
    );
}

#[test]
fn a_folded_subtree_stays_folded_when_it_moves() {
    let mut h = setup(FOLDED);
    h.keys("TAB M-<down>");
    assert_eq!(rows(&h), ["# Two", "two", "# One…"]);
    assert_eq!(line(&mut h), "# One");
    h.keys("C-p C-p TAB M-<down>");
    assert_eq!(rows(&h), ["# One…", "# Two…"]);
    h.keys("M-<up>");
    assert_eq!(rows(&h), ["# Two…", "# One…"]);
    // Its level changes with its text hidden.
    h.keys("M-S-<right>");
    assert_eq!(rows(&h), ["## Two…", "# One…"]);
    assert!(h.text().starts_with("## Two\ntwo\n# One\n"), "{}", h.text());
}
