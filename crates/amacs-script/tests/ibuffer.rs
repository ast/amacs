//! The buffer list, which is written in Scheme: `runtime/lib/ibuffer.scm`.

mod common;

use std::path::Path;

use common::Harness;

/// A project with two files and a file outside it, all open, with changes
/// in `a.txt` that are not saved; and the buffer list, shown from `b.rs`.
fn setup() -> (tempfile::TempDir, Harness) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let project = root.join("proj");
    std::fs::create_dir_all(project.join(".git")).unwrap();
    std::fs::write(project.join("a.txt"), "alpha\n").unwrap();
    std::fs::write(project.join("b.rs"), "fn main() {}\n").unwrap();
    std::fs::create_dir(root.join("elsewhere")).unwrap();
    std::fs::write(root.join("elsewhere/notes.txt"), "notes\n").unwrap();
    let mut h = Harness::new(100, 24);
    visit(&mut h, &project.join("a.txt"));
    h.eval("(insert \"changed \")");
    visit(&mut h, &root.join("elsewhere/notes.txt"));
    visit(&mut h, &project.join("b.rs"));
    h.keys("C-x C-b");
    (dir, h)
}

fn visit(h: &mut Harness, path: &Path) {
    h.eval(&format!("(find-file {:?})", path.display().to_string()));
}

/// The names listed, in order.
fn names(h: &mut Harness) -> String {
    h.eval("(map ibuffer-row-name (ibuffer-rows))")
}

fn at_point(h: &mut Harness) -> String {
    h.eval("(ibuffer-buffer-at-point)")
}

fn marked(h: &mut Harness) -> String {
    h.eval("(ibuffer-marked-buffers)")
}

