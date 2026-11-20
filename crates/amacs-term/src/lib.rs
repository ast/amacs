//! A terminal's screen for the editor.
//!
//! The state machine is Alacritty's (`alacritty_terminal`): bytes from the
//! program go in, and a grid of cells with colours comes out. This crate
//! turns the grid into rows of text with [`FaceAttrs`] spans, which the
//! editor shows in a buffer, keeps the lines that scroll off the top until
//! the editor has copied them, and encodes the keys the user presses as the
//! bytes a terminal would send. It knows nothing about buffers or
//! processes.

mod keys;
mod palette;

use std::ops::Range;
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor, Rgb};
use amacs_proto::{FaceAttrs, Key};

pub use keys::{encode_key, encode_paste};
pub use palette::Palette;

/// How many lines Alacritty keeps for us between two readings of the
/// screen. Feed less than this many lines at a time, or the first of them
/// are lost: see [`Terminal::feed`].
pub const HISTORY: usize = 2000;

/// One row of the screen, or one that scrolled off it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Row {
    /// The text, with the trailing blanks that carry no colour left out.
    pub text: String,
    /// Colours and attributes over ranges of chars of the text, in order
    /// and apart; stretches in the default look have none.
    pub spans: Vec<(Range<usize>, FaceAttrs)>,
}

/// What the program asked of the terminal, for the editor to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermEvent {
    /// Bytes to write back to the program: answers to its questions.
    Write(Vec<u8>),
    Title(String),
    ResetTitle,
    Bell,
    /// Text the program put on the clipboard (OSC 52).
    Clipboard(String),
}

/// Collects what Alacritty reports while bytes are fed.
#[derive(Clone, Default)]
struct Listener(Arc<Mutex<Vec<Event>>>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        if let Ok(mut events) = self.0.lock() {
            events.push(event);
        }
    }
}

/// The size of the screen, as Alacritty asks for it.
#[derive(Debug, Clone, Copy)]
struct Size {
    cols: usize,
    rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

pub struct Terminal {
    term: Term<Listener>,
    parser: Processor,
    listener: Listener,
    palette: Palette,
    size: Size,
}

impl Terminal {
    pub fn new(cols: usize, rows: usize, palette: Palette) -> Self {
        let size = Size {
            cols: cols.max(1),
            rows: rows.max(1),
        };
        let config = Config {
            scrolling_history: HISTORY,
            ..Config::default()
        };
        let listener = Listener::default();
        Self {
            term: Term::new(config, &size, listener.clone()),
            parser: Processor::new(),
            listener,
            palette,
            size,
        }
    }

    pub fn cols(&self) -> usize {
        self.size.cols
    }

    pub fn rows(&self) -> usize {
        self.size.rows
    }

    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
    }

    /// Feeds what the program wrote. Lines that scroll off the top wait in
    /// Alacritty's history until [`Terminal::take_scrolled`] takes them,
    /// and the history holds [`HISTORY`] lines: feed fewer than that many
    /// line breaks between two takings.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    /// The lines that scrolled off the top since the last call, oldest
    /// first. Alacritty forgets them: the caller keeps them.
    pub fn take_scrolled(&mut self) -> Vec<Row> {
        let history = self.term.grid().history_size();
        let rows = (1..=history)
            .rev()
            .map(|back| self.row(Line(-(back as i32))))
            .collect();
        if history > 0 {
            self.term.grid_mut().clear_history();
        }
        rows
    }

    /// The rows of the screen, top to bottom.
    pub fn screen(&self) -> Vec<Row> {
        (0..self.size.rows)
            .map(|line| self.row(Line(line as i32)))
            .collect()
    }

    /// Where the cursor is, as a row and the index of the char it is on in
    /// that row's text, or `None` while the program hides it.
    pub fn cursor(&self) -> Option<(usize, usize)> {
        if !self.term.mode().contains(TermMode::SHOW_CURSOR) {
            return None;
        }
        let point = self.term.grid().cursor.point;
        let row = &self.term.grid()[point.line];
        let chars = (0..point.column.0.min(self.size.cols))
            .filter(|column| !is_spacer(row[Column(*column)].flags))
            .count();
        Some((point.line.0.max(0) as usize, chars))
    }

    /// Whether the program has switched to the alternate screen, as full
    /// screen programs do.
    pub fn alt_screen(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.size = Size {
            cols: cols.max(1),
            rows: rows.max(1),
        };
        self.term.resize(self.size);
    }

    /// The bytes that a key sends to the program, if it sends any.
    pub fn encode_key(&self, key: &Key) -> Option<Vec<u8>> {
        keys::encode_key(key, self.term.mode())
    }

    /// The bytes that pasting text sends to the program.
    pub fn encode_paste(&self, text: &str) -> Vec<u8> {
        keys::encode_paste(text, self.term.mode())
    }

