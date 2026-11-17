//! Getting about and the smaller conveniences: marks over the text, jumping
//! by label, windows, projects, the undo tree, rectangles, files changed by
//! other programs, and assorted editing commands.

use std::time::Duration;

use amacs_core::Value;
use amacs_proto::FaceAttrs;
use amacs_testkit::Harness;

fn harness() -> Harness {
    Harness::new(60, 14)
}

fn face(h: &Harness, name: &str) -> FaceAttrs {
    let faces = h.editor.faces();
    faces.table()[faces.face_id(name).unwrap().0 as usize]
}

fn enable(h: &mut Harness, mode: &str) {
    h.editor.set_minor_mode(mode, false, true).unwrap();
    h.sync();
}

fn mode_line(h: &Harness) -> String {
    h.screen.row(h.screen.size().1 - 2)
}

#[test]
fn the_current_line_is_marked() {
    let mut h = harness();
    h.editor.set_language(Some("rust")).unwrap();
    h.paste("let a = 1;\nlet b = 2;");
    enable(&mut h, "hl-line-mode");
    let marked = face(&h, "hl-line").bg;
    assert_eq!(h.screen.face_at(1, 4).bg, marked);
    assert_eq!(
        h.screen.face_at(1, 40).bg,
        marked,
        "to the edge of the window"
    );
    assert_ne!(h.screen.face_at(0, 4).bg, marked);
    // The text keeps its own colours.
    assert_eq!(h.screen.face_at(1, 0).fg, face(&h, "keyword").fg);
    assert!(h.screen.face_at(1, 0).bold);
    h.keys("C-p");
    assert_eq!(h.screen.face_at(0, 4).bg, marked);
    assert_ne!(h.screen.face_at(1, 4).bg, marked);
}

#[test]
fn other_occurrences_of_the_symbol_at_point_are_marked() {
    let mut h = harness();
    h.paste("count + counter + count\ncount_x count");
    enable(&mut h, "symbol-highlight-mode");
    h.keys("M-<");
    let marked = face(&h, "symbol-highlight").bg;
    let plain = face(&h, "default").bg;
    let bg = |h: &Harness, row, col| h.screen.face_at(row, col).bg;
    assert_eq!(bg(&h, 0, 0), marked, "the one at point too");
    assert_eq!(bg(&h, 0, 18), marked);
    assert_eq!(bg(&h, 1, 8), marked);
    assert_eq!(bg(&h, 0, 8), plain, "not part of a longer name");
    assert_eq!(bg(&h, 1, 0), plain);
    h.keys("M-f C-f C-f C-f");
    assert_eq!(bg(&h, 0, 8), marked, "now the longer name is the symbol");
    assert_eq!(bg(&h, 0, 0), plain);
}

#[test]
fn words_like_todo_stand_out_in_comments_only() {
    let mut h = harness();
    h.editor.set_language(Some("rust")).unwrap();
    h.paste("// TODO: fix\nlet TODO = 1; // FIXME later");
    enable(&mut h, "hl-todo-mode");
    let todo = face(&h, "todo").fg;
    assert_eq!(h.screen.face_at(0, 3).fg, todo);
    assert!(h.screen.face_at(0, 3).bold);
    assert_ne!(h.screen.face_at(1, 4).fg, todo, "not in code");
    assert_eq!(h.screen.face_at(1, 17).fg, todo);
    assert_ne!(h.screen.face_at(0, 9).fg, todo);
}

#[test]
fn stray_whitespace_and_long_lines_are_shown() {
    let mut h = harness();
    h.editor
        .set_option("whitespace-line-column", Value::Int(10))
        .unwrap();
    h.paste("trailing   \n0123456789abc\nfine\nend  ");
    enable(&mut h, "whitespace-mode");
    let trailing = face(&h, "trailing-whitespace").bg;
    let long = face(&h, "long-line").bg;
    assert_eq!(h.screen.face_at(0, 8).bg, trailing);
    assert_eq!(h.screen.face_at(0, 10).bg, trailing);
    assert_ne!(h.screen.face_at(0, 7).bg, trailing);
    assert_eq!(h.screen.face_at(1, 10).bg, long);
    assert_eq!(h.screen.face_at(1, 12).bg, long);
    assert_ne!(h.screen.face_at(1, 9).bg, long);
    // Not where it is still being typed.
    assert_ne!(h.screen.face_at(3, 3).bg, trailing);
    h.keys("C-p");
    assert_eq!(h.screen.face_at(3, 3).bg, trailing);
}

