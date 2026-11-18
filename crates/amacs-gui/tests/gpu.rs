//! The backend that draws on the GPU, held against the one that draws on
//! the CPU: the same session, the same picture.
//!
//! The pictures are drawn into memory, so that no window is needed, but a
//! GPU is, or a Vulkan that computes on the CPU. Where there is neither,
//! as when the package is built, these tests say so and pass.

use std::path::PathBuf;

use amacs_core::Editor;
use amacs_gui::render::{Fonts, Gpu, Renderer, Software};
use amacs_proto::{InputEvent, Key, parse_keys};

const FONT_PX: f32 = 16.0;

fn fonts(font_px: f32) -> Fonts {
    match std::env::var_os("AMACS_TEST_FONTS") {
        Some(dir) => Fonts::with_font_dir(&PathBuf::from(dir), "JetBrains Mono", font_px),
        None => Fonts::new("monospace", font_px),
    }
}

/// An editor shown by both backends at once.
struct Pair {
    editor: Editor,
    cpu: Renderer<Software>,
    gpu: Renderer<Gpu>,
}

impl Pair {
    /// `None` where there is no GPU.
    fn new(cols: u16, rows: u16) -> Option<Self> {
        let gpu = match Gpu::offscreen() {
            Ok(gpu) => gpu,
            Err(error) => {
                eprintln!("skipped: {error:#}");
                return None;
            }
        };
        let mut pair = Self {
            editor: Editor::new(cols, rows),
            cpu: Renderer::with_backend(fonts(FONT_PX), Software::default()),
            gpu: Renderer::with_backend(fonts(FONT_PX), gpu),
        };
        let (cell_w, cell_h) = pair.cpu.cell_size();
        pair.resize(u32::from(cols) * cell_w, u32::from(rows) * cell_h);
        Some(pair)
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.cpu.resize(width, height);
        self.gpu.resize(width, height);
    }

    fn keys(&mut self, keys: &str) {
        for key in parse_keys(keys).unwrap() {
            self.editor.handle_event(InputEvent::Key(key));
        }
    }

    /// Draws what the editor shows now with both, and holds the pictures
    /// against each other.
    fn check(&mut self, step: &str) {
        let ops = self.editor.redisplay();
        self.cpu.apply(&ops);
        self.gpu.apply(&ops);
        self.cpu.paint();
        self.gpu.paint();
        let cpu = self.cpu.pixels();
        let gpu = self
            .gpu
            .backend()
            .read_pixels()
            .expect("the picture is read");
        assert_eq!(cpu.len(), gpu.len(), "{step}: the pictures have one size");
        let (width, _) = self.cpu.size();
        // Blending rounds down on the CPU and to the nearest on the GPU:
        // a step apart, or two where one glyph is blended over another.
        let mut worst = 0;
        let mut first = None;
        for (index, (a, b)) in cpu.iter().zip(&gpu).enumerate() {
            for shift in [0, 8, 16] {
                let difference = (a >> shift & 0xff).abs_diff(b >> shift & 0xff);
                if difference > worst {
                    worst = difference;
                    first = Some((index as u32 % width, index as u32 / width, *a, *b));
                }
            }
        }
        if std::env::var_os("AMACS_GPU_VERBOSE").is_some() {
            eprintln!(
                "{step}: worst difference {worst} on {}",
                self.gpu.backend().adapter()
            );
        }
        assert!(
            worst <= 2,
            "{step}: a pixel differs by {worst}, at {first:x?} (x, y, CPU, GPU)"
        );
    }
}

#[test]
fn the_gpu_draws_what_the_cpu_draws() {
    let Some(mut pair) = Pair::new(64, 14) else {
        return;
    };
    pair.check("empty");
    pair.editor.handle_event(InputEvent::Paste(
        "fn main() -> Result<(), Error> { a != b && c >= d }\n\
         åäö ÅÄÖ  日本語  \u{f121} \u{e7a8}  🙂\n\
         0123456789 the quick brown fox jumps over the lazy dog\n\tindented by a tab"
            .to_owned(),
    ));
    pair.check("text");
    for keys in [
        "C-x 3",
        "M-<",
        "C-SPC M-f M-f",
        "C-n",
        "C-e",
        "C-x o",
        "C-g",
        "M-x",
        "C-g",
        "C-x 2",
        "C-x 1",
    ] {
        pair.keys(keys);
        pair.check(keys);
    }
    for c in "typed != here".chars() {
        pair.editor.handle_event(InputEvent::Key(Key::char(c)));
        pair.check("typing");
    }
    // Nothing happened: both have nothing to do, and the picture stays.
    pair.check("idle");

    // Another size of font and of picture, with a part of a cell left over
    // at the edges.
    pair.cpu.set_font_size(20.0);
    pair.gpu.set_font_size(20.0);
    let (cell_w, cell_h) = pair.cpu.cell_size();
    pair.editor
        .handle_event(InputEvent::Resize { cols: 40, rows: 8 });
    pair.resize(40 * cell_w + 5, 8 * cell_h + 3);
    pair.check("resized");
    for keys in ["C-a", "C-p", "C-x h", "C-g"] {
        pair.keys(keys);
        pair.check(keys);
    }
}

