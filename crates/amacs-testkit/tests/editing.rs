//! End-to-end editing tests: keys in, screen out.

use amacs_testkit::Harness;

fn harness() -> Harness {
    Harness::new(40, 8)
}

#[test]
fn typed_text_appears_with_cursor_after_it() {
    let mut h = harness();
    h.type_text("hello\nwörld");
    assert_eq!(h.screen.row(0), "hello");
    assert_eq!(h.screen.row(1), "wörld");
    assert_eq!(h.screen.cursor(), Some((1, 5)));
    assert_eq!(h.screen.row(6), " **  *scratch*   L2 C5  (Fundamental)");
    insta::assert_snapshot!(h.screen.text());
}

#[test]
fn a_typed_word_is_undone_as_a_unit_and_redone() {
    let mut h = harness();
    h.type_text("hello").keys("C-f").type_text("world");
    assert_eq!(h.buffer_text(), "helloworld");
    h.keys("C-/");
    assert_eq!(h.buffer_text(), "hello");
    assert_eq!(h.echo(), "Undo");
    h.keys("C-/");
    assert_eq!(h.buffer_text(), "");
    assert_eq!(
        h.screen.row(6),
        " --  *scratch*   L1 C0  (Fundamental)",
        "undoing everything clears the modified flag"
    );
    h.keys("C-/");
    assert_eq!(h.echo(), "No further undo information");
    h.keys("C-? C-?");
    assert_eq!(h.buffer_text(), "helloworld");
    assert_eq!(h.screen.cursor(), Some((0, 10)));
}

#[test]
fn editing_after_undo_starts_a_new_branch() {
    let mut h = harness();
    h.type_text("one").keys("RET").type_text("two");
    h.keys("C-/").type_text("2");
    assert_eq!(h.buffer_text(), "one\n2");
    h.keys("C-?");
    assert_eq!(h.echo(), "No further redo information");
    h.keys("C-/ C-/ C-/ C-/");
    assert_eq!(h.buffer_text(), "");
}

#[test]
fn kill_and_yank() {
    let mut h = harness();
    h.type_text("alpha beta\ngamma");
    h.keys("M-< C-k C-k");
    assert_eq!(h.buffer_text(), "gamma");
    h.keys("C-e C-y");
    assert_eq!(
        h.buffer_text(),
        "gammaalpha beta\n",
        "consecutive kills form one entry"
    );

    h.keys("M-DEL");
    assert_eq!(h.buffer_text(), "gammaalpha ");
    h.keys("C-y M-y");
    assert_eq!(h.buffer_text(), "gammaalpha alpha beta\n");
    // Not after a yank, M-y offers the kill ring to choose from.
    h.keys("C-f");
    h.keys("M-y");
    assert_eq!(h.prompt(), "1/2 Yank:");
    assert_eq!(h.candidates(), ["beta", "alpha beta"]);
    h.keys("C-n RET");
    assert_eq!(h.buffer_text(), "gammaalpha alpha beta\nalpha beta\n");
}

#[test]
fn region_is_highlighted_and_killed() {
    let mut h = harness();
    h.type_text("one two three");
    h.keys("C-a M-f C-SPC M-f");
    assert_eq!(h.echo(), "");
    let region_bg = h.screen.face_at(0, 4).bg;
    let plain_bg = h.screen.face_at(1, 0).bg;
    assert_ne!(region_bg, plain_bg, "selected text has a background");
    assert_eq!(h.screen.face_at(0, 2).bg, plain_bg);
    assert_eq!(h.screen.face_at(0, 8).bg, plain_bg);

    h.keys("C-w");
    assert_eq!(h.buffer_text(), "one three");
    assert_eq!(h.screen.face_at(0, 4).bg, plain_bg);
    h.keys("M-f C-w");
    assert_eq!(
        h.buffer_text(),
        "one",
        "the mark stays set after it is deactivated"
    );
}

#[test]
fn kill_region_without_mark_reports_an_error() {
    let mut h = Harness::new(60, 8);
    h.type_text("text").keys("C-w");
    assert_eq!(h.echo(), "The mark is not set now, so there is no region");
    assert_eq!(h.buffer_text(), "text");
}

#[test]
fn long_lines_wrap_with_a_continuation_marker() {
    let mut h = Harness::new(10, 6);
    h.type_text("abcdefghijklm");
    assert_eq!(h.screen.row(0), "abcdefghi\\");
    assert_eq!(h.screen.row(1), "jklm");
    assert_eq!(h.screen.cursor(), Some((1, 4)));
    h.keys("C-a");
    assert_eq!(h.screen.cursor(), Some((0, 0)));
    h.keys("C-u 9 C-f");
    assert_eq!(
        h.screen.cursor(),
        Some((1, 0)),
        "the wrap boundary is on the second row"
    );
}

