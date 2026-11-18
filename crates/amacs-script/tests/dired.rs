//! The file manager, which is written in Scheme: `runtime/lib/dired.scm`.

mod common;

use std::path::Path;

use common::Harness;

/// A directory with a few things in it, and an editor showing it.
fn setup() -> (tempfile::TempDir, Harness) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("b.txt"), "hello").unwrap();
    std::fs::write(root.join("a.rs"), "fn main() {}\n").unwrap();
    std::fs::write(root.join(".hidden"), "").unwrap();
    std::fs::create_dir(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/inner.txt"), "x".repeat(2048)).unwrap();
    std::fs::create_dir(root.join("empty")).unwrap();
    let mut h = Harness::new(80, 16);
    h.eval("(set-option! 'dired-icons #f)");
    open(&mut h, root);
    (dir, h)
}

fn open(h: &mut Harness, dir: &Path) {
    h.eval(&format!("(dired-open {:?})", dir.display().to_string()));
}

/// A path as the file manager writes it: with the home directory as ~,
/// which is where temporary directories are in some build environments.
fn shown(h: &mut Harness, path: &Path) -> String {
    let quoted = h.eval(&format!(
        "(abbreviate-file-name {:?})",
        path.display().to_string()
    ));
    quoted.trim_matches('"').to_owned()
}

/// The names listed, in order.
fn names(h: &mut Harness) -> String {
    h.eval("(map dired-row-name (vector->list (dired-state-rows (dired-state))))")
}

fn at_point(h: &mut Harness) -> String {
    h.eval("(file-name-nondirectory (dired-file-at-point))")
}

#[test]
fn a_directory_is_listed_with_directories_first() {
    let (dir, mut h) = setup();
    assert_eq!(
        names(&mut h),
        "(\"empty\" \"sub\" \".hidden\" \"a.rs\" \"b.txt\")"
    );
    let top = shown(&mut h, dir.path());
    assert_eq!(h.screen.row(0), format!("{top}:"));
    let row = h.screen.row(1);
    assert!(row.ends_with("▸ empty"), "{row}");
    let row = h.screen.row(5);
    assert!(
        row.starts_with("       5  20") && row.ends_with("    b.txt"),
        "{row}"
    );
    assert!(h.mode_line().contains("(Dired)"), "{}", h.mode_line());
    // Point starts on the first name, and directories look different.
    assert_eq!(at_point(&mut h), "\"empty\"");
    let name_column = h.screen.row(1).chars().count() - 5;
    assert_eq!(
        h.screen.face_at(1, name_column).fg,
        h.face("dired-directory").fg
    );
    assert_ne!(
        h.screen.face_at(5, name_column + 2).fg,
        h.face("dired-directory").fg
    );
    // The buffer cannot be typed into, and letters are commands.
    h.keys("n");
    assert_eq!(at_point(&mut h), "\"sub\"");
    h.keys("p p p");
    assert_eq!(at_point(&mut h), "\"empty\"");
    h.keys("C-d");
    assert!(h.echo().contains("read-only"), "{}", h.echo());

    // Hidden files can be left out; the order can be by time or size.
    h.keys(".");
    assert_eq!(names(&mut h), "(\"empty\" \"sub\" \"a.rs\" \"b.txt\")");
    h.keys(". s s");
    assert_eq!(h.echo(), "Sorted by size");
    assert_eq!(
        names(&mut h),
        "(\"empty\" \"sub\" \"a.rs\" \"b.txt\" \".hidden\")"
    );
}

#[test]
fn names_have_icons_from_the_nerd_fonts() {
    let (_dir, mut h) = setup();
    h.eval("(set-option! 'dired-icons #t) (dired-refresh)");
    // A folder, a closed one; Rust's crab; a plain file.
    assert!(
        h.screen.row(1).ends_with("▸ \u{f07b} empty"),
        "{}",
        h.screen.row(1)
    );
    assert!(
        h.screen.row(4).ends_with("\u{e7a8} a.rs"),
        "{}",
        h.screen.row(4)
    );
    assert!(
        h.screen.row(3).ends_with("\u{f016} .hidden"),
        "{}",
        h.screen.row(3)
    );
    h.keys("n TAB");
    assert!(
        h.screen.row(2).ends_with("▾ \u{f07c} sub"),
        "{}",
        h.screen.row(2)
    );
    // Every icon is one glyph, and none was lost on the way into the file.
    assert_eq!(
        h.eval(
            "(filter (lambda (entry) (not (= (string-length (cdr entry)) 1))) %dired-icon-table)"
        ),
        "()"
    );
}

