//! Help that is made from keymaps and the first sentence of the
//! documentation of commands:
//! `runtime/lib/help.scm`, and the keys that `M-x` shows.

mod common;

use common::Harness;

/// An editor that shows a directory in the file manager.
fn in_dired() -> (tempfile::TempDir, Harness) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "a").unwrap();
    let mut h = Harness::new(100, 40);
    h.eval("(set-option! 'dired-icons #f)");
    h.eval(&format!(
        "(dired-open {:?})",
        dir.path().display().to_string()
    ));
    (dir, h)
}

/// The keys that the help shows for what a command does: the row that
/// ends with the text, without the text.
fn keys_of(h: &Harness, does: &str) -> String {
    let text = h.text();
    text.lines()
        .find_map(|line| line.strip_suffix(does))
        .unwrap_or_else(|| panic!("no row for {does:?} in:\n{text}"))
        .trim()
        .to_owned()
}

#[test]
fn a_key_shows_what_the_keys_of_a_mode_do() {
    let (_dir, mut h) = in_dired();
    h.keys("?");
    assert!(h.mode_line().contains("*Help*"), "{}", h.mode_line());
    let text = h.text();
    assert!(
        text.starts_with("dired-mode\n\nMajor mode of the file manager"),
        "{text}"
    );
    assert!(text.contains("\nKeys of dired-mode\n"), "{text}");
    // What a key does is the first sentence of its command's
    // documentation, which goes on.
    assert_eq!(
        keys_of(
            &h,
            "Compress or uncompress the marked files, or the file on this line."
        ),
        "Z"
    );
    assert!(
        h.eval("(command-documentation \"dired-do-compress\")")
            .starts_with(
                "\"Compress or uncompress the marked files, or the file on this line. The"
            ),
    );
    assert!(text.contains("  Z             Compress or"), "{text}");
    // The keys of one command are on one row, without those of the
    // pointer; a command that only the pointer runs has them.
    assert_eq!(keys_of(&h, "Visit the file on this line."), "e, f, RET");
    assert_eq!(
        keys_of(
            &h,
            "Show the directory that this one is in, with point on this one."
        ),
        "^, DEL"
    );
    assert!(text.contains("<mouse-3>"), "{text}");
    // A letter is next to its capital, and the pointer comes last.
    let at = |does: &str| text.find(does).unwrap_or_else(|| panic!("{does}: {text}"));
    assert!(at("Flag the file on this line") < at("Delete the marked files"));
    assert!(at("Delete the marked files") < at("Visit the file on this line."));
    assert!(at("Leave the file manager.") < at("<mouse-3>"));
    // The keys of the parent mode are there, and those that hold
    // everywhere are not.
    assert!(text.contains("\nKeys of special-mode\n"), "{text}");
    assert_eq!(
        keys_of(
            &h,
            "Show what the mode of this buffer is for and what its keys do."
        ),
        "?, h"
    );
    assert!(!text.contains("Visit a file."), "{text}");

    // The help is in the window that asked for it, and q shows what was
    // there before.
    assert_eq!(h.eval("(length (window-list))"), "1");
    h.keys("q");
    assert!(h.mode_line().contains("(Dired)"), "{}", h.mode_line());

    // h is the same key.
    h.keys("h");
    assert!(h.mode_line().contains("*Help*"), "{}", h.mode_line());
    h.keys("q");
}

#[test]
fn the_help_follows_the_keys_as_they_are_bound() {
    let (_dir, mut h) = in_dired();
    h.eval("(define-key 'dired-mode \"h\" 'dired-up-directory)");
    h.eval("(define-key 'dired-mode \"J\" 'dired-do-compress)");
    h.keys("?");
    // A mode's own key hides its parent's.
    assert_eq!(
        keys_of(
            &h,
            "Show the directory that this one is in, with point on this one."
        ),
        "^, h, DEL"
    );
    assert_eq!(
        keys_of(
            &h,
            "Show what the mode of this buffer is for and what its keys do."
        ),
        "?"
    );
    assert_eq!(
        keys_of(
            &h,
            "Compress or uncompress the marked files, or the file on this line."
        ),
        "J, Z"
    );
}

#[test]
fn help_is_on_two_prefixes_and_lists_all_keys_when_asked() {
    let mut h = Harness::new(100, 40);
    h.keys("<f1> m");
    assert!(h.mode_line().contains("*Help*"), "{}", h.mode_line());
    assert!(!h.text().contains("find-file"));
    h.keys("q");
    h.keys("C-h b");
    let text = h.text();
    assert!(text.contains("\nKeys that hold everywhere\n"), "{text}");
    // Here the commands are named.
    let row = text
        .lines()
        .find(|line| line.trim_start().starts_with("C-x C-f "))
        .unwrap_or_else(|| panic!("{text}"));
    let parts: Vec<&str> = row.split_whitespace().collect();
    assert_eq!(parts[..3], ["C-x", "C-f", "find-file"], "{row}");
    h.keys("q");
    assert_eq!(
        h.eval("(command-keys \"describe-mode\")"),
        "(\"<f1> m\" \"C-h m\")"
    );
}

#[test]
fn a_command_is_shown_with_its_keys_when_it_is_asked_for_by_name() {
    let mut h = Harness::new(100, 12);
    h.keys("M-x").type_text("find-file-other");
    let screen = h.screen.text();
    let row = screen
        .lines()
        .find(|line| line.contains("find-file-other-window"))
        .unwrap_or_else(|| panic!("{screen}"));
    assert!(
        row.contains("C-x 4 f  Visit a file in another window"),
        "{row}"
    );
    h.keys("C-g");

    // A command that no key runs has only what it is for.
    h.keys("M-x").type_text("describe-bind");
    h.keys("C-g");
    h.eval("(undefine-key 'global \"C-h b\")");
    h.eval("(undefine-key 'global \"<f1> b\")");
    h.keys("M-x").type_text("describe-bind");
    let screen = h.screen.text();
    let row = screen
        .lines()
        .find(|line| line.contains("describe-bindings"))
        .unwrap_or_else(|| panic!("{screen}"));
    assert!(
        row.contains("describe-bindings  Show what all keys do in this buffer"),
        "{row}"
    );
    h.keys("C-g");

    // The keys are those of the buffer that the command would run in.
    let (_dir, mut h) = in_dired();
    h.keys("M-x").type_text("dired-do-compress-to");
    let screen = h.screen.text();
    let row = screen
        .lines()
        .find(|line| line.contains("dired-do-compress-to") && !line.contains("M-x"))
        .unwrap_or_else(|| panic!("{screen}"));
    assert!(row.contains("z  Pack the marked files"), "{row}");
}
