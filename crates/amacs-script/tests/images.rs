//! Pictures that the text of a buffer names, shown under their lines.

mod common;

use std::path::{Path, PathBuf};

use amacs_proto::InputEvent;
use common::Harness;

/// Writes a picture of a size in pixels.
fn picture(dir: &Path, name: &str, width: u32, height: u32) -> PathBuf {
    let path = dir.join(name);
    let file = std::fs::File::create(&path).unwrap();
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let data: Vec<u8> = (0..width * height).flat_map(|_| [30, 30, 200]).collect();
    encoder
        .write_header()
        .and_then(|mut writer| writer.write_image_data(&data))
        .unwrap();
    path
}

/// A frame that draws pictures in cells of 8 by 16 pixels.
fn harness() -> Harness {
    let mut h = Harness::new(60, 14);
    h.session.handle_event(InputEvent::Resize {
        cols: 60,
        rows: 14,
        cell: Some((8, 16)),
    });
    h.redisplay();
    h
}

fn visit(h: &mut Harness, file: &Path) {
    h.eval(&format!("(find-file {:?})", file.display().to_string()));
    h.redisplay();
}

/// The rows of the frame that show a band of a picture.
fn picture_rows(h: &Harness) -> Vec<usize> {
    (0..12)
        .filter(|row| !h.screen.bands(*row).is_empty())
        .collect()
}

#[test]
fn code_shows_and_removes_a_picture() {
    let dir = tempfile::tempdir().unwrap();
    let file = picture(dir.path(), "blue.png", 24, 32);
    let mut h = harness();
    h.type_text("first");
    h.keys("RET");
    h.type_text("second");
    assert_eq!(h.eval("(frame-draws-images?)"), "#t");
    let id = h.eval(&format!("(add-image! 0 {:?})", file.display().to_string()));
    h.redisplay();
    assert_eq!(picture_rows(&h), [1, 2]);
    assert_eq!(h.screen.row(3), "second");
    assert_eq!(
        h.eval("(map cadr (buffer-images))"),
        "(0)",
        "the picture is at the position it was given"
    );
    assert_eq!(h.eval("(map cadddr (buffer-images))"), "(2)");
    // No more rows than it is given.
    h.eval(&format!("(remove-image! {id})"));
    h.eval(&format!(
        "(add-image! 0 {:?} 1)",
        file.display().to_string()
    ));
    h.redisplay();
    assert_eq!(picture_rows(&h), [1]);
    h.eval("(clear-images!)");
    h.redisplay();
    assert_eq!(picture_rows(&h), [] as [usize; 0]);
    assert_eq!(h.screen.row(1), "second");
}

#[test]
fn markdown_shows_the_pictures_that_it_names() {
    let dir = tempfile::tempdir().unwrap();
    picture(dir.path(), "plan.png", 40, 32);
    let notes = dir.path().join("notes.md");
    std::fs::write(
        &notes,
        "# Notes\n\n![the plan](plan.png)\n\nA [link](plan.png) is no picture, \
         nor is ![one of the web](https://example.org/a.png) or ![none](missing.png).\n",
    )
    .unwrap();
    let mut h = harness();
    visit(&mut h, &notes);
    assert_eq!(picture_rows(&h), [] as [usize; 0]);
    h.keys("M-x").type_text("inline-images-mode").keys("RET");
    // Under the third line, and nowhere else.
    assert_eq!(picture_rows(&h), [3, 4]);
    assert_eq!(h.screen.bands(3)[0].cols, 5);
    // The text is as it was.
    assert!(h.text().contains("![the plan](plan.png)"));
    assert_eq!(h.eval("(buffer-modified?)"), "#f");

    // Another picture that is written in shows a moment later.
    picture(dir.path(), "more.png", 16, 16);
    h.keys("M->").type_text("![more](more.png)");
    assert_eq!(h.eval("(length (buffer-images))"), "1");
    h.wait_until(|h| h.eval("(length (buffer-images))") == "2");

    h.keys("M-x").type_text("inline-images-mode").keys("RET");
    assert_eq!(picture_rows(&h), [] as [usize; 0]);
    assert_eq!(h.eval("(buffer-images)"), "()");
}

#[test]
fn org_shows_the_pictures_that_it_names() {
    let dir = tempfile::tempdir().unwrap();
    picture(dir.path(), "plan.png", 40, 16);
    let notes = dir.path().join("notes.org");
    std::fs::write(
        &notes,
        "* Notes\n[[file:plan.png]]\nand [[plan.png][with a text]] and [[notes.org]]\n",
    )
    .unwrap();
    let mut h = harness();
    visit(&mut h, &notes);
    h.keys("M-x").type_text("inline-images-mode").keys("RET");
    assert_eq!(picture_rows(&h), [2, 4]);
}

#[test]
fn a_terminal_shows_the_text_alone() {
    let dir = tempfile::tempdir().unwrap();
    picture(dir.path(), "plan.png", 40, 32);
    let notes = dir.path().join("notes.md");
    std::fs::write(&notes, "![the plan](plan.png)\nnext\n").unwrap();
    let mut h = Harness::new(60, 14);
    visit(&mut h, &notes);
    h.keys("M-x").type_text("inline-images-mode").keys("RET");
    assert_eq!(h.eval("(frame-draws-images?)"), "#f");
    assert_eq!(h.screen.row(1), "next");
}