#[test]
fn tabs_wide_chars_and_control_chars_take_the_right_cells() {
    let mut h = harness();
    h.paste("a\tb日本\u{1}c");
    assert_eq!(h.screen.row(0), "a       b日本^Ac");
    assert_eq!(h.screen.cursor(), Some((0, 16)));
    h.keys("C-b C-b");
    assert_eq!(h.screen.cursor(), Some((0, 13)));
}

#[test]
fn window_scrolls_minimally_to_keep_point_visible() {
    let mut h = harness(); // six text rows
    let lines: Vec<String> = (1..=30).map(|n| format!("line {n}")).collect();
    h.paste(&lines.join("\n"));
    assert_eq!(h.screen.row(5), "line 30");
    assert_eq!(h.screen.row(0), "line 25");

    h.keys("M-<");
    assert_eq!(h.screen.row(0), "line 1");
    h.keys("C-u 6 C-n");
    assert_eq!(
        h.screen.row(0),
        "line 2",
        "moving one row past the bottom scrolls one row"
    );
    assert_eq!(h.screen.cursor(), Some((5, 0)));

    h.keys("C-v");
    assert_eq!(h.screen.row(0), "line 6");
    assert_eq!(
        h.screen.cursor(),
        Some((1, 0)),
        "point stays put while it is still visible"
    );
    h.keys("M-v M-v");
    assert_eq!(h.screen.row(0), "line 1");
    assert_eq!(h.screen.cursor(), Some((5, 0)));
    h.keys("M-v");
    assert_eq!(h.echo(), "Beginning of buffer");

    h.keys("M-> C-l");
    assert_eq!(h.screen.row(3), "line 30");
    assert_eq!(h.screen.cursor(), Some((3, 7)));
}

#[test]
fn vertical_motion_keeps_the_goal_column() {
    let mut h = harness();
    h.paste("long line here\nab\nanother long line");
    h.keys("M-< C-e C-n");
    assert_eq!(h.screen.cursor(), Some((1, 2)));
    h.keys("C-n");
    assert_eq!(h.screen.cursor(), Some((2, 14)));
    h.keys("C-n");
    assert_eq!(h.echo(), "End of buffer");
}

#[test]
fn only_changed_rows_are_redrawn() {
    let mut h = harness();
    h.paste("one\ntwo\nthree");
    h.keys("M-<");
    assert_eq!(
        h.screen.rows_drawn, 1,
        "only the mode line changes when point moves"
    );
    h.type_text("x");
    assert_eq!(h.screen.rows_drawn, 2, "the edited row and the mode line");
    h.keys("C-f");
    h.keys("C-b");
    assert_eq!(h.screen.row(0), "xone");
}

#[test]
fn prefix_arguments() {
    let mut h = harness();
    h.keys("C-u").type_text("x");
    assert_eq!(h.buffer_text(), "xxxx");
    h.keys("C-u C-u");
    assert_eq!(h.echo(), "C-u C-u-");
    h.type_text("y");
    assert_eq!(h.buffer_text().len(), 20);
    h.keys("C-u 1 2 C-b");
    assert_eq!(h.screen.cursor(), Some((0, 8)));
    h.keys("M-3 C-f");
    assert_eq!(h.screen.cursor(), Some((0, 11)));
    h.keys("C-u - 2 C-f");
    assert_eq!(h.screen.cursor(), Some((0, 9)));
}

#[test]
fn key_sequences_echo_and_report_unbound_keys() {
    let mut h = harness();
    h.keys("C-x");
    assert_eq!(h.echo(), "C-x-");
    h.keys("C-z");
    assert_eq!(h.echo(), "C-x C-z is undefined");
    h.keys("C-x C-g");
    assert_eq!(h.echo(), "Quit");
    h.type_text("a");
    assert_eq!(
        h.buffer_text(),
        "a",
        "the cancelled prefix does not swallow the next key"
    );
}

#[test]
fn escape_acts_as_meta() {
    let mut h = harness();
    h.type_text("one two").keys("ESC b");
    assert_eq!(h.screen.cursor(), Some((0, 4)));
    h.keys("ESC <");
    assert_eq!(h.screen.cursor(), Some((0, 0)));
}

