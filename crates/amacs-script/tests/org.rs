//! Org, which is written in Scheme, `runtime/lib/org.scm`, but for its
//! colours, which are in `crates/amacs-treesit/src/org.rs`.

mod common;

use common::Harness;

#[test]
fn an_org_file_opens_in_org_mode_with_its_colours() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes.org");
    std::fs::write(
        &file,
        "#+title: Notes\n* TODO One :tag:\n** DONE Two\nSee ~code~ and [[https://example.org][a site]].\n\
         #+begin_src sh\necho hi # there\n#+end_src\n",
    )
    .unwrap();
    let mut h = Harness::new(60, 12);
    h.eval(&format!("(find-file {:?})", file.display().to_string()));
    assert!(h.mode_line().contains("(Org)"), "{}", h.mode_line());

    assert_eq!(h.screen.face_at(0, 0), h.face("prose-metadata"));
    assert_eq!(h.screen.face_at(0, 9), h.face("heading"));
    assert_eq!(h.screen.face_at(1, 0), h.face("heading"));
    assert_eq!(h.screen.face_at(1, 2), h.face("prose-todo"));
    assert_eq!(h.screen.face_at(1, 11), h.face("prose-tag"));
    assert_eq!(h.screen.face_at(2, 0), h.face("heading-2"));
    assert_eq!(h.screen.face_at(2, 3), h.face("prose-done"));
    assert_eq!(h.screen.face_at(3, 5), h.face("code"));
    assert_eq!(h.screen.face_at(3, 38), h.face("link"));
    assert_eq!(h.screen.face_at(4, 0), h.face("prose-metadata"));
    assert_eq!(h.screen.face_at(5, 0), h.face("function-name"));
    assert_eq!(h.screen.face_at(5, 8), h.face("comment"));
    assert_eq!(
        h.eval("(begin (goto-char 3) (syntax-at))"),
        "\"prose-metadata\""
    );

    // The words for the states of a heading are those that the buffer is
    // given.
    h.eval("(set-syntax-keywords! '(\"WAIT\") '(\"TODO\"))");
    assert_eq!(h.screen.face_at(1, 2), h.face("prose-done"));
    assert_eq!(h.screen.face_at(2, 3), h.face("heading-2"));
}

