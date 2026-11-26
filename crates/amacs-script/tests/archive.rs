//! Compressed files and archives in the file manager, which are written in
//! Scheme: `runtime/lib/archive.scm`. The tests run tar, zstd, gzip, zip
//! and unzip.

mod common;

use std::path::Path;

use common::Harness;

/// A directory with a few things in it, and an editor showing it.
fn setup() -> (tempfile::TempDir, Harness) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("a.rs"), "fn main() {}\n").unwrap();
    std::fs::write(root.join("b.txt"), "hello").unwrap();
    std::fs::write(root.join("with space.txt"), "spaced").unwrap();
    std::fs::create_dir(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/inner.txt"), "inner").unwrap();
    let mut h = Harness::new(80, 16);
    h.eval("(set-option! 'dired-icons #f)");
    h.eval(&format!("(dired-open {:?})", root.display().to_string()));
    (dir, h)
}

/// The names listed by the file manager, in order.
fn names(h: &mut Harness) -> String {
    h.eval("(map dired-row-name (vector->list (dired-state-rows (dired-state))))")
}

fn at_point(h: &mut Harness) -> String {
    h.eval("(file-name-nondirectory (dired-file-at-point))")
}

/// The names of the members that the listing of an archive shows.
fn members(h: &mut Harness) -> String {
    h.eval("(map archive-member-name (vector->list (archive-state-members (archive-state))))")
}

/// Answers the question of where to pack to with a name in a directory.
fn pack_into(h: &mut Harness, root: &Path, name: &str) {
    h.keys("z C-a C-k")
        .type_text(&root.join(name).display().to_string())
        .keys("RET");
}

/// Lists an archive. The listing is there once tar or unzip has answered.
fn show_archive(h: &mut Harness, path: &Path) {
    h.eval(&format!("(archive-open {:?})", path.display().to_string()));
    h.wait_until(|h| h.mode_line().contains("(Archive)"));
}

#[test]
fn a_file_is_compressed_where_it_is_and_uncompressed_again() {
    let (dir, mut h) = setup();
    let root = dir.path();
    h.keys("n n Z");
    h.wait_until(|h| h.echo() == "Compressed b.txt: b.txt.zst");
    assert_eq!(
        names(&mut h),
        "(\"sub\" \"a.rs\" \"b.txt.zst\" \"with space.txt\")"
    );
    assert_eq!(at_point(&mut h), "\"b.txt.zst\"");
    assert!(!root.join("b.txt").exists());
    h.keys("Z");
    h.wait_until(|h| h.echo() == "Uncompressed b.txt.zst: b.txt");
    assert_eq!(at_point(&mut h), "\"b.txt\"");
    assert_eq!(
        std::fs::read_to_string(root.join("b.txt")).unwrap(),
        "hello"
    );
    assert!(!root.join("b.txt.zst").exists());

    // The option says what to compress with.
    h.eval("(set-option! 'archive-compress-extension \"gz\")");
    h.keys("Z");
    h.wait_until(|h| h.echo() == "Compressed b.txt: b.txt.gz");
    assert!(root.join("b.txt.gz").exists());
    // What is in the way is left alone.
    std::fs::write(root.join("b.txt"), "other").unwrap();
    h.keys("Z");
    h.wait_until(|h| h.echo() == "b.txt is in the way");
    assert_eq!(
        std::fs::read_to_string(root.join("b.txt")).unwrap(),
        "other"
    );
}

#[test]
fn a_directory_is_packed_and_an_archive_unpacked() {
    let (dir, mut h) = setup();
    let root = dir.path();
    h.keys("Z");
    h.wait_until(|h| h.echo() == "Packed sub: sub.tar.zst");
    assert!(root.join("sub").is_dir(), "the directory stays");
    assert_eq!(at_point(&mut h), "\"sub.tar.zst\"");
    // The one thing in the archive has its name taken, and so has the
    // directory that would be made for it.
    h.keys("Z");
    h.wait_until(|h| h.echo() == "sub is in the way");
    std::fs::remove_dir_all(root.join("sub")).unwrap();
    h.keys("Z");
    h.wait_until(|h| h.echo() == "Unpacked sub.tar.zst: sub");
    assert_eq!(
        std::fs::read_to_string(root.join("sub/inner.txt")).unwrap(),
        "inner"
    );
    assert!(root.join("sub.tar.zst").exists(), "the archive stays");
    assert_eq!(at_point(&mut h), "\"sub\"");
}

#[test]
fn marked_files_are_packed_into_one_archive() {
    let (dir, mut h) = setup();
    let root = dir.path();
    h.keys("n m m z");
    let folder = root.file_name().unwrap().to_str().unwrap();
    assert!(
        h.prompt().starts_with("Pack 2 files into: ")
            && h.prompt().ends_with(&format!("/{folder}.tar.zst")),
        "{}",
        h.prompt()
    );
    h.keys("C-g");
    pack_into(&mut h, root, "both.rar");
    assert!(
        h.echo()
            .starts_with("The name must end in one of .tar.zst, "),
        "{}",
        h.echo()
    );
    pack_into(&mut h, root, "both.tar.gz");
    h.wait_until(|h| h.echo() == "Packed 2 files into both.tar.gz");
    assert!(root.join("a.rs").exists(), "the files stay");
    assert_eq!(at_point(&mut h), "\"both.tar.gz\"");
    assert!(h.screen.row(2).starts_with("   "), "the marks are gone");

    // Visiting the archive lists it.
    h.keys("RET");
    h.wait_until(|h| h.mode_line().contains("(Archive)"));
    assert_eq!(members(&mut h), "(\"a.rs\" \"b.txt\")");
    assert!(
        h.screen.row(0).ends_with("/both.tar.gz:"),
        "{}",
        h.screen.row(0)
    );
    let row = h.screen.row(2);
    assert!(
        row.starts_with("       5  20") && row.ends_with("  b.txt"),
        "{row}"
    );
    // Up is the directory, with point on the archive.
    h.keys("^");
    assert_eq!(at_point(&mut h), "\"both.tar.gz\"");
    // Two things in it: they go into a directory of their own.
    h.keys("Z");
    h.wait_until(|h| h.echo() == "Unpacked both.tar.gz: both");
    assert_eq!(
        std::fs::read_to_string(root.join("both/b.txt")).unwrap(),
        "hello"
    );
}