#[test]
fn directories_are_entered_and_left() {
    let (dir, mut h) = setup();
    let top = h.eval("(buffer-name)");
    h.keys("n RET");
    assert_eq!(names(&mut h), "(\"inner.txt\")");
    assert_eq!(
        h.eval("(working-directory)"),
        format!("{:?}", dir.path().join("sub").display().to_string())
    );
    assert_eq!(
        h.eval(&format!("(member {top} (buffer-list))")),
        "#f",
        "the buffer of the directory left is not kept"
    );
    // Up again, with point on where we came from.
    h.keys("^");
    assert_eq!(at_point(&mut h), "\"sub\"");
    h.keys("n n RET");
    assert_eq!(h.eval("(buffer-name)"), "\"a.rs\"");
    assert_eq!(h.text(), "fn main() {}\n");

    // Visiting a directory as a file shows it, and so does the command.
    h.keys("C-x C-f");
    h.keys("C-a C-k")
        .type_text(&dir.path().join("sub").display().to_string())
        .keys("M-RET");
    assert_eq!(names(&mut h), "(\"inner.txt\")");
    h.keys("DEL");
    assert_eq!(at_point(&mut h), "\"sub\"");
    h.keys("q");
    assert_eq!(h.eval("(buffer-name)"), "\"a.rs\"");
    h.keys("C-x d");
    assert!(
        h.prompt()
            .ends_with(&format!("Directory: {}/", dir.path().display())),
        "{}",
        h.prompt()
    );
    h.keys("RET");
    assert_eq!(
        h.eval("(length (vector->list (dired-state-rows (dired-state))))"),
        "5"
    );
    // A listing that was killed like any buffer is made anew when asked for.
    let listing = h.eval("(buffer-name)");
    h.eval(&format!("(kill-buffer {listing})"));
    open(&mut h, dir.path());
    assert!(h.mode_line().contains("(Dired)"), "{}", h.mode_line());
    h.keys("n RET");
    assert_eq!(names(&mut h), "(\"inner.txt\")");
    h.keys("^");
    // The directories visited are remembered.
    h.keys("r");
    assert!(h.prompt().contains("Directory:"), "{}", h.prompt());
    h.type_text("sub").keys("RET");
    assert_eq!(names(&mut h), "(\"inner.txt\")");
}

#[test]
fn a_directory_opens_in_place_and_a_listing_narrows() {
    let (_dir, mut h) = setup();
    h.keys("n TAB");
    assert_eq!(
        names(&mut h),
        "(\"empty\" \"sub\" \"inner.txt\" \".hidden\" \"a.rs\" \"b.txt\")"
    );
    let row = h.screen.row(2);
    assert!(row.ends_with("▾ sub"), "{row}");
    let row = h.screen.row(3);
    assert!(
        row.contains("2.0K") && row.ends_with("      inner.txt"),
        "{row}"
    );
    assert_eq!(at_point(&mut h), "\"sub\"", "point stays on the directory");
    // What is inside is a file like any other.
    h.keys("n");
    assert!(h.eval("(dired-file-at-point)").ends_with("sub/inner.txt\""));
    h.keys("p TAB");
    assert_eq!(
        names(&mut h),
        "(\"empty\" \"sub\" \".hidden\" \"a.rs\" \"b.txt\")"
    );
    h.keys("n n TAB");
    assert_eq!(h.echo(), "That is not a directory");

    h.keys("/").type_text("B").keys("RET");
    assert_eq!(names(&mut h), "(\"sub\" \"b.txt\")");
    assert!(
        h.screen.row(0).ends_with("(narrowed to B)"),
        "{}",
        h.screen.row(0)
    );
    h.keys("g");
    assert_eq!(
        names(&mut h),
        "(\"empty\" \"sub\" \".hidden\" \"a.rs\" \"b.txt\")"
    );
}

