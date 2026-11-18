//! Editing Markdown, which is written in Scheme: `runtime/lib/markdown.scm`.

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

/// The text with `^` where point is.
fn with_point(h: &mut Harness) -> String {
    let point: usize = h.eval("(point)").parse().unwrap();
    let text = h.text();
    let byte = text
        .char_indices()
        .nth(point)
        .map_or(text.len(), |(byte, _)| byte);
    format!("{}^{}", &text[..byte], &text[byte..])
}

#[test]
fn tab_lines_up_a_table_and_goes_from_cell_to_cell() {
    let mut h = setup("|Key|Does|Count|\n|---|:-:|--:|\n|`RET`|Accept|1|\n|å|ö\n\nafter\n");
    h.keys("TAB");
    assert_eq!(
        with_point(&mut h),
        "\
| Key   |  ^Does  | Count |
| ----- | :----: | ----: |
| `RET` | Accept |     1 |
| å     |   ö    |       |

after
"
    );
    // Over the row of dashes, to the next row; and back again.
    h.keys("TAB TAB");
    assert!(
        with_point(&mut h).contains("\n| ^`RET` |"),
        "{}",
        with_point(&mut h)
    );
    h.keys("S-TAB");
    assert!(
        with_point(&mut h).contains("| ^Count |"),
        "{}",
        with_point(&mut h)
    );
    // After the last cell comes a new row.
    h.keys("C-n C-n C-n TAB");
    assert_eq!(
        with_point(&mut h),
        "\
| Key   |  Does  | Count |
| ----- | :----: | ----: |
| `RET` | Accept |     1 |
| å     |   ö    |       |
| ^      |        |       |

after
"
    );
    // What is typed widens the column the next time.
    h.type_text("a longer cell").keys("TAB");
    assert!(
        h.text()
            .starts_with("| Key           |  Does  | Count |\n| ------------- |"),
        "{}",
        h.text()
    );
    assert!(
        with_point(&mut h).contains("| a longer cell | ^       |"),
        "{}",
        with_point(&mut h)
    );
    // Lining up is one step to undo.
    h.keys("C-/");
    assert!(
        h.text().starts_with("| Key   |  Does  | Count |\n"),
        "{}",
        h.text()
    );
    assert!(h.text().contains("| a longer cell"), "{}", h.text());
}

#[test]
fn a_table_keeps_bars_that_are_escaped_and_wide_chars_in_line() {
    let mut h = setup("  | a\\|b | 日本 |\n  |--|--|\n  | c | d |\n");
    h.keys("C-c C-c");
    assert_eq!(
        h.text(),
        "  | a\\|b | 日本 |\n  | ---- | ---- |\n  | c    | d    |\n"
    );
    assert_eq!(
        h.eval(r#"(markdown-format-table '("|a|" "|-|" "|b|c|"))"#),
        r#"("| a   |     |" "| --- | --- |" "| b   | c   |")"#
    );
}

#[test]
fn a_table_is_inserted_ready_to_fill_in() {
    let mut h = setup("");
    h.keys("C-c |").type_text("2").keys("RET");
    // The prompt starts with 3.
    assert_eq!(h.text().lines().count(), 3, "{}", h.text());
    h.type_text("x");
    assert!(h.text().starts_with("| x "), "{}", h.text());
}

#[test]
fn return_goes_on_with_a_list_and_ends_it_on_an_empty_item() {
    let mut h = setup("- one");
    h.keys("C-e RET").type_text("two").keys("RET RET");
    assert_eq!(with_point(&mut h), "- one\n- two\n^");

    let mut h = setup("  9. nine");
    h.keys("C-e RET").type_text("ten");
    assert_eq!(h.text(), "  9. nine\n  10. ten");

    let mut h = setup("- [x] done");
    h.keys("C-e RET").type_text("next");
    assert_eq!(h.text(), "- [x] done\n- [ ] next");
    // In the middle of an item, the rest becomes the next item.
    h.keys("C-b C-b RET");
    assert_eq!(h.text(), "- [x] done\n- [ ] ne\n- [ ] xt");

    // Ordinary lines are ordinary.
    let mut h = setup("plain");
    h.keys("C-e RET").type_text("text");
    assert_eq!(h.text(), "plain\ntext");
}

#[test]
fn boxes_are_ticked_and_cleared() {
    let mut h = setup("- [ ] task\n* plain\n");
    h.keys("C-e C-c C-c");
    assert_eq!(with_point(&mut h), "- [x] task^\n* plain\n");
    h.keys("C-c C-c");
    assert!(h.text().starts_with("- [ ] task"));
    h.keys("C-n C-c C-c");
    assert_eq!(h.text(), "- [ ] task\n* [ ] plain\n");
    h.keys("M-> C-c C-c");
    assert!(h.echo().contains("not in an item"), "{}", h.echo());
}

#[test]
fn headings_are_places_to_go_and_can_change_level() {
    let mut h = setup("# One\n\ntext\n\n```sh\n# not a heading\n```\n\n## Two\n\n#hashtag\n");
    assert_eq!(
        h.eval("(map cdr (markdown-headings))"),
        r#"((1 "One") (2 "Two"))"#
    );
    h.keys("C-c C-n");
    assert!(
        with_point(&mut h).contains("^## Two"),
        "{}",
        with_point(&mut h)
    );
    h.keys("C-c C-n");
    assert_eq!(h.echo(), "No heading after this");
    h.keys("M-<right>");
    assert!(h.text().contains("\n### Two"), "{}", h.text());
    h.keys("M-<left> M-<left>");
    assert!(h.text().contains("\n# Two"), "{}", h.text());
    h.keys("M-<left>");
    assert!(h.echo().contains("levels 1 to 6"), "{}", h.echo());
    h.keys("C-c C-p");
    assert!(
        with_point(&mut h).starts_with("^# One"),
        "{}",
        with_point(&mut h)
    );
    h.keys("M-> C-c C-j").type_text("Two").keys("RET");
    assert!(
        with_point(&mut h).contains("^# Two"),
        "{}",
        with_point(&mut h)
    );
}
