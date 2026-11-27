//! The renderer, driven by a real editor: keys in, pixels out.
//!
//! The assertions are about structure (which cells have ink, which colours
//! fill where, how much is repainted) rather than exact images, so they hold
//! across font and rasteriser versions. Each test also writes a PNG to the
//! target directory for a person to look at.

use std::path::PathBuf;

use amacs_core::Editor;
use amacs_gui::render::{DEFAULT_BG, Rect, Renderer};
use amacs_proto::{Color, InputEvent, Key, SubpixelOrder, parse_keys};

const FONT_PX: f32 = 16.0;

struct Harness {
    editor: Editor,
    renderer: Renderer,
    name: &'static str,
}

fn rgb(color: Color) -> u32 {
    0xff00_0000 | u32::from(color.r) << 16 | u32::from(color.g) << 8 | u32::from(color.b)
}

impl Harness {
    /// A frame of `cols` by `rows` cells, drawn with JetBrains Mono from the
    /// directory in `AMACS_TEST_FONTS`, or else the system's monospace font.
    fn new(name: &'static str, cols: u16, rows: u16) -> Self {
        let mut renderer = match std::env::var_os("AMACS_TEST_FONTS") {
            Some(dir) => Renderer::with_font_dir(&PathBuf::from(dir), "JetBrains Mono", FONT_PX),
            None => Renderer::new("monospace", FONT_PX),
        };
        let (cell_w, cell_h) = renderer.cell_size();
        renderer.resize(u32::from(cols) * cell_w, u32::from(rows) * cell_h);
        let mut harness = Self {
            editor: Editor::new(cols, rows),
            renderer,
            name,
        };
        harness.paint();
        harness
    }

    fn paint(&mut self) -> Option<Rect> {
        let ops = self.editor.redisplay();
        self.renderer.apply(&ops);
        let damage = self.renderer.paint();
        self.save();
        damage
    }

    fn keys(&mut self, keys: &str) -> Option<Rect> {
        for key in parse_keys(keys).unwrap() {
            self.editor.handle_event(InputEvent::Key(key));
        }
        self.paint()
    }

    fn type_text(&mut self, text: &str) -> Option<Rect> {
        for c in text.chars() {
            self.editor.handle_event(InputEvent::Key(Key::char(c)));
        }
        self.paint()
    }

    fn paste(&mut self, text: &str) -> Option<Rect> {
        self.editor.handle_event(InputEvent::Paste(text.to_owned()));
        self.paint()
    }

    /// The pixels of one cell.
    fn cell(&self, col: u32, row: u32) -> Vec<u32> {
        let (cell_w, cell_h) = self.renderer.cell_size();
        let (width, _) = self.renderer.size();
        let pixels = self.renderer.pixels();
        let mut out = Vec::new();
        for y in row * cell_h..(row + 1) * cell_h {
            let start = (y * width + col * cell_w) as usize;
            out.extend_from_slice(&pixels[start..start + cell_w as usize]);
        }
        out
    }

    /// Whether every pixel of a cell has this colour.
    fn cell_is(&self, col: u32, row: u32, color: Color) -> bool {
        self.cell(col, row).iter().all(|pixel| *pixel == rgb(color))
    }

    /// The share of a cell's pixels that are darker than mid grey.
    fn ink(&self, col: u32, row: u32) -> f32 {
        let cell = self.cell(col, row);
        let dark = cell
            .iter()
            .filter(|pixel| (*pixel >> 16 & 0xff) + (*pixel >> 8 & 0xff) + (*pixel & 0xff) < 384)
            .count();
        dark as f32 / cell.len() as f32
    }

    fn save(&self) {
        let (width, height) = self.renderer.size();
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("render");
        std::fs::create_dir_all(&dir).unwrap();
        let file = std::fs::File::create(dir.join(format!("{}.png", self.name))).unwrap();
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let bytes: Vec<u8> = self
            .renderer
            .pixels()
            .iter()
            .flat_map(|pixel| [(pixel >> 16) as u8, (pixel >> 8) as u8, *pixel as u8])
            .collect();
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&bytes)
            .unwrap();
    }
}