#[test]
fn windows_are_selected_by_direction() {
    let mut h = Harness::new(80, 20);
    h.paste("text");
    // Three windows: two side by side above one.
    h.keys("C-x 2 C-x 3");
    let selected = |h: &Harness| h.screen.cursor().unwrap();
    assert_eq!(selected(&h), (0, 4));
    h.keys("S-<right>");
    assert_eq!(selected(&h).1, 45);
    h.keys("S-<right>");
    assert_eq!(h.echo(), "No window to the right");
    h.keys("S-<down>");
    assert_eq!(selected(&h).0, 10);
    h.keys("S-<up>");
    assert_eq!(selected(&h), (0, 4), "above the cursor's column");
    h.keys("S-<left>");
    assert_eq!(h.echo(), "No window to the left");
}

#[test]
fn windows_can_be_resized_and_balanced() {
    let mut h = Harness::new(80, 21);
    h.keys("C-x 2");
    let divider = |h: &Harness| {
        (0..20)
            .find(|row| h.screen.row(*row).contains("*scratch*"))
            .unwrap()
    };
    assert_eq!(divider(&h), 9);
    h.keys("C-x ^ C-x ^");
    assert_eq!(divider(&h), 11);
    h.keys("C-u 3 M-x").type_text("shrink-window").keys("M-RET");
    assert_eq!(divider(&h), 8);
    h.keys("C-x +");
    assert_eq!(divider(&h), 9);
    h.keys("C-x }");
    assert_eq!(
        h.echo(),
        "No window beside this one to take columns from or give them to"
    );
    h.keys("C-x 3 C-x }");
    assert!(
        h.screen.row(0).chars().nth(41) == Some('│'),
        "{}",
        h.screen.row(0)
    );
    h.keys("C-u 3 0 C-x ^");
    assert_eq!(
        h.echo(),
        "No window above or below to take rows from or give them to",
        "a window keeps a few rows"
    );
}

/// A project of a few files, with a nested crate.
fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("tools/helper/src")).unwrap();
    std::fs::write(root.join("README.md"), "readme\n").unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(root.join("src/lib.rs"), "// lib\n").unwrap();
    std::fs::write(root.join("tools/helper/Cargo.toml"), "[package]\n").unwrap();
    std::fs::write(root.join("tools/helper/src/main.rs"), "// helper\n").unwrap();
    dir
}

#[test]
fn files_and_buffers_of_a_project_are_found_by_name() {
    let dir = project();
    let root = dir.path().canonicalize().unwrap();
    let mut h = Harness::new(100, 20);
    h.keys("C-c p f");
    assert_eq!(h.echo(), "This buffer is not in a project");

    h.editor.find_file(&root.join("README.md")).unwrap();
    h.sync();
    h.keys("C-c p f");
    assert!(
        h.prompt().starts_with("1/5 Find file in "),
        "{}",
        h.prompt()
    );
    assert_eq!(
        h.candidates(),
        [
            "README.md",
            "src/lib.rs",
            "src/main.rs",
            "tools/helper/Cargo.toml",
            "tools/helper/src/main.rs"
        ]
    );
    h.type_text("helper main").keys("RET");
    assert_eq!(h.buffer_text(), "// helper\n");

    h.keys("C-c p b");
    assert_eq!(
        h.candidates(),
        [
            "README.md  README.md",
            "main.rs    tools/helper/src/main.rs"
        ]
    );
    h.keys("RET");
    assert_eq!(h.buffer_text(), "readme\n");

    // With more markers, the crate inside is a project of its own.
    let markers = Value::List(vec![Value::str(".git"), Value::str("Cargo.toml")]);
    h.editor
        .set_option("project-root-markers", markers)
        .unwrap();
    h.keys("C-x b").type_text("main.rs").keys("RET C-c p f");
    assert_eq!(h.candidates(), ["Cargo.toml", "src/main.rs"]);
    h.keys("C-g");

    h.keys("C-c p k");
    assert!(
        h.echo().starts_with("Kill the 1 buffers of "),
        "{}",
        h.echo()
    );
    h.type_text("y");
    assert_eq!(h.buffer_text(), "readme\n");
}

