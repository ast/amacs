//! What a row looks like, as rectangles and glyphs in pixels: the display
//! list that every backend draws from. How text looks is decided here and
//! nowhere else.

use amacs_proto::{Color, DisplayLine, ImageId, PaneRect};
use unicode_width::UnicodeWidthChar;

use super::Rect;
use super::scene::{Images, Scene};
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

/// A picture and where the whole of it is, in pixels. A row shows the part
/// of it that is in the row's area: its top can be above that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageQuad {
    pub id: ImageId,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// One row of a pane. The backgrounds and pictures of every row are drawn
/// first, then the glyphs of every row, then the overlays of every row.
/// Everything but the glyphs stays in the row's area. A glyph can reach
/// into the rows above and below, as an accent on a capital or a tall
/// icon does, and is cut only at [`Row::ink_area`] and where no row is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub area: Rect,
    /// What is behind the text. Together they cover the area.
    pub backgrounds: Vec<Quad>,
    /// The pictures that the row shows a band of.
    pub images: Vec<ImageQuad>,
    pub glyphs: Vec<GlyphQuad>,
    /// What is over the text: underlines, the lines of a box.
    pub overlays: Vec<Quad>,
}

impl Row {
    /// Where the row's glyphs can reach: its own columns, from a row's
    /// height above it to a row's height below it. Sideways a glyph stays
    /// in its pane.
    pub fn ink_area(&self) -> Rect {
        let y = self.area.y.saturating_sub(self.area.h);
        Rect {
            y,
            h: self.area.y + 2 * self.area.h - y,
            ..self.area
        }
    }
}

/// Where the glyphs of `rows[index]` show: the parts of rows, its own and
/// those around it, that its ink area covers, for which `keep` holds of the
/// row's index. Parts one under the other are joined, so that a glyph is
/// mostly drawn once. Nothing shows where no row is, as in the margin at
/// the edge of the window.
pub fn ink_clips(rows: &[Row], index: usize, keep: impl Fn(usize) -> bool) -> Vec<Rect> {
    let ink = rows[index].ink_area();
    let mut clips: Vec<Rect> = Vec::new();
    for (other, row) in rows.iter().enumerate() {
        let Some(part) = row.area.intersect(ink) else {
            continue;
        };
        if !keep(other) {
            continue;
        }
        match clips.last_mut() {
            Some(last) if last.x == part.x && last.w == part.w && last.y + last.h == part.y => {
                last.h += part.h;
            }
            _ => clips.push(part),
        }
    }
    clips
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
    /// The pictures that rows show.
    pub images: &'a Images,
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

    // A picture is as large as fits in its cells, and no larger than it is.
    let images = line
        .images
        .iter()
        .filter_map(|slice| {
            let image = scene.images().get(&slice.id)?;
            let room_w = u64::from(slice.cols) * u64::from(cell_w);
            let room_h = u64::from(slice.bands) * u64::from(cell_h);
            let (width, height) = (u64::from(image.width), u64::from(image.height));
            if width == 0 || height == 0 || room_w == 0 || room_h == 0 {
                return None;
            }
            let (w, h) = if width <= room_w && height <= room_h {
                (width, height)
            } else if width * room_h > height * room_w {
                (room_w, (height * room_w / width).max(1))
            } else {
                ((width * room_h / height).max(1), room_h)
            };
            Some(ImageQuad {
                id: slice.id,
                x: (area.x + u32::from(slice.col) * cell_w) as i32,
                y: area.y as i32 - (u32::from(slice.band) * cell_h) as i32,
                w: w as u32,
                h: h as u32,
            })
        })
        .collect();

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
        images,
        glyphs,
        overlays,
    }
}