#[test]
fn the_members_of_an_archive_are_read_and_copied_out() {
    let (dir, mut h) = setup();
    let root = dir.path();
    h.keys("n m m m");
    pack_into(&mut h, root, "all.tar.zst");
    h.wait_until(|h| h.echo() == "Packed 3 files into all.tar.zst");
    h.keys("RET");
    h.wait_until(|h| h.mode_line().contains("(Archive)"));
    assert_eq!(members(&mut h), "(\"a.rs\" \"b.txt\" \"with space.txt\")");

    h.keys("RET");
    h.wait_until(|h| h.mode_line().contains("all.tar.zst:a.rs"));
    assert_eq!(h.text(), "fn main() {}\n");
    assert!(h.mode_line().contains("(Rust)"), "{}", h.mode_line());
    assert_eq!(h.eval("(buffer-read-only?)"), "#t");
    assert_eq!(h.eval("(buffer-file-name)"), "#f");

    show_archive(&mut h, &root.join("all.tar.zst"));
    h.keys("n n RET");
    h.wait_until(|h| h.mode_line().contains("all.tar.zst:with space.txt"));
    assert_eq!(h.text(), "spaced");

    show_archive(&mut h, &root.join("all.tar.zst"));
    h.keys("p p m n m");
    assert!(h.screen.row(1).starts_with("*  "), "{}", h.screen.row(1));
    assert!(h.screen.row(3).starts_with("*  "), "{}", h.screen.row(3));
    h.keys("C");
    assert!(h.prompt().contains("Copy 2 members to: "), "{}", h.prompt());
    h.type_text("out/").keys("RET");
    assert!(
        h.prompt().starts_with("Create the directory "),
        "{}",
        h.prompt()
    );
    h.keys("y");
    h.wait_until(|h| h.echo().starts_with("Copied 2 members to "));
    assert_eq!(
        std::fs::read_to_string(root.join("out/with space.txt")).unwrap(),
        "spaced"
    );
    assert!(root.join("out/a.rs").exists());
    assert!(!root.join("out/b.txt").exists());
    assert!(h.screen.row(1).starts_with("   "), "the marks are gone");
}

#[test]
fn a_zip_file_is_packed_listed_and_taken_from() {
    let (dir, mut h) = setup();
    let root = dir.path();
    h.keys("m m");
    pack_into(&mut h, root, "z.zip");
    h.wait_until(|h| h.echo() == "Packed 2 files into z.zip");
    h.keys("RET");
    h.wait_until(|h| h.mode_line().contains("(Archive)"));
    assert_eq!(members(&mut h), "(\"sub\" \"sub/inner.txt\" \"a.rs\")");
    let row = h.screen.row(1);
    assert!(row.ends_with("  sub/"), "{row}");
    assert_eq!(
        h.screen.face_at(1, row.chars().count() - 2).fg,
        h.face("dired-directory").fg
    );

    // A directory comes out with what is in it.
    h.keys("C").type_text("out/").keys("RET y");
    h.wait_until(|h| h.echo().starts_with("Copied sub to "));
    assert_eq!(
        std::fs::read_to_string(root.join("out/sub/inner.txt")).unwrap(),
        "inner"
    );
    assert!(!root.join("out/a.rs").exists());

    h.keys("n RET");
    h.wait_until(|h| h.mode_line().contains("z.zip:sub/inner.txt"));
    assert_eq!(h.text(), "inner");

    // unzip lists names that are not ASCII in a way of its own.
    std::fs::write(root.join("smörgås 😀.txt"), "bord").unwrap();
    h.eval(&format!(
        "(dired-open {:?} {:?})",
        root.display().to_string(),
        root.join("smörgås 😀.txt").display().to_string()
    ));
    pack_into(&mut h, root, "names.zip");
    h.wait_until(|h| h.echo() == "Packed smörgås 😀.txt into names.zip");
    show_archive(&mut h, &root.join("names.zip"));
    assert_eq!(members(&mut h), "(\"smörgås 😀.txt\")");
    h.keys("RET");
    h.wait_until(|h| h.mode_line().contains("names.zip:smörgås"));
    assert_eq!(h.text(), "bord");

    // Unpacking the whole of it: two things, so a directory of its own.
    show_archive(&mut h, &root.join("z.zip"));
    h.keys("Z");
    h.wait_until(|h| h.echo() == "Unpacked z.zip: z");
    assert_eq!(at_point(&mut h), "\"z\"");
    assert_eq!(
        std::fs::read_to_string(root.join("z/sub/inner.txt")).unwrap(),
        "inner"
    );
}