#[test]
fn marked_files_are_copied_to_the_other_pane() {
    let (dir, mut h) = setup();
    let root = dir.path();
    // Two panes: the directory on the left, sub on the right.
    h.keys("C-x 3 C-x o n RET C-x o");
    assert_eq!(h.eval("(length (window-list))"), "2");
    h.keys("n n n m m");
    assert!(h.screen.row(4).starts_with("*  "), "{}", h.screen.row(4));
    assert_eq!(h.screen.face_at(4, 0).fg, h.face("dired-mark").fg);
    h.keys("C");
    let top = shown(&mut h, root);
    assert!(
        h.prompt()
            .ends_with(&format!("Copy 2 files to: {top}/sub/")),
        "{}",
        h.prompt()
    );
    h.keys("RET");
    h.wait_until(|h| h.echo() == "Copied 2 files");
    assert_eq!(
        std::fs::read_to_string(root.join("sub/b.txt")).unwrap(),
        "hello"
    );
    assert!(root.join("b.txt").exists(), "a copy leaves the original");
    // Both panes show what is there now, and the marks are gone.
    assert_eq!(
        names(&mut h),
        "(\"empty\" \"sub\" \".hidden\" \"a.rs\" \"b.txt\")"
    );
    assert!(h.screen.row(4).starts_with("   "), "{}", h.screen.row(4));
    h.keys("C-x o");
    assert_eq!(names(&mut h), "(\"a.rs\" \"b.txt\" \"inner.txt\")");

    // A directory is copied with what is in it; what exists is asked about.
    h.keys("C-x o M-< n n C");
    h.keys("C-a C-k")
        .type_text(&format!("{}/empty/", root.display()))
        .keys("RET");
    h.wait_until(|h| h.echo() == "Copied 1 file");
    assert!(root.join("empty/sub/inner.txt").exists());
    h.keys("n n n C RET");
    assert!(
        h.echo().ends_with("sub/b.txt exists; replace it? (y or n)"),
        "{}",
        h.echo()
    );
    h.keys("n");
    h.wait_until(|h| h.echo() == "Copied 0 files");
}

#[test]
fn files_are_renamed_moved_deleted_and_made() {
    let (dir, mut h) = setup();
    let root = dir.path();
    // With one pane, the directory itself is offered.
    let top = shown(&mut h, root);
    h.keys("n n n n R");
    assert!(
        h.prompt().ends_with(&format!("Move b.txt to: {top}/")),
        "{}",
        h.prompt()
    );
    h.type_text("c.txt").keys("RET");
    h.wait_until(|h| h.echo() == "Moved 1 file");
    assert!(root.join("c.txt").exists() && !root.join("b.txt").exists());
    assert_eq!(
        names(&mut h),
        "(\"empty\" \"sub\" \".hidden\" \"a.rs\" \"c.txt\")"
    );
    // Into a directory, which is made if need be.
    h.keys("R").type_text("new/").keys("RET");
    assert!(h.echo().starts_with("Create the directory"), "{}", h.echo());
    h.keys("y");
    h.wait_until(|h| h.echo() == "Moved 1 file");
    assert!(root.join("new/c.txt").exists());

    h.keys("+").type_text("made/deep").keys("RET");
    assert!(root.join("made/deep").is_dir());
    assert_eq!(at_point(&mut h), "\"made\"");

    // Flagged files are deleted on x, after asking; n leaves them.
    h.keys("M-< n d");
    assert!(h.screen.row(1).starts_with("D  "), "{}", h.screen.row(1));
    h.keys("x");
    assert_eq!(h.echo(), "Delete empty? (y or n)");
    h.keys("n");
    assert!(root.join("empty").exists());
    h.keys("x y");
    assert_eq!(h.echo(), "Deleted empty");
    assert!(!root.join("empty").exists());
    // D deletes at once what is marked or at point: a directory with all
    // that is in it.
    h.keys("M-< n m m D");
    assert_eq!(h.echo(), "Delete 2 files? (y or n)");
    h.keys("y");
    assert!(!root.join("new").exists() && !root.join("made").exists());
    assert_eq!(names(&mut h), "(\"sub\" \".hidden\" \"a.rs\")");
    h.keys("u U t");
    assert_eq!(h.eval("(length (dired-marked-files))"), "3");
    h.keys("w");
    assert_eq!(h.echo(), "sub .hidden a.rs");
    assert_eq!(h.eval("(current-kill)"), "\"sub .hidden a.rs\"");
}