#[test]
fn projects_visited_before_can_be_switched_to() {
    let (first, second) = (project(), project());
    let mut h = Harness::new(100, 20);
    h.keys("C-c p p");
    assert_eq!(h.echo(), "No projects have been visited yet");
    for dir in [&first, &second] {
        let root = dir.path().canonicalize().unwrap();
        h.editor.find_file(&root.join("README.md")).unwrap();
    }
    h.sync();
    h.keys("C-c p p");
    assert_eq!(h.candidates().len(), 2);
    // The earlier project is the second candidate.
    h.keys("C-n RET");
    assert!(
        h.prompt().starts_with("1/5 Find file in "),
        "{}",
        h.prompt()
    );
    h.type_text("lib").keys("RET");
    let expected = first.path().canonicalize().unwrap().join("src/lib.rs");
    assert_eq!(h.editor.buffer().file.as_deref(), Some(expected.as_path()));
}

#[test]
fn the_undo_history_is_a_tree_to_walk() {
    let mut h = harness();
    // Three steps of history: moving about ends a run of typing.
    h.type_text("one ").keys("C-a C-e");
    h.type_text("two ").keys("C-a C-e");
    h.type_text("three");
    // Back two steps, then a new branch.
    h.keys("C-/ C-/");
    assert_eq!(h.buffer_text(), "one ");
    h.type_text("zwei");
    h.keys("M-x").type_text("undo-tree-visualize").keys("RET");
    assert_eq!(
        h.echo_lines(),
        [
            "Undo tree: b f move, p n branch, q done, C-g cancel",
            "◎──○──○──○",
            "   └──●",
        ]
    );
    h.type_text("b");
    assert_eq!(h.buffer_text(), "one ");
    // Forward goes down the branch last used; p crosses to the other.
    h.type_text("f");
    assert_eq!(h.buffer_text(), "one zwei");
    h.type_text("p");
    assert_eq!(h.buffer_text(), "one two ");
    assert_eq!(h.echo_lines()[1], "◎──○──●──○");
    h.type_text("f");
    assert_eq!(h.buffer_text(), "one two three");
    h.type_text("f");
    assert_eq!(h.echo_lines()[0], "This is the last state of the branch");
    h.type_text("p");
    assert_eq!(h.echo_lines()[0], "No other branch that way");
    h.type_text("q");
    assert_eq!(h.buffer_text(), "one two three");
    assert_eq!(h.echo_lines().len(), 1, "the tree is gone");
    // Typing goes into the buffer again.
    h.type_text("!");
    assert_eq!(h.buffer_text(), "one two three!");

    // Cancelling returns to where the tree was opened.
    h.keys("M-x").type_text("undo-tree-visualize").keys("RET");
    h.type_text("bb").keys("C-g");
    assert_eq!(h.buffer_text(), "one two three!");
    assert_eq!(h.echo(), "Quit");
    // Any other command leaves the tree where it is.
    h.keys("M-x").type_text("undo-tree-visualize").keys("RET");
    h.type_text("b").keys("C-a");
    assert_eq!(h.buffer_text(), "one two three");
    h.type_text("x");
    assert_eq!(h.buffer_text(), "xone two three");
}

