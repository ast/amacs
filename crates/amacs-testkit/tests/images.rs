//! Pictures in buffers: the rows that they take under their lines, and
//! scrolling over them.

use std::path::{Path, PathBuf};

use amacs_proto::{InputEvent, MouseEvent};
use amacs_testkit::Harness;

/// Writes a picture of one colour and a size in pixels.
fn picture(dir: &Path, name: &str, width: u32, height: u32) -> PathBuf {
    let path = dir.join(name);
    let file = std::fs::File::create(&path).unwrap();
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    let data: Vec<u8> = (0..width * height).flat_map(|_| [200, 30, 30]).collect();
    writer.write_image_data(&data).unwrap();
    path
}

/// A frame that draws pictures in cells of 8 by 16 pixels, with three
/// lines of text and point at their end.
fn three_lines() -> Harness {
    let mut h = Harness::new(40, 10);
    h.draw_pictures((8, 16));
    h.type_text("one\ntwo\nthree");
    h
}

fn rows(h: &Harness, count: usize) -> Vec<String> {
    (0..count).map(|row| h.screen.row(row)).collect()
}

/// The band of a picture that a row shows, if it shows one.
fn band(h: &Harness, row: usize) -> Option<usize> {
    h.screen.bands(row).first().map(|band| band.band)
}

#[test]
fn a_picture_takes_rows_under_its_line() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = three_lines();
    // 16 by 48 pixels: two cells across and three down.
    let file = picture(dir.path(), "red.png", 16, 48);
    h.editor.add_image(1, &file, None, None);
    h.sync();
    assert_eq!(rows(&h, 6), ["one", "", "", "", "two", "three"]);
    assert_eq!(
        (0..6).map(|row| band(&h, row)).collect::<Vec<_>>(),
        [None, Some(0), Some(1), Some(2), None, None]
    );
    let shown = h.screen.bands(1)[0];
    assert_eq!((shown.col, shown.cols, shown.bands), (0, 2, 3));
    assert_eq!(h.screen.image_size(shown.id), Some((16, 48)));
    // The cursor is on the text, below the picture.
    assert_eq!(h.screen.cursor(), Some((5, 5)));
    assert_eq!(h.buffer_text(), "one\ntwo\nthree");
}

#[test]
fn a_frame_that_draws_no_pictures_shows_the_text_alone() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::new(40, 10);
    h.type_text("one\ntwo\nthree");
    let file = picture(dir.path(), "red.png", 16, 48);
    h.editor.add_image(1, &file, None, None);
    h.sync();
    assert_eq!(rows(&h, 3), ["one", "two", "three"]);
    assert!((0..9).all(|row| h.screen.bands(row).is_empty()));
}

#[test]
fn a_large_picture_is_scaled_to_the_rows_it_may_take() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = three_lines();
    // 160 by 160 pixels in at most two rows: 32 by 32 pixels, four cells
    // across.
    let file = picture(dir.path(), "large.png", 160, 160);
    h.editor.add_image(0, &file, Some(2), None);
    h.sync();
    assert_eq!(rows(&h, 5), ["one", "", "", "two", "three"]);
    let shown = h.screen.bands(1)[0];
    assert_eq!((shown.cols, shown.bands), (4, 2));
    assert_eq!(h.screen.image_size(shown.id), Some((32, 32)));
}

#[test]
fn a_window_scrolls_through_a_picture_row_by_row() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = three_lines();
    let file = picture(dir.path(), "red.png", 16, 48);
    h.editor.add_image(0, &file, None, None);
    h.sync();
    let scroll = |h: &mut Harness, command: &str| {
        h.keys("M-x").type_text(command).keys("RET");
    };
    scroll(&mut h, "scroll-up-line");
    assert_eq!(rows(&h, 5), ["", "", "", "two", "three"]);
    assert_eq!(band(&h, 0), Some(0));
    scroll(&mut h, "scroll-up-line");
    scroll(&mut h, "scroll-up-line");
    assert_eq!(rows(&h, 3), ["", "two", "three"]);
    assert_eq!(band(&h, 0), Some(2));
    scroll(&mut h, "scroll-down-line");
    assert_eq!(rows(&h, 4), ["", "", "two", "three"]);
    assert_eq!((band(&h, 0), band(&h, 1)), (Some(1), Some(2)));
    scroll(&mut h, "scroll-down-line");
    scroll(&mut h, "scroll-down-line");
    assert_eq!(rows(&h, 5), ["one", "", "", "", "two"]);
    // There is no more above.
    scroll(&mut h, "scroll-down-line");
    assert_eq!(rows(&h, 2), ["one", ""]);
}