    /// What the program asked for since the last call.
    pub fn take_events(&mut self) -> Vec<TermEvent> {
        let events = match self.listener.0.lock() {
            Ok(mut events) => std::mem::take(&mut *events),
            Err(_) => Vec::new(),
        };
        let mut out = Vec::new();
        for event in events {
            match event {
                Event::PtyWrite(text) => out.push(TermEvent::Write(text.into_bytes())),
                Event::Title(title) => out.push(TermEvent::Title(title)),
                Event::ResetTitle => out.push(TermEvent::ResetTitle),
                Event::Bell => out.push(TermEvent::Bell),
                Event::ClipboardStore(_, text) => out.push(TermEvent::Clipboard(text)),
                // Reading the clipboard is not for programs to do.
                Event::ClipboardLoad(_, format) => {
                    out.push(TermEvent::Write(format("").into_bytes()));
                }
                Event::ColorRequest(index, format) => {
                    let rgb = self.indexed(index).unwrap_or(Rgb { r: 0, g: 0, b: 0 });
                    out.push(TermEvent::Write(format(rgb).into_bytes()));
                }
                Event::TextAreaSizeRequest(format) => {
                    let size = WindowSize {
                        num_lines: self.size.rows as u16,
                        num_cols: self.size.cols as u16,
                        cell_width: 0,
                        cell_height: 0,
                    };
                    out.push(TermEvent::Write(format(size).into_bytes()));
                }
                Event::MouseCursorDirty
                | Event::CursorBlinkingChange
                | Event::Wakeup
                | Event::Exit
                | Event::ChildExit(_) => {}
            }
        }
        out
    }

    /// One row of the grid as text and spans.
    fn row(&self, line: Line) -> Row {
        let grid = self.term.grid();
        let row = &grid[line];
        let mut text = String::new();
        let mut spans: Vec<(Range<usize>, FaceAttrs)> = Vec::new();
        let mut chars = 0;
        // Where the text would end without the trailing blanks that carry
        // no colour.
        let mut kept = (0, 0);
        for column in 0..self.size.cols.min(row.len()) {
            let cell = &row[Column(column)];
            if is_spacer(cell.flags) {
                continue;
            }
            let attrs = self.attrs(cell);
            let blank = cell.c == ' ' && attrs.bg.is_none() && !attrs.inverse;
            text.push(cell.c);
            if let Some(extra) = cell.zerowidth() {
                text.extend(extra);
            }
            let start = chars;
            chars += 1;
            if attrs != FaceAttrs::default() {
                match spans.last_mut() {
                    Some((range, last)) if *last == attrs && range.end == start => {
                        range.end = chars;
                    }
                    _ => spans.push((start..chars, attrs)),
                }
            }
            if !blank {
                kept = (text.len(), chars);
            }
        }
        text.truncate(kept.0);
        spans.retain_mut(|(range, _)| {
            range.end = range.end.min(kept.1);
            range.start < range.end
        });
        Row { text, spans }
    }

    /// The look of a cell, with the default look as `None` colours.
    fn attrs(&self, cell: &alacritty_terminal::term::cell::Cell) -> FaceAttrs {
        let flags = cell.flags;
        let mut fg = self.color(cell.fg);
        let mut bg = self.color(cell.bg);
        if flags.contains(Flags::HIDDEN) {
            fg = bg;
        }
        if flags.contains(Flags::DIM) && !flags.contains(Flags::BOLD) {
            fg = fg.map(dim);
        }
        if flags.contains(Flags::INVERSE) {
            // Inverse video with the default colours is left to the face.
            if fg.is_some() || bg.is_some() {
                std::mem::swap(&mut fg, &mut bg);
            }
        }
        FaceAttrs {
            fg,
            bg,
            bold: flags.contains(Flags::BOLD),
            italic: flags.contains(Flags::ITALIC),
            underline: flags.intersects(Flags::ALL_UNDERLINES),
            inverse: flags.contains(Flags::INVERSE) && fg.is_none() && bg.is_none(),
            underline_color: cell.underline_color().and_then(|color| self.color(color)),
            box_color: None,
        }
    }

    /// A colour of the program as the editor draws it: `None` for the
    /// default foreground or background.
    fn color(&self, color: Color) -> Option<amacs_proto::Color> {
        let rgb = match color {
            Color::Spec(rgb) => rgb,
            Color::Indexed(index) => self.indexed(usize::from(index))?,
            Color::Named(named) => match named {
                NamedColor::Foreground
                | NamedColor::Background
                | NamedColor::Cursor
                | NamedColor::BrightForeground
                | NamedColor::DimForeground => return None,
                NamedColor::DimBlack
                | NamedColor::DimRed
                | NamedColor::DimGreen
                | NamedColor::DimYellow
                | NamedColor::DimBlue
                | NamedColor::DimMagenta
                | NamedColor::DimCyan
                | NamedColor::DimWhite => {
                    let base = named as usize - NamedColor::DimBlack as usize;
                    to_rgb(dim(to_color(self.indexed(base)?)))
                }
                other => self.indexed(other as usize)?,
            },
        };
        Some(to_color(rgb))
    }