#[test]
fn rectangles_are_killed_yanked_and_filled() {
    let mut h = harness();
    h.paste("abcdef\nghijkl\nmnopqr");
    // Columns 1 to 3 of all three lines.
    h.keys("M-< C-f C-SPC C-n C-n C-f C-f");
    h.keys("C-x r k");
    assert_eq!(h.buffer_text(), "adef\ngjkl\nmpqr");
    assert_eq!(h.screen.cursor(), Some((0, 1)));
    h.keys("C-e C-x r y");
    assert_eq!(h.buffer_text(), "adefbc\ngjklhi\nmpqrno");
    assert_eq!(h.screen.cursor(), Some((2, 6)));
    // Past the end of the text, lines are made and padded.
    h.keys("M-> RET C-x r y");
    assert_eq!(h.buffer_text(), "adefbc\ngjklhi\nmpqrno\nbc\nhi\nno");
    h.keys("C-/ C-/");
    assert_eq!(h.buffer_text(), "adefbc\ngjklhi\nmpqrno");

    h.keys("M-< C-SPC C-n C-n C-f C-f C-x r t");
    h.type_text("> ").keys("RET");
    assert_eq!(h.buffer_text(), "> efbc\n> klhi\n> qrno");
    h.keys("M-< C-SPC C-n C-f C-x r o");
    assert_eq!(h.buffer_text(), " > efbc\n > klhi\n> qrno");
    h.keys("M-< C-SPC C-n C-f C-x r d");
    assert_eq!(h.buffer_text(), "> efbc\n> klhi\n> qrno");
}

#[test]
fn a_marked_rectangle_is_shown_and_killed_as_one() {
    let mut h = harness();
    h.paste("abcdef\nghijkl\nmnopqr");
    h.keys("M-< C-f C-x SPC C-n C-f C-f");
    let region = face(&h, "region").bg;
    let bg = |h: &Harness, row, col| h.screen.face_at(row, col).bg;
    assert_eq!(bg(&h, 0, 1), region);
    assert_eq!(bg(&h, 1, 2), region);
    assert_ne!(bg(&h, 0, 4), region, "only the columns between");
    assert_ne!(bg(&h, 1, 0), region);
    h.keys("M-w C-g M-> RET C-x r y");
    assert_eq!(h.buffer_text(), "abcdef\nghijkl\nmnopqr\nbc\nhi");
    // The usual kill command takes the rectangle.
    h.keys("M-< C-x SPC C-n C-f C-w");
    assert_eq!(h.buffer_text(), "bcdef\nhijkl\nmnopqr\nbc\nhi");
    // And the next region is an ordinary one again.
    h.keys("C-SPC C-n C-w");
    assert_eq!(h.buffer_text(), "hijkl\nmnopqr\nbc\nhi");
}

#[test]
fn a_file_changed_on_disk_is_read_again() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes.txt");
    std::fs::write(&file, "one\ntwo\nthree\n").unwrap();
    let mut h = harness();
    h.editor.find_file(&file).unwrap();
    h.sync();
    h.keys("C-n C-f");

    // Another program rewrites the file.
    let later = std::time::SystemTime::now() + Duration::from_secs(10);
    std::fs::write(&file, "zero\none\ntwo, changed\nthree\n").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(later)
        .unwrap();
    h.wait(Duration::from_secs(6));
    assert_eq!(h.buffer_text(), "zero\none\ntwo, changed\nthree\n");
    assert_eq!(h.echo(), "Reverted notes.txt: its file changed on disk");
    assert_eq!(h.screen.cursor(), Some((1, 1)), "the same line and column");
    assert!(!h.editor.buffer().is_modified());

    // A buffer with changes of its own is left alone.
    h.type_text("mine ");
    let much_later = later + Duration::from_secs(10);
    std::fs::write(&file, "theirs\n").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(much_later)
        .unwrap();
    h.wait(Duration::from_secs(12));
    assert!(h.buffer_text().contains("mine"));
    // Until the user says to drop them.
    h.keys("M-x").type_text("revert-buffer").keys("RET");
    assert_eq!(h.echo(), "Discard the changes to notes.txt? (y or n)");
    h.type_text("y");
    assert_eq!(h.buffer_text(), "theirs\n");
    assert_eq!(h.echo(), "Reverted");

    // The option turns the watching off.
    h.editor
        .set_option("auto-revert", Value::Bool(false))
        .unwrap();
    std::fs::write(&file, "again\n").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(much_later + Duration::from_secs(10))
        .unwrap();
    h.wait(Duration::from_secs(30));
    assert_eq!(h.buffer_text(), "theirs\n");
}