#[test]
fn point_is_kept_in_sight_below_a_picture_taller_than_the_window() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::new(40, 6);
    h.draw_pictures((8, 16));
    h.type_text("one\ntwo");
    // Eight rows of picture in a window of four rows of text.
    let file = picture(dir.path(), "tall.png", 16, 128);
    h.editor.add_image(0, &file, None, None);
    h.sync();
    // Point is on the last line, which the window ends with.
    assert_eq!(h.screen.row(3), "two");
    assert_eq!((band(&h, 0), band(&h, 2)), (Some(5), Some(7)));
    h.keys("M-<");
    assert_eq!(h.screen.row(0), "one");
    assert_eq!((band(&h, 1), band(&h, 3)), (Some(0), Some(2)));
}

#[test]
fn a_click_on_a_picture_goes_to_the_end_of_its_line() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = three_lines();
    let file = picture(dir.path(), "red.png", 16, 48);
    h.editor.add_image(0, &file, None, None);
    h.sync();
    h.editor.handle_event(InputEvent::Mouse(MouseEvent::Press {
        col: 1,
        row: 2,
        button: 1,
        mods: amacs_proto::Mods::empty(),
        clicks: 1,
    }));
    h.sync();
    assert_eq!(h.editor.point(), 3);
    // And one below it to the text that is there.
    h.editor.handle_event(InputEvent::Mouse(MouseEvent::Press {
        col: 1,
        row: 4,
        button: 1,
        mods: amacs_proto::Mods::empty(),
        clicks: 1,
    }));
    h.sync();
    assert_eq!(h.editor.point(), 5);
}

#[test]
fn a_picture_stays_with_its_char_and_goes_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = three_lines();
    let file = picture(dir.path(), "red.png", 16, 16);
    // Anchored at the "t" of "two".
    h.editor.add_image(4, &file, None, None);
    h.sync();
    assert_eq!(rows(&h, 4), ["one", "two", "", "three"]);
    // A line put in above moves it down with its line.
    h.keys("M-<").type_text("zero\n");
    assert_eq!(rows(&h, 5), ["zero", "one", "two", "", "three"]);
    assert_eq!(band(&h, 3), Some(0));
    // Without the char that it is anchored at, it is gone.
    h.keys("C-n C-a C-d");
    assert_eq!(rows(&h, 4), ["zero", "one", "wo", "three"]);
    assert!(h.screen.bands(3).is_empty());
}

#[test]
fn a_file_that_is_no_picture_shows_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = three_lines();
    let file = dir.path().join("notes.png");
    std::fs::write(&file, "not a picture").unwrap();
    h.editor.add_image(0, &file, None, None);
    h.sync();
    assert_eq!(rows(&h, 3), ["one", "two", "three"]);
}

#[test]
fn a_picture_that_is_no_longer_shown_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = three_lines();
    let file = picture(dir.path(), "red.png", 16, 16);
    let id = h.editor.add_image(0, &file, None, None);
    h.sync();
    let shown = h.screen.bands(1)[0].id;
    assert!(h.screen.image_size(shown).is_some());
    h.editor.buffer_mut().remove_image(id);
    h.sync();
    assert_eq!(rows(&h, 3), ["one", "two", "three"]);
    assert_eq!(h.screen.image_size(shown), None);
}

#[test]
fn scrolling_takes_point_off_a_line_whose_text_leaves_the_window() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = three_lines();
    let file = picture(dir.path(), "red.png", 16, 48);
    h.editor.add_image(0, &file, None, None);
    h.keys("M-<");
    h.keys("M-x").type_text("scroll-up-line").keys("RET");
    // The text of the first line is above the window: point is on the
    // next line, and the window stays where it was scrolled to.
    assert_eq!(rows(&h, 5), ["", "", "", "two", "three"]);
    assert_eq!(h.editor.point(), 4);
    assert_eq!(h.screen.cursor(), Some((3, 0)));
}