#[test]
fn buffers_are_listed_by_project_with_the_current_one_at_point() {
    let (dir, mut h) = setup();
    assert!(h.mode_line().contains("(Ibuffer)"), "{}", h.mode_line());
    assert_eq!(
        names(&mut h),
        "(\"b.rs\" \"a.txt\" \"notes.txt\" \"*scratch*\")"
    );
    // The projects first, then files in none, then the rest; the summary
    // on the first line.
    let project = h
        .eval(&format!(
            "(abbreviate-file-name {:?})",
            dir.path().join("proj").display().to_string()
        ))
        .trim_matches('"')
        .to_owned();
    assert_eq!(h.screen.row(0), "4 buffers, 1 unsaved");
    assert_eq!(h.screen.row(1), format!("▾ {project}  2"));
    assert!(h.screen.row(4).starts_with("▾ Files outside projects  1"));
    assert!(h.screen.row(6).starts_with("▾ Other buffers  1"));
    // Each buffer with its flags, size, mode and file within the project.
    let fields = |row: String| {
        row.split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    assert_eq!(fields(h.screen.row(2)), ["b.rs", "13", "rust", "b.rs"]);
    assert_eq!(
        fields(h.screen.row(3)),
        ["*", "a.txt", "14", "text", "a.txt"]
    );
    assert_eq!(h.screen.row(3).find("a.txt"), h.screen.row(2).find("b.rs"));
    assert!(h.screen.row(7).contains("*scratch*"), "{}", h.screen.row(7));
    // Point is on the buffer the list was opened from, at its name.
    assert_eq!(at_point(&mut h), "\"b.rs\"");
    assert_eq!(h.eval("(current-column)"), "6");
    // n and p go from buffer to buffer, over the headings of groups.
    h.keys("n n");
    assert_eq!(at_point(&mut h), "\"notes.txt\"");
    h.keys("p p");
    assert_eq!(at_point(&mut h), "\"b.rs\"");
    h.keys("p");
    assert_eq!(at_point(&mut h), "\"b.rs\"");
    // The list cannot be typed into.
    h.keys("C-d");
    assert!(h.echo().contains("read-only"), "{}", h.echo());
}

#[test]
fn marks_and_the_commands_that_act_on_them() {
    let (_dir, mut h) = setup();
    // m marks and moves on; the marks show on the line and in the counts.
    h.keys("m");
    assert_eq!(at_point(&mut h), "\"a.txt\"");
    assert_eq!(marked(&mut h), "(\"b.rs\")");
    assert!(
        h.screen.row(2).starts_with("  >   b.rs"),
        "{}",
        h.screen.row(2)
    );
    assert_eq!(h.screen.row(0), "4 buffers, 1 unsaved, 1 marked");
    assert!(
        h.screen.row(1).ends_with("  2, 1 marked"),
        "{}",
        h.screen.row(1)
    );
    // Without marks, the commands act on the buffer at point.
    h.keys("u p u");
    assert_eq!(marked(&mut h), "(\"b.rs\")");
    // t toggles, U clears, and m on a group's heading marks the group.
    h.keys("t");
    assert_eq!(marked(&mut h), "(\"a.txt\" \"notes.txt\" \"*scratch*\")");
    h.keys("U");
    assert_eq!(marked(&mut h), "(\"notes.txt\")");
    h.eval("(goto-line 2)");
    h.keys("m");
    assert_eq!(marked(&mut h), "(\"b.rs\" \"a.txt\")");
    // The menu marks by kind.
    h.keys("U * u");
    assert_eq!(h.echo(), "Marked 1 buffer");
    assert_eq!(marked(&mut h), "(\"a.txt\")");
    // Killing asks, and asks again for changes that would be lost.
    h.keys("D");
    assert!(h.prompt().starts_with("Kill a.txt?"), "{}", h.prompt());
    h.keys("y");
    assert!(
        h.prompt()
            .starts_with("a.txt has unsaved changes; kill it anyway?"),
        "{}",
        h.prompt()
    );
    h.keys("y");
    assert_eq!(h.echo(), "Killed 1 buffer");
    assert_eq!(names(&mut h), "(\"b.rs\" \"notes.txt\" \"*scratch*\")");
    assert_eq!(h.eval("(member \"a.txt\" (buffer-list))"), "#f");
    assert_eq!(h.screen.row(0), "3 buffers");
    // d flags, x kills the flagged.
    h.keys("n n d");
    assert_eq!(h.screen.row(4).trim_start().chars().next(), Some('D'));
    h.keys("x y");
    assert_eq!(h.echo(), "Killed 1 buffer");
    assert_eq!(names(&mut h), "(\"b.rs\" \"*scratch*\")");
    assert_eq!(at_point(&mut h), "\"*scratch*\"");
}

#[test]
fn marked_buffers_are_saved_together() {
    let (dir, mut h) = setup();
    h.eval("(with-current-buffer \"notes.txt\" (insert \"more \"))");
    h.keys("g");
    assert_eq!(h.screen.row(0), "4 buffers, 2 unsaved");
    // Nothing of the buffer at point needs saving.
    h.keys("S");
    assert_eq!(h.echo(), "Nothing here needs saving");
    h.keys("t S");
    assert_eq!(h.echo(), "Saved 2 buffers");
    assert_eq!(h.screen.row(0), "4 buffers, 4 marked");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("proj/a.txt")).unwrap(),
        "changed alpha\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("elsewhere/notes.txt")).unwrap(),
        "more notes\n"
    );
    // The list stayed on screen throughout.
    assert!(h.mode_line().contains("*Ibuffer*"), "{}", h.mode_line());
}