#[test]
fn lines_are_commented_moved_and_joined() {
    let mut h = harness();
    h.editor.set_language(Some("rust")).unwrap();
    h.paste("fn f() {\n    one();\n\n    two();\n}");
    h.keys("M-< C-n C-SPC C-n C-n C-n M-;");
    assert_eq!(
        h.buffer_text(),
        "fn f() {\n    // one();\n\n    // two();\n}"
    );
    h.keys("M-< C-n C-SPC C-n C-n C-n M-;");
    assert_eq!(h.buffer_text(), "fn f() {\n    one();\n\n    two();\n}");
    // Without a region, the line of point.
    h.keys("M-< M-;");
    assert_eq!(h.screen.row(0), "// fn f() {");
    h.keys("M-;");
    assert_eq!(h.screen.row(0), "fn f() {");

    h.keys("M-x").type_text("move-text-down").keys("RET");
    assert_eq!(h.buffer_text(), "    one();\nfn f() {\n\n    two();\n}");
    assert_eq!(h.screen.cursor(), Some((1, 0)), "point goes with the line");
    h.keys("M-x").type_text("move-text-up").keys("RET");
    assert_eq!(h.buffer_text(), "fn f() {\n    one();\n\n    two();\n}");
    h.keys("M-x").type_text("move-text-up").keys("RET");
    assert_eq!(h.echo(), "Already at the top");

    h.keys("C-n M-m");
    assert_eq!(h.screen.cursor(), Some((1, 4)));
    h.keys("M-^");
    assert_eq!(
        h.screen.row(0),
        "fn f() {one();",
        "no space after a bracket"
    );
    h.keys("C-/ M-> M-^");
    assert_eq!(
        h.buffer_text(),
        "fn f() {\n    one();\n\n    two();}",
        "nor before one"
    );

    h.keys("M-< M-f SPC SPC SPC C-b M-SPC");
    assert_eq!(h.screen.row(0), "fn f() {");
    h.keys("M-\\");
    assert_eq!(h.screen.row(0), "fnf() {");
}

#[test]
fn letter_case_and_read_only() {
    let mut h = harness();
    h.paste("hello big world");
    h.keys("M-< M-u M-c M-l");
    assert_eq!(h.buffer_text(), "HELLO Big world");
    h.keys("C-x h C-x C-u");
    assert_eq!(h.buffer_text(), "HELLO BIG WORLD");
    h.keys("C-x h C-x C-l");
    assert_eq!(h.buffer_text(), "hello big world");
    h.keys("C-x C-q").type_text("x");
    assert_eq!(h.echo(), "Buffer is read-only: *scratch*");
    assert!(mode_line(&h).starts_with(" %%"));
    h.keys("C-x C-q").type_text("x");
    assert!(h.buffer_text().contains('x'));
}

#[test]
fn text_is_replaced_one_place_at_a_time_or_all_at_once() {
    let mut h = harness();
    h.paste("cat hat cat mat cat sat cat");
    h.keys("M-< M-%").type_text("cat").keys("RET");
    assert_eq!(h.prompt(), "Query replace cat with:");
    h.type_text("dog").keys("RET");
    assert_eq!(h.echo(), "Replace with dog? (y, n, ! for all, q to stop)");
    assert_eq!(
        h.screen.face_at(0, 1),
        face(&h, "region"),
        "the match is shown"
    );
    h.type_text("y");
    assert_eq!(h.buffer_text(), "dog hat cat mat cat sat cat");
    h.type_text("n");
    assert_eq!(h.screen.face_at(0, 17), face(&h, "region"));
    h.type_text("!");
    assert_eq!(h.buffer_text(), "dog hat cat mat dog sat dog");
    assert_eq!(h.echo(), "Replaced 3 occurrences");

    h.keys("M-< M-%")
        .type_text("dog")
        .keys("RET")
        .type_text("ox")
        .keys("RET");
    h.type_text("yq");
    assert_eq!(h.buffer_text(), "ox hat cat mat dog sat dog");
    assert_eq!(h.echo(), "Replaced 1 occurrences");
    h.keys("M-%").type_text("zebra").keys("RET RET");
    assert_eq!(h.echo(), "Replaced 0 occurrences");
}

