//! Drawing on the CPU, into a picture in memory.
//!
//! The picture is kept between paints. Painting redraws only the rows that
//! changed and reports the area that did, so that a keystroke costs a row
//! or two, not a screen.

use amacs_proto::Color;

use super::list::{Frame, Row};
use super::text::{GlyphContent, GlyphKey, GlyphSource};
use super::{Backend, DEFAULT_BG, Rect};

fn pixel(color: Color) -> u32 {
    0xff00_0000 | u32::from(color.r) << 16 | u32::from(color.g) << 8 | u32::from(color.b)
}

#[derive(Default)]
pub struct Software {
    width: u32,
    height: u32,
    /// The frame as XRGB, one `u32` per pixel.
    pixels: Vec<u32>,
    /// The cell that the cursor is drawn in.
    drawn_cursor: Option<Rect>,
}

impl Software {
    /// The picture, one XRGB `u32` per pixel, row by row.
    pub fn pixels(&self) -> &[u32] {
        &self.pixels
    }

    fn fill(&mut self, rect: Rect, color: u32) {
        let x1 = (rect.x + rect.w).min(self.width);
        let y1 = (rect.y + rect.h).min(self.height);
        for y in rect.y.min(y1)..y1 {
            let row = y as usize * self.width as usize;
            self.pixels[row + rect.x.min(x1) as usize..row + x1 as usize].fill(color);
        }
    }

    /// Draws a glyph with its origin at (`x`, `y`), clipped to `clip`.
    fn draw_glyph(
        &mut self,
        glyphs: &mut dyn GlyphSource,
        key: GlyphKey,
        x: i32,
        y: i32,
        color: u32,
        clip: Rect,
    ) {
        let Some(image) = glyphs.image(key) else {
            return;
        };
        let (width, height) = (image.width as i32, image.height as i32);
        let (left, top) = (x + image.left, y - image.top);
        let clip_x1 = (clip.x + clip.w).min(self.width) as i32;
        let clip_y1 = (clip.y + clip.h).min(self.height) as i32;
        let channels = match image.content {
            GlyphContent::Mask => 1,
            GlyphContent::Color | GlyphContent::Subpixel => 4,
        };
        let (r, g, b) = (color >> 16 & 0xff, color >> 8 & 0xff, color & 0xff);

        for row in 0..height {
            let py = top + row;
            if py < clip.y as i32 || py >= clip_y1 {
                continue;
            }
            for column in 0..width {
                let px = left + column;
                if px < clip.x as i32 || px >= clip_x1 {
                    continue;
                }
                let source = (row * width + column) as usize * channels;
                let Some(data) = image.data.get(source..source + channels) else {
                    return;
                };
                // A mask is coverage of the text colour; a colour glyph, such
                // as an emoji, brings its own colour; a glyph drawn per
                // subpixel covers each part of the pixel by its own amount.
                let [sr, sg, sb, ar, ag, ab] = match image.content {
                    GlyphContent::Mask => {
                        let alpha = u32::from(data[0]);
                        [r, g, b, alpha, alpha, alpha]
                    }
                    GlyphContent::Color => {
                        let alpha = u32::from(data[3]);
                        [
                            u32::from(data[0]),
                            u32::from(data[1]),
                            u32::from(data[2]),
                            alpha,
                            alpha,
                            alpha,
                        ]
                    }
                    GlyphContent::Subpixel => [
                        r,
                        g,
                        b,
                        u32::from(data[0]),
                        u32::from(data[1]),
                        u32::from(data[2]),
                    ],
                };
                if ar | ag | ab == 0 {
                    continue;
                }
                let index = py as usize * self.width as usize + px as usize;
                let dest = self.pixels[index];
                let blend = |source: u32, dest: u32, alpha: u32| {
                    (source * alpha + dest * (255 - alpha)) / 255
                };
                self.pixels[index] = 0xff00_0000
                    | blend(sr, dest >> 16 & 0xff, ar) << 16
                    | blend(sg, dest >> 8 & 0xff, ag) << 8
                    | blend(sb, dest & 0xff, ab);
            }
        }
    }

    fn paint_row(&mut self, row: &Row, glyphs: &mut dyn GlyphSource) {
        for quad in &row.backgrounds {
            self.fill(quad.rect, pixel(quad.color));
        }
        for glyph in &row.glyphs {
            self.draw_glyph(
                glyphs,
                glyph.key,
                glyph.x,
                glyph.y,
                pixel(glyph.color),
                row.area,
            );
        }
        for quad in &row.overlays {
            self.fill(quad.rect, pixel(quad.color));
        }
    }
}

impl Backend for Software {
    fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.pixels = vec![pixel(DEFAULT_BG); width as usize * height as usize];
    }

    /// Glyphs are blended into the picture as they come; none are kept.
    fn forget_glyphs(&mut self) {}

    fn paint(&mut self, frame: &Frame, glyphs: &mut dyn GlyphSource) -> Option<Rect> {
        let mut damage: Option<Rect> = None;
        let mut add = |rect: Rect| damage = Some(damage.map_or(rect, |old| old.union(rect)));

        // The cursor is drawn by inverting its cell. When it moves, the row
        // it was on is painted afresh to erase it. A row that is painted
        // for any other reason loses its cursor too, which is then redrawn.
        let cursor_moved = frame.cursor != self.drawn_cursor;
        let old_cursor_y = self.drawn_cursor.map(|cell| cell.y);

        if frame.full {
            let whole = Rect {
                x: 0,
                y: 0,
                w: self.width,
                h: self.height,
            };
            self.fill(whole, pixel(frame.background));
            if self.width > 0 && self.height > 0 {
                add(whole);
            }
        }

        let mut changed = frame.changed.iter().copied().peekable();
        let mut cursor_row_painted = false;
        for (index, row) in frame.rows.iter().enumerate() {
            let is_changed = changed.next_if_eq(&index).is_some();
            let holds_old_cursor = cursor_moved && old_cursor_y == Some(row.area.y);
            if !(frame.full || is_changed || holds_old_cursor) {
                continue;
            }
            self.paint_row(row, glyphs);
            add(row.area);
            cursor_row_painted |= frame.cursor.is_some_and(|cell| cell.y == row.area.y);
        }

        if let Some(cell) = frame.cursor
            && (cursor_moved || cursor_row_painted || frame.full)
        {
            let x1 = (cell.x + cell.w).min(self.width);
            let y1 = (cell.y + cell.h).min(self.height);
            for y in cell.y.min(y1)..y1 {
                for x in cell.x.min(x1)..x1 {
                    let index = y as usize * self.width as usize + x as usize;
                    self.pixels[index] ^= 0x00ff_ffff;
                }
            }
            add(cell);
        }
        self.drawn_cursor = frame.cursor;

        damage.map(|rect| Rect {
            w: rect.w.min(self.width.saturating_sub(rect.x)),
            h: rect.h.min(self.height.saturating_sub(rect.y)),
            ..rect
        })
    }
}
