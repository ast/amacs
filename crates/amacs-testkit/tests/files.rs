//! Visiting and saving files through the minibuffer.

use std::fs;
use std::path::Path;

use amacs_testkit::Harness;

fn harness() -> Harness {
    Harness::new(100, 8)
}

/// Opens `path` with `C-x C-f`, replacing the prefilled directory.
fn visit(h: &mut Harness, path: &Path) {
    h.keys("C-x C-f C-a C-k");
    h.type_text(path.to_str().unwrap()).keys("RET");
}

#[test]
fn visit_edit_and_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("note.txt");
    fs::write(&path, "first\nsecond\n").unwrap();

    let mut h = harness();
    visit(&mut h, &path);
    assert_eq!(h.screen.row(0), "first");
    assert_eq!(h.screen.row(6), " --  note.txt   L1 C0");

    h.keys("C-x C-s");
    assert_eq!(h.echo(), "(No changes need to be saved)");

    h.keys("M->").type_text("third");
    assert_eq!(h.screen.row(6), " **  note.txt   L3 C5");
    h.keys("C-x C-s");
    assert!(h.echo().starts_with("Wrote "), "{}", h.echo());
    assert_eq!(fs::read_to_string(&path).unwrap(), "first\nsecond\nthird");
    assert_eq!(h.screen.row(6), " --  note.txt   L3 C5");

    h.keys("C-/");
    assert_eq!(
        h.screen.row(6),
        " **  note.txt   L3 C0",
        "undoing past the save point modifies the buffer"
    );
    let leftovers: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(leftovers.len(), 1, "no temporary files are left behind");
}

#[test]
fn visiting_a_file_twice_reuses_its_buffer() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.txt");
    fs::write(&path, "content").unwrap();

    let mut h = harness();
    visit(&mut h, &path);
    h.type_text("X");
    h.keys("C-x b RET");
    visit(&mut h, &path);
    assert_eq!(h.buffer_text(), "Xcontent");
    assert_eq!(h.screen.row(6), " **  a.txt   L1 C1");
}

#[test]
fn files_with_the_same_name_get_distinct_buffer_names() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("sub")).unwrap();
    fs::write(dir.path().join("x.txt"), "one").unwrap();
    fs::write(dir.path().join("sub/x.txt"), "two").unwrap();

    let mut h = harness();
    visit(&mut h, &dir.path().join("x.txt"));
    visit(&mut h, &dir.path().join("sub/x.txt"));
    assert_eq!(h.screen.row(6), " --  x.txt<2>   L1 C0");
    assert_eq!(h.buffer_text(), "two");
}

#[test]
fn a_missing_file_becomes_a_new_file_on_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new.txt");

    let mut h = harness();
    visit(&mut h, &path);
    assert_eq!(h.echo(), "(New file)");
    assert!(!path.exists());
    h.type_text("hello").keys("C-x C-s");
    assert_eq!(fs::read_to_string(&path).unwrap(), "hello");
}

#[test]
fn directories_and_unreadable_paths_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = harness();
    visit(&mut h, dir.path());
    assert!(h.echo().ends_with("is a directory"), "{}", h.echo());
    assert_eq!(h.screen.row(6), " --  *scratch*   L1 C0");

    h.type_text("x").keys("C-x C-w C-a C-k");
    h.type_text(dir.path().join("missing/dir/file").to_str().unwrap())
        .keys("RET");
    assert!(h.echo().starts_with("Cannot write "), "{}", h.echo());
    assert_eq!(
        h.screen.row(6),
        " **  *scratch*   L1 C1",
        "a failed save leaves the buffer modified"
    );
}

#[test]
fn write_file_renames_the_buffer_and_asks_before_overwriting() {
    let dir = tempfile::tempdir().unwrap();
    let existing = dir.path().join("existing.txt");
    fs::write(&existing, "old").unwrap();

    let mut h = harness();
    h.type_text("new text").keys("C-x C-s");
    assert_eq!(
        h.echo(),
        format!("Write file: {}/", Harness::DEFAULT_DIRECTORY)
    );
    h.keys("C-a C-k")
        .type_text(existing.to_str().unwrap())
        .keys("RET");
    assert!(
        h.echo().ends_with("exists; overwrite? (y or n)"),
        "{}",
        h.echo()
    );
    h.type_text("q");
    assert!(h.echo().ends_with("(y or n)"), "any other key asks again");
    h.type_text("n");
    assert_eq!(fs::read_to_string(&existing).unwrap(), "old");

    h.keys("C-x C-w C-a C-k")
        .type_text(existing.to_str().unwrap())
        .keys("RET")
        .type_text("y");
    assert_eq!(fs::read_to_string(&existing).unwrap(), "new text");
    assert_eq!(h.screen.row(6), " --  existing.txt   L1 C8");
}

#[test]
fn saving_asks_when_the_file_changed_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shared.txt");
    fs::write(&path, "ours").unwrap();

    let mut h = harness();
    visit(&mut h, &path);
    // Another program rewrites the file, with a visibly different timestamp.
    fs::write(&path, "theirs").unwrap();
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(later)
        .unwrap();

    h.type_text("!").keys("C-x C-s");
    assert!(
        h.echo()
            .ends_with("changed on disk; overwrite it? (y or n)"),
        "{}",
        h.echo()
    );
    h.keys("C-g");
    assert_eq!(h.echo(), "Quit");
    assert_eq!(fs::read_to_string(&path).unwrap(), "theirs");

    h.keys("C-x C-s").type_text("y");
    assert_eq!(fs::read_to_string(&path).unwrap(), "!ours");
}

