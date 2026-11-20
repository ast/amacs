//! The terminal in a buffer, with real programs on a pseudo-terminal.

mod common;

use common::Harness;

/// A terminal running `sh` with a plain prompt, in a frame of the given
/// size, which the terminal fills.
fn shell(cols: u16, rows: u16) -> Harness {
    let mut h = Harness::new(cols, rows);
    h.eval(
        r#"(set-option! 'term-shell '("sh"))
           (term-start "*term*" '("env" "PS1=$ " "sh" "-i"))"#,
    );
    h.wait_until(|h| h.screen.row(0).starts_with("$"));
    h
}

#[test]
fn a_shell_runs_in_the_buffer_and_keys_reach_it() {
    let mut h = shell(40, 8);
    assert!(h.mode_line().contains("Term"), "{}", h.mode_line());
    assert_eq!(h.eval("(terminal-size)"), "(39 6)");
    h.type_text("echo hello world").keys("RET");
    h.wait_until(|h| h.screen.row(1) == "hello world");
    assert_eq!(h.screen.row(0), "$ echo hello world");
    // The prompt comes after the output, and may come later.
    h.wait_until(|h| h.screen.row(2).starts_with("$"));
    // Point is at the shell's cursor, after the prompt on the third row.
    assert_eq!(h.screen.cursor(), Some((2, 2)));
    // The text is an ordinary buffer's.
    assert!(h.eval("(buffer-string)").contains("hello world"));
    assert_eq!(h.eval("(buffer-read-only?)"), "#t");
}

#[test]
fn colours_show_and_the_scrollback_is_kept() {
    let mut h = shell(40, 5);
    h.type_text("printf '\\033[31mred\\033[0m\\n'").keys("RET");
    h.wait_until(|h| h.screen.row(1) == "red");
    let red = h.screen.face_at(1, 0);
    h.eval("(set-face! 'term-test 'fg 'fg-term-red)");
    assert_eq!(red.fg, h.face("term-test").fg);
    assert_ne!(red.fg, h.screen.face_at(1, 3).fg);
    // Enough lines to scroll: the first ones leave the screen but stay in
    // the buffer, and the window shows the live screen.
    h.type_text("for i in 1 2 3 4 5 6 7 8; do echo line$i; done")
        .keys("RET");
    // Three rows of text: line8, then the prompt, with line7 above.
    h.wait_until(|h| h.screen.row(1) == "line8");
    assert_eq!(h.screen.row(0), "line7");
    let text = h.text();
    assert!(text.contains("red\n"), "{text}");
    assert!(
        text.contains("line1\n") && text.contains("line8\n"),
        "{text}"
    );
    assert_eq!(h.eval("(terminal-follows?)"), "#t");
}

#[test]
fn editor_keys_keep_their_meaning_and_point_can_leave_the_cursor() {
    let mut h = shell(40, 6);
    h.type_text("echo one").keys("RET");
    h.wait_until(|h| h.screen.row(1) == "one");
    // M-x and C-x are the editor's; the rest goes to the shell.
    h.keys("M-x");
    assert!(h.prompt().contains("M-x"), "{}", h.prompt());
    h.keys("C-g");
    h.keys("C-x b");
    assert!(h.prompt().contains("Switch to buffer"), "{}", h.prompt());
    h.keys("C-g");
    // Moving point away stops it following the cursor; C-q C-l brings it
    // back; a key for the program does too.
    h.keys("M-<");
    assert_eq!(h.eval("(point)"), "0");
    assert_eq!(h.eval("(terminal-follows?)"), "#f");
    // C-q C-l brings point back to the cursor and makes it follow again.
    h.keys("C-q C-l");
    assert_eq!(h.eval("(terminal-follows?)"), "#t");
    assert_eq!(h.eval("(= (point) (terminal-cursor))"), "#t");
    // A key sent to the program also makes it follow again.
    h.keys("M-<");
    assert_eq!(h.eval("(terminal-follows?)"), "#f");
    h.type_text("echo two").keys("RET");
    assert_eq!(h.eval("(terminal-follows?)"), "#t");
    h.wait_until(|h| h.screen.row(2) == "two");
}

#[test]
fn an_interrupt_and_the_end_of_the_program() {
    let mut h = shell(40, 6);
    h.type_text("sh -c 'echo started; sleep 30'").keys("RET");
    // Wait until the program itself has written, so the interrupt lands on
    // it and not on a shell that has yet to start it: the echo of the
    // command comes before that.
    h.wait_until(|h| h.text().contains("\nstarted\n"));
    // C-c is the program's, as in any terminal.
    h.keys("C-c");
    // A second prompt appears once the sleep is interrupted.
    h.wait_until(|h| {
        h.text()
            .lines()
            .filter(|line| line.starts_with("$"))
            .count()
            >= 2
    });
    h.type_text("exit 3").keys("RET");
    h.wait_until(|h| h.eval("(terminal-buffer?)") == "#f");
    let text = h.eval("(buffer-string)");
    assert!(text.contains("[Process exited with status 3]"), "{text}");
    // Keys no longer go anywhere; the buffer is plain text.
    h.type_text("a");
    assert!(h.echo().contains("has ended"), "{}", h.echo());
}

#[test]
fn the_terminal_follows_the_window_size() {
    let mut h = shell(40, 8);
    assert_eq!(h.eval("(terminal-size)"), "(39 6)");
    h.keys("C-x 2");
    h.wait_until(|h| h.eval("(terminal-size)") != "(39 6)");
    let size = h.eval("(terminal-size)");
    assert!(size.starts_with("(39 ") && size != "(39 6)", "{size}");
    h.type_text("echo $(stty size)").keys("RET");
    let rows = size[4..size.len() - 1].to_owned();
    h.wait_until(|h| h.text().contains(&format!("\n{rows} 39\n")));
}

#[test]
fn a_full_screen_program_draws_and_leaves() {
    let mut h = shell(40, 8);
    // Clear the screen and draw at a place, as full screen programs do.
    h.type_text("printf '\\033[2J\\033[3;5Hhere\\033[44m \\033[0m'")
        .keys("RET");
    // The prompt follows the blue cell, which may come after the word.
    h.wait_until(|h| h.screen.row(2).starts_with("    here $"));
    h.eval("(set-face! 'term-test 'fg 'fg-term-blue)");
    assert_eq!(h.screen.face_at(2, 8).bg, h.face("term-test").fg);
    assert_ne!(h.screen.face_at(2, 8).bg, h.screen.face_at(2, 0).bg);
}

#[test]
fn the_minibuffer_works_over_a_terminal() {
    let mut h = shell(50, 10);
    // Open a prompt from the terminal: keys must go to the minibuffer, not
    // the program, and C-g must cancel it.
    h.keys("C-x C-f");
    assert!(h.prompt().contains("Find file"), "{}", h.prompt());
    h.type_text("zzz");
    assert!(
        h.prompt().ends_with("zzz"),
        "keys went to the terminal: {}",
        h.prompt()
    );
    h.keys("C-g");
    assert!(
        !h.prompt().contains("Find file"),
        "C-g did not cancel: {}",
        h.prompt()
    );
    assert_eq!(h.eval("(buffer-name)"), "\"*term*\"");
    // Switching buffers by name still works too.
    h.keys("C-x b").type_text("*scratch*").keys("RET");
    assert_eq!(h.eval("(buffer-name)"), "\"*scratch*\"");
}

#[test]
fn char_mode_sends_the_reserved_keys_to_the_program() {
    let mut h = shell(50, 10);
    // Semi-char: C-x is the editor's, so C-x C-f opens find-file.
    h.keys("C-x C-f");
    assert!(h.prompt().contains("Find file"), "{}", h.prompt());
    h.keys("C-g");
    // Enter char mode. Now C-x goes to the program, so C-x C-f does not
    // open find-file.
    h.keys("C-q C-k");
    assert!(h.mode_line().contains("Char"), "{}", h.mode_line());
    assert_eq!(h.eval("(key-binding \"C-x\")"), "term-send-key");
    assert_eq!(h.eval("(key-binding \"M-x\")"), "term-send-key");
    h.keys("C-x");
    assert!(
        !h.prompt().contains("Find file"),
        "C-x opened a prompt in char mode: {}",
        h.prompt()
    );
    // The escape key is still the editor's: C-q C-k leaves char mode.
    h.keys("C-q C-k");
    assert!(
        !h.mode_line().contains("Char"),
        "did not leave char mode: {}",
        h.mode_line()
    );
    assert_eq!(h.eval("(key-binding \"C-x\")"), "#f");
    h.keys("C-x C-f");
    assert!(
        h.prompt().contains("Find file"),
        "C-x reserved again: {}",
        h.prompt()
    );
    h.keys("C-g");
}

#[test]
fn the_escape_key_sends_a_reserved_key_and_can_be_changed() {
    let mut h = shell(50, 10);
    // C-x is the editor's; after the escape key it goes to the program,
    // which echoes it as ^X. The escape key itself goes after itself.
    h.keys("C-q C-x");
    h.wait_until(|h| h.screen.row(0).ends_with("^X"));
    assert_eq!(h.eval("(key-binding \"C-q C-q\")"), "term-send-key");
    assert_eq!(h.eval("(key-binding \"C-q C-l\")"), "term-recenter");
    // The option names the key; the next terminal is bound by it, and a key
    // put back among the editor's, C-c, is the editor's again.
    h.eval(
        r#"(set-option! 'term-escape-key "C-]")
           (set-option! 'term-emacs-keys (cons "C-c" (option 'term-emacs-keys)))
           (term-start "*term2*" '("env" "PS1=$ " "sh" "-i"))"#,
    );
    h.wait_until(|h| h.screen.row(0).starts_with("$"));
    assert_eq!(h.eval("(key-binding \"C-q\")"), "term-send-key");
    assert_eq!(h.eval("(key-binding \"C-] C-k\")"), "term-char-mode");
    assert_eq!(h.eval("(key-binding \"C-] C-c\")"), "term-send-key");
    assert_eq!(h.eval("(key-binding \"C-c v\")"), "term");
    h.keys("C-] C-x");
    h.wait_until(|h| h.screen.row(0).ends_with("^X"));
}