    /// A colour of the 256, as the program set it with OSC 4 or as the
    /// palette has it.
    fn indexed(&self, index: usize) -> Option<Rgb> {
        if let Some(rgb) = self.term.colors()[index] {
            return Some(rgb);
        }
        self.palette.indexed(index).map(to_rgb)
    }
}

fn is_spacer(flags: Flags) -> bool {
    flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
}

/// A colour at two thirds of its strength, for dim text.
fn dim(color: amacs_proto::Color) -> amacs_proto::Color {
    amacs_proto::Color::rgb(
        (u16::from(color.r) * 2 / 3) as u8,
        (u16::from(color.g) * 2 / 3) as u8,
        (u16::from(color.b) * 2 / 3) as u8,
    )
}

fn to_color(rgb: Rgb) -> amacs_proto::Color {
    amacs_proto::Color::rgb(rgb.r, rgb.g, rgb.b)
}

fn to_rgb(color: amacs_proto::Color) -> Rgb {
    Rgb {
        r: color.r,
        g: color.g,
        b: color.b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terminal(cols: usize, rows: usize) -> Terminal {
        Terminal::new(cols, rows, Palette::default())
    }

    fn texts(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|row| row.text.as_str()).collect()
    }

    #[test]
    fn text_lands_on_the_screen_and_the_cursor_follows() {
        let mut t = terminal(10, 3);
        t.feed(b"hello\r\nworld");
        assert_eq!(texts(&t.screen()), ["hello", "world", ""]);
        assert_eq!(t.cursor(), Some((1, 5)));
        t.feed(b"\x1b[H");
        assert_eq!(t.cursor(), Some((0, 0)));
        t.feed(b"\x1b[?25l");
        assert_eq!(t.cursor(), None);
    }

    #[test]
    fn lines_that_scroll_off_are_kept_until_taken() {
        let mut t = terminal(10, 2);
        t.feed(b"one\r\ntwo\r\nthree\r\nfour");
        assert_eq!(texts(&t.screen()), ["three", "four"]);
        let scrolled = t.take_scrolled();
        assert_eq!(texts(&scrolled), ["one", "two"]);
        assert!(t.take_scrolled().is_empty());
        t.feed(b"\r\nfive");
        assert_eq!(texts(&t.take_scrolled()), ["three"]);
    }

    #[test]
    fn colours_become_spans_and_trailing_blanks_go() {
        let mut t = terminal(20, 1);
        t.feed(b"\x1b[31mred\x1b[0m plain\x1b[1;4m!\x1b[0m   ");
        let rows = t.screen();
        assert_eq!(rows[0].text, "red plain!");
        assert_eq!(rows[0].spans.len(), 2);
        let (range, red) = &rows[0].spans[0];
        assert_eq!(*range, 0..3);
        assert_eq!(red.fg, Palette::default().indexed(1));
        let (range, bang) = &rows[0].spans[1];
        assert_eq!(*range, 9..10);
        assert!(bang.bold && bang.underline);
    }

    #[test]
    fn a_coloured_background_keeps_its_blanks() {
        let mut t = terminal(10, 1);
        t.feed(b"\x1b[44m   \x1b[0m");
        let rows = t.screen();
        assert_eq!(rows[0].text, "   ");
        assert_eq!(rows[0].spans[0].1.bg, Palette::default().indexed(4));
    }

    #[test]
    fn truecolor_and_indexed_colours() {
        let mut t = terminal(10, 1);
        t.feed(b"\x1b[38;2;1;2;3ma\x1b[48;5;196mb");
        let rows = t.screen();
        assert_eq!(
            rows[0].spans[0].1.fg,
            Some(amacs_proto::Color::rgb(1, 2, 3))
        );
        assert_eq!(
            rows[0].spans[1].1.bg,
            Some(amacs_proto::Color::rgb(255, 0, 0))
        );
    }

    #[test]
    fn wide_characters_count_once_in_the_text() {
        let mut t = terminal(10, 1);
        t.feed("日本x".as_bytes());
        let rows = t.screen();
        assert_eq!(rows[0].text, "日本x");
        assert_eq!(t.cursor(), Some((0, 3)));
    }

    #[test]
    fn questions_from_the_program_are_answered() {
        let mut t = terminal(10, 2);
        // Where is the cursor?
        t.feed(b"\x1b[6n");
        assert_eq!(
            t.take_events(),
            vec![TermEvent::Write(b"\x1b[1;1R".to_vec())]
        );
        t.feed(b"\x1b]2;a title\x07\x07");
        assert_eq!(
            t.take_events(),
            vec![TermEvent::Title("a title".into()), TermEvent::Bell]
        );
    }

    #[test]
    fn resizing_keeps_the_text() {
        let mut t = terminal(10, 3);
        t.feed(b"one\r\ntwo\r\nthree");
        t.resize(10, 2);
        assert_eq!(texts(&t.screen()), ["two", "three"]);
        assert_eq!(texts(&t.take_scrolled()), ["one"]);
        assert!(!t.alt_screen());
    }
}
