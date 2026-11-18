//! Rendering of draw operations, in parts that do not know of each other:
//!
//! - [`Scene`] keeps what the server said to show: faces, panes, lines and
//!   the cursor, and which rows changed.
//! - [`Fonts`] owns the fonts: the size of a cell, the shaping of a line
//!   into glyphs, and the picture of each glyph.
//! - [`list`] turns a row of the scene into a [`Row`] of rectangles and
//!   glyphs in pixels. Everything about how text looks is decided there:
//!   what is drawn over what, how thick an underline is.
//! - A [`Backend`] puts rows on screen. [`Software`] blends them into a
//!   picture in memory.
//!
//! [`Renderer`] ties them together, and is all that the window needs.
//!
//! # Adding a backend
//!
//! A backend implements [`Backend`], and nothing else changes. Each paint
//! it is given a [`Frame`]: every row as it is to look, the rows that
//! changed since the last time, and where the cursor is. One that keeps
//! its picture, as [`Software`] does, repaints the changed rows; one that
//! draws the whole frame each time, as a GPU does, draws them all. The
//! pictures of glyphs come from the [`GlyphSource`] by the key in each
//! [`GlyphQuad`]: a GPU backend puts them in an atlas under the same key,
//! and empties the atlas in [`Backend::forget_glyphs`].
//!
//! Text is drawn on a cell grid. Each run of a line is shaped as a whole,
//! so that programming ligatures form, and each glyph is then placed in
//! the cell of the character it came from.

mod gpu;
pub mod list;
mod scene;
mod software;
mod text;

use std::path::Path;

use amacs_proto::{Color, DrawOp};

pub use gpu::Gpu;
pub use list::{Frame, GlyphQuad, Quad, Row};
pub use scene::Scene;
pub use software::Software;
pub use text::{Fonts, GlyphContent, GlyphImage, GlyphKey, GlyphSource};

/// Colours for faces that leave theirs unset, and for the window before the
/// server has sent its faces. The server's themes set both for every face.
pub const DEFAULT_FG: Color = Color::rgb(0x00, 0x00, 0x00);
pub const DEFAULT_BG: Color = Color::rgb(0xfb, 0xf7, 0xf0);

/// A rectangle in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub(crate) fn union(self, other: Rect) -> Rect {
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

/// What puts rows on screen.
pub trait Backend {
    /// Sets the size of what is drawn on, in pixels.
    fn resize(&mut self, width: u32, height: u32);

    /// The pictures of glyphs got so far are no longer right, as after a
    /// change of font size.
    fn forget_glyphs(&mut self);

    /// Brings what is shown up to date with a frame. Returns the area that
    /// changed, or `None` if nothing did.
    fn paint(&mut self, frame: &Frame, glyphs: &mut dyn GlyphSource) -> Option<Rect>;
}

/// Turns draw operations into what a backend shows.
pub struct Renderer<B: Backend = Software> {
    scene: Scene,
    fonts: Fonts,
    /// Every row of every pane as it is to look, pane after pane in the
    /// order of the layout.
    rows: Vec<Row>,
    width: u32,
    height: u32,
    backend: B,
}

impl Renderer<Software> {
    /// Creates a renderer using the system's fonts.
    pub fn new(family: &str, font_px: f32) -> Self {
        Self::with_backend(Fonts::new(family, font_px), Software::default())
    }

    /// Creates a renderer that knows only the fonts in a directory, so that
    /// tests do not depend on what is installed.
    pub fn with_font_dir(dir: &Path, family: &str, font_px: f32) -> Self {
        Self::with_backend(
            Fonts::with_font_dir(dir, family, font_px),
            Software::default(),
        )
    }

    /// The picture, one XRGB `u32` per pixel, row by row.
    pub fn pixels(&self) -> &[u32] {
        self.backend.pixels()
    }
}

impl<B: Backend> Renderer<B> {
    pub fn with_backend(fonts: Fonts, backend: B) -> Self {
        Self {
            scene: Scene::default(),
            fonts,
            rows: Vec::new(),
            width: 0,
            height: 0,
            backend,
        }
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// Changes the font size in pixels, as when the output's scale changes.
    pub fn set_font_size(&mut self, font_px: f32) {
        self.fonts.set_size(font_px);
        self.backend.forget_glyphs();
        self.scene.set_stale();
    }

    /// Size of a character cell in pixels.
    pub fn cell_size(&self) -> (u32, u32) {
        self.fonts.cell_size()
    }

    /// How many columns and rows fit in an area of pixels.
    pub fn grid_size(&self, width: u32, height: u32) -> (u16, u16) {
        let (cell_w, cell_h) = self.fonts.cell_size();
        let cols = (width / cell_w).clamp(1, u32::from(u16::MAX));
        let rows = (height / cell_h).clamp(1, u32::from(u16::MAX));
        (cols as u16, rows as u16)
    }

    /// Sets the size of the picture in pixels.
    pub fn resize(&mut self, width: u32, height: u32) {
        if (width, height) != (self.width, self.height) {
            self.width = width;
            self.height = height;
            self.backend.resize(width, height);
            self.scene.set_stale();
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Records draw operations. Returns true if they complete a frame, which
    /// is then ready to be painted.
    pub fn apply(&mut self, ops: &[DrawOp]) -> bool {
        let (flushed, shapes_changed) = self.scene.apply(ops);
        if shapes_changed {
            self.fonts.forget_shapes();
        }
        flushed
    }

    /// Brings the picture up to date. Returns the area that changed, or
    /// `None` if nothing did.
    pub fn paint(&mut self) -> Option<Rect> {
        let full = self.scene.is_stale();
        if full {
            self.rows.clear();
        }
        // The rows that are built anew, by their place among all rows.
        let mut changed = Vec::new();
        let mut index = 0;
        for pane in self.scene.panes() {
            for row in 0..pane.rect.h {
                if full || pane.dirty[usize::from(row)] {
                    let line = &pane.lines[usize::from(row)];
                    let built = list::build_row(&self.scene, &mut self.fonts, pane.rect, row, line);
                    if full {
                        self.rows.push(built);
                    } else {
                        self.rows[index] = built;
                    }
                    changed.push(index);
                }
                index += 1;
            }
        }
        let (cell_w, cell_h) = self.fonts.cell_size();
        let cursor = self.scene.cursor_cell().map(|(col, row)| Rect {
            x: col * cell_w,
            y: row * cell_h,
            w: cell_w,
            h: cell_h,
        });
        let frame = Frame {
            size: (self.width, self.height),
            // The margin outside the panes has the default face's background.
            background: self.scene.colors(0).1,
            full,
            rows: &self.rows,
            changed: &changed,
            cursor,
        };
        let damage = self.backend.paint(&frame, &mut self.fonts);
        self.scene.painted();
        damage
    }
}