#[test]
fn saving_through_a_symlink_keeps_the_link_and_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.sh");
    let link = dir.path().join("link.sh");
    fs::write(&target, "echo hi\n").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o750)).unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let mut h = harness();
    visit(&mut h, &link);
    h.type_text("#!/bin/sh").keys("RET C-x C-s");
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), "#!/bin/sh\necho hi\n");
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o750
    );
}

#[test]
fn invalid_utf8_opens_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("latin1.txt");
    fs::write(&path, b"caf\xe9\n").unwrap();

    let mut h = harness();
    visit(&mut h, &path);
    assert_eq!(h.screen.row(0), "caf\u{fffd}");
    assert_eq!(h.screen.row(6), " %%  latin1.txt   L1 C0");
    h.type_text("x");
    assert_eq!(h.echo(), "Buffer is read-only: latin1.txt");
    assert_eq!(fs::read(&path).unwrap(), b"caf\xe9\n");
}

#[test]
fn tab_completes_file_names() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("report-final.txt"), "").unwrap();
    fs::write(dir.path().join("report-draft.txt"), "").unwrap();
    fs::create_dir(dir.path().join("archive")).unwrap();
    let base = dir.path().to_str().unwrap();

    let mut h = Harness::new(200, 8);
    h.keys("C-x C-f C-a C-k")
        .type_text(&format!("{base}/rep"))
        .keys("TAB");
    assert_eq!(
        h.echo(),
        format!("Find file: {base}/report- [report-draft.txt, report-final.txt]")
    );
    h.type_text("f").keys("TAB");
    assert_eq!(h.echo(), format!("Find file: {base}/report-final.txt"));
    h.keys("C-a C-k")
        .type_text(&format!("{base}/ar"))
        .keys("TAB");
    assert_eq!(h.echo(), format!("Find file: {base}/archive/"));
    h.type_text("zzz").keys("TAB");
    assert_eq!(
        h.echo(),
        format!("Find file: {base}/archive/zzz [No match]")
    );
}

#[test]
fn quitting_with_unsaved_files_asks_first() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("work.txt");
    fs::write(&path, "data").unwrap();

    let mut h = harness();
    h.type_text("scratch changes do not count");
    visit(&mut h, &path);
    h.type_text("x").keys("C-x C-c");
    assert_eq!(
        h.echo(),
        "1 modified buffer has not been saved; exit anyway? (y or n)"
    );
    assert!(!h.editor.should_exit());
    h.type_text("n");
    assert!(!h.editor.should_exit());
    h.keys("C-x C-s C-x C-c");
    assert!(h.editor.should_exit());
}

#[test]
fn killing_a_modified_file_buffer_asks_first() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("work.txt");
    fs::write(&path, "data").unwrap();

    let mut h = harness();
    visit(&mut h, &path);
    h.type_text("x").keys("C-x k RET");
    assert_eq!(h.echo(), "Buffer work.txt modified; kill anyway? (y or n)");
    h.type_text("n");
    assert_eq!(h.screen.row(6), " **  work.txt   L1 C1");
    h.keys("C-x k RET").type_text("y");
    assert_eq!(h.screen.row(6), " --  *scratch*   L1 C0");
}

/// A 100 MB file must open, edit at both ends, undo and save without delay.
/// Run with `cargo test --release -- --ignored large_file`.
#[test]
#[ignore = "writes 200 MB to the temporary directory"]
fn large_file_round_trip() {
    use std::io::Write;
    use std::time::Instant;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.txt");
    let line = "The quick brown fox jumps over the lazy dog. 0123456789 abcdefghij\n";
    let lines = 100 * 1024 * 1024 / line.len();
    {
        let mut file = std::io::BufWriter::new(fs::File::create(&path).unwrap());
        for _ in 0..lines {
            file.write_all(line.as_bytes()).unwrap();
        }
    }

    let mut h = harness();
    let start = Instant::now();
    visit(&mut h, &path);
    let opened = start.elapsed();

    let start = Instant::now();
    h.type_text("top ");
    h.keys("M->");
    h.type_text("bottom");
    h.keys("M-< C-v C-v M-> M-v C-l M->");
    let edited = start.elapsed();
    assert_eq!(h.screen.row(6), format!(" **  big.txt   L{} C6", lines + 1));

    let start = Instant::now();
    h.keys("C-/");
    assert_eq!(h.screen.row(6), format!(" **  big.txt   L{} C0", lines + 1));
    h.keys("C-? C-x C-s");
    let saved = start.elapsed();

    let written = fs::metadata(&path).unwrap().len();
    assert_eq!(written, (lines * line.len() + "top bottom".len()) as u64);
    eprintln!("open {opened:?}, edit and scroll {edited:?}, undo/redo/save {saved:?}");
    assert!(
        edited.as_millis() < 200,
        "editing a large file must not scan it: {edited:?}"
    );
}
