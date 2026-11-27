//! Pictures in buffers.
//!
//! A picture is of a file and is anchored at a position of a buffer. It is
//! no text: it is drawn in rows of its own under the line that its anchor
//! is on, in frames whose frontend draws pictures, and takes whole cells.
//! The file is read and decoded on a thread, scaled down to the cells that
//! it gets, and sent to a frontend once.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::SystemTime;

use amacs_proto::ImageId;

use crate::editor::Editor;

/// No larger file is read as a picture.
const MAX_FILE: u64 = 32 << 20;
/// No picture of more pixels is decoded: what a file says of its size
/// is from whoever made the file.
const MAX_PIXELS: u64 = 64 << 20;
/// The longer side that a decoded picture is kept with, in pixels. More
/// than a window shows, and little enough to scale again at once.
const KEEP_SIDE: u32 = 2048;
/// How many scaled pictures are kept beyond those that are shown.
const KEEP_SCALED: usize = 64;
/// The rows a picture gets at most, where the code that showed it did
/// not say.
pub(crate) const DEFAULT_MAX_ROWS: usize = 16;

/// Pixels, four bytes each of red, green, blue and opacity, by rows from
/// the top.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// A picture in a buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferImage {
    pub id: u64,
    /// The char that it is anchored at. It is shown under that char's line.
    pub pos: usize,
    pub file: PathBuf,
    /// The rows and columns that it gets at most. Without a number of
    /// columns, as many as the window has.
    pub max_rows: usize,
    pub max_cols: Option<usize>,
    /// What it is drawn as in the frame that was last laid out, if it is.
    pub placed: Option<Placed>,
}

/// The cells that a picture takes, and the scaled picture that fills them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placed {
    pub image: ImageId,
    pub cols: u16,
    pub rows: u16,
}

impl BufferImage {
    /// The rows that the picture takes under its line.
    pub fn rows(&self) -> usize {
        self.placed.map_or(0, |placed| usize::from(placed.rows))
    }
}

enum Source {
    /// Being read on a thread.
    Loading,
    Ready(Arc<Picture>, Option<SystemTime>),
    /// Not a picture that can be shown. Said once, when it was found.
    Failed(Option<SystemTime>),
}

struct Scaled {
    id: ImageId,
    picture: Arc<Picture>,
    /// The layout that last used it.
    used: u64,
}

/// The pictures that are read, and those scaled for the frames.
pub(crate) struct ImageStore {
    sender: mpsc::Sender<(PathBuf, Result<Picture, String>)>,
    done: mpsc::Receiver<(PathBuf, Result<Picture, String>)>,
    sources: HashMap<PathBuf, Source>,
    scaled: HashMap<(PathBuf, u32, u32), Scaled>,
    by_id: HashMap<ImageId, (PathBuf, u32, u32)>,
    next_image: u32,
    next_handle: u64,
    layout: u64,
}

impl Default for ImageStore {
    fn default() -> Self {
        let (sender, done) = mpsc::channel();
        Self {
            sender,
            done,
            sources: HashMap::new(),
            scaled: HashMap::new(),
            by_id: HashMap::new(),
            next_image: 0,
            next_handle: 0,
            layout: 0,
        }
    }
}

impl ImageStore {
    /// The pixels of a scaled picture, to send to a frontend.
    pub fn picture(&self, id: ImageId) -> Option<&Arc<Picture>> {
        let key = self.by_id.get(&id)?;
        self.scaled.get(key).map(|scaled| &scaled.picture)
    }

    /// The picture of a file at a size, scaled now if it was not before.
    fn scaled(&mut self, file: &Path, source: &Arc<Picture>, width: u32, height: u32) -> ImageId {
        let key = (file.to_owned(), width, height);
        if let Some(scaled) = self.scaled.get_mut(&key) {
            scaled.used = self.layout;
            return scaled.id;
        }
        let id = ImageId(self.next_image);
        self.next_image += 1;
        let picture = if (source.width, source.height) == (width, height) {
            Arc::clone(source)
        } else {
            Arc::new(scale(source, width, height))
        };
        self.by_id.insert(id, key.clone());
        self.scaled.insert(
            key,
            Scaled {
                id,
                picture,
                used: self.layout,
            },
        );
        id
    }

    /// Forgets scaled pictures that no layout has used for a while, when
    /// there are many.
    fn trim(&mut self) {
        if self.scaled.len() <= KEEP_SCALED {
            return;
        }
        let layout = self.layout;
        let by_id = &mut self.by_id;
        self.scaled.retain(|_, scaled| {
            let keep = scaled.used + 2 > layout;
            if !keep {
                by_id.remove(&scaled.id);
            }
            keep
        });
    }