#[test]
fn m_x_narrows_commands_as_words_are_typed() {
    let mut h = Harness::new(70, 16);
    h.type_text("abc").keys("M-x");
    assert!(h.prompt().ends_with(" M-x"), "{}", h.prompt());
    assert_eq!(h.candidates().len(), 10, "ten candidates are listed");

    h.type_text("line beg");
    assert_eq!(h.prompt(), "1/1 M-x line beg");
    assert_eq!(
        h.candidates(),
        ["move-beginning-of-line  Move point to the beginning of the line."]
    );
    h.keys("RET");
    assert_eq!(h.screen.cursor(), Some((0, 0)));
    assert_eq!(
        h.screen.row(14),
        " **  *scratch*   L1 C0  (Fundamental)",
        "the windows get their rows back"
    );

    h.keys("M-x").type_text("no-such-command");
    assert_eq!(h.prompt(), "*/0 M-x no-such-command");
    assert!(h.candidates().is_empty());
    h.keys("RET");
    assert_eq!(h.echo(), "Unknown command: no-such-command");
}

#[test]
fn the_selection_moves_through_the_candidates_and_onto_the_input() {
    let mut h = Harness::new(70, 16);
    h.keys("M-x").type_text("kill-");
    // Every command with the word in its name, wherever it is.
    let mut expected: Vec<String> = h
        .editor
        .command_names()
        .filter(|name| name.contains("kill-"))
        .map(str::to_owned)
        .collect();
    expected.sort();
    let count = expected.len();
    assert!(count > 10, "more than fit on screen");
    assert!(expected.contains(&"save-buffers-kill-terminal".to_owned()));
    assert_eq!(h.prompt(), format!("1/{count} M-x kill-"));
    let shown = h.candidates();
    assert_eq!(shown.len(), 10, "ten at a time");
    for row in &shown {
        let name = row.split_whitespace().next().unwrap().to_owned();
        assert!(expected.contains(&name), "{name}");
    }
    // The selected candidate has the highlight.
    assert_ne!(h.screen.face_at(6, 0).bg, h.screen.face_at(7, 0).bg);

    h.keys("C-n C-n");
    assert_eq!(h.prompt(), format!("3/{count} M-x kill-"));
    assert_ne!(h.screen.face_at(8, 0).bg, h.screen.face_at(7, 0).bg);
    h.keys("M->");
    assert_eq!(h.prompt(), format!("{count}/{count} M-x kill-"));
    h.keys("M-< C-p");
    assert_eq!(
        h.prompt(),
        format!("*/{count} M-x kill-"),
        "above the first candidate, the input is selected"
    );
    h.keys("RET");
    assert_eq!(h.echo(), "Unknown command: kill-");

    h.type_text("x").keys("M-x").type_text("kill-").keys("C-g");
    assert_eq!(h.echo(), "Quit");
    h.type_text("y");
    assert_eq!(h.buffer_text(), "xy", "quitting returns to the buffer");
}

#[test]
fn candidates_can_be_inserted_and_input_recalled_from_history() {
    let mut h = Harness::new(70, 16);
    h.type_text("word")
        .keys("M-x")
        .type_text("backward-w")
        .keys("TAB");
    assert_eq!(h.prompt(), "1/1 M-x backward-word");
    h.keys("RET");
    assert_eq!(h.screen.cursor(), Some((0, 0)));

    // Commands that were run come first the next time.
    h.keys("M-x");
    assert!(
        h.candidates()[0].starts_with("backward-word "),
        "{:?}",
        h.candidates()
    );
    h.keys("M-p");
    assert_eq!(h.prompt(), "1/1 M-x backward-word");
    h.keys("C-g");
}