#[test]
fn the_pointer_goes_to_files_opens_them_and_marks_them() {
    let (dir, mut h) = setup();
    h.press("<mouse-1>", 30, 4);
    assert_eq!(at_point(&mut h), "\"a.rs\"");
    // Beside the listing there is no file, and nothing happens.
    h.press("<mouse-1>", 30, 9);
    assert_eq!(at_point(&mut h), "\"a.rs\"");

    h.press("C-<mouse-1>", 3, 5);
    assert!(h.screen.row(5).starts_with('*'), "{}", h.screen.row(5));
    h.press("C-<mouse-1>", 3, 5);
    assert!(h.screen.row(5).starts_with(' '), "{}", h.screen.row(5));
    h.press("<mouse-1>", 3, 2).press("S-<mouse-1>", 3, 4);
    assert_eq!(
        h.eval("(map file-name-nondirectory (dired-marked-files))"),
        "(\"sub\" \".hidden\" \"a.rs\")"
    );
    h.keys("U");

    // The button for menus says what can be done with the file.
    h.press("<mouse-3>", 3, 5);
    assert!(
        h.screen.text().contains("Copy the name"),
        "{}",
        h.screen.text()
    );
    h.keys("w");
    assert_eq!(h.echo(), "b.txt");
    // The menu took rows from the window, which scrolled to keep point in
    // sight; back to the top.
    h.keys("M-<");

    // A second press opens: a directory here, a file in its buffer.
    h.press("<mouse-1>", 30, 2).press("<double-mouse-1>", 30, 2);
    let sub = shown(&mut h, &dir.path().join("sub"));
    assert_eq!(h.screen.row(0), format!("{sub}:"));
    h.press("<mouse-1>", 30, 1).press("<double-mouse-1>", 30, 1);
    assert!(h.mode_line().contains("inner.txt"), "{}", h.mode_line());
}

#[test]
fn files_are_dragged_into_a_directory_of_the_listing() {
    let (dir, mut h) = setup();
    let root = dir.path();
    let none = amacs_proto::Mods::empty();
    // Over the directory, the echo area says what letting go would do.
    h.press("<mouse-1>", 30, 5).drag_to(30, 3).drag_to(30, 2);
    assert!(
        h.echo().starts_with("Move b.txt to ") && h.echo().ends_with("/sub/"),
        "{}",
        h.echo()
    );
    assert_eq!(
        at_point(&mut h),
        "\"sub\"",
        "the directory is shown as where to"
    );
    h.release(30, 2, none);
    assert!(h.prompt().starts_with("Move b.txt to "), "{}", h.prompt());
    h.keys("y");
    assert!(root.join("sub/b.txt").exists());
    assert!(!root.join("b.txt").exists());
    assert_eq!(h.echo(), "Moved 1 file");
    assert_eq!(names(&mut h), "(\"empty\" \"sub\" \".hidden\" \"a.rs\")");

    // Let go where it came from, nothing happens; answered no, neither.
    h.press("<mouse-1>", 30, 4)
        .drag_to(30, 3)
        .release(30, 3, none);
    assert_eq!(h.echo(), "Nothing was moved");
    h.press("<mouse-1>", 30, 4)
        .drag_to(30, 1)
        .release(30, 1, none);
    h.keys("n");
    assert!(root.join("a.rs").exists());

    // The marked files go together, and with Control they are copied.
    h.eval("(set-option! 'dired-confirm-drop #f)");
    h.press("C-<mouse-1>", 3, 3).press("C-<mouse-1>", 3, 4);
    h.press("<mouse-1>", 30, 4)
        .drag_to(30, 1)
        .release(30, 1, amacs_proto::Mods::CTRL);
    h.wait_until(|h| h.echo() == "Copied 2 files");
    assert!(root.join("empty/a.rs").exists() && root.join("empty/.hidden").exists());
    assert!(root.join("a.rs").exists());

    // A directory does not go into itself.
    h.press("<mouse-1>", 30, 2)
        .drag_to(30, 3)
        .drag_to(30, 2)
        .release(30, 2, none);
    assert_eq!(h.echo(), "Nothing was moved");
    assert!(root.join("sub/inner.txt").exists());
}

