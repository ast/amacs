//! The display list, seen through a backend that draws nothing: what a
//! row is made of, and what a backend is told has changed.
//!
//! The backend here is the second one there is, beside the one that draws
//! on the CPU. That it can be written with nothing but what the renderer
//! makes public is what a backend for the GPU relies on.

use std::path::PathBuf;

use amacs_gui::render::{Backend, Fonts, Frame, GlyphSource, Rect, Renderer, Row};
use amacs_proto::{Color, CursorShape, DisplayLine, DrawOp, FaceAttrs, FaceId, PaneId, PaneRect};

const RED: Color = Color::rgb(0xff, 0, 0);
const BLUE: Color = Color::rgb(0, 0, 0xff);
const PAPER: Color = Color::rgb(0xee, 0xee, 0xee);
const INK: Color = Color::rgb(0x11, 0x11, 0x11);

/// What a frame came to, kept for looking at.
#[derive(Debug, Clone, PartialEq)]
struct Painted {
    size: (u32, u32),
    background: Color,
    full: bool,
    rows: Vec<Row>,
    changed: Vec<usize>,
    cursor: Option<Rect>,
    /// How many of the glyphs have a picture.
    pictures: usize,
}

/// A backend that keeps the frames it is given.
#[derive(Default)]
struct Recording {
    size: (u32, u32),
    forgotten: usize,
    frames: Vec<Painted>,
}

impl Backend for Recording {
    fn resize(&mut self, width: u32, height: u32) {
        self.size = (width, height);
    }

    fn forget_glyphs(&mut self) {
        self.forgotten += 1;
    }

    fn paint(&mut self, frame: &Frame, glyphs: &mut dyn GlyphSource) -> Option<Rect> {
        let pictures = frame
            .rows
            .iter()
            .flat_map(|row| &row.glyphs)
            .filter(|glyph| glyphs.image(glyph.key).is_some())
            .count();
        self.frames.push(Painted {
            size: frame.size,
            background: frame.background,
            full: frame.full,
            rows: frame.rows.to_vec(),
            changed: frame.changed.to_vec(),
            cursor: frame.cursor,
            pictures,
        });
        None
    }
}

fn renderer() -> Renderer<Recording> {
    let fonts = match std::env::var_os("AMACS_TEST_FONTS") {
        Some(dir) => Fonts::with_font_dir(&PathBuf::from(dir), "JetBrains Mono", 16.0),
        None => Fonts::new("monospace", 16.0),
    };
    Renderer::with_backend(fonts, Recording::default())
}

/// Face 0 is plain, 1 is underlined in red on blue, 2 has a box and is
/// drawn inverted.
fn faces() -> DrawOp {
    let plain = FaceAttrs {
        fg: Some(INK),
        bg: Some(PAPER),
        ..FaceAttrs::default()
    };
    DrawOp::DefineFaces(vec![
        plain,
        FaceAttrs {
            bg: Some(BLUE),
            underline: true,
            underline_color: Some(RED),
            ..plain
        },
        FaceAttrs {
            inverse: true,
            box_color: Some(RED),
            ..plain
        },
    ])
}

/// A pane of text over a one-row pane, as a window over its mode line.
fn layout() -> DrawOp {
    let pane = |id, y, h| PaneRect {
        id: PaneId(id),
        x: 0,
        y,
        w: 10,
        h,
    };
    DrawOp::Layout {
        cols: 10,
        rows: 4,
        panes: vec![pane(1, 0, 3), pane(2, 3, 1)],
    }
}

fn lines(pane: u32, first_row: u16, lines: Vec<DisplayLine>) -> DrawOp {
    DrawOp::Lines {
        pane: PaneId(pane),
        first_row,
        lines,
    }
}

fn last(renderer: &Renderer<Recording>) -> &Painted {
    renderer
        .backend()
        .frames
        .last()
        .expect("a frame was painted")
}

