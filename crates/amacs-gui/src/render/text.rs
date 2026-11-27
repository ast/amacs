//! The fonts: how large a cell is, which glyphs a line is made of, and what
//! each glyph looks like.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;
use std::rc::Rc;

use amacs_proto::{DisplayLine, FaceAttrs, SubpixelOrder};
use cosmic_text::{
    Attrs, Buffer, CacheKey, CacheKeyFlags, Family, FontSystem, Metrics, Shaping, Style, Weight,
    Wrap,
};
use swash::scale::image::{Content as SwashContent, Image as SwashImage};
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::zeno::{Angle, Format, Transform, Vector};
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
    /// Four bytes per pixel: how much of the pixel's red, green and blue
    /// parts the glyph covers, each to be filled with that part of the
    /// colour of the text, and a fourth byte that says nothing.
    Subpixel,
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
    scaler: ScaleContext,
    /// The pictures of glyphs drawn so far. Glyphs without one, as a space,
    /// are remembered too.
    images: HashMap<GlyphKey, Option<SwashImage>>,
    subpixel: SubpixelOrder,
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
            scaler: ScaleContext::new(),
            images: HashMap::new(),
            subpixel: SubpixelOrder::None,
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

    /// Draws glyphs for a screen whose pixels have their parts in this
    /// order, or in shades of grey for `None`.
    pub fn with_subpixel(mut self, order: SubpixelOrder) -> Self {
        self.set_subpixel(order);
        self
    }

    /// Changes the order of subpixels to draw for. Every glyph's picture is
    /// another after this.
    pub fn set_subpixel(&mut self, order: SubpixelOrder) {
        self.subpixel = order;
        self.images.clear();
    }

    /// The order of subpixels that glyphs are drawn for.
    pub fn subpixel(&self) -> SubpixelOrder {
        self.subpixel
    }

    /// Changes the font size in pixels. Every glyph's picture is another
    /// after this.
    pub fn set_size(&mut self, font_px: f32) {
        self.font_px = font_px;
        self.shaped.clear();
        self.images.clear();
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
            // Where the first glyph of the current column starts.
            let mut cell: Option<(u32, f32)> = None;
            for glyph in run.glyphs {
                // The cell decides where a column starts. Only a glyph's
                // place within its column is kept: a combining mark is
                // placed from the end of the letter it is on, which is in
                // the same column.
                let glyph_col = columns.get(glyph.start).copied().unwrap_or(col);
                let start = match cell {
                    Some((cell_col, x)) if cell_col == glyph_col => x,
                    _ => {
                        cell = Some((glyph_col, glyph.x));
                        glyph.x
                    }
                };
                let mut placed = glyph.clone();
                placed.x = glyph.x - start;
                placed.y = 0.0;
                let physical = placed.physical((0.0, 0.0), 1.0);
                glyphs.push(Glyph {
                    col: glyph_col,
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
        // The red and blue parts of a pixel are a third of a pixel to each
        // side of the green, in the order the screen has them.
        let format = match self.subpixel {
            SubpixelOrder::None => Format::Alpha,
            SubpixelOrder::Rgb => Format::Subpixel,
            SubpixelOrder::Bgr => Format::subpixel_bgra(),
        };
        let image = self
            .images
            .entry(key)
            .or_insert_with(|| rasterize(&mut self.fonts, &mut self.scaler, key, format))
            .as_ref()?;
        Some(GlyphImage {
            left: image.placement.left,
            top: image.placement.top,
            width: image.placement.width,
            height: image.placement.height,
            content: match image.content {
                SwashContent::Mask => GlyphContent::Mask,
                SwashContent::Color => GlyphContent::Color,
                SwashContent::SubpixelMask => GlyphContent::Subpixel,
            },
            data: &image.data,
        })
    }
}

/// Draws a glyph: a colour outline or bitmap if the font has one, as for
/// an emoji, else its outline, hinted, in the format asked for.
fn rasterize(
    fonts: &mut FontSystem,
    context: &mut ScaleContext,
    key: CacheKey,
    format: Format,
) -> Option<SwashImage> {
    let font = fonts.get_font(key.font_id, key.font_weight)?;
    let font = font.as_swash();
    let mut builder = context
        .builder(font)
        .size(f32::from_bits(key.font_size_bits))
        .hint(!key.flags.contains(CacheKeyFlags::DISABLE_HINTING));
    // A variable font is set to the weight asked for.
    let weight = swash::Tag::from_be_bytes(*b"wght");
    if let Some(axis) = font.variations().find_by_tag(weight) {
        let value = f32::from(key.font_weight.0).clamp(axis.min_value(), axis.max_value());
        builder = builder.normalized_coords(font.variations().normalized_coords([(weight, value)]));
    }
    let mut scaler = builder.build();
    let offset = Vector::new(key.x_bin.as_float(), key.y_bin.as_float());
    // Italic from a font that has none is the upright glyph, slanted.
    let slant = key
        .flags
        .contains(CacheKeyFlags::FAKE_ITALIC)
        .then(|| Transform::skew(Angle::from_degrees(14.0), Angle::from_degrees(0.0)));
    Render::new(&[
        Source::ColorOutline(0),
        Source::ColorBitmap(StrikeWith::BestFit),
        Source::Outline,
    ])
    .format(format)
    .offset(offset)
    .transform(slant)
    .render(&mut scaler, key.glyph_id)
}
