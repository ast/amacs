//! What is laid over the text of a window besides its syntax colours: the
//! current line, other occurrences of the symbol at point, words such as
//! TODO, whitespace that should not be there, diagnostics, and the labels
//! of places to jump to.

use std::collections::HashMap;
use std::ops::Range;

use amacs_proto::FaceId;
use amacs_treesit::{Highlight, Highlights};

use crate::buffer::Buffer;
use crate::display::column_of;
use crate::editor::Editor;
use crate::faces::{self, Faces};
use crate::window::WindowId;

pub const HL_LINE_MODE: &str = "hl-line-mode";
pub const SYMBOL_HIGHLIGHT_MODE: &str = "symbol-highlight-mode";
pub const HL_TODO_MODE: &str = "hl-todo-mode";
pub const WHITESPACE_MODE: &str = "whitespace-mode";

/// Words that stand out in comments.
const TODO_WORDS: &[&str] = &[
    "TODO", "FIXME", "HACK", "XXX", "BUG", "NOTE", "DONE", "FAIL", "TEMP", "REVIEW",
];

/// A key sequence shown over the text, to jump to where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Label {
    pub window: WindowId,
    /// Where in the window's buffer, or `None` for the window itself, whose
    /// label goes in its corner.
    pub pos: Option<usize>,
    /// The keys still to be pressed.
    pub keys: String,
}

/// The labels showing while the user picks a place.
#[derive(Debug, Default)]
pub(crate) struct Labels {
    pub entries: Vec<Label>,
    /// Whether the text under the labels is dimmed.
    pub dim: bool,
}

/// Whether a char is part of a symbol: a word char, or in Lisp also the
/// punctuation that names are made of there.
pub(crate) fn is_symbol_char(c: char, lisp: bool) -> bool {
    c.is_alphanumeric() || c == '_' || (lisp && "-?!*<>=/+:%&$~^.".contains(c))
}

/// The region as columns of lines, when it is a rectangle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rectangle {
    pub lines: Range<usize>,
    pub columns: Range<usize>,
}

/// Everything laid over one window's text.
pub(crate) struct Decorations<'a> {
    faces: &'a Faces,
    /// Faces laid over ranges of chars.
    ranges: Vec<(Range<usize>, FaceId)>,
    /// The line to mark as current.
    hl_line: Option<usize>,
    /// The column that lines should end before, when whitespace is shown.
    whitespace: Option<usize>,
    /// Point, where trailing whitespace is still being typed.
    point: usize,
    pub region: Range<usize>,
    pub rectangle: Option<Rectangle>,
    /// Labels in this window, by position, and whether the text is dimmed.
    pub labels: Vec<(Option<usize>, &'a str)>,
    dim: bool,
    /// Combined faces already looked up, by base face and overlay bits.
    memo: HashMap<(FaceId, u64), FaceId>,
}

/// What is known about a row when its chars are drawn.
pub(crate) struct RowContext {
    /// Index of the line the row is part of.
    pub line: usize,
    /// Display column of the row's first char in its line.
    pub base_column: usize,
    /// Where the line's trailing whitespace starts, and where the line ends.
    pub trailing_from: usize,
    pub line_end: usize,
}

impl Editor {
    fn mode_on(&self, buffer: &Buffer, mode: &str) -> bool {
        buffer.minor_modes.iter().any(|name| name == mode)
            || self
                .script
                .global_minor_modes
                .iter()
                .any(|name| name == mode)
    }

    /// The symbol that point is in or next to, in the current buffer.
    pub(crate) fn symbol_range_at(&self, buffer: &Buffer, point: usize) -> Range<usize> {
        let lisp = buffer
            .language()
            .is_some_and(|language| language.indent == amacs_treesit::IndentStyle::Lisp);
        let text = buffer.text();
        let before = text
            .chars_before(point)
            .take_while(|c| is_symbol_char(*c, lisp))
            .count();
        let after = text
            .chars_from(point)
            .take_while(|c| is_symbol_char(*c, lisp))
            .count();
        point - before..point + after
    }

    /// Works out what is laid over the chars `shown` of a window.
    pub(crate) fn decorations(
        &self,
        id: WindowId,
        shown: Range<usize>,
        highlights: Option<&Highlights>,
    ) -> Decorations<'_> {
        let window = &self.windows[id];
        let buffer = &self.buffers[window.buffer];
        let text = buffer.text();
        let point = buffer.marker(window.point);
        let selected = id == self.selected();
        let mut ranges: Vec<(Range<usize>, FaceId)> = Vec::new();