#[test]
fn lines_are_searched_with_a_live_preview() {
    let mut h = Harness::new(60, 20);
    let text: Vec<String> = (1..=40).map(|n| format!("line number {n}")).collect();
    h.paste(&text.join("\n"));
    h.keys("M-<");

    h.keys("C-s");
    assert_eq!(h.prompt(), "1/40 Go to line:");
    h.type_text("3 number");
    assert_eq!(h.prompt(), "1/13 Go to line: 3 number");
    assert!(
        h.candidates()[0].starts_with("line number 3 "),
        "{:?}",
        h.candidates()
    );
    assert!(
        h.candidates()[0].ends_with(" 3"),
        "the line number is the annotation"
    );
    // The window already shows the selected line.
    assert_eq!(h.screen.row(8), " **  *scratch*   L3 C0  (Fundamental)");
    h.keys("C-n C-n");
    assert_ne!(
        h.screen.row(8),
        " **  *scratch*   L3 C0  (Fundamental)",
        "the preview follows"
    );
    assert!(h.screen.row(8).contains('3'), "{}", h.screen.row(8));

    h.keys("C-g");
    assert_eq!(
        h.screen.row(18),
        " **  *scratch*   L1 C0  (Fundamental)",
        "quitting goes back"
    );
    assert_eq!(h.screen.row(0), "line number 1");

    h.keys("C-s").type_text("number 37").keys("RET");
    assert_eq!(h.screen.row(18), " **  *scratch*   L37 C0  (Fundamental)");
    h.keys("C-x C-x");
    assert_eq!(
        h.screen.row(18),
        " **  *scratch*   L1 C0  (Fundamental)",
        "the mark is where the search began"
    );
}

#[test]
fn a_prefix_key_shows_what_can_follow_after_a_pause() {
    let mut h = Harness::new(100, 20);
    h.keys("C-x");
    assert_eq!(h.echo(), "C-x-");
    h.wait(std::time::Duration::from_secs(1));
    assert_eq!(h.prompt(), "C-x-");
    let hint = h.candidates().join("\n");
    assert!(hint.contains("C-f → find-file"), "{hint}");
    assert!(hint.contains("5 → +prefix"), "{hint}");
    assert!(hint.contains("o → other-window"), "{hint}");

    h.keys("o");
    assert!(
        h.candidates().is_empty(),
        "the hint goes away with the next key"
    );
    h.keys("C-x C-g");
    h.wait(std::time::Duration::from_secs(1));
    assert!(
        h.candidates().is_empty(),
        "a cancelled prefix shows no hint"
    );
}

#[test]
fn minibuffer_scrolls_to_keep_its_cursor_visible() {
    let mut h = Harness::new(20, 5);
    h.keys("M-g g").type_text("abcdefghijklmnopqrstuvwxyz");
    assert_eq!(h.echo(), "ghijklmnopqrstuvwxyz"[1..].to_owned());
    assert_eq!(h.screen.cursor(), Some((4, 19)));
    h.keys("C-a");
    assert_eq!(h.echo(), "Goto line: abcdefghi");
    assert_eq!(h.screen.cursor(), Some((4, 11)));
}

#[test]
fn windows_split_share_a_buffer_and_close() {
    let mut h = Harness::new(61, 10);
    h.type_text("shared");
    h.keys("C-x 3");
    assert_eq!(h.screen.row(0), format!("{:30}│shared", "shared"));
    h.keys("C-x o").type_text("!");
    assert_eq!(h.screen.row(0), format!("{:30}│shared!", "shared!"));
    assert_eq!(h.screen.cursor(), Some((0, 38)));

    h.keys("C-x 2");
    insta::assert_snapshot!("three_windows", h.screen.text());

    h.keys("C-x 0");
    // The left window's point stays before the "!" typed at it in the right one.
    let mode_line = " **  *scratch*   L1 C7  (Fundamental)";
    // Each mode line is cut at its window's width: 31 and 30 columns.
    assert_eq!(
        h.screen.row(8),
        format!(
            "{}{}",
            &" **  *scratch*   L1 C6  (Fundamental)"[..31],
            &mode_line[..30]
        )
    );
    assert_eq!(h.screen.row(7), format!("{:30}│", ""));
    h.keys("C-x 1");
    assert_eq!(h.screen.row(8), mode_line);
    h.keys("C-x 0");
    assert_eq!(h.echo(), "Attempt to delete sole window");
}

#[test]
fn another_windows_point_follows_insertions_before_it() {
    let mut h = Harness::new(61, 10);
    h.type_text("tail");
    h.keys("C-x 3 M-<").type_text("head ");
    // The right window's point was at the end and must still be.
    h.keys("C-x o").type_text("!");
    assert_eq!(h.buffer_text(), "head tail!");
}

#[test]
fn small_windows_refuse_to_split() {
    let mut h = Harness::new(40, 5);
    h.keys("C-x 2");
    assert_eq!(h.echo(), "");
    h.keys("C-x 2");
    assert_eq!(h.echo(), "Window too small for splitting");
}

#[test]
fn resizing_redraws_everything() {
    let mut h = harness();
    h.type_text("text");
    h.resize(20, 4);
    assert_eq!(h.screen.size(), (20, 4));
    assert_eq!(h.screen.row(0), "text");
    assert_eq!(h.screen.row(2), " **  *scratch*   L1");
    h.resize(1, 1);
    h.resize(0, 0);
    h.type_text("x");
    h.resize(30, 5);
    assert_eq!(h.screen.row(0), "textx");
}