#[test]
fn files_are_dragged_from_one_window_to_another() {
    let (dir, mut h) = setup();
    let root = dir.path();
    let none = amacs_proto::Mods::empty();
    h.eval("(set-option! 'dired-confirm-drop #f)");
    h.keys("C-x 3 C-x o");
    open(&mut h, &root.join("sub"));
    h.keys("C-x o");
    // From the listing on the left to the empty part of that on the right.
    h.press("<mouse-1>", 30, 5)
        .drag_to(60, 8)
        .release(60, 8, none);
    assert!(root.join("sub/b.txt").exists(), "{}", h.echo());
    assert!(!root.join("b.txt").exists());
    assert!(h.screen.row(1).contains("b.txt"), "{}", h.screen.row(1));

    // Over a window that shows something else, a file is visited there.
    h.keys("C-x o");
    h.eval(r#"(switch-to-buffer "*scratch*")"#);
    h.keys("C-x o");
    h.press("<mouse-1>", 30, 4)
        .drag_to(60, 8)
        .release(60, 8, none);
    assert!(
        h.screen.row(0).contains("fn main() {}"),
        "{}",
        h.screen.row(0)
    );
}

#[test]
fn the_first_line_leads_to_the_directories_above() {
    let (dir, mut h) = setup();
    let root = dir.path();
    let none = amacs_proto::Mods::empty();
    h.eval("(set-option! 'dired-confirm-drop #f)");
    open(&mut h, &root.join("sub"));
    let top = shown(&mut h, root);
    let sub = format!("{top}/sub");
    assert_eq!(h.screen.row(0), format!("{sub}:"));
    // The last part is where one is; past the name there is nothing.
    let last = sub.chars().count() as u16 - 1;
    h.press("<mouse-1>", last, 0)
        .press("<mouse-1>", last + 5, 0);
    assert_eq!(h.screen.row(0), format!("{sub}:"));

    // A file is dragged up to a part of the name.
    let parent = top.chars().count() as u16 - 1;
    h.press("<mouse-1>", 30, 1).drag_to(parent, 0);
    assert_eq!(h.echo(), format!("Move inner.txt to {top}/"));
    h.release(parent, 0, none);
    assert!(root.join("inner.txt").exists(), "{}", h.echo());
    assert!(!root.join("sub/inner.txt").exists());

    // A press on a part goes there, with point on where one came from.
    h.press("<mouse-1>", parent, 0);
    assert_eq!(h.screen.row(0), format!("{top}:"));
    assert_eq!(at_point(&mut h), "\"sub\"");
    // Several levels at once, to the top, and the bar counts as its part.
    open(&mut h, &root.join("sub"));
    h.press("<mouse-1>", 0, 0);
    let first = h.eval("(abbreviate-file-name (dired-directory))");
    assert!(first == "\"/\"" || first == "\"~\"", "{first}");

    // The button at the side of a mouse goes up.
    open(&mut h, &root.join("sub"));
    assert_eq!(h.eval("(key-binding \"<mouse-8>\")"), "dired-up-directory");
    h.press("<mouse-8>", 30, 5);
    assert_eq!(h.screen.row(0), format!("{top}:"));
    assert_eq!(at_point(&mut h), "\"sub\"");
}

/// A program that writes the names it is given to a file, as the program
/// that opens files, and the file it writes to.
fn recording_opener(h: &mut Harness, dir: &Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let (program, record) = (dir.join("opener"), dir.join("opened"));
    std::fs::write(
        &program,
        format!("#!/bin/sh\necho \"$1\" >> {}\n", record.display()),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    h.eval(&format!(
        "(set-option! 'dired-open-program {:?})",
        program.display().to_string()
    ));
    record
}

fn wait_for_record(record: &Path, wanted: &str) {
    let start = std::time::Instant::now();
    loop {
        let found = std::fs::read_to_string(record).unwrap_or_default();
        if found == wanted {
            return;
        }
        assert!(
            start.elapsed() < std::time::Duration::from_secs(10),
            "the opener wrote {found:?}, not {wanted:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn files_that_are_not_text_open_with_another_program() {
    let tools = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("a.iso"), [b"CD001".as_slice(), &[0; 64]].concat()).unwrap();
    std::fs::write(root.join("b.html"), "<p>text</p>\n").unwrap();
    let mut h = Harness::new(80, 12);
    h.eval("(set-option! 'dired-icons #f)");
    let record = recording_opener(&mut h, tools.path());
    open(&mut h, root);

    // RET and a second press of the button do the same.
    h.keys("RET");
    let iso = root.canonicalize().unwrap().join("a.iso");
    wait_for_record(&record, &format!("{}\n", iso.display()));
    assert!(h.echo().starts_with("Opened a.iso with "), "{}", h.echo());
    assert!(h.mode_line().contains("(Dired)"), "{}", h.mode_line());
    h.press("<mouse-1>", 30, 1).press("<double-mouse-1>", 30, 1);
    wait_for_record(&record, &format!("{0}\n{0}\n", iso.display()));
    assert_eq!(h.eval("(member \"a.iso\" (buffer-list))"), "#f");

    // Text opens in the editor, unless the other program is asked for.
    std::fs::remove_file(&record).unwrap();
    h.keys("n E");
    let html = root.canonicalize().unwrap().join("b.html");
    wait_for_record(&record, &format!("{}\n", html.display()));
    h.keys("RET");
    assert!(h.mode_line().contains("b.html"), "{}", h.mode_line());

    // A program that is not there is an error and no more.
    h.keys("C-x k RET");
    h.eval("(set-option! 'dired-open-program \"/nonexistent/opener\")");
    h.keys("p RET");
    assert!(
        h.echo().contains("cannot run /nonexistent/opener"),
        "{}",
        h.echo()
    );
}

#[test]
fn a_large_file_is_asked_about_in_the_file_manager() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("big.log"), "a line of a log\n".repeat(100_000)).unwrap();
    let mut h = Harness::new(80, 12);
    h.eval("(set-option! 'dired-icons #f) (set-option! 'large-file-threshold 1)");
    open(&mut h, root);
    h.keys("RET");
    assert_eq!(h.prompt(), "big.log is 1.5M; open it anyway? (y or n)");
    h.keys("n");
    assert!(h.mode_line().contains("(Dired)"), "{}", h.mode_line());
    h.keys("RET y");
    assert!(h.mode_line().contains("big.log"), "{}", h.mode_line());
}

#[test]
fn the_directory_of_the_current_file_is_a_key_away() {
    let (dir, mut h) = setup();
    let root = dir.path();
    let top = shown(&mut h, root);
    h.eval(&format!(
        "(find-file {:?})",
        root.join("sub/inner.txt").display().to_string()
    ));
    h.keys("C-x C-j");
    assert_eq!(h.screen.row(0), format!("{top}/sub:"));
    assert_eq!(at_point(&mut h), "\"inner.txt\"");
    // From a listing, the same key goes up, with point on where one was.
    h.keys("C-x C-j");
    assert_eq!(h.screen.row(0), format!("{top}:"));
    assert_eq!(at_point(&mut h), "\"sub\"");
    // A buffer without a file has a working directory.
    h.eval(r#"(switch-to-buffer "*scratch*")"#);
    h.eval(&format!(
        "(set-buffer-directory! {:?})",
        root.join("empty").display().to_string()
    ));
    h.keys("C-x C-j");
    assert_eq!(h.screen.row(0), format!("{top}/empty:"));
}
