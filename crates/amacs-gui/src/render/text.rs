//! The fonts: how large a cell is, which glyphs a line is made of, and what
//! each glyph looks like.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;
use std::rc::Rc;

use amacs_proto::{DisplayLine, FaceAttrs};
use cosmic_text::{
    Attrs, Buffer, CacheKey, Family, FontSystem, Metrics, Shaping, Style, SwashCache, SwashContent,
    Weight, Wrap,
};
use unicode_width::UnicodeWidthChar;

/// Shaped lines kept for reuse. Scrolling shows mostly lines seen before.
const SHAPE_CACHE_LIMIT: usize = 4096;

/// Names the picture of one glyph: of a font, at a size.
pub type GlyphKey = CacheKey;

/// What the bytes of a glyph's picture are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlyphContent {
    /// A byte per pixel: how much of the pixel the glyph covers, to be
    /// filled with the colour of the text.
    Mask,
    /// Four bytes per pixel, red, green, blue and alpha: a glyph that
    /// brings its own colours, such as an emoji.
    Color,
}

/// The picture of a glyph.
pub struct GlyphImage<'a> {
    /// Where the left and the top of the picture are from the glyph's
    /// origin: to the right, and upward.
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub content: GlyphContent,
    pub data: &'a [u8],
}

/// Where backends get the pictures of glyphs.
pub trait GlyphSource {
    /// The picture of a glyph, or `None` if it has none, as a space.
    fn image(&mut self, key: GlyphKey) -> Option<GlyphImage<'_>>;
}

/// One glyph of a shaped line, placed relative to its cell.
pub struct Glyph {
    /// Column of the character the glyph came from.
    pub col: u32,
    pub key: GlyphKey,
    /// Offset of the glyph origin from the left of its cell and from the
    /// baseline, in pixels.
    pub dx: i32,
    pub dy: i32,
    pub face: usize,
}

/// Text attributes selecting a font family by name. `monospace` means
/// whatever the system's default monospace font is.
fn family_attrs(family: &str) -> Attrs<'_> {
    Attrs::new().family(match family {
        "monospace" => Family::Monospace,
        name => Family::Name(name),
    })
}

pub struct Fonts {
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
    shaped: HashMap<u64, Rc<Vec<Glyph>>>,
}

impl Fonts {
    /// The system's fonts.
    pub fn new(family: &str, font_px: f32) -> Self {
        Self::with_fonts(FontSystem::new(), family, font_px)
    }

    /// Only the fonts in a directory, so that tests do not depend on what
    /// is installed.
    pub fn with_font_dir(dir: &Path, family: &str, font_px: f32) -> Self {
        let mut db = cosmic_text::fontdb::Database::new();
        db.load_fonts_dir(dir);
        let fonts = FontSystem::new_with_locale_and_db("en-US".to_owned(), db);
        Self::with_fonts(fonts, family, font_px)
    }

    fn with_fonts(mut fonts: FontSystem, family: &str, font_px: f32) -> Self {
        let scratch = Buffer::new(&mut fonts, Metrics::new(font_px, font_px));
        let mut this = Self {
            fonts,
            glyph_cache: SwashCache::new(),
            scratch,
            family: family.to_owned(),
            font_px,
            cell_w: 1,
            cell_h: 1,
            baseline: 0,
            shaped: HashMap::new(),
        };
        this.set_size(font_px);
        this
    }

    /// Changes the font size in pixels. Every glyph's picture is another
    /// after this.
    pub fn set_size(&mut self, font_px: f32) {
        self.font_px = font_px;
        self.shaped.clear();
        self.glyph_cache = SwashCache::new();
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

    /// Distance from the top of a cell to the baseline.
    pub fn baseline(&self) -> i32 {
        self.baseline
    }

    /// Lines shaped so far may look different now, as when the faces
    /// changed.
    pub fn forget_shapes(&mut self) {
        self.shaped.clear();
    }

    /// Shapes a line into glyphs placed on the cell grid.
    pub fn shape(&mut self, line: &DisplayLine, faces: &[FaceAttrs]) -> Rc<Vec<Glyph>> {
        let face = |index: usize| faces.get(index).copied().unwrap_or_default();
        let mut hasher = DefaultHasher::new();
        line.text.hash(&mut hasher);
        for run in &line.runs {
            let face = face(run.face.0 as usize);
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
        let spans = line.segments().map(|(text, id)| {
            let index = id.0 as usize;
            let attrs = face(index);
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
                    key: physical.cache_key,
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
}

impl GlyphSource for Fonts {
    fn image(&mut self, key: GlyphKey) -> Option<GlyphImage<'_>> {
        let image = self.glyph_cache.get_image(&mut self.fonts, key).as_ref()?;
        Some(GlyphImage {
            left: image.placement.left,
            top: image.placement.top,
            width: image.placement.width,
            height: image.placement.height,
            content: match image.content {
                SwashContent::Mask => GlyphContent::Mask,
                SwashContent::Color | SwashContent::SubpixelMask => GlyphContent::Color,
            },
            data: &image.data,
        })
    }
}