#[test]
fn the_symbol_at_point_is_stepped_through_and_renamed() {
    let mut h = harness();
    h.paste("total = total + subtotal;\nprint(total)");
    h.keys("M-< C-f");
    h.keys("M-x").type_text("symbol-next").keys("RET");
    assert_eq!(h.screen.cursor(), Some((0, 9)));
    assert_eq!(h.echo(), "total: 2 of 3");
    h.keys("M-x").type_text("symbol-next").keys("RET");
    assert_eq!(h.screen.cursor(), Some((1, 7)), "not the one in subtotal");
    h.keys("M-x").type_text("symbol-next").keys("RET");
    assert_eq!(h.screen.cursor(), Some((0, 1)), "round to the first");
    h.keys("M-x").type_text("symbol-previous").keys("RET");
    assert_eq!(h.screen.cursor(), Some((1, 7)));

    h.keys("M-x").type_text("symbol-rename").keys("RET");
    assert_eq!(h.prompt(), "Rename total in this buffer to: total");
    h.keys("C-a C-k").type_text("sum").keys("RET");
    assert_eq!(h.buffer_text(), "sum = sum + subtotal;\nprint(sum)");
    assert_eq!(h.echo(), "Renamed 3 occurrences");
    h.keys("C-/");
    assert_eq!(h.buffer_text(), "total = total + subtotal;\nprint(total)");
}

#[test]
fn bookmarks_remember_places_in_files() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes.txt");
    std::fs::write(&file, "first\nsecond\nthird\n").unwrap();
    let mut h = harness();
    h.keys("C-x r m");
    assert_eq!(h.echo(), "This buffer does not visit a file");
    h.keys("C-x r b");
    assert_eq!(h.echo(), "No bookmarks have been set");

    h.editor.find_file(&file).unwrap();
    h.sync();
    h.keys("C-n C-n C-x r m");
    assert_eq!(h.prompt(), "Set bookmark named: notes.txt");
    h.keys("C-a C-k").type_text("the third").keys("RET");
    assert_eq!(h.echo(), "Bookmark the third set");
    h.keys("C-x k RET C-x r b");
    assert!(
        h.candidates()[0].starts_with("the third  "),
        "{:?}",
        h.candidates()
    );
    h.keys("RET");
    assert_eq!(h.screen.cursor(), Some((2, 0)));
    assert_eq!(h.editor.buffer().name, "notes.txt");
    // Setting it again moves it rather than making a second.
    h.keys("M-< C-x r m C-a C-k")
        .type_text("the third")
        .keys("RET");
    h.keys("M-> C-x r b");
    assert_eq!(h.candidates().len(), 1);
    h.keys("RET");
    assert_eq!(h.screen.cursor(), Some((0, 0)));
    h.keys("M-x").type_text("bookmark-delete").keys("RET RET");
    assert_eq!(h.echo(), "Bookmark the third deleted");
}

#[test]
fn shell_commands_show_or_replace() {
    let mut h = Harness::new(60, 16);
    h.paste("b\na\nc\n");
    h.keys("M-!").type_text("echo hello").keys("RET");
    assert_eq!(h.echo(), "hello");
    h.keys("C-x h M-|").type_text("sort").keys("RET");
    // Several lines go to a buffer in another window.
    assert!(h.screen.text().contains("*Shell Command Output*"));
    assert_eq!(h.screen.row(8), "a");
    assert_eq!(h.buffer_text(), "b\na\nc\n", "the region is as it was");
    h.keys("C-x h C-u M-|").type_text("sort").keys("RET");
    assert_eq!(h.buffer_text(), "a\nb\nc\n");
    h.keys("C-x h C-u M-|").type_text("exit 3").keys("RET");
    assert!(h.echo().starts_with("The command failed"), "{}", h.echo());
    assert_eq!(h.buffer_text(), "a\nb\nc\n");
    h.keys("M-!").type_text("true").keys("RET");
    assert_eq!(h.echo(), "(The command wrote nothing)");
}

#[test]
fn keys_and_commands_can_be_asked_about() {
    let mut h = Harness::new(100, 16);
    h.keys("C-h k");
    assert_eq!(h.echo(), "Describe key:");
    h.keys("C-x");
    assert_eq!(h.echo(), "Describe key: C-x");
    h.keys("C-f");
    assert_eq!(
        h.echo(),
        "C-x C-f runs find-file: Visit a file in its own buffer."
    );
    h.keys("C-h k").type_text("a");
    assert_eq!(h.echo(), "a inserts itself");
    h.keys("C-h k C-c C-c");
    assert_eq!(h.echo(), "C-c C-c is not bound");
    assert_eq!(h.buffer_text(), "", "the keys were not acted on");

    h.keys("C-h f").type_text("find-file").keys("RET");
    let screen = h.screen.text();
    assert!(
        screen.contains("Visit a file in its own buffer."),
        "{screen} / {}",
        h.echo()
    );
    assert!(screen.contains("Keys: C-x C-f"), "{screen}");
}

