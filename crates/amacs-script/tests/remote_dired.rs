//! The file manager on directories of other hosts, reached here through
//! the `sftp-server` of this machine: `/sftp-test::/path`. See `remote.rs`
//! for files, and `dired.rs` for the file manager on this machine.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use common::Harness;

/// The name of a local file as that of a file on the host of these tests.
fn there(path: &Path) -> String {
    format!("/sftp-test::{}", path.display())
}

/// A directory with a few things in it, by its real name.
fn setup() -> (tempfile::TempDir, PathBuf, Harness) {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    fs::write(root.join("b.txt"), "hello").unwrap();
    fs::write(root.join("a.rs"), "fn main() {}\n").unwrap();
    fs::create_dir(root.join("sub")).unwrap();
    fs::write(root.join("sub/inner.txt"), "inside\n").unwrap();
    std::os::unix::fs::symlink("sub", root.join("link")).unwrap();
    let mut h = Harness::new(120, 16);
    h.eval("(set-option! 'dired-icons #f)");
    (dir, root, h)
}

/// The names listed in the current buffer, in order; `#f` if it is not
/// one of the file manager.
fn names(h: &mut Harness) -> String {
    h.eval(
        "(guard (e (#t #f)) (map dired-row-name (vector->list (dired-state-rows (dired-state)))))",
    )
}

fn wait_for_names(h: &mut Harness, wanted: &str) {
    h.wait_until(|h| names(h) == wanted);
}

fn directory(h: &mut Harness) -> String {
    h.eval("(dired-directory)").trim_matches('"').to_owned()
}

/// Shows a directory by typing its name at the prompt of `C-x d`, over
/// what is there. The name is taken as it stands: `RET` would complete
/// it to what the host has listed by then, if it has.
fn open(h: &mut Harness, name: &str) {
    h.keys("C-x d").type_text(&format!("/{name}")).keys("M-RET");
}

#[test]
fn a_directory_on_another_host_is_listed_and_walked() {
    let (_dir, root, mut h) = setup();
    let top = there(&root);
    open(&mut h, &top);
    wait_for_names(&mut h, "(\"link\" \"sub\" \"a.rs\" \"b.txt\")");
    assert_eq!(directory(&mut h), top);
    assert_eq!(h.screen.row(0), format!("{top}:"));
    let row = h.screen.row(1);
    assert!(row.ends_with("▸ link -> sub"), "{row}");
    assert_eq!(h.eval("(working-directory)"), format!("{top:?}"));
    assert_eq!(h.echo(), "");

    // Into a directory and up again, with point on where we came from.
    h.keys("n RET");
    wait_for_names(&mut h, "(\"inner.txt\")");
    assert_eq!(directory(&mut h), format!("{top}/sub"));
    h.keys("RET");
    h.wait_until(|h| h.text() == "inside\n");
    assert_eq!(
        h.eval("(buffer-file-name)"),
        format!("{:?}", format!("{top}/sub/inner.txt"))
    );
    // From the file to its directory, and from there up.
    h.keys("C-x C-j");
    wait_for_names(&mut h, "(\"inner.txt\")");
    h.keys("^");
    wait_for_names(&mut h, "(\"link\" \"sub\" \"a.rs\" \"b.txt\")");
    assert_eq!(
        h.eval("(file-name-nondirectory (dired-file-at-point))"),
        "\"sub\""
    );

    // What is under a directory shows under it; marks need no host.
    h.keys("TAB");
    wait_for_names(
        &mut h,
        "(\"link\" \"sub\" \"inner.txt\" \"a.rs\" \"b.txt\")",
    );
    h.keys("TAB");
    wait_for_names(&mut h, "(\"link\" \"sub\" \"a.rs\" \"b.txt\")");
    h.keys("n m");
    assert!(h.screen.row(3).starts_with("*  "), "{}", h.screen.row(3));
    assert_eq!(
        h.eval("(map file-name-nondirectory (dired-marked-files))"),
        "(\"a.rs\")"
    );

    // Programs here cannot open what is there.
    h.keys("E");
    assert_eq!(
        h.echo(),
        "a.rs is on another host: copy it here to open it with xdg-open"
    );

    // What another program made is there after g.
    fs::write(root.join("new.txt"), "").unwrap();
    h.keys("g");
    wait_for_names(&mut h, "(\"link\" \"sub\" \"a.rs\" \"b.txt\" \"new.txt\")");
}