#[test]
fn cells_have_a_sensible_size_for_the_font() {
    let h = Harness::new("metrics", 20, 4);
    let (cell_w, cell_h) = h.renderer.cell_size();
    // A monospace font's advance is about 0.6 em and its line about 1.3 em.
    assert!((8..=11).contains(&cell_w), "cell width {cell_w}");
    assert!((18..=24).contains(&cell_h), "cell height {cell_h}");
    assert_eq!(
        h.renderer
            .grid_size(20 * cell_w + cell_w - 1, 4 * cell_h + 1),
        (20, 4)
    );
    assert_eq!(h.renderer.grid_size(0, 0), (1, 1));
}

#[test]
fn text_is_drawn_dark_on_the_background_with_a_block_cursor() {
    let mut h = Harness::new("text", 40, 6);
    h.type_text("Hello, world");
    assert!(h.ink(0, 0) > 0.1, "H has ink: {}", h.ink(0, 0));
    assert!(h.ink(4, 0) > 0.05, "o has ink");
    assert!(h.cell_is(6, 0, DEFAULT_BG), "the space is empty");
    assert!(h.cell_is(20, 0, DEFAULT_BG), "past the text is empty");
    assert!(h.cell_is(0, 2, DEFAULT_BG), "an empty row is empty");
    // The cursor inverts its cell: an empty cell becomes almost black.
    assert!(h.ink(12, 0) > 0.95, "cursor cell: {}", h.ink(12, 0));
}

#[test]
fn the_cursor_moves_without_leaving_a_trace() {
    let mut h = Harness::new("cursor", 40, 6);
    h.type_text("abc");
    assert!(h.ink(3, 0) > 0.95);
    h.keys("C-a");
    assert!(
        h.cell_is(3, 0, DEFAULT_BG),
        "the old cursor cell is restored"
    );
    assert!(h.ink(0, 0) > 0.5, "the cursor is on the a, drawn inverted");
    h.keys("C-e");
    assert!(h.ink(0, 0) < 0.5, "the a is back to dark on light");
    assert!(h.ink(0, 0) > 0.05);
}

#[test]
fn faces_colour_the_mode_line_and_the_region() {
    let mut h = Harness::new("faces", 40, 6);
    h.type_text("select this");
    let mode_line_bg = Color::rgb(0xca, 0xb9, 0xb2);
    // Apart from the thin lines along its top and bottom.
    let filled = h
        .cell(39, 4)
        .iter()
        .filter(|pixel| **pixel == rgb(mode_line_bg))
        .count();
    assert!(
        filled * 10 >= h.cell(39, 4).len() * 8,
        "the mode line fills its row"
    );
    assert!(h.ink(2, 4) > 0.0, "and has text");

    h.keys("C-a C-SPC M-f");
    let region_bg = rgb(Color::rgb(0xc2, 0xbc, 0xb5));
    assert!(
        h.cell(1, 0).contains(&region_bg),
        "selected text has the region background"
    );
    assert!(
        !h.cell(8, 0).contains(&region_bg),
        "text after the region does not"
    );
}

#[test]
fn a_keystroke_repaints_only_what_changed() {
    let mut h = Harness::new("damage", 60, 20);
    h.paste("one\ntwo\nthree\nfour");
    h.keys("M-<");
    let (cell_w, cell_h) = h.renderer.cell_size();
    let (width, height) = h.renderer.size();

    let damage = h.type_text("x").expect("typing changes the picture");
    // The edited row at the top and the mode line near the bottom.
    assert_eq!((damage.x, damage.y, damage.w), (0, 0, width));
    assert!(damage.h < height, "the echo area is untouched");

    let damage = h
        .keys("C-n")
        .expect("moving the cursor changes the picture");
    assert!(damage.h <= 19 * cell_h);
    assert_eq!(
        h.keys("C-g").map(|d| d.h),
        Some(cell_h),
        "only the echo area says Quit"
    );
    let _ = cell_w;

    // Nothing to do, nothing repainted.
    let ops = h.editor.redisplay();
    assert!(ops.is_empty());
    h.renderer.apply(&ops);
    assert_eq!(h.renderer.paint(), None);
}

#[test]
fn wide_characters_take_two_cells() {
    let mut h = Harness::new("wide", 40, 4);
    h.paste("a日b");
    assert!(h.ink(0, 0) > 0.03);
    assert!(h.ink(3, 0) > 0.03, "b is in the fourth cell");
    assert!(h.ink(4, 0) > 0.95, "the cursor is in the fifth");
}