    /// Forgets what was read of a file, and what was scaled from it.
    fn forget(&mut self, file: &Path) {
        self.sources.remove(file);
        let by_id = &mut self.by_id;
        self.scaled.retain(|key, scaled| {
            let keep = key.0 != file;
            if !keep {
                by_id.remove(&scaled.id);
            }
            keep
        });
    }
}

fn modified(file: &Path) -> Option<SystemTime> {
    std::fs::metadata(file)
        .and_then(|meta| meta.modified())
        .ok()
}

/// Reads a file as a picture, no larger than is kept.
fn read(file: &Path) -> Result<Picture, String> {
    let size = std::fs::metadata(file)
        .map_err(|err| err.to_string())?
        .len();
    if size > MAX_FILE {
        return Err("the file is too large for a picture".to_owned());
    }
    let bytes = std::fs::read(file).map_err(|err| err.to_string())?;
    let picture = decode(&bytes)?;
    let longer = picture.width.max(picture.height);
    if longer <= KEEP_SIDE {
        return Ok(picture);
    }
    let side = |side: u32| ((u64::from(side) * u64::from(KEEP_SIDE)) / u64::from(longer)).max(1);
    Ok(scale(
        &picture,
        side(picture.width) as u32,
        side(picture.height) as u32,
    ))
}

fn check_size(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("the picture is empty".to_owned());
    }
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err("the picture has too many pixels".to_owned());
    }
    Ok(())
}

/// Decodes a PNG, JPEG, WebP or GIF file. Of a picture that moves, the
/// first frame.
pub fn decode(bytes: &[u8]) -> Result<Picture, String> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        decode_png(bytes)
    } else if bytes.starts_with(&[0xff, 0xd8]) {
        decode_jpeg(bytes)
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        decode_webp(bytes)
    } else if bytes.starts_with(b"GIF8") {
        decode_gif(bytes)
    } else {
        Err("not a PNG, JPEG, WebP or GIF picture".to_owned())
    }
}

fn decode_png(bytes: &[u8]) -> Result<Picture, String> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|err| err.to_string())?;
    let (width, height) = (reader.info().width, reader.info().height);
    check_size(width, height)?;
    let size = reader
        .output_buffer_size()
        .ok_or("the picture has too many pixels")?;
    let mut data = vec![0; size];
    let info = reader
        .next_frame(&mut data)
        .map_err(|err| err.to_string())?;
    data.truncate(info.buffer_size());
    let pixels = width as usize * height as usize;
    let rgba = match info.color_type {
        png::ColorType::Rgba => data,
        png::ColorType::Rgb => data
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::Grayscale => data.iter().flat_map(|g| [*g, *g, *g, 255]).collect(),
        png::ColorType::GrayscaleAlpha => data
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Indexed => return Err("the picture's colours are not known".to_owned()),
    };
    if rgba.len() != pixels * 4 {
        return Err("the picture is not of its size".to_owned());
    }
    Ok(Picture {
        width,
        height,
        rgba,
    })
}

fn decode_jpeg(bytes: &[u8]) -> Result<Picture, String> {
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;

    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(bytes, options);
    decoder.decode_headers().map_err(|err| err.to_string())?;
    let info = decoder.info().ok_or("the picture has no size")?;
    let (width, height) = (u32::from(info.width), u32::from(info.height));
    check_size(width, height)?;
    let rgba = decoder.decode().map_err(|err| err.to_string())?;
    if rgba.len() != width as usize * height as usize * 4 {
        return Err("the picture is not of its size".to_owned());
    }
    Ok(Picture {
        width,
        height,
        rgba,
    })
}

fn decode_webp(bytes: &[u8]) -> Result<Picture, String> {
    let mut decoder =
        image_webp::WebPDecoder::new(Cursor::new(bytes)).map_err(|err| err.to_string())?;
    let (width, height) = decoder.dimensions();
    check_size(width, height)?;
    let size = decoder
        .output_buffer_size()
        .ok_or("the picture has too many pixels")?;
    let mut data = vec![0; size];
    decoder
        .read_image(&mut data)
        .map_err(|err| err.to_string())?;
    let rgba = if decoder.has_alpha() {
        data
    } else {
        data.as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect()
    };
    if rgba.len() != width as usize * height as usize * 4 {
        return Err("the picture is not of its size".to_owned());
    }
    Ok(Picture {
        width,
        height,
        rgba,
    })
}