#[test]
fn more_glyphs_than_the_atlas_holds_are_still_drawn() {
    let Some(mut pair) = Pair::new(80, 30) else {
        return;
    };
    // Large glyphs fill the atlas fast; many different ones overflow it.
    pair.cpu.set_font_size(96.0);
    pair.gpu.set_font_size(96.0);
    let (cell_w, cell_h) = pair.cpu.cell_size();
    pair.editor
        .handle_event(InputEvent::Resize { cols: 30, rows: 10 });
    pair.resize(30 * cell_w, 10 * cell_h);
    let alphabet: String = ('!'..='~').chain('À'..='ž').collect();
    for round in 0..12 {
        let text: String = alphabet
            .chars()
            .cycle()
            .skip(round * 37)
            .take(29 * 8)
            .collect::<Vec<char>>()
            .chunks(29)
            .map(|line| line.iter().collect::<String>() + "\n")
            .collect();
        pair.keys("C-x h C-w");
        pair.editor.handle_event(InputEvent::Paste(text));
        pair.keys("M-<");
        pair.check("a page of large glyphs");
    }
}

/// Times the work a keystroke and a page of scrolling cause in each
/// backend at 4K. Run with
/// `cargo test --release -p amacs-gui --test gpu -- --ignored --nocapture timing`.
///
/// For the CPU that is painting and copying the picture to where the
/// compositor reads it; for the GPU, building the frame and the GPU
/// drawing it. What the compositor then does with either is not counted.
#[test]
#[ignore = "a measurement, not a check"]
fn timing_of_both_backends_at_4k() {
    use std::time::{Duration, Instant};

    let Some(mut pair) = Pair::new(80, 24) else {
        return;
    };
    let (cell_w, cell_h) = pair.cpu.cell_size();
    let (cols, rows) = ((3840 / cell_w) as u16, (2160 / cell_h) as u16);
    pair.editor.handle_event(InputEvent::Resize { cols, rows });
    pair.resize(3840, 2160);
    // Every line is another, so that a page of scrolling changes every row.
    let text: String = (0..usize::from(rows) * 6)
        .map(|n| {
            format!(
                "let value_{n} = compute(a, b) != other && flag >= {n}; // comment text here, \
                 and more of it to fill a wide window with glyphs from edge to edge of a 4K screen\n"
            )
        })
        .collect();
    pair.editor.handle_event(InputEvent::Paste(text));
    pair.keys("M-<");
    pair.check("a full screen");

    let mut canvas = vec![0u8; 3840 * 2160 * 4];
    let mut measure = |pair: &mut Pair, rounds: u32, act: &dyn Fn(&mut Pair, u32)| {
        let (mut cpu, mut gpu) = (Duration::ZERO, Duration::ZERO);
        for round in 0..rounds {
            act(pair, round);
            let ops = pair.editor.redisplay();
            let start = Instant::now();
            pair.cpu.apply(&ops);
            pair.cpu.paint();
            for (bytes, pixel) in canvas
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(pair.cpu.pixels())
            {
                *bytes = pixel.to_le_bytes();
            }
            cpu += start.elapsed();
            let start = Instant::now();
            pair.gpu.apply(&ops);
            pair.gpu.paint();
            pair.gpu.backend().finish().unwrap();
            gpu += start.elapsed();
        }
        (cpu / rounds, gpu / rounds)
    };
    let typed = measure(&mut pair, 200, &|pair, round| {
        let c = char::from(b'a' + (round % 26) as u8);
        pair.editor.handle_event(InputEvent::Key(Key::char(c)));
    });
    let scrolled = measure(&mut pair, 40, &|pair, round| {
        pair.keys(if round % 8 < 4 { "C-v" } else { "M-v" });
    });
    eprintln!(
        "3840x2160, {cols}x{rows} cells, on {}\n  keystroke: CPU {:?}, GPU {:?}\n  page scroll: CPU {:?}, GPU {:?}",
        pair.gpu.backend().adapter(),
        typed.0,
        typed.1,
        scrolled.0,
        scrolled.1,
    );
}