#[test]
fn bigger_fonts_make_bigger_cells_and_resizing_repaints_everything() {
    let mut h = Harness::new("scaled", 30, 4);
    h.type_text("scale");
    let small = h.renderer.cell_size();
    h.renderer.set_font_size(FONT_PX * 1.25);
    let large = h.renderer.cell_size();
    assert!(
        large.0 > small.0 && large.1 > small.1,
        "{small:?} -> {large:?}"
    );

    h.renderer.resize(30 * large.0, 4 * large.1);
    let damage = h.renderer.paint().unwrap();
    assert_eq!((damage.w, damage.h), (30 * large.0, 4 * large.1));
    h.save();
    assert!(h.ink(0, 0) > 0.05);
    assert!(h.cell_is(10, 0, DEFAULT_BG));
}

#[test]
fn a_gallery_of_text_for_a_person_to_check() {
    let mut h = Harness::new("gallery", 64, 10);
    h.paste(
        "fn main() -> Result<(), Error> { a != b && c >= d }\n\
         åäö ÅÄÖ  日本語  \u{f121} \u{e7a8}  🙂\n\
         0123456789 the quick brown fox jumps over the lazy dog\n\tindented by a tab",
    );
    h.keys("C-x 3");
    assert!(h.ink(1, 0) > 0.03);
}

/// Times the work a keystroke causes in the client at full HD. Run with
/// `cargo test --release -p amacs-gui -- --ignored --nocapture timing`.
#[test]
#[ignore = "a measurement, not a check"]
fn timing_of_a_keystroke_at_full_hd() {
    use std::time::Instant;

    let mut h = Harness::new("timing", 200, 50);
    let (cell_w, cell_h) = h.renderer.cell_size();
    let line = "let value = compute(a, b) != other && flag >= limit; // comment text here\n";
    h.paste(&line.repeat(45));
    h.keys("M-<");

    let rounds = 200;
    let (mut apply_paint, mut copy) = (std::time::Duration::ZERO, std::time::Duration::ZERO);
    let mut canvas = vec![0u8; h.renderer.pixels().len() * 4];
    for round in 0..rounds {
        let c = char::from(b'a' + (round % 26) as u8);
        h.editor.handle_event(InputEvent::Key(Key::char(c)));
        let ops = h.editor.redisplay();
        let start = Instant::now();
        h.renderer.apply(&ops);
        h.renderer.paint();
        apply_paint += start.elapsed();
        let start = Instant::now();
        for (bytes, pixel) in canvas
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(h.renderer.pixels())
        {
            *bytes = pixel.to_le_bytes();
        }
        copy += start.elapsed();
    }

    let start = Instant::now();
    h.keys("C-v");
    let scroll = start.elapsed();
    eprintln!(
        "{}x{} px ({cell_w}x{cell_h} cells): keystroke paint {:?}, buffer copy {:?}, full-page scroll {:?}",
        h.renderer.size().0,
        h.renderer.size().1,
        apply_paint / rounds,
        copy / rounds,
        scroll,
    );
}

#[test]
fn the_mode_line_has_a_line_above_and_below() {
    let h = Harness::new("mode-line-box", 60, 6);
    let border = rgb(h.editor.faces().color("border-mode-line-active").unwrap());
    let fill = rgb(h.editor.faces().color("bg-mode-line-active").unwrap());
    let (cell_w, cell_h) = h.renderer.cell_size();
    // The mode line is the row above the echo area. Look at a cell with no
    // text on it, far to the right.
    let cell = h.cell(58, 4);
    let row = |y: u32| &cell[(y * cell_w) as usize..((y + 1) * cell_w) as usize];
    assert!(row(0).iter().all(|pixel| *pixel == border), "top line");
    assert!(
        row(cell_h - 1).iter().all(|pixel| *pixel == border),
        "bottom line"
    );
    assert!(row(cell_h / 2).iter().all(|pixel| *pixel == fill));
    // The text rows have no such lines.
    assert!(h.cell_is(58, 1, DEFAULT_BG));
}

/// How unevenly the channels of the pixels are covered: the largest
/// difference, over the pixels above the mode line, between how much of
/// the red and how much of the blue of the background a pixel has lost.
/// Grey text loses both alike.
fn channel_unevenness(renderer: &Renderer, rows: u16) -> f32 {
    let pixels = renderer.pixels();
    let (width, _) = renderer.size();
    let (_, cell_h) = renderer.cell_size();
    let text_rows = usize::from(rows - 2) * cell_h as usize;
    let mut counts = std::collections::HashMap::new();
    for &pixel in &pixels[..text_rows * width as usize] {
        *counts.entry(pixel).or_insert(0usize) += 1;
    }
    let background = counts
        .into_iter()
        .max_by_key(|&(_, count)| count)
        .map(|(pixel, _)| pixel)
        .unwrap();
    let channel = |pixel: u32, shift: u32| (pixel >> shift & 0xff) as f32;
    let lost = |pixel: u32, shift: u32| {
        (channel(background, shift) - channel(pixel, shift)) / channel(background, shift)
    };
    pixels[..text_rows * width as usize]
        .iter()
        .map(|&pixel| (lost(pixel, 16) - lost(pixel, 0)).abs())
        .fold(0.0, f32::max)
}

