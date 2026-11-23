//! What a row looks like, as rectangles and glyphs in pixels: the display
//! list that every backend draws from. How text looks is decided here and
//! nowhere else.

use amacs_proto::{Color, DisplayLine, PaneRect};
use unicode_width::UnicodeWidthChar;

use super::Rect;
use super::scene::Scene;
use super::text::{Fonts, GlyphKey};

/// A rectangle filled with one colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quad {
    pub rect: Rect,
    pub color: Color,
}

/// A glyph with its origin, on the baseline, at a pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlyphQuad {
    pub key: GlyphKey,
    pub x: i32,
    pub y: i32,
    /// The colour of the text, for a glyph that has none of its own.
    pub color: Color,
}

/// One row of a pane. It is drawn in the order of its fields, and nothing
/// of it shows outside its area.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub area: Rect,
    /// What is behind the text. Together they cover the area.
    pub backgrounds: Vec<Quad>,
    pub glyphs: Vec<GlyphQuad>,
    /// What is over the text: underlines, the lines of a box.
    pub overlays: Vec<Quad>,
}

/// What is to be on screen.
pub struct Frame<'a> {
    /// The size of what is drawn on, in pixels.
    pub size: (u32, u32),
    /// The colour of what no row covers.
    pub background: Color,
    /// Everything is to be drawn afresh, as after a resize: what was drawn
    /// before says nothing about this frame.
    pub full: bool,
    /// Every row of every pane.
    pub rows: &'a [Row],
    /// Which of them changed since the last paint, in rising order. In a
    /// full frame, all.
    pub changed: &'a [usize],
    /// The cell that the cursor is in, which is shown with its colours
    /// inverted.
    pub cursor: Option<Rect>,
}

/// Lays out one row of a pane.
pub fn build_row(
    scene: &Scene,
    fonts: &mut Fonts,
    rect: PaneRect,
    row: u16,
    line: &DisplayLine,
) -> Row {
    let (cell_w, cell_h) = fonts.cell_size();
    let area = Rect {
        x: u32::from(rect.x) * cell_w,
        y: (u32::from(rect.y) + u32::from(row)) * cell_h,
        w: u32::from(rect.w) * cell_w,
        h: cell_h,
    };

    // Backgrounds come first, so that a glyph reaching into the next cell,
    // as ligatures do, is not painted over.
    let mut backgrounds = Vec::new();
    let mut underlines = Vec::new();
    // Thin lines along the top and bottom of boxed text.
    let mut boxes: Vec<(Rect, Color)> = Vec::new();
    let mut col = 0;
    for (text, face) in line.segments() {
        let index = face.0 as usize;
        let cells: u32 = text.chars().map(|c| c.width().unwrap_or(1) as u32).sum();
        let cells = cells.min(u32::from(rect.w).saturating_sub(col));
        let span = Rect {
            x: area.x + col * cell_w,
            w: cells * cell_w,
            ..area
        };
        let (fg, bg) = scene.colors(index);
        backgrounds.push(Quad {
            rect: span,
            color: bg,
        });
        let face = scene.face(index);
        if face.underline {
            let color = face.underline_color.unwrap_or(fg);
            underlines.push((span, color, face.undercurl));
        }
        if let Some(color) = face.box_color {
            boxes.push((span, color));
        }
        col += cells;
    }
    let rest = Rect {
        x: area.x + col * cell_w,
        w: area.w - col * cell_w,
        ..area
    };
    let fill = line.fill.0 as usize;
    backgrounds.push(Quad {
        rect: rest,
        color: scene.colors(fill).1,
    });
    if let Some(color) = scene.face(fill).box_color {
        boxes.push((rest, color));
    }

    let baseline = area.y as i32 + fonts.baseline();
    let glyphs = fonts
        .shape(line, scene.faces())
        .iter()
        .map(|glyph| GlyphQuad {
            key: glyph.key,
            x: (area.x + glyph.col * cell_w) as i32 + glyph.dx,
            y: baseline + glyph.dy,
            color: scene.colors(glyph.face).0,
        })
        .collect();

    let mut overlays = Vec::new();
    let border = (cell_h / 20).max(1).min(area.h);
    for (span, color) in boxes {
        overlays.push(Quad {
            rect: Rect { h: border, ..span },
            color,
        });
        overlays.push(Quad {
            rect: Rect {
                y: span.y + span.h - border,
                h: border,
                ..span
            },
            color,
        });
    }
    for (span, color, wavy) in underlines {
        let thickness = (cell_h / 14).max(1);
        let y = (baseline as u32 + thickness).min(area.y + area.h - thickness);
        if !wavy {
            overlays.push(Quad {
                rect: Rect {
                    y,
                    h: thickness,
                    ..span
                },
                color,
            });
            continue;
        }
        // A wave is a row of squares that step down and up again. Where a
        // square is depends on where it is on the screen, not in its run,
        // so that the waves of neighbouring runs join.
        let lowest = area.y + area.h - thickness;
        let mut x = span.x;
        while x < span.x + span.w {
            let step = x / thickness;
            let w = ((step + 1) * thickness).min(span.x + span.w) - x;
            let drop = [0, 1, 2, 1][(step % 4) as usize] * thickness;
            overlays.push(Quad {
                rect: Rect {
                    x,
                    y: (y + drop).min(lowest),
                    w,
                    h: thickness,
                },
                color,
            });
            x += w;
        }
    }
    Row {
        area,
        backgrounds,
        glyphs,
        overlays,
    }
}