#[test]
fn groups_fold_and_the_list_sorts_narrows_and_shows_hidden_buffers() {
    let (_dir, mut h) = setup();
    // TAB folds the group of the buffer at point, and opens it again.
    h.keys("TAB");
    assert!(h.screen.row(1).starts_with("▸ "), "{}", h.screen.row(1));
    assert!(h.screen.row(2).starts_with("▾ Files outside projects"));
    assert_eq!(h.eval("(line-number-at-pos)"), "2");
    h.keys("n");
    assert_eq!(at_point(&mut h), "\"notes.txt\"");
    h.eval("(goto-line 2)");
    h.keys("TAB n");
    assert!(h.screen.row(1).starts_with("▾ "), "{}", h.screen.row(1));
    assert_eq!(at_point(&mut h), "\"b.rs\"");
    // s sorts each group: by name, by size, by mode, by last use.
    h.keys("s");
    assert_eq!(h.echo(), "Sorted by name");
    assert_eq!(
        names(&mut h),
        "(\"a.txt\" \"b.rs\" \"notes.txt\" \"*scratch*\")"
    );
    assert_eq!(at_point(&mut h), "\"b.rs\"");
    h.keys("s s s");
    assert_eq!(h.echo(), "Sorted by last use");
    assert_eq!(
        names(&mut h),
        "(\"b.rs\" \"a.txt\" \"notes.txt\" \"*scratch*\")"
    );
    // / narrows to names and files that contain a text; g shows all.
    h.keys("/");
    h.type_text("NOTES");
    h.keys("RET");
    assert_eq!(names(&mut h), "(\"notes.txt\")");
    assert_eq!(h.screen.row(0), "1 buffer  (narrowed to NOTES)");
    assert_eq!(at_point(&mut h), "\"notes.txt\"");
    h.keys("g");
    assert_eq!(h.screen.row(0), "4 buffers, 1 unsaved");
    // . shows the buffers that the editor keeps for itself.
    h.eval("(switch-to-buffer \" hidden\") (switch-to-buffer \"*Ibuffer*\")");
    h.keys("g");
    assert!(!names(&mut h).contains("hidden"));
    h.keys(".");
    assert!(names(&mut h).contains("\" hidden\""), "{}", names(&mut h));
    h.keys(".");
    assert!(!names(&mut h).contains("hidden"));
}

#[test]
fn a_buffer_is_visited_from_the_list_and_the_list_is_left() {
    let (_dir, mut h) = setup();
    h.keys("n RET");
    assert!(h.mode_line().contains("a.txt"), "{}", h.mode_line());
    assert_eq!(h.text(), "changed alpha\n");
    // The list is still there, with its marks, and point follows.
    h.keys("C-x C-b");
    assert_eq!(at_point(&mut h), "\"a.txt\"");
    h.keys("m p");
    assert_eq!(marked(&mut h), "(\"a.txt\")");
    // Visiting a.txt made it the latest; notes.txt is two on from it.
    h.keys("C-x C-b n n o");
    assert_eq!(h.eval("(length (window-list))"), "2");
    assert!(h.mode_line().contains("notes.txt"), "{}", h.mode_line());
    h.keys("C-x 0 C-x C-b");
    assert_eq!(marked(&mut h), "(\"a.txt\")");
    // w copies the files of the marked buffers.
    h.keys("w");
    assert!(h.echo().ends_with("proj/a.txt"), "{}", h.echo());
    h.keys("q");
    assert_eq!(h.eval("(member \"*Ibuffer*\" (buffer-list))"), "#f");
}

#[test]
fn the_pointer_folds_groups_and_opens_buffers() {
    let (_dir, mut h) = setup();
    // A press on a heading folds the group; a press on a buffer goes to it.
    h.press("<mouse-1>", 3, 1);
    assert!(h.screen.row(1).starts_with("▸ "), "{}", h.screen.row(1));
    h.press("<mouse-1>", 3, 1);
    h.press("<mouse-1>", 10, 3);
    assert_eq!(at_point(&mut h), "\"a.txt\"");
    assert_eq!(h.eval("(current-column)"), "6");
    // With Control held, a press marks.
    h.press("C-<mouse-1>", 10, 2);
    assert_eq!(marked(&mut h), "(\"b.rs\")");
    h.press("C-<mouse-1>", 10, 2);
    assert_eq!(marked(&mut h), "(\"b.rs\")");
    assert!(
        h.screen.row(2).starts_with("      b.rs"),
        "{}",
        h.screen.row(2)
    );
    // A second press opens.
    h.press("<mouse-1>", 10, 3);
    h.press("<double-mouse-1>", 10, 3);
    assert!(h.mode_line().contains("a.txt"), "{}", h.mode_line());
}
