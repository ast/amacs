//! Software rendering of draw operations into a pixel buffer.
//!
//! The renderer keeps what every pane shows and a picture of the whole
//! frame. Painting redraws only the rows that changed and reports the area
//! that did, so that a keystroke costs a row or two, not a screen.
//!
//! Text is drawn on a cell grid. Each run of a line is shaped as a whole, so
//! that programming ligatures form, and each glyph is then placed in the cell
//! of the character it came from.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;
use std::rc::Rc;

use amacs_proto::{Color, DisplayLine, DrawOp, FaceAttrs, PaneId, PaneRect};
use cosmic_text::{
    Attrs, Buffer, CacheKey, Family, FontSystem, Metrics, Shaping, Style, SwashCache, SwashContent,
    Weight, Wrap,
};
use unicode_width::UnicodeWidthChar;

/// Colours for faces that leave theirs unset, and for the window before the
/// server has sent its faces. The server's themes set both for every face.
pub const DEFAULT_FG: Color = Color::rgb(0x00, 0x00, 0x00);
pub const DEFAULT_BG: Color = Color::rgb(0xfb, 0xf7, 0xf0);

/// Shaped lines kept for reuse. Scrolling shows mostly lines seen before.
const SHAPE_CACHE_LIMIT: usize = 4096;

/// Text attributes selecting a font family by name. `monospace` means
/// whatever the system's default monospace font is.
fn family_attrs(family: &str) -> Attrs<'_> {
    Attrs::new().family(match family {
        "monospace" => Family::Monospace,
        name => Family::Name(name),
    })
}

fn pixel(color: Color) -> u32 {
    0xff00_0000 | u32::from(color.r) << 16 | u32::from(color.g) << 8 | u32::from(color.b)
}

/// A rectangle in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    fn union(self, other: Rect) -> Rect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Rect {
            x,
            y,
            w: (self.x + self.w).max(other.x + other.w) - x,
            h: (self.y + self.h).max(other.y + other.h) - y,
        }
    }
}

/// One glyph of a shaped line, placed relative to its cell.
struct Glyph {
    /// Column of the character the glyph came from.
    col: u32,
    cache_key: CacheKey,
    /// Offset of the glyph origin from the left of its cell and from the
    /// baseline, in pixels.
    dx: i32,
    dy: i32,
    face: usize,
}

struct Pane {
    rect: PaneRect,
    lines: Vec<DisplayLine>,
    dirty: Vec<bool>,
}

pub struct Renderer {
    fonts: FontSystem,
    glyph_cache: SwashCache,
    /// Reused for shaping one line at a time.
    scratch: Buffer,
    family: String,
    font_px: f32,
    cell_w: u32,
    cell_h: u32,
    /// Distance from the top of a cell to the baseline.
    baseline: i32,

    faces: Vec<FaceAttrs>,
    panes: HashMap<PaneId, Pane>,
    cursor: Option<(PaneId, u16, u16)>,
    /// Cell the cursor is drawn in, as (column, row) on the frame.
    drawn_cursor: Option<(u32, u32)>,
    shaped: HashMap<u64, Rc<Vec<Glyph>>>,

    width: u32,
    height: u32,
    /// The frame as XRGB, one `u32` per pixel.
    pixels: Vec<u32>,
    /// Everything must be repainted, as after a resize.
    stale: bool,
}

impl Renderer {
    /// Creates a renderer using the system's fonts.
    pub fn new(family: &str, font_px: f32) -> Self {
        Self::with_fonts(FontSystem::new(), family, font_px)
    }

    /// Creates a renderer that knows only the fonts in a directory, so that
    /// tests do not depend on what is installed.
    pub fn with_font_dir(dir: &Path, family: &str, font_px: f32) -> Self {
        let mut db = cosmic_text::fontdb::Database::new();
        db.load_fonts_dir(dir);
        let fonts = FontSystem::new_with_locale_and_db("en-US".to_owned(), db);
        Self::with_fonts(fonts, family, font_px)
    }

    fn with_fonts(mut fonts: FontSystem, family: &str, font_px: f32) -> Self {
        let scratch = Buffer::new(&mut fonts, Metrics::new(font_px, font_px));
        let mut renderer = Self {
            fonts,
            glyph_cache: SwashCache::new(),
            scratch,
            family: family.to_owned(),
            font_px,
            cell_w: 1,
            cell_h: 1,
            baseline: 0,
            faces: Vec::new(),
            panes: HashMap::new(),
            cursor: None,
            drawn_cursor: None,
            shaped: HashMap::new(),
            width: 0,
            height: 0,
            pixels: Vec::new(),
            stale: true,
        };
        renderer.set_font_size(font_px);
        renderer
    }