fn decode_gif(bytes: &[u8]) -> Result<Picture, String> {
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = options
        .read_info(Cursor::new(bytes))
        .map_err(|err| err.to_string())?;
    let (width, height) = (u32::from(decoder.width()), u32::from(decoder.height()));
    check_size(width, height)?;
    let frame = decoder
        .read_next_frame()
        .map_err(|err| err.to_string())?
        .ok_or("the picture has no frame")?;
    // The frame is a part of the whole picture, which is clear around it.
    let mut rgba = vec![0; width as usize * height as usize * 4];
    let (left, top) = (usize::from(frame.left), usize::from(frame.top));
    let (frame_w, frame_h) = (usize::from(frame.width), usize::from(frame.height));
    if frame.buffer.len() != frame_w * frame_h * 4 {
        return Err("the picture is not of its size".to_owned());
    }
    for y in 0..frame_h.min((height as usize).saturating_sub(top)) {
        let w = frame_w.min((width as usize).saturating_sub(left));
        let from = y * frame_w * 4;
        let to = ((top + y) * width as usize + left) * 4;
        rgba[to..to + w * 4].copy_from_slice(&frame.buffer[from..from + w * 4]);
    }
    Ok(Picture {
        width,
        height,
        rgba,
    })
}

/// A picture made smaller: each pixel is the mean of those that it is
/// made of, weighted by how opaque they are.
pub fn scale(picture: &Picture, width: u32, height: u32) -> Picture {
    let (width, height) = (width.max(1), height.max(1));
    let (from_w, from_h) = (picture.width as usize, picture.height as usize);
    let (to_w, to_h) = (width as usize, height as usize);
    let mut rgba = Vec::with_capacity(to_w * to_h * 4);
    // The pixels that one of the new ones is made of: at least one.
    let span = |index: usize, to: usize, from: usize| {
        let first = index * from / to;
        let last = ((index + 1) * from / to).max(first + 1).min(from);
        first.min(from - 1)..last
    };
    for y in 0..to_h {
        let rows = span(y, to_h, from_h);
        for x in 0..to_w {
            let cols = span(x, to_w, from_w);
            let (mut r, mut g, mut b, mut a) = (0u64, 0u64, 0u64, 0u64);
            for row in rows.clone() {
                let line = &picture.rgba[(row * from_w + cols.start) * 4..];
                for pixel in line.as_chunks::<4>().0.iter().take(cols.len()) {
                    let alpha = u64::from(pixel[3]);
                    r += u64::from(pixel[0]) * alpha;
                    g += u64::from(pixel[1]) * alpha;
                    b += u64::from(pixel[2]) * alpha;
                    a += alpha;
                }
            }
            let count = (rows.len() * cols.len()) as u64;
            // Where nothing is seen there is no colour to give.
            let mean = |sum: u64| sum.checked_div(a).unwrap_or(0) as u8;
            rgba.extend_from_slice(&[mean(r), mean(g), mean(b), (a / count) as u8]);
        }
    }
    Picture {
        width,
        height,
        rgba,
    }
}

/// The size in pixels and in cells of a picture that is to fit in so many
/// columns and rows of cells of a size, no larger than it is.
pub(crate) fn fit(
    picture: (u32, u32),
    cell: (u16, u16),
    max_cols: usize,
    max_rows: usize,
) -> ((u32, u32), (u16, u16)) {
    let (cell_w, cell_h) = (u64::from(cell.0.max(1)), u64::from(cell.1.max(1)));
    let (width, height) = (u64::from(picture.0.max(1)), u64::from(picture.1.max(1)));
    let room_w = cell_w * max_cols.clamp(1, usize::from(u16::MAX)) as u64;
    let room_h = cell_h * max_rows.clamp(1, usize::from(u16::MAX)) as u64;
    // Whichever of the two sides fits the worse decides the scale.
    let (w, h) = if width <= room_w && height <= room_h {
        (width, height)
    } else if width * room_h > height * room_w {
        (room_w, (height * room_w / width).max(1))
    } else {
        ((width * room_h / height).max(1), room_h)
    };
    let cols = w.div_ceil(cell_w) as u16;
    let rows = h.div_ceil(cell_h) as u16;
    ((w as u32, h as u32), (cols, rows))
}

impl Editor {
    /// Shows the picture of a file under the line of a position of the
    /// current buffer. Returns a number by which it is removed again.
    pub fn add_image(
        &mut self,
        pos: usize,
        file: &Path,
        max_rows: Option<usize>,
        max_cols: Option<usize>,
    ) -> u64 {
        // A file that was written again since it was read is read again.
        let now = modified(file);
        let stale = match self.images.sources.get(file) {
            Some(Source::Ready(_, then) | Source::Failed(then)) => *then != now,
            Some(Source::Loading) | None => false,
        };
        if stale {
            self.images.forget(file);
        }
        let id = self.images.next_handle;
        self.images.next_handle += 1;
        let pos = pos.min(self.buffer().len());
        self.buffer_mut().add_image(BufferImage {
            id,
            pos,
            file: file.to_owned(),
            max_rows: max_rows.unwrap_or(DEFAULT_MAX_ROWS).max(1),
            max_cols: max_cols.map(|cols| cols.max(1)),
            placed: None,
        });
        id
    }