#[test]
fn the_top_and_the_home_directory_of_a_host_are_told_apart() {
    let (_dir, root, mut h) = setup();
    // Without a path, a host shows the directory that it starts in, by
    // its whole name. For these tests that is where they run.
    open(&mut h, "/sftp-test::");
    h.wait_until(|h| names(h).contains("\"Cargo.toml\""));
    let home = fs::canonicalize(".").unwrap();
    assert_eq!(directory(&mut h), there(&home));

    // The top directory keeps its slash, and has nothing above it.
    open(&mut h, "/sftp-test::/");
    h.wait_until(|h| h.screen.row(0) == "/sftp-test::/:");
    assert_eq!(directory(&mut h), "/sftp-test::/");
    h.keys("^");
    assert_eq!(h.echo(), "This is the top directory");
    for (asked, answer) in [
        ("(expand-file-name \"/sftp-test::/\")", "\"/sftp-test::/\""),
        (
            "(expand-file-name \"/sftp-test::/a/../b/\")",
            "\"/sftp-test::/b\"",
        ),
        ("(expand-file-name \"/sftp-test::\")", "\"/sftp-test::\""),
        (
            "(file-name-directory \"/sftp-test::/etc\")",
            "\"/sftp-test::/\"",
        ),
        ("(file-name-directory \"/sftp-test::/\")", "#f"),
        (
            "(file-name-directory \"/sftp-test::notes\")",
            "\"/sftp-test::\"",
        ),
        ("(file-name-nondirectory \"/sftp-test::/\")", "\"\""),
        (
            "(file-name-nondirectory \"/sftp-test::/etc/hosts\")",
            "\"hosts\"",
        ),
        (
            "(file-remote-prefix \"/ssh:me@evo#22:/etc/hosts\")",
            "\"/ssh:me@evo#22:\"",
        ),
        ("(file-remote-prefix \"/etc/hosts\")", "#f"),
        (
            "(file-local-name \"/ssh:evo:/etc/hosts\")",
            "\"/etc/hosts\"",
        ),
        ("(file-local-name \"/etc/hosts\")", "\"/etc/hosts\""),
    ] {
        assert_eq!(h.eval(asked), answer, "{asked}");
    }

    // The part of the first line that names the host leads to the top.
    open(&mut h, &there(&root));
    wait_for_names(&mut h, "(\"link\" \"sub\" \"a.rs\" \"b.txt\")");
    // Point is then on where one came from.
    h.press("<mouse-1>", 3, 0);
    let below = root.components().nth(1).unwrap().as_os_str();
    let below = format!("{:?}", format!("/sftp-test::/{}", below.to_str().unwrap()));
    h.wait_until(|h| {
        h.eval("(guard (e (#t #f)) (dired-file-at-point))") == below
            && directory(h) == "/sftp-test::/"
    });

    // What is no directory, or not there, says so.
    h.eval(&format!(
        "(define said #f)
         (run-later (lambda ()
           (set! said (guard (e (#t (error-object-message e)))
                        (dired-open {:?})))))",
        there(&root.join("none"))
    ));
    h.wait_until(|h| h.eval("said") != "#f");
    assert!(
        h.eval("said")
            .ends_with("none: No such file or directory\""),
        "{}",
        h.eval("said")
    );
}

#[test]
fn files_are_copied_between_this_machine_and_another_host() {
    let (_dir, root, mut h) = setup();
    let far = tempfile::tempdir().unwrap();
    let far_root = fs::canonicalize(far.path()).unwrap();
    fs::set_permissions(root.join("a.rs"), fs::Permissions::from_mode(0o750)).unwrap();
    fs::write(far_root.join("z.txt"), "").unwrap();
    let then = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
    fs::File::open(root.join("a.rs"))
        .unwrap()
        .set_modified(then)
        .unwrap();

    // Two panes: this machine on the left, the host on the right.
    h.eval(&format!("(dired-open {:?})", root.display().to_string()));
    h.keys("C-x 3 C-x o");
    open(&mut h, &there(&far_root));
    wait_for_names(&mut h, "(\"z.txt\")");
    h.keys("C-x o");
    assert_eq!(names(&mut h), "(\"link\" \"sub\" \"a.rs\" \"b.txt\")");

    // Up: a directory with what is in it, a link and a file, to the
    // other pane.
    h.keys("m m m C");
    assert!(
        h.prompt()
            .ends_with(&format!("Copy 3 files to: {}/", there(&far_root))),
        "{}",
        h.prompt()
    );
    h.keys("RET");
    h.wait_until(|h| h.echo() == "Copied 3 files");
    assert_eq!(
        fs::read_to_string(far_root.join("sub/inner.txt")).unwrap(),
        "inside\n"
    );
    assert_eq!(
        fs::read_link(far_root.join("link")).unwrap(),
        Path::new("sub")
    );
    let copied = fs::metadata(far_root.join("a.rs")).unwrap();
    assert_eq!(copied.permissions().mode() & 0o7777, 0o750);
    assert_eq!(copied.modified().unwrap(), then);
    assert!(root.join("a.rs").exists(), "a copy leaves the original");
    // Both panes show what is there now.
    h.keys("C-x o");
    assert_eq!(names(&mut h), "(\"link\" \"sub\" \"a.rs\" \"z.txt\")");

    // Down, under another name; what is in the way is asked about.
    h.keys("M-< n n n C");
    h.keys("C-a C-k")
        .type_text(&format!("{}/fetched.rs", root.display()))
        .keys("RET");
    h.wait_until(|h| h.echo() == "Copied 1 file");
    assert_eq!(
        fs::read_to_string(root.join("fetched.rs")).unwrap(),
        "fn main() {}\n"
    );
    fs::write(far_root.join("a.rs"), "changed there\n").unwrap();
    h.keys("C");
    h.keys("C-a C-k")
        .type_text(&format!("{}/", root.display()))
        .keys("RET");
    h.wait_until(|h| h.echo().ends_with("a.rs exists; replace it? (y or n)"));
    h.keys("y");
    h.wait_until(|h| h.echo() == "Copied 1 file");
    assert_eq!(
        fs::read_to_string(root.join("a.rs")).unwrap(),
        "changed there\n"
    );

    // From one place on the host to another there.
    h.keys("M-< n n C C-a C-k")
        .type_text(&format!("{}/twice", there(&far_root)))
        .keys("RET");
    h.wait_until(|_| far_root.join("twice/inner.txt").exists());
    assert_eq!(
        fs::read_to_string(far_root.join("twice/inner.txt")).unwrap(),
        "inside\n"
    );
    wait_for_names(&mut h, "(\"link\" \"sub\" \"twice\" \"a.rs\" \"z.txt\")");
}

#[test]
fn files_on_another_host_are_moved_deleted_and_made() {
    let (_dir, root, mut h) = setup();
    let here = tempfile::tempdir().unwrap();
    let here_root = fs::canonicalize(here.path()).unwrap();
    let top = there(&root);
    open(&mut h, &top);
    wait_for_names(&mut h, "(\"link\" \"sub\" \"a.rs\" \"b.txt\")");

    // A new name on the same host is a rename there.
    h.keys("n n n n R");
    assert!(
        h.prompt().ends_with(&format!("Move b.txt to: {top}/")),
        "{}",
        h.prompt()
    );
    h.type_text("c.txt").keys("RET");
    h.wait_until(|h| h.echo() == "Moved 1 file");
    assert!(root.join("c.txt").exists() && !root.join("b.txt").exists());
    wait_for_names(&mut h, "(\"link\" \"sub\" \"a.rs\" \"c.txt\")");

    // Into a directory there, which is made if need be.
    h.keys("R").type_text("new/").keys("RET");
    h.wait_until(|h| h.echo().starts_with("Create the directory"));
    h.keys("y");
    h.wait_until(|h| h.echo() == "Moved 1 file");
    assert_eq!(fs::read_to_string(root.join("new/c.txt")).unwrap(), "hello");

    // To this machine: the directory arrives whole and is gone there.
    h.keys("M-< n n n R");
    h.keys("C-a C-k")
        .type_text(&format!("{}/", here_root.display()))
        .keys("RET");
    h.wait_until(|h| h.echo() == "Moved 1 file");
    assert_eq!(
        fs::read_to_string(here_root.join("sub/inner.txt")).unwrap(),
        "inside\n"
    );
    assert!(!root.join("sub").exists());
    // The link leads nowhere now, and is listed as a file.
    wait_for_names(&mut h, "(\"new\" \"a.rs\" \"link\")");

    h.keys("+").type_text("made/deep").keys("RET");
    h.wait_until(|h| {
        h.eval("(guard (e (#t #f)) (file-name-nondirectory (dired-file-at-point)))") == "\"made\""
    });
    assert!(root.join("made/deep").is_dir());

    // Deleting: a link and not what it led to, a directory with all
    // that is in it.
    fs::create_dir(root.join("sub")).unwrap();
    fs::write(root.join("sub/kept"), "").unwrap();
    h.keys("g");
    wait_for_names(&mut h, "(\"link\" \"made\" \"new\" \"sub\" \"a.rs\")");
    h.keys("M-< n m m m D");
    assert_eq!(h.echo(), "Delete 3 files? (y or n)");
    h.keys("y");
    h.wait_until(|h| h.echo() == "Deleted 3 files");
    wait_for_names(&mut h, "(\"sub\" \"a.rs\")");
    assert!(root.join("sub/kept").exists());
    assert!(!root.join("new").exists() && !root.join("made").exists());

    // The procedures that Scheme has for this, anywhere.
    h.eval(&format!(
        "(define done #f)
         (run-later (lambda ()
           (copy-file {a:?} {b:?})
           (move-file {b:?} {c:?})
           (rename-file {c:?} {d:?})
           (set! done (guard (e (#t (error-object-message e)))
                        (rename-file {d:?} {a:?})))
           (delete-file {d:?})))",
        a = format!("{top}/a.rs"),
        b = format!("{}/b.rs", here_root.display()),
        c = format!("{top}/c.rs"),
        d = format!("{top}/sub/d.rs"),
    ));
    h.wait_until(|h| h.eval("done") != "#f" && !root.join("sub/d.rs").exists());
    assert!(
        h.eval("done").ends_with("a.rs exists already\""),
        "{}",
        h.eval("done")
    );
    assert!(root.join("a.rs").exists() && !here_root.join("b.rs").exists());
    assert!(!root.join("c.rs").exists());
}