/// A shell in a directory of its own with a subdirectory `sub`, in a frame
/// wide enough for a listing beside it.
fn shell_in_directory() -> (Harness, tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let top = dir.path().canonicalize().unwrap();
    std::fs::create_dir(top.join("sub")).unwrap();
    std::fs::write(top.join("sub/file.txt"), "").unwrap();
    let top = top.to_str().unwrap().to_owned();
    let mut h = Harness::new(100, 10);
    h.eval(&format!(
        r#"(set-option! 'dired-icons #f)
           (term-start "*term*" '("env" "PS1=$ " "sh" "-i") {top:?})"#
    ));
    h.wait_until(|h| h.screen.row(0).starts_with("$"));
    (h, dir, top)
}

#[test]
fn the_buffer_is_in_the_directory_of_the_shell() {
    let (mut h, _dir, top) = shell_in_directory();
    assert_eq!(h.eval("(working-directory)"), format!("{top:?}"));
    h.type_text("cd sub").keys("RET");
    let sub = format!("\"{top}/sub\"");
    h.wait_until(|h| h.eval("(working-directory)") == sub);
}

#[test]
fn a_listing_beside_the_terminal_follows_the_shell() {
    let (mut h, _dir, top) = shell_in_directory();
    h.keys("C-q C-d");
    // The listing is in a window to the right; the terminal keeps the keys.
    assert_eq!(h.eval("(length (window-list))"), "2");
    assert_eq!(h.eval("(buffer-name)"), "\"*term*\"");
    assert_eq!(
        h.eval("(with-selected-window (cadr (window-list)) (dired-directory))"),
        format!("{top:?}")
    );
    // The heading shortens a directory under the home directory, which the
    // temporary one is in some builds.
    let last = top.rsplit('/').next().unwrap();
    assert!(
        h.screen.row(0).contains(&format!("/{last}:")),
        "{}",
        h.screen.row(0)
    );

    h.type_text("cd sub").keys("RET");
    h.wait_until(|h| h.screen.row(1).contains("file.txt"));
    assert_eq!(h.eval("(buffer-name)"), "\"*term*\"");
    // The listing that was left is not kept.
    assert_eq!(
        h.eval("(length (filter (lambda (b) (with-current-buffer b (eq? (major-mode) 'dired-mode))) (buffer-list)))"),
        "1"
    );

    // Again selects the listing; `c` on a directory sends the shell there.
    h.keys("C-q C-d ^");
    assert_eq!(h.eval("(dired-directory)"), format!("{top:?}"));
    assert_eq!(h.eval("(term-dired-terminal)"), "\"*term*\"");
    h.keys("c");
    let sub = format!("\"{top}/sub\"");
    h.wait_until(|h| h.eval("(dired-directory)") == sub);
    h.keys("^ ^ c");
    h.wait_until(|h| h.eval("(dired-directory)") == format!("{top:?}"));

    // A listing that is gone is not followed, and nothing complains.
    h.keys("q");
    h.eval(r#"(select-window (car (filter (lambda (w) (equal? (window-buffer w) "*term*")) (window-list))))"#);
    h.type_text("cd sub").keys("RET");
    h.wait_until(|h| h.eval("(working-directory)") == sub);
    assert_eq!(h.eval("(hash-table-count %term-dired-links)"), "0");
    assert_eq!(h.echo(), "");
}