#[test]
fn a_row_is_backgrounds_then_glyphs_then_what_lies_over_them() {
    let mut r = renderer();
    let (cell_w, cell_h) = r.cell_size();
    r.resize(10 * cell_w, 4 * cell_h);
    let mut text = DisplayLine::default();
    text.push("ab", FaceId(0));
    text.push("cde", FaceId(1));
    r.apply(&[
        faces(),
        layout(),
        lines(1, 1, vec![text]),
        lines(2, 0, vec![DisplayLine::plain("mode", FaceId(2))]),
        DrawOp::Flush,
    ]);
    r.paint();

    let frame = last(&r);
    assert!(frame.full);
    assert_eq!(frame.size, (10 * cell_w, 4 * cell_h));
    assert_eq!(frame.background, PAPER);
    assert_eq!(frame.rows.len(), 4, "every row of every pane");
    assert_eq!(frame.changed, [0, 1, 2, 3]);

    // The row with two runs: a background for each and one for the rest.
    let row = &frame.rows[1];
    let area = Rect {
        x: 0,
        y: cell_h,
        w: 10 * cell_w,
        h: cell_h,
    };
    assert_eq!(row.area, area);
    let spans: Vec<(u32, u32, Color)> = row
        .backgrounds
        .iter()
        .map(|quad| (quad.rect.x / cell_w, quad.rect.w / cell_w, quad.color))
        .collect();
    assert_eq!(spans, [(0, 2, PAPER), (2, 3, BLUE), (5, 5, PAPER)]);
    assert!(
        row.backgrounds
            .iter()
            .all(|quad| (quad.rect.y, quad.rect.h) == (area.y, area.h))
    );
    // A glyph for each letter, in its cell, on a baseline within the row.
    assert_eq!(row.glyphs.len(), 5);
    assert_eq!(
        frame.pictures,
        5 + 4,
        "the letters of both rows have pictures"
    );
    for (index, glyph) in row.glyphs.iter().enumerate() {
        let cell = (index as u32 * cell_w) as i32;
        assert!(
            (cell - 2..cell + cell_w as i32).contains(&glyph.x),
            "glyph {index} at {}",
            glyph.x
        );
        assert!((area.y as i32..(area.y + area.h) as i32).contains(&glyph.y));
        assert_eq!(glyph.color, INK);
    }
    // The underline lies over the text of its run, near the bottom.
    assert_eq!(row.overlays.len(), 1);
    let underline = row.overlays[0];
    assert_eq!(underline.color, RED);
    assert_eq!(
        (underline.rect.x, underline.rect.w),
        (2 * cell_w, 3 * cell_w)
    );
    assert!(underline.rect.h >= 1 && underline.rect.h < cell_h / 4);
    assert!(underline.rect.y > area.y + cell_h / 2);
    assert!(underline.rect.y + underline.rect.h <= area.y + area.h);

    // The inverted row with a box: colours swapped, a line at the top and
    // one at the bottom of the text and of the rest of the row.
    let mode = &frame.rows[3];
    assert!(mode.backgrounds.iter().all(|quad| quad.color == INK));
    assert!(mode.glyphs.iter().all(|glyph| glyph.color == PAPER));
    assert_eq!(mode.overlays.len(), 4);
    assert!(mode.overlays.iter().all(|quad| quad.color == RED));
    let tops = mode
        .overlays
        .iter()
        .filter(|quad| quad.rect.y == mode.area.y)
        .count();
    let bottoms = mode
        .overlays
        .iter()
        .filter(|quad| quad.rect.y + quad.rect.h == mode.area.y + mode.area.h)
        .count();
    assert_eq!((tops, bottoms), (2, 2));

    // An empty row is its background and nothing else.
    let empty = &frame.rows[0];
    assert_eq!(empty.backgrounds.len(), 1);
    assert!(empty.glyphs.is_empty() && empty.overlays.is_empty());
}

#[test]
fn a_backend_is_told_what_changed_and_where_the_cursor_is() {
    let mut r = renderer();
    let (cell_w, cell_h) = r.cell_size();
    r.resize(10 * cell_w, 4 * cell_h);
    r.apply(&[
        faces(),
        layout(),
        lines(1, 0, vec![DisplayLine::plain("one", FaceId(0))]),
        DrawOp::Flush,
    ]);
    r.paint();
    assert_eq!(last(&r).cursor, None);
    let first = last(&r).rows.clone();

    // One line changes and the cursor is placed: the frame is no longer
    // full, names that row alone, and still holds every row.
    r.apply(&[
        lines(1, 2, vec![DisplayLine::plain("three", FaceId(0))]),
        DrawOp::Cursor {
            pane: PaneId(2),
            row: 0,
            col: 4,
            shape: CursorShape::Block,
        },
        DrawOp::Flush,
    ]);
    r.paint();
    let frame = last(&r);
    assert!(!frame.full);
    assert_eq!(frame.changed, [2]);
    assert_eq!(frame.rows.len(), 4);
    assert_eq!(
        frame.rows[0], first[0],
        "a row that did not change is the same"
    );
    assert_eq!(frame.rows[2].glyphs.len(), 5);
    assert_eq!(
        frame.cursor,
        Some(Rect {
            x: 4 * cell_w,
            y: 3 * cell_h,
            w: cell_w,
            h: cell_h,
        }),
        "the cursor's cell, by the place of its pane"
    );

    // Nothing new: nothing changed.
    r.apply(&[DrawOp::Flush]);
    r.paint();
    assert!(!last(&r).full);
    assert!(last(&r).changed.is_empty());

    // More faces at the end of the table change nothing that is drawn;
    // other faces change everything.
    let DrawOp::DefineFaces(mut table) = faces() else {
        unreachable!()
    };
    table.push(FaceAttrs::default());
    r.apply(&[DrawOp::DefineFaces(table.clone()), DrawOp::Flush]);
    r.paint();
    assert!(!last(&r).full);
    table[0].fg = Some(RED);
    r.apply(&[DrawOp::DefineFaces(table), DrawOp::Flush]);
    r.paint();
    assert!(last(&r).full);
    assert_eq!(last(&r).rows[0].glyphs[0].color, RED);

    // A new size or font size makes the frame full, and the backend hears
    // of both.
    assert_eq!(r.backend().forgotten, 0);
    r.set_font_size(24.0);
    assert_eq!(r.backend().forgotten, 1);
    let (cell_w, cell_h) = r.cell_size();
    r.resize(10 * cell_w, 4 * cell_h);
    assert_eq!(r.backend().size, (10 * cell_w, 4 * cell_h));
    r.paint();
    let frame = last(&r);
    assert!(frame.full);
    assert_eq!(frame.changed, [0, 1, 2, 3]);
    assert_eq!(frame.rows[3].area.y, 3 * cell_h);
    assert_ne!(
        frame.rows[0].glyphs[0].key, first[0].glyphs[0].key,
        "a glyph at another size is another picture"
    );
}
