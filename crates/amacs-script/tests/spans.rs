//! Colours laid over text without a named face: `add-span!` and
//! `clear-spans!`, as seen on the screen.

mod common;

use amacs_proto::Color;
use common::Harness;

const RED: Color = Color::rgb(255, 0, 0);
const BLUE: Color = Color::rgb(0, 0, 255);

#[test]
fn a_span_colours_its_text_and_moves_with_it() {
    let mut h = Harness::new(40, 6);
    h.eval(
        r##"(insert "hello world")
           (add-span! 0 5 'fg "#ff0000" 'bold #t)
           (add-span! 6 11 'bg "#0000ff" 'inverse #t)"##,
    );
    let plain = h.screen.face_at(0, 5);
    let red = h.screen.face_at(0, 0);
    assert_eq!(red.fg, Some(RED));
    assert!(red.bold);
    assert_eq!(red.bg, plain.bg);
    let blue = h.screen.face_at(0, 7);
    assert_eq!(blue.bg, Some(BLUE));
    assert!(blue.inverse);
    assert_eq!(blue.fg, plain.fg);
    // Text put in front moves the spans along; text put inside widens one.
    h.keys("M-<").type_text(">> ");
    assert_eq!(h.screen.face_at(0, 0).fg, plain.fg);
    assert_eq!(h.screen.face_at(0, 3).fg, Some(RED));
    h.keys("C-f C-f").type_text("XX");
    assert_eq!(h.screen.face_at(0, 5).fg, Some(RED));
    assert_eq!(h.screen.face_at(0, 9).fg, Some(RED));
    assert_eq!(h.screen.face_at(0, 10).fg, plain.fg);
}

#[test]
fn a_later_span_takes_the_place_of_an_earlier_one_where_they_meet() {
    let mut h = Harness::new(40, 6);
    h.eval(
        r##"(insert "0123456789")
           (add-span! 0 10 'fg "#ff0000")
           (add-span! 3 6 'fg "#0000ff")"##,
    );
    assert_eq!(h.screen.face_at(0, 2).fg, Some(RED));
    assert_eq!(h.screen.face_at(0, 3).fg, Some(BLUE));
    assert_eq!(h.screen.face_at(0, 5).fg, Some(BLUE));
    assert_eq!(h.screen.face_at(0, 6).fg, Some(RED));
    h.eval("(clear-spans! 0 4)");
    let plain = h.screen.face_at(1, 0);
    assert_eq!(h.screen.face_at(0, 2).fg, plain.fg);
    assert_eq!(h.screen.face_at(0, 4).fg, Some(BLUE));
    h.eval("(clear-spans!)");
    assert_eq!(h.screen.face_at(0, 8).fg, plain.fg);
}

#[test]
fn palette_names_and_faces_work_together() {
    let mut h = Harness::new(40, 6);
    h.eval(
        r##"(insert "mixed")
           (set-face! 'spans-test 'bg "#101010" 'italic #t)
           (add-face! 0 5 'spans-test)
           (add-span! 0 3 'fg 'red)"##,
    );
    let both = h.screen.face_at(0, 0);
    assert!(both.italic);
    assert_eq!(both.bg, Some(Color::rgb(0x10, 0x10, 0x10)));
    h.eval("(set-face! 'spans-red 'fg 'red)");
    assert_eq!(both.fg, h.face("spans-red").fg);
    let only_face = h.screen.face_at(0, 4);
    assert!(only_face.italic);
    assert_ne!(only_face.fg, both.fg);
    let error = h
        .session
        .eval("(add-span! 0 1 'fg 'no-such-colour)")
        .unwrap_err();
    assert!(
        error.summary.contains("unknown colour"),
        "{}",
        error.summary
    );
}

#[test]
fn a_buffer_default_face_colours_empty_cells() {
    let mut h = Harness::new(20, 4);
    h.eval(
        r##"(insert "x")
           (set-face! 'term-bg 'bg "#1e1e2e" 'fg "#cdd6f4")
           (set-buffer-default-face! 'term-bg)"##,
    );
    let bg = amacs_proto::Color::rgb(0x1e, 0x1e, 0x2e);
    // The written char, and a cell past the text, both take the default bg.
    assert_eq!(h.screen.face_at(0, 0).bg, Some(bg));
    assert_eq!(h.screen.face_at(0, 5).bg, Some(bg));
    assert_eq!(
        h.screen.face_at(0, 0).fg,
        Some(amacs_proto::Color::rgb(0xcd, 0xd6, 0xf4))
    );
    // A span still wins over the default.
    h.eval("(add-span! 0 1 'fg 'red)");
    assert_ne!(
        h.screen.face_at(0, 0).fg,
        Some(amacs_proto::Color::rgb(0xcd, 0xd6, 0xf4))
    );
    assert_eq!(h.screen.face_at(0, 0).bg, Some(bg));
    h.eval("(set-buffer-default-face! #f)");
    assert_ne!(h.screen.face_at(0, 5).bg, Some(bg));
}