    /// Changes the font size in pixels, as when the output's scale changes.
    pub fn set_font_size(&mut self, font_px: f32) {
        self.font_px = font_px;
        self.shaped.clear();
        self.glyph_cache = SwashCache::new();
        self.stale = true;
        self.scratch.set_metrics(Metrics::new(font_px, font_px));
        self.scratch.set_wrap(Wrap::None);
        self.measure();
    }

    /// Derives the cell size from the font: the advance of `M` and the
    /// font's own line height.
    fn measure(&mut self) {
        let family = self.family.clone();
        self.scratch
            .set_text("M", &family_attrs(&family), Shaping::Advanced, None);
        self.scratch.shape_until_scroll(&mut self.fonts, false);
        let glyph = self
            .scratch
            .layout_runs()
            .next()
            .and_then(|run| run.glyphs.first().cloned());

        // Fallbacks for a system with no usable font at all.
        let mut advance = self.font_px * 0.6;
        let (mut ascent, mut descent) = (self.font_px * 0.8, self.font_px * 0.2);
        if let Some(glyph) = glyph {
            advance = glyph.w;
            if let Some(font) = self.fonts.get_font(glyph.font_id, glyph.font_weight) {
                let metrics = font.as_swash().metrics(&[]).scale(self.font_px);
                ascent = metrics.ascent;
                descent = metrics.descent + metrics.leading;
            }
        }
        self.cell_w = (advance.round() as u32).max(1);
        self.baseline = ascent.round() as i32;
        self.cell_h = ((ascent.round() + descent.ceil()) as u32).max(1);
    }

    /// Size of a character cell in pixels.
    pub fn cell_size(&self) -> (u32, u32) {
        (self.cell_w, self.cell_h)
    }

    /// How many columns and rows fit in an area of pixels.
    pub fn grid_size(&self, width: u32, height: u32) -> (u16, u16) {
        let cols = (width / self.cell_w).clamp(1, u32::from(u16::MAX));
        let rows = (height / self.cell_h).clamp(1, u32::from(u16::MAX));
        (cols as u16, rows as u16)
    }