#[test]
fn the_window_scrolls_by_lines_without_moving_point_far() {
    let mut h = Harness::new(40, 8);
    let text: String = (1..=30).map(|n| format!("line {n}\n")).collect();
    h.paste(&text);
    h.keys("M-<");
    h.editor.scroll_lines(2).unwrap();
    h.sync();
    assert_eq!(h.screen.row(0), "line 3");
    assert_eq!(h.screen.cursor(), Some((0, 0)), "point came along");
    h.keys("C-n C-n");
    h.editor.scroll_lines(-1).unwrap();
    h.sync();
    assert_eq!(h.screen.row(0), "line 2");
    assert_eq!(h.screen.cursor(), Some((3, 0)), "and stays where it can");
    h.keys("C-u 4 M-x").type_text("scroll-up-line").keys("RET");
    assert_eq!(h.screen.row(0), "line 6");
}

#[test]
fn the_mouse_scrolls_and_sets_point() {
    use amacs_proto::{InputEvent, MouseEvent};
    let mut h = Harness::new(40, 12);
    let lines: Vec<String> = (1..=40).map(|n| format!("line {n}")).collect();
    h.paste(&lines.join("\n"));
    h.keys("M-< C-x 2");
    let mouse = |h: &mut Harness, event: MouseEvent| {
        h.editor.handle_event(InputEvent::Mouse(event));
        h.sync();
    };
    // The window under the pointer scrolls, though the other is selected.
    let lower = (0..12)
        .find(|row| h.screen.row(*row).contains("*scratch*"))
        .unwrap()
        + 1;
    assert_eq!(h.screen.row(lower), "line 1");
    mouse(
        &mut h,
        MouseEvent::Scroll {
            col: 3,
            row: lower as u16 + 1,
            lines: 3,
        },
    );
    assert_eq!(h.screen.row(lower), "line 4");
    assert_eq!(h.screen.row(0), "line 1", "the selected window stays");
    assert_eq!(h.screen.cursor(), Some((0, 0)));
    mouse(
        &mut h,
        MouseEvent::Scroll {
            col: 3,
            row: lower as u16,
            lines: -2,
        },
    );
    assert_eq!(h.screen.row(lower), "line 2");
    // Backwards past the start stops at the start.
    mouse(
        &mut h,
        MouseEvent::Scroll {
            col: 3,
            row: lower as u16,
            lines: -9,
        },
    );
    assert_eq!(h.screen.row(lower), "line 1");

    // A click selects the window and puts point where it was made.
    mouse(
        &mut h,
        MouseEvent::Press {
            col: 2,
            row: lower as u16 + 2,
        },
    );
    assert_eq!(h.screen.cursor(), Some((lower + 2, 2)));
    h.type_text("X");
    assert!(h.buffer_text().contains("liXne 3"), "{}", h.buffer_text());
    // Beyond the end of a line is the end of the line; on the mode line,
    // nothing.
    mouse(&mut h, MouseEvent::Press { col: 30, row: 1 });
    assert_eq!(h.screen.cursor(), Some((1, 6)));
    mouse(
        &mut h,
        MouseEvent::Press {
            col: 5,
            row: lower as u16 - 1,
        },
    );
    assert_eq!(h.screen.cursor(), Some((1, 6)));
    // With line numbers, the click is on the text beside them.
    h.editor
        .set_option("line-numbers", Value::Bool(true))
        .unwrap();
    h.sync();
    let gutter = h.screen.row(0).find("line").unwrap();
    mouse(
        &mut h,
        MouseEvent::Press {
            col: gutter as u16 + 3,
            row: 0,
        },
    );
    assert_eq!(h.screen.cursor(), Some((0, gutter + 3)));
}