#[test]
fn pasted_text_is_one_undo_step() {
    let mut h = harness();
    h.type_text("a");
    h.paste("pasted\ntext");
    assert_eq!(h.buffer_text(), "apasted\ntext");
    h.keys("C-/");
    assert_eq!(h.buffer_text(), "a");
}

#[test]
fn buffers_switch_and_are_created_on_demand() {
    let mut h = Harness::new(60, 16);
    h.type_text("scratch text");
    h.keys("C-x b").type_text("notes");
    assert_eq!(h.prompt(), "*/0 Switch to buffer: notes");
    h.keys("RET");
    assert_eq!(
        h.screen.row(14),
        " --  notes   L1 C0  (Fundamental)",
        "an unknown name makes a buffer"
    );
    h.type_text("note");

    h.keys("C-x b");
    assert_eq!(h.prompt(), "1/2 Switch to buffer:");
    assert_eq!(
        h.candidates(),
        ["*scratch*", "notes"],
        "the other buffer comes first"
    );
    h.keys("RET");
    assert_eq!(h.screen.row(0), "scratch text");
    assert_eq!(
        h.screen.cursor(),
        Some((0, 12)),
        "the buffer remembers its point"
    );

    h.keys("C-x k");
    assert_eq!(
        h.candidates(),
        ["*scratch*", "notes"],
        "the current buffer comes first"
    );
    h.keys("RET");
    assert_eq!(h.screen.row(0), "note");
    h.keys("C-x k RET");
    assert_eq!(
        h.screen.row(14),
        " --  *scratch*   L1 C0  (Fundamental)",
        "killing the last buffer leaves a fresh scratch"
    );
}

#[test]
fn a_candidate_can_be_acted_on_instead_of_accepted() {
    let mut h = Harness::new(60, 16);
    h.keys("C-x b")
        .type_text("other")
        .keys("RET C-x b")
        .type_text("scr")
        .keys("C-.");
    assert_eq!(
        h.prompt(),
        "Act on *scratch*: w copy, i insert, k kill, o other window"
    );
    h.type_text("o");
    assert_eq!(h.screen.row(0), format!("{:30}│", ""));
    assert!(h.screen.row(14).contains("other") && h.screen.row(14).contains("*scratch*"));

    h.keys("C-x b").type_text("oth").keys("C-.").type_text("i");
    assert_eq!(
        h.buffer_text(),
        "other",
        "the candidate is inserted where the prompt was opened"
    );
    h.keys("C-x b C-.").type_text("k");
    assert!(!h.screen.row(14).contains("other"), "{}", h.screen.row(14));
}

/// A clipboard shared with the test, standing in for the system's.
#[derive(Clone, Default)]
struct FakeClipboard(std::rc::Rc<std::cell::RefCell<Option<String>>>);

impl amacs_core::Clipboard for FakeClipboard {
    fn set(&mut self, text: &str) {
        *self.0.borrow_mut() = Some(text.to_owned());
    }

    fn get(&mut self) -> Option<String> {
        self.0.borrow().clone()
    }
}

#[test]
fn kills_go_to_the_clipboard_and_foreign_text_can_be_yanked() {
    let clipboard = FakeClipboard::default();
    let mut h = harness();
    h.editor.set_clipboard(Box::new(clipboard.clone()));

    h.type_text("one two").keys("M-DEL");
    assert_eq!(clipboard.0.borrow().as_deref(), Some("two"));
    h.keys("M-DEL");
    assert_eq!(
        clipboard.0.borrow().as_deref(),
        Some("one two"),
        "consecutive kills grow the entry"
    );

    h.keys("C-y C-y");
    assert_eq!(
        h.buffer_text(),
        "one twoone two",
        "yanking our own kill does not duplicate it"
    );

    // Another program copies something.
    *clipboard.0.borrow_mut() = Some("from elsewhere".to_owned());
    h.keys("SPC C-y");
    assert_eq!(h.buffer_text(), "one twoone two from elsewhere");
    h.keys("M-y");
    assert_eq!(
        h.buffer_text(),
        "one twoone two one two",
        "our kill is on the ring behind it"
    );
    assert_eq!(
        clipboard.0.borrow().as_deref(),
        Some("from elsewhere"),
        "yanking does not copy"
    );
}