    /// Sets the size of the picture in pixels.
    pub fn resize(&mut self, width: u32, height: u32) {
        if (width, height) != (self.width, self.height) {
            self.width = width;
            self.height = height;
            self.pixels = vec![pixel(DEFAULT_BG); width as usize * height as usize];
            self.stale = true;
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// The picture, one XRGB `u32` per pixel, row by row.
    pub fn pixels(&self) -> &[u32] {
        &self.pixels
    }

    /// Records draw operations. Returns true if they complete a frame, which
    /// is then ready to be painted.
    pub fn apply(&mut self, ops: &[DrawOp]) -> bool {
        let mut flushed = false;
        for op in ops {
            match op {
                DrawOp::DefineFaces(faces) => {
                    // A table that only adds faces changes nothing drawn.
                    let grown = faces.starts_with(&self.faces);
                    self.faces.clone_from(faces);
                    if !grown {
                        self.shaped.clear();
                        self.stale = true;
                    }
                }
                DrawOp::Layout { panes, .. } => {
                    self.panes = panes
                        .iter()
                        .map(|rect| {
                            let rows = usize::from(rect.h);
                            let pane = Pane {
                                rect: *rect,
                                lines: vec![DisplayLine::default(); rows],
                                dirty: vec![true; rows],
                            };
                            (rect.id, pane)
                        })
                        .collect();
                    self.cursor = None;
                    self.stale = true;
                }
                DrawOp::Lines {
                    pane,
                    first_row,
                    lines,
                } => {
                    let Some(pane) = self.panes.get_mut(pane) else {
                        continue;
                    };
                    for (offset, line) in lines.iter().enumerate() {
                        let row = usize::from(*first_row) + offset;
                        if row < pane.lines.len() {
                            pane.lines[row].clone_from(line);
                            pane.dirty[row] = true;
                        }
                    }
                }
                DrawOp::Cursor { pane, row, col, .. } => self.cursor = Some((*pane, *row, *col)),
                DrawOp::Flush => flushed = true,
            }
        }
        flushed
    }

    fn face(&self, index: usize) -> FaceAttrs {
        self.faces.get(index).copied().unwrap_or_default()
    }

    /// Foreground and background of a face as pixels, with defaults filled
    /// in and inverse video applied.
    fn colors(&self, index: usize) -> (u32, u32) {
        let face = self.face(index);
        let fg = pixel(face.fg.unwrap_or(DEFAULT_FG));
        let bg = pixel(face.bg.unwrap_or(DEFAULT_BG));
        if face.inverse { (bg, fg) } else { (fg, bg) }
    }

    /// Shapes a line into glyphs placed on the cell grid.
    fn shape(&mut self, line: &DisplayLine) -> Rc<Vec<Glyph>> {
        let mut hasher = DefaultHasher::new();
        line.text.hash(&mut hasher);
        for run in &line.runs {
            let face = self.face(run.face.0 as usize);
            (run.len, run.face.0, face.bold, face.italic).hash(&mut hasher);
        }
        let key = hasher.finish();
        if let Some(glyphs) = self.shaped.get(&key) {
            return glyphs.clone();
        }

        // Column of the character at each byte offset of the text.
        let mut columns = vec![0u32; line.text.len() + 1];
        let mut col = 0;
        for (offset, c) in line.text.char_indices() {
            columns[offset..offset + c.len_utf8()].fill(col);
            col += c.width().unwrap_or(1) as u32;
        }
        columns[line.text.len()] = col;

        let family = self.family.clone();
        let base = family_attrs(&family);
        let faces = &self.faces;
        let spans = line.segments().map(|(text, face)| {
            let index = face.0 as usize;
            let attrs = faces.get(index).copied().unwrap_or_default();
            let mut span = base.clone().metadata(index);
            if attrs.bold {
                span = span.weight(Weight::BOLD);
            }
            if attrs.italic {
                span = span.style(Style::Italic);
            }
            (text, span)
        });
        self.scratch
            .set_rich_text(spans, &base, Shaping::Advanced, None);
        self.scratch.shape_until_scroll(&mut self.fonts, false);

        let mut glyphs = Vec::new();
        for run in self.scratch.layout_runs() {
            for glyph in run.glyphs {
                // Only the glyph's offset within its own advance is kept:
                // the cell decides where the advance starts.
                let mut placed = glyph.clone();
                placed.x = 0.0;
                placed.y = 0.0;
                let physical = placed.physical((0.0, 0.0), 1.0);
                glyphs.push(Glyph {
                    col: columns.get(glyph.start).copied().unwrap_or(col),
                    cache_key: physical.cache_key,
                    dx: physical.x,
                    dy: physical.y,
                    face: glyph.metadata,
                });
            }
        }
        if self.shaped.len() >= SHAPE_CACHE_LIMIT {
            self.shaped.clear();
        }
        let glyphs = Rc::new(glyphs);
        self.shaped.insert(key, glyphs.clone());
        glyphs
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
    fn draw_glyph(&mut self, key: CacheKey, x: i32, y: i32, color: u32, clip: Rect) {
        let Some(image) = self.glyph_cache.get_image(&mut self.fonts, key) else {
            return;
        };
        let (width, height) = (image.placement.width as i32, image.placement.height as i32);
        let (left, top) = (x + image.placement.left, y - image.placement.top);
        let clip_x1 = (clip.x + clip.w).min(self.width) as i32;
        let clip_y1 = (clip.y + clip.h).min(self.height) as i32;
        let channels = match image.content {
            SwashContent::Mask => 1,
            SwashContent::Color | SwashContent::SubpixelMask => 4,
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
                // as an emoji, brings its own colour.
                let (sr, sg, sb, alpha) = match image.content {
                    SwashContent::Mask => (r, g, b, u32::from(data[0])),
                    _ => (
                        u32::from(data[0]),
                        u32::from(data[1]),
                        u32::from(data[2]),
                        u32::from(data[3]),
                    ),
                };
                if alpha == 0 {
                    continue;
                }
                let index = py as usize * self.width as usize + px as usize;
                let dest = self.pixels[index];
                let blend = |source: u32, dest: u32| (source * alpha + dest * (255 - alpha)) / 255;
                self.pixels[index] = 0xff00_0000
                    | blend(sr, dest >> 16 & 0xff) << 16
                    | blend(sg, dest >> 8 & 0xff) << 8
                    | blend(sb, dest & 0xff);
            }
        }
    }

    /// Paints one row of a pane and returns the area it covers.
    fn paint_row(&mut self, rect: PaneRect, row: u16, line: &DisplayLine) -> Rect {
        let area = Rect {
            x: u32::from(rect.x) * self.cell_w,
            y: (u32::from(rect.y) + u32::from(row)) * self.cell_h,
            w: u32::from(rect.w) * self.cell_w,
            h: self.cell_h,
        };

        // Backgrounds first, so that a glyph reaching into the next cell,
        // as ligatures do, is not painted over.
        let mut col = 0;
        let mut underlines = Vec::new();
        // Thin lines along the top and bottom of boxed text.
        let mut boxes: Vec<(Rect, u32)> = Vec::new();
        for (text, face) in line.segments() {
            let index = face.0 as usize;
            let cells: u32 = text.chars().map(|c| c.width().unwrap_or(1) as u32).sum();
            let cells = cells.min(u32::from(rect.w).saturating_sub(col));
            let span = Rect {
                x: area.x + col * self.cell_w,
                w: cells * self.cell_w,
                ..area
            };
            let (fg, bg) = self.colors(index);
            self.fill(span, bg);
            let face = self.face(index);
            if face.underline {
                underlines.push((span, face.underline_color.map_or(fg, pixel)));
            }
            if let Some(color) = face.box_color {
                boxes.push((span, pixel(color)));
            }
            col += cells;
        }
        let rest = Rect {
            x: area.x + col * self.cell_w,
            w: area.w - col * self.cell_w,
            ..area
        };
        let (_, fill_bg) = self.colors(line.fill.0 as usize);
        self.fill(rest, fill_bg);
        if let Some(color) = self.face(line.fill.0 as usize).box_color {
            boxes.push((rest, pixel(color)));
        }

        let glyphs = self.shape(line);
        let baseline = area.y as i32 + self.baseline;
        for glyph in glyphs.iter() {
            let (fg, _) = self.colors(glyph.face);
            let x = (area.x + glyph.col * self.cell_w) as i32 + glyph.dx;
            self.draw_glyph(glyph.cache_key, x, baseline + glyph.dy, fg, area);
        }
        let border = (self.cell_h / 20).max(1).min(area.h);
        for (span, color) in boxes {
            self.fill(Rect { h: border, ..span }, color);
            self.fill(
                Rect {
                    y: span.y + span.h - border,
                    h: border,
                    ..span
                },
                color,
            );
        }
        for (span, color) in underlines {
            let thickness = (self.cell_h / 14).max(1);
            let y = (baseline as u32 + thickness).min(area.y + area.h - thickness);
            self.fill(
                Rect {
                    y,
                    h: thickness,
                    ..span
                },
                color,
            );
        }
        area
    }

    fn cell_rect(&self, (col, row): (u32, u32)) -> Rect {
        Rect {
            x: col * self.cell_w,
            y: row * self.cell_h,
            w: self.cell_w,
            h: self.cell_h,
        }
    }

    /// Brings the picture up to date. Returns the area that changed, or
    /// `None` if nothing did.
    pub fn paint(&mut self) -> Option<Rect> {
        let mut damage: Option<Rect> = None;
        let mut add = |rect: Rect| damage = Some(damage.map_or(rect, |old| old.union(rect)));

        let cursor = self.cursor.and_then(|(pane, row, col)| {
            let rect = self.panes.get(&pane)?.rect;
            Some((
                u32::from(rect.x) + u32::from(col),
                u32::from(rect.y) + u32::from(row),
            ))
        });
        // The cursor is drawn by inverting its cell. When it moves, the row
        // it was on is painted afresh to erase it. A row that is painted
        // for any other reason loses its cursor too, which is then redrawn.
        let cursor_moved = cursor != self.drawn_cursor;
        let old_cursor_row = self.drawn_cursor.map(|(_, row)| row);

        if self.stale {
            let whole = Rect {
                x: 0,
                y: 0,
                w: self.width,
                h: self.height,
            };
            // The margin outside the panes has the default face's background.
            let (_, background) = self.colors(0);
            self.fill(whole, background);
            if self.width > 0 && self.height > 0 {
                add(whole);
            }
        }

        let ids: Vec<PaneId> = self.panes.keys().copied().collect();
        let mut cursor_row_painted = false;
        for id in ids {
            let pane = &self.panes[&id];
            let rect = pane.rect;
            for row in 0..rect.h {
                let frame_row = u32::from(rect.y) + u32::from(row);
                let holds_old_cursor = cursor_moved && old_cursor_row == Some(frame_row);
                let holds_cursor = cursor.is_some_and(|(_, cursor_row)| cursor_row == frame_row);
                let pane = &self.panes[&id];
                if !(self.stale || pane.dirty[usize::from(row)] || holds_old_cursor) {
                    continue;
                }
                let line = pane.lines[usize::from(row)].clone();
                add(self.paint_row(rect, row, &line));
                cursor_row_painted |= holds_cursor;
                if let Some(pane) = self.panes.get_mut(&id) {
                    pane.dirty[usize::from(row)] = false;
                }
            }
        }

        if let Some(cell) = cursor
            && (cursor_moved || cursor_row_painted || self.stale)
        {
            let rect = self.cell_rect(cell);
            let x1 = (rect.x + rect.w).min(self.width);
            let y1 = (rect.y + rect.h).min(self.height);
            for y in rect.y.min(y1)..y1 {
                for x in rect.x.min(x1)..x1 {
                    let index = y as usize * self.width as usize + x as usize;
                    self.pixels[index] ^= 0x00ff_ffff;
                }
            }
            add(rect);
        }
        self.drawn_cursor = cursor;
        self.stale = false;

        damage.map(|rect| Rect {
            w: rect.w.min(self.width.saturating_sub(rect.x)),
            h: rect.h.min(self.height.saturating_sub(rect.y)),
            ..rect
        })
    }
}