/// An editor with a buffer of Org that holds a text, point at its start.
fn setup(text: &str) -> Harness {
    let mut h = Harness::new(80, 16);
    h.eval(r#"(switch-to-buffer "notes.org") (org-mode)"#);
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

/// The line that point is on.
fn line(h: &mut Harness) -> String {
    let quoted = h.eval("(buffer-substring (line-beginning-position) (line-end-position))");
    quoted.trim_matches('"').to_owned()
}

#[test]
fn a_heading_is_read_into_its_parts() {
    let mut h = Harness::new(60, 8);
    let mut parts = |line: &str| h.eval(&format!("(org-parse-heading {line:?})"));
    assert_eq!(parts("* One"), r#"(1 #f "One" ())"#);
    assert_eq!(
        parts("** TODO Write it  :work:at_home:"),
        r#"(2 "TODO" "Write it" ("work" "at_home"))"#
    );
    assert_eq!(parts("* DONE"), r#"(1 "DONE" "" ())"#);
    assert_eq!(parts("* TODOS are words"), r#"(1 #f "TODOS are words" ())"#);
    assert_eq!(
        parts("* A time 10:30: no tags"),
        r#"(1 #f "A time 10:30: no tags" ())"#
    );
    assert_eq!(parts("*strong* text"), "#f");
    assert_eq!(parts(" * an item"), "#f");
    assert_eq!(
        h.eval(r#"(org-parse-heading "* WAIT for it" '("WAIT"))"#),
        r#"(1 "WAIT" "for it" ())"#
    );
    assert_eq!(
        h.eval(r#"(org-headings-in "text\n* One :a:\n** TODO Two\n*not\n")"#),
        r#"((5 1 "One") (15 2 "TODO Two"))"#
    );
}

#[test]
fn what_is_at_point_is_known_by_its_kind() {
    let mut h = setup(
        "#+title: Notes\n* TODO One :a:\ntext\n- [ ] item\n| a | b |\n#+begin_src sh :tangle x\necho\n#+end_src\n",
    );
    let mut at = |text: &str| {
        goto(&mut h, text);
        h.eval("(org-element-at)")
    };
    assert_eq!(at("#+title"), r#"(keyword 0 "title" "Notes")"#);
    assert_eq!(at("* TODO"), r#"(heading 15 1 "TODO" "One" ("a"))"#);
    assert_eq!(at("text"), "(paragraph 30)");
    assert_eq!(at("- [ ]"), r#"(item 35 "" "-" "[ ]" "item")"#);
    assert_eq!(at("| a"), "(table 46 55)");
    assert_eq!(at("echo"), r#"(block 56 95 "src" "sh :tangle x")"#);
    assert_eq!(at("#+end_src"), r#"(block 56 95 "src" "sh :tangle x")"#);
}

#[test]
fn tab_lines_up_a_table_with_a_rule_of_plus_signs() {
    let mut h = setup("| Pin | Function |\n|-\n| B3 | SPI SCK | 7 |\n");
    h.keys("TAB");
    assert_eq!(
        with_point(&mut h),
        "\
| Pin | ^Function |   |
|-----+----------+---|
| B3  | SPI SCK  | 7 |
"
    );
    // The rule holds no cells: the next cell after a row is in the row
    // after the rule.
    h.keys("TAB TAB");
    assert!(
        with_point(&mut h).contains("| ^B3  |"),
        "{}",
        with_point(&mut h)
    );
    h.keys("S-TAB");
    assert!(
        with_point(&mut h).contains("| ^  |\n|--"),
        "{}",
        with_point(&mut h)
    );
    // After the last cell, a new row.
    h.keys("M-> C-p C-e TAB");
    assert_eq!(
        with_point(&mut h),
        "\
| Pin | Function |   |
|-----+----------+---|
| B3  | SPI SCK  | 7 |
| ^    |          |   |
"
    );
    h.keys("C-c C-c");
    assert_eq!(h.echo(), "");

    // A table that is made has a rule of this kind.
    h.keys("M-> RET C-c | DEL").type_text("2");
    h.keys("RET");
    assert!(
        h.text().ends_with("\n\n|   |   |\n|---+---|\n|   |   |"),
        "{}",
        h.text()
    );
}

#[test]
fn a_column_of_numbers_is_lined_up_to_the_right() {
    let mut h = setup("|Pin|code|note|\n|-\n|B3|7|x|\n|B10|11|12|\n|B4||y|\n");
    h.keys("C-c C-c");
    assert_eq!(
        h.text(),
        "\
| Pin | code | note |
|-----+------+------|
| B3  |    7 | x    |
| B10 |   11 | 12   |
| B4  |      | y    |
"
    );
}

#[test]
fn a_ticked_box_gets_a_capital_x_and_lists_go_on() {
    let mut h = setup("- [ ] one\n- [-] two\n");
    h.keys("C-c C-c");
    assert_eq!(h.text(), "- [X] one\n- [-] two\n");
    h.keys("C-c C-c C-n C-c C-c");
    assert_eq!(h.text(), "- [ ] one\n- [X] two\n");
    h.keys("C-e RET").type_text("three");
    assert_eq!(h.text(), "- [ ] one\n- [X] two\n- [ ] three\n");
    // An item in the middle of its text gets its follower with M-RET.
    h.keys("C-a M-RET").type_text("four");
    assert_eq!(h.text(), "- [ ] one\n- [X] two\n- [ ] three\n- [ ] four\n");

    // A star at the start of a line is a heading, not an item.
    let mut h = setup("* [ ] not an item\n");
    h.keys("C-c C-c");
    assert_eq!(h.echo(), "There is nothing to do here");
    assert_eq!(h.text(), "* [ ] not an item\n");
}

#[test]
fn meta_return_starts_a_heading_after_the_subtree() {
    let mut h = setup("* One\ntext\n** One a\nmore\n* Two\n");
    h.keys("M-RET").type_text("New");
    assert_eq!(h.text(), "* One\ntext\n** One a\nmore\n* New\n* Two\n");
    goto(&mut h, "more");
    h.keys("M-RET").type_text("One b");
    assert_eq!(
        h.text(),
        "* One\ntext\n** One a\nmore\n** One b\n* New\n* Two\n"
    );

    // At the end of a text without a last newline, and before any heading.
    let mut h = setup("* One\ntext");
    h.keys("M-RET").type_text("Two");
    assert_eq!(h.text(), "* One\ntext\n* Two");
    let mut h = setup("text\n");
    h.keys("M-RET").type_text("One");
    assert_eq!(h.text(), "text\n* One\n");
}

#[test]
fn headings_move_and_change_level_as_in_an_outline() {
    let mut h = setup("* One\none\n** One a\n* Two\ntwo words here\n");
    h.keys("C-c C-n C-c C-n");
    assert_eq!(line(&mut h), "* Two");
    h.keys("M-<up>");
    assert_eq!(h.text(), "* Two\ntwo words here\n* One\none\n** One a\n");
    h.keys("M-<right>");
    assert!(h.text().starts_with("** Two\n"), "{}", h.text());
    h.keys("M-<left> M-<left>");
    assert_eq!(h.echo(), "A heading of level 1 cannot go up");
    // Outside a heading, the keys move by words.
    h.keys("C-n C-a M-<right> M-<right>");
    assert_eq!(h.eval("(current-column)"), "9");
    h.keys("M-<left>");
    assert_eq!(h.eval("(current-column)"), "4");
    h.keys("C-c C-n M-S-<right>");
    assert!(
        h.text().ends_with("** One\none\n*** One a\n"),
        "{}",
        h.text()
    );
    h.keys("C-c C-j").type_text("Two");
    h.keys("RET");
    assert_eq!(line(&mut h), "* Two");
}

#[test]
fn links_of_org_lead_to_headings_files_and_addresses() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(
        root.join("a.org"),
        "[[*Second part][below]] [[#named]] [[id:abc-1]] [[file:b.org::*There][other]] [[Second part]] [[marked]] [[nowhere]]\n\
         * First\n* TODO Second part :tag:\n* Third\n:PROPERTIES:\n:CUSTOM_ID: named\n:END:\n\
         * Fourth\n:PROPERTIES:\n:ID:       abc-1\n:END:\ntext <<marked>> here\n",
    )
    .unwrap();
    std::fs::write(root.join("b.org"), "* Here\n* There\n").unwrap();
    let mut h = Harness::new(100, 12);
    let visit = |h: &mut Harness| {
        h.eval(&format!(
            "(find-file {:?})",
            root.join("a.org").display().to_string()
        ));
    };
    let follow = |h: &mut Harness, link: &str| {
        visit(h);
        goto(h, link);
        h.keys("C-f C-f C-f C-c C-o");
        line(h)
    };
    assert_eq!(follow(&mut h, "[[*Second"), "* TODO Second part :tag:");
    assert_eq!(h.eval("(mark)"), "3");
    assert_eq!(follow(&mut h, "[[#named"), ":CUSTOM_ID: named");
    assert_eq!(follow(&mut h, "[[id:"), "* Fourth");
    assert_eq!(
        follow(&mut h, "[[Second part]]"),
        "* TODO Second part :tag:"
    );
    assert_eq!(follow(&mut h, "[[marked"), "text <<marked>> here");
    follow(&mut h, "[[nowhere");
    assert_eq!(h.echo(), "Found no place for nowhere");
    assert_eq!(follow(&mut h, "[[file:"), "* There");
    assert_eq!(h.eval("(buffer-name)"), "\"b.org\"");

    // The parts of a link.
    visit(&mut h);
    assert_eq!(h.eval("(link-at 3)"), r#"(0 23 "*Second part" "below")"#);
    assert_eq!(h.eval("(link-at 26)"), r##"(24 34 "#named" #f)"##);
}

#[test]
fn a_link_is_inserted_with_its_text() {
    let mut h = setup("see the site now\n");
    h.keys("C-c C-l").type_text("https://example.org");
    h.keys("RET RET");
    assert_eq!(h.text(), "[[https://example.org]]see the site now\n");
    goto(&mut h, "the site");
    h.keys("C-SPC M-f M-f C-c C-l").type_text("file:site.org");
    h.keys("RET RET");
    assert_eq!(
        h.text(),
        "[[https://example.org]]see [[file:site.org][the site]] now\n"
    );
    h.keys("C-c C-l RET");
    assert_eq!(h.echo(), "A link must have a target");
}

#[test]
fn the_state_of_a_heading_goes_through_the_keywords_and_keeps_its_tags() {
    let mut h = setup("* One  :tag:\ntext\n");
    h.eval(r#"(set-option! 'org-todo-keywords '(("TODO" "WAIT") ("DONE")))"#);
    h.keys("C-n C-c C-t");
    assert_eq!(h.text(), "* TODO One  :tag:\ntext\n");
    assert_eq!(h.echo(), "TODO");
    assert_eq!(line(&mut h), "text");
    h.keys("C-c C-t");
    assert_eq!(h.text(), "* WAIT One  :tag:\ntext\n");
    h.keys("C-c C-t");
    assert_eq!(h.text(), "* DONE One  :tag:\ntext\n");
    h.keys("C-c C-t");
    assert_eq!(h.text(), "* One  :tag:\ntext\n");
    assert_eq!(h.echo(), "No state");

    // Point in the title stays in its word.
    goto(&mut h, "ne  :tag:");
    h.keys("C-c C-t");
    assert_eq!(with_point(&mut h), "* TODO O^ne  :tag:\ntext\n");

    // A heading that is stars alone.
    let mut h = setup("* ");
    h.keys("C-e C-c C-t");
    assert_eq!(with_point(&mut h), "* TODO^");
    h.keys("C-c C-t C-c C-t");
    assert_eq!(h.text(), "* ");

    let mut h = setup("text\n");
    h.keys("C-c C-t");
    assert_eq!(h.echo(), "Point is not under a heading");
}

#[test]
fn the_states_of_the_option_have_the_colours_of_states() {
    let mut h = Harness::new(60, 8);
    h.eval(r#"(set-option! 'org-todo-keywords '(("TODO" "WAIT") ("DONE" "DELEGATED")))"#);
    h.eval(r#"(switch-to-buffer "notes.org") (org-mode)"#);
    h.paste("* WAIT one\n* DELEGATED two\n");
    assert_eq!(h.screen.face_at(0, 2), h.face("prose-todo"));
    assert_eq!(h.screen.face_at(1, 2), h.face("prose-done"));
}

#[test]
fn a_timestamp_of_today_is_inserted() {
    let mut h = setup("");
    h.keys("C-c .");
    let text = h.text();
    let bytes = text.as_bytes();
    assert_eq!(text.len(), 16, "{text}");
    assert!(text.starts_with('<') && text.ends_with('>'), "{text}");
    assert!(bytes[1..5].iter().all(u8::is_ascii_digit), "{text}");
    assert_eq!(
        (bytes[5], bytes[8], bytes[11]),
        (b'-', b'-', b' '),
        "{text}"
    );
    assert!(
        ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"].contains(&&text[12..15]),
        "{text}"
    );
    assert_eq!(h.screen.face_at(0, 3), h.face("date"));
}

#[test]
fn a_block_is_written_from_a_snippet() {
    let mut h = setup("");
    h.type_text("src");
    h.keys("M-+");
    h.type_text("sh");
    assert_eq!(h.text(), "#+begin_src sh\n\n#+end_src");
}

/// The rows of the window that hold text.
fn rows(h: &Harness) -> Vec<String> {
    (0..14)
        .map(|row| h.screen.row(row))
        .take_while(|row| !row.is_empty())
        .collect()
}

#[test]
fn tab_and_shift_tab_fold_what_is_under_headings() {
    let mut h = setup("* One\none\n** One a\na\n* TODO Two :tag:\n| a | b |\n");
    h.keys("TAB");
    assert_eq!(rows(&h), ["* One…", "* TODO Two :tag:", "| a | b |"]);
    assert_eq!(h.screen.face_at(0, 5), h.face("fold"));
    h.keys("TAB");
    assert_eq!(
        rows(&h),
        ["* One", "one", "** One a…", "* TODO Two :tag:", "| a | b |"]
    );
    h.keys("TAB S-TAB");
    assert_eq!(rows(&h), ["* One…", "* TODO Two :tag:…"]);
    h.keys("S-TAB");
    assert_eq!(rows(&h), ["* One…", "** One a…", "* TODO Two :tag:…"]);
    h.keys("S-TAB");
    assert_eq!(rows(&h).len(), 6);

    // In a table the keys go from cell to cell, and the state of a folded
    // heading changes with its text hidden.
    h.keys("M-> C-p TAB");
    assert_eq!(h.eval("(current-column)"), "6");
    h.keys("C-p TAB C-c C-t");
    assert_eq!(rows(&h)[4], "* DONE Two :tag:…");
    // A new heading goes after the text that is folded.
    h.keys("M-RET").type_text("Three");
    assert!(h.text().ends_with("| a | b |\n* Three"), "{}", h.text());
    assert_eq!(rows(&h)[4..], ["* DONE Two :tag:…", "* Three"]);
}

#[test]
fn a_file_says_how_it_is_folded_when_it_is_visited() {
    let dir = tempfile::tempdir().unwrap();
    let text = "* One\none\n** One a\na\n* Two\ntwo\n";
    let write = |name: &str, first: &str| {
        let path = dir.path().join(name);
        std::fs::write(&path, format!("{first}{text}")).unwrap();
        path.display().to_string()
    };
    let (plain, overview, content, all) = (
        write("plain.org", ""),
        write("overview.org", "#+STARTUP: overview\n"),
        write("content.org", "#+startup: indent content\n"),
        write("all.org", "#+STARTUP: showall\n"),
    );
    let mut h = Harness::new(60, 14);
    let visit = |h: &mut Harness, path: &str| {
        h.eval(&format!("(find-file {path:?})"));
        rows(h)
    };
    assert_eq!(visit(&mut h, &plain).len(), 6);
    assert_eq!(
        visit(&mut h, &overview),
        ["#+STARTUP: overview", "* One…", "* Two…"]
    );
    assert_eq!(
        visit(&mut h, &content),
        ["#+startup: indent content", "* One…", "** One a…", "* Two…"]
    );

    // The option says it for files that do not.
    h.eval("(kill-buffer \"plain.org\") (set-option! 'org-startup-folded 'overview)");
    assert_eq!(visit(&mut h, &plain), ["* One…", "* Two…"]);
    assert_eq!(visit(&mut h, &all).len(), 7);
}