    /// Whether the current frame's frontend draws pictures.
    pub fn frame_draws_images(&self) -> bool {
        self.frame().cell.is_some()
    }

    /// Reads a file as a picture: on a thread, where there is an event
    /// loop to wake when that is done, and at once where there is none.
    fn load_image(&mut self, file: &Path) {
        if !self.parse_in_background {
            let source = match read(file) {
                Ok(picture) => Source::Ready(Arc::new(picture), modified(file)),
                Err(problem) => {
                    self.log(&format!("{}: {problem}", file.display()));
                    Source::Failed(modified(file))
                }
            };
            self.images.sources.insert(file.to_owned(), source);
            return;
        }
        self.images.sources.insert(file.to_owned(), Source::Loading);
        let (sender, waker) = (self.images.sender.clone(), self.waker.clone());
        let file = file.to_owned();
        std::thread::spawn(move || {
            let outcome = read(&file);
            if sender.send((file, outcome)).is_ok() {
                waker();
            }
        });
    }

    /// Takes in the pictures that threads have read.
    pub(crate) fn images_poll(&mut self) {
        while let Ok((file, outcome)) = self.images.done.try_recv() {
            // A file that was forgotten meanwhile is asked for again.
            if !matches!(self.images.sources.get(&file), Some(Source::Loading)) {
                continue;
            }
            let source = match outcome {
                Ok(picture) => Source::Ready(Arc::new(picture), modified(&file)),
                Err(problem) => {
                    self.log(&format!("{}: {problem}", file.display()));
                    Source::Failed(modified(&file))
                }
            };
            self.images.sources.insert(file, source);
        }
    }

    /// Gives the pictures of the buffers that the current frame shows the
    /// cells that they take in it: none in a frame whose frontend draws no
    /// pictures. Rows are counted with these from here on.
    pub(crate) fn place_images(&mut self) {
        let cell = self.frame().cell;
        let mut used = false;
        for id in self.frame().layout.leaves() {
            let buffer = self.windows[id].buffer;
            if self.buffers[buffer].images().is_empty() {
                continue;
            }
            if !used {
                used = true;
                self.images.layout += 1;
            }
            let width = self.text_width(id).saturating_sub(1).max(1);
            for index in 0..self.buffers[buffer].images().len() {
                let image = &self.buffers[buffer].images()[index];
                let placed = match cell {
                    None => None,
                    Some(cell) => {
                        let file = image.file.clone();
                        let max_cols = image.max_cols.map_or(width, |cols| cols.min(width));
                        let max_rows = image.max_rows;
                        if !self.images.sources.contains_key(&file) {
                            self.load_image(&file);
                        }
                        match self.images.sources.get(&file) {
                            Some(Source::Ready(picture, _)) => {
                                let picture = Arc::clone(picture);
                                let size = (picture.width, picture.height);
                                let ((w, h), (cols, rows)) = fit(size, cell, max_cols, max_rows);
                                let image = self.images.scaled(&file, &picture, w, h);
                                Some(Placed { image, cols, rows })
                            }
                            _ => None,
                        }
                    }
                };
                self.buffers[buffer].place_image(index, placed);
            }
        }
        if used {
            self.images.trim();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_that_fits_keeps_its_size() {
        assert_eq!(fit((40, 20), (8, 16), 80, 16), ((40, 20), (5, 2)));
    }

    #[test]
    fn a_picture_is_scaled_to_the_side_that_fits_worse() {
        // Too wide: 10 columns of 8 pixels.
        assert_eq!(fit((160, 160), (8, 16), 10, 16), ((80, 80), (10, 5)));
        // Too high: 2 rows of 16 pixels.
        assert_eq!(fit((160, 160), (8, 16), 80, 2), ((32, 32), (4, 2)));
    }

    #[test]
    fn scaling_takes_the_mean_of_what_is_seen() {
        let picture = Picture {
            width: 2,
            height: 2,
            rgba: vec![
                255, 0, 0, 255, 255, 0, 0, 255, //
                0, 0, 0, 0, 0, 0, 0, 0,
            ],
        };
        let small = scale(&picture, 1, 1);
        // The clear pixels have no colour to give: only their opacity counts.
        assert_eq!(small.rgba, [255, 0, 0, 127]);
    }

    #[test]
    fn what_is_no_picture_is_refused() {
        assert!(decode(b"hello").is_err());
        assert!(decode(b"\x89PNG\r\n\x1a\nnonsense").is_err());
    }
}