#[test]
fn text_is_drawn_per_subpixel_when_the_order_is_known() {
    let unevenness = |order: SubpixelOrder| {
        let (cols, rows) = (40, 6);
        let mut renderer = match std::env::var_os("AMACS_TEST_FONTS") {
            Some(dir) => Renderer::with_font_dir(&PathBuf::from(dir), "JetBrains Mono", FONT_PX),
            None => Renderer::new("monospace", FONT_PX),
        }
        .with_subpixel(order);
        let (cell_w, cell_h) = renderer.cell_size();
        renderer.resize(u32::from(cols) * cell_w, u32::from(rows) * cell_h);
        let mut editor = Editor::new(cols, rows);
        editor.handle_event(InputEvent::Paste(
            "The quick brown fox jumps over it".to_owned(),
        ));
        renderer.apply(&editor.redisplay());
        renderer.paint();
        channel_unevenness(&renderer, rows)
    };
    let grey = unevenness(SubpixelOrder::None);
    let rgb = unevenness(SubpixelOrder::Rgb);
    let bgr = unevenness(SubpixelOrder::Bgr);
    assert!(grey < 0.05, "grey text covers the channels alike: {grey}");
    assert!(
        rgb > 0.2,
        "text drawn per subpixel covers them unevenly: {rgb}"
    );
    assert!(
        bgr > 0.2,
        "text drawn per subpixel covers them unevenly: {bgr}"
    );
}

/// Writes a picture of one colour and a size in pixels, and returns where.
fn picture(name: &str, width: u32, height: u32, color: [u8; 3]) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("render");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let file = std::fs::File::create(&path).unwrap();
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let data: Vec<u8> = (0..width * height).flat_map(|_| color).collect();
    encoder
        .write_header()
        .and_then(|mut writer| writer.write_image_data(&data))
        .unwrap();
    path
}

#[test]
fn a_picture_is_drawn_in_the_cells_under_its_line() {
    let mut h = Harness::new("picture", 30, 8);
    let (cell_w, cell_h) = h.renderer.cell_size();
    h.editor.handle_event(InputEvent::Resize {
        cols: 30,
        rows: 8,
        cell: Some((cell_w as u16, cell_h as u16)),
    });
    h.type_text("one");
    h.keys("RET");
    h.type_text("two");
    // Three cells across and two down, to the pixel.
    let file = picture("source-red.png", cell_w * 3, cell_h * 2, [200, 30, 30]);
    h.editor.add_image(0, &file, None, None);
    h.paint();
    let red = rgb(Color::rgb(200, 30, 30));
    for (col, row) in [(0, 1), (2, 1), (0, 2), (2, 2)] {
        assert!(
            h.cell(col, row).iter().all(|pixel| *pixel == red),
            "cell {col}, {row} is of the picture"
        );
    }
    // Beside it and under it there is none of it.
    for (col, row) in [(3, 1), (3, 2), (0, 3), (0, 0)] {
        assert!(
            h.cell(col, row).iter().all(|pixel| *pixel != red),
            "cell {col}, {row} is not of the picture"
        );
    }
    // A picture larger than its rows is made smaller by the server: two
    // rows of a square are two cells' height across.
    let file = picture("source-blue.png", 400, 400, [20, 40, 220]);
    h.editor.add_image(5, &file, Some(2), None);
    h.paint();
    let blue = rgb(Color::rgb(20, 40, 220));
    let (width, _) = h.renderer.size();
    let pixels = h.renderer.pixels();
    let at = |x: u32, y: u32| pixels[(y * width + x) as usize];
    // The second line is now under the two rows of the first picture.
    let top = 4 * cell_h;
    assert_eq!(at(0, top), blue);
    assert_eq!(at(2 * cell_h - 1, top + 2 * cell_h - 1), blue);
    assert_ne!(at(2 * cell_h, top), blue);
}