        // Symbols and words: one pass over what is shown.
        let lisp = buffer
            .language()
            .is_some_and(|language| language.indent == amacs_treesit::IndentStyle::Lisp);
        let symbol = if selected && self.mode_on(buffer, SYMBOL_HIGHLIGHT_MODE) {
            let range = self.symbol_range_at(buffer, point);
            Some(text.slice(range)).filter(|symbol| symbol.chars().count() >= 2)
        } else {
            None
        };
        let todo = self.mode_on(buffer, HL_TODO_MODE);
        if symbol.is_some() || todo {
            let mut word = String::new();
            let (mut word_start, mut word_byte) = (shown.start, 0);
            let mut byte = text.char_to_byte(shown.start);
            let chars = text.chars_from(shown.start).take(shown.len());
            for (offset, c) in chars.chain(Some('\n')).enumerate() {
                if is_symbol_char(c, lisp) {
                    if word.is_empty() {
                        (word_start, word_byte) = (shown.start + offset, byte);
                    }
                    word.push(c);
                } else if !word.is_empty() {
                    let range = word_start..shown.start + offset;
                    if symbol.as_deref() == Some(word.as_str()) {
                        ranges.push((range.clone(), faces::SYMBOL_HIGHLIGHT));
                    }
                    // In code that is highlighted, only comments count.
                    let in_comment = highlights.is_none_or(|found| {
                        matches!(
                            found.get(word_byte),
                            Some(Highlight::Comment | Highlight::Doc)
                        )
                    });
                    if todo && in_comment && TODO_WORDS.contains(&word.as_str()) {
                        ranges.push((range, faces::TODO));
                    }
                    word.clear();
                }
                byte += c.len_utf8();
            }
        }

        // The most serious diagnostic on a char is the one shown, so the
        // less serious go first and are laid under.
        let mut diagnostics: Vec<_> = buffer
            .diagnostics()
            .map(|(range, diagnostic)| {
                (
                    range.start..range.end.max(range.start + 1),
                    diagnostic.severity,
                )
            })
            .filter(|(range, _)| range.start <= shown.end && range.end > shown.start)
            .collect();
        diagnostics.sort_by_key(|(_, severity)| std::cmp::Reverse(*severity));
        ranges.extend(diagnostics.into_iter().map(|(range, severity)| {
            let face = FaceId(faces::DIAGNOSTIC_ERROR.0 + severity as u32);
            (range, face)
        }));

        let region = match buffer.mark() {
            Some(mark) if buffer.mark_active && selected => mark.min(point)..mark.max(point),
            _ => 0..0,
        };
        let rectangle = (buffer.rectangle_mark && !region.is_empty()).then(|| {
            let tab = buffer.tab_width;
            let (a, b) = (
                column_of(text, region.start, tab),
                column_of(text, region.end, tab),
            );
            Rectangle {
                lines: text.line_of(region.start)..text.line_of(region.end) + 1,
                columns: a.min(b)..a.max(b),
            }
        });
        let whitespace = self.mode_on(buffer, WHITESPACE_MODE).then(|| {
            match self.option_for(window.buffer, "whitespace-line-column") {
                Some(crate::script::Value::Int(column)) if column > 0 => column as usize,
                _ => usize::MAX,
            }
        });
        let (labels, dim) = match &self.labels {
            Some(labels) => (
                labels
                    .entries
                    .iter()
                    .filter(|label| label.window == id)
                    .map(|label| (label.pos, label.keys.as_str()))
                    .collect(),
                labels.dim,
            ),
            None => (Vec::new(), false),
        };
        Decorations {
            faces: &self.faces,
            ranges,
            hl_line: self
                .mode_on(buffer, HL_LINE_MODE)
                .then(|| text.line_of(point)),
            whitespace,
            point,
            region,
            rectangle,
            labels,
            dim,
            memo: HashMap::new(),
        }
    }
}

impl Decorations<'_> {
    /// Whether a char is in the region. `column` is its column in its line.
    pub fn in_region(&self, pos: usize, line: usize, column: usize) -> bool {
        match &self.rectangle {
            Some(rectangle) => {
                rectangle.lines.contains(&line) && rectangle.columns.contains(&column)
            }
            None => self.region.contains(&pos),
        }
    }

    /// The face of `base` with `over` laid over it. Only the built-in faces
    /// are laid over text, and those have small ids.
    fn combine(&mut self, base: FaceId, over: &[FaceId]) -> FaceId {
        if over.is_empty() {
            return base;
        }
        let bits = over
            .iter()
            .fold(0u64, |bits, face| bits | 1 << (face.0 % 64));
        let faces = self.faces;
        *self
            .memo
            .entry((base, bits))
            .or_insert_with(|| faces.overlay(base, over))
    }

    /// The face a char is drawn in, given the face its syntax gives it.
    pub fn face(&mut self, base: FaceId, pos: usize, row: &RowContext, column: usize) -> FaceId {
        if self.in_region(pos, row.line, row.base_column + column) {
            return faces::REGION;
        }
        let mut over = [faces::DEFAULT; 12];
        let mut count = 0;
        let mut add = |face: FaceId| {
            if count < over.len() {
                over[count] = face;
                count += 1;
            }
        };
        if self.dim {
            add(faces::JUMP_BACKGROUND);
        }
        if self.hl_line == Some(row.line) {
            add(faces::HL_LINE);
        }
        if let Some(limit) = self.whitespace {
            if row.base_column + column >= limit {
                add(faces::LONG_LINE);
            }
            // Not on the line where point is at the end of it, since
            // there the whitespace is still being typed.
            let typing = (row.trailing_from..=row.line_end).contains(&self.point);
            if pos >= row.trailing_from && !typing {
                add(faces::TRAILING_WHITESPACE);
            }
        }
        for (range, face) in &self.ranges {
            if range.contains(&pos) {
                add(*face);
            }
        }
        self.combine(base, &over[..count])
    }

    /// The face of the cells after the end of a line's text.
    pub fn fill(&mut self, line: usize) -> FaceId {
        if self.hl_line == Some(line) {
            self.combine(faces::DEFAULT, &[faces::HL_LINE])
        } else {
            faces::DEFAULT
        }
    }
}
