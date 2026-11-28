//! How text maps to screen cells: char widths, columns and wrapped rows.

use amacs_proto::{DisplayLine, FaceId};
use amacs_text::Text;
use std::ops::Range;

use crate::fold::Folds;

/// Number of cells `c` occupies when drawn starting at column `col`.
pub fn char_cells(c: char, col: usize, tab_width: usize) -> usize {
    match c {
        '\t' => tab_width - col % tab_width,
        c if c.is_control() => 2,
        c => amacs_proto::char_cells(c),
    }
}

/// Number of cells a string without tabs occupies when drawn.
pub fn string_cells(text: &str) -> usize {
    text.chars().map(|c| char_cells(c, 0, 1)).sum()
}

/// Appends what is drawn for `c`: spaces for a tab, `^X` for a control char.
pub fn push_glyph(out: &mut String, c: char, cells: usize) {
    match c {
        '\t' => out.extend(std::iter::repeat_n(' ', cells)),
        c if c.is_ascii_control() => {
            out.push('^');
            out.push(char::from(c as u8 ^ 0x40));
        }
        // C1 controls have no caret form.
        c if c.is_control() => out.push_str("^?"),
        c => out.push(c),
    }
}

/// Display width of a string that contains no tabs.
pub fn str_cells(s: &str) -> usize {
    s.chars().map(|c| char_cells(c, 0, 1)).sum()
}

/// The minor mode in which long lines are broken between words.
pub const VISUAL_LINE_MODE: &str = "visual-line-mode";

/// The minor mode in which markup is left out of what is drawn.
pub const HIDE_MARKUP_MODE: &str = "hide-markup-mode";

/// The minor mode in which text is drawn indented under its heading.
pub const ORG_INDENT_MODE: &str = "org-indent-mode";

/// The option that says how many cells a level of headings indents by,
/// in that mode, and what it is where nothing says.
pub const ORG_INDENT_STEP: &str = "org-indent-step";
pub const DEFAULT_INDENT_STEP: usize = 2;

/// How the text of a buffer flows into rows: how wide its tabs are,
/// whether a line too long for the pane is broken between words, as
/// `visual-line-mode` has it, or wherever the pane ends, and which lines
/// are hidden and get no rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flow<'a> {
    pub tab_width: usize,
    pub words: bool,
    pub folds: &'a Folds,
    /// The pictures shown under lines, in the order of their anchors,
    /// each with the rows that it takes.
    pub images: &'a [crate::image::BufferImage],
    /// Chars within lines that get no cells: markup, while it is hidden.
    /// Ranges in order and apart, none across the end of a line.
    pub hidden: &'a [Range<usize>],
    /// The chars among which nothing is left out, from the first to before
    /// the second: the line that point is
    /// on in the window that the rows are for, so that what is edited is
    /// seen as it is written.
    pub reveal: (usize, usize),
    /// The headings that text is drawn indented under: the char that each
    /// starts at and its level from 1, in order. Empty for no indentation.
    pub headings: &'a [(usize, usize)],
    /// How many cells a level of headings indents the text under it by.
    pub indent_step: usize,
}

impl Flow<'_> {
    /// How many cells the row that starts at a position is indented by,
    /// in a pane of some width: the rows of a heading by a cell for each
    /// level above its own, so that the last of its stars is under the
    /// text of the heading above, and the rows of text under a heading as
    /// far as the text of the heading. Never by more than half the pane.
    pub fn indent(&self, text: &Text, start: usize, width: usize) -> usize {
        let index = self.headings.partition_point(|(at, _)| *at <= start);
        let Some((at, level)) = index.checked_sub(1).map(|index| self.headings[index]) else {
            return 0;
        };
        let cells = if text.line_of(start.min(text.len())) == text.line_of(at.min(text.len())) {
            level - 1
        } else {
            self.indent_step * level
        };
        cells.min(width / 2)
    }
}

/// Goes over the chars of a row and says which of them are left out.
struct Hidden<'a> {
    ranges: &'a [Range<usize>],
    reveal: (usize, usize),
    next: usize,
}

impl<'a> Hidden<'a> {
    fn from(flow: &Flow<'a>, start: usize) -> Self {
        Self {
            ranges: flow.hidden,
            reveal: flow.reveal,
            next: flow.hidden.partition_point(|range| range.end <= start),
        }
    }

    /// Whether the char at a position is left out. Positions are asked
    /// about in rising order.
    fn at(&mut self, pos: usize) -> bool {
        while self
            .ranges
            .get(self.next)
            .is_some_and(|range| range.end <= pos)
        {
            self.next += 1;
        }
        self.ranges.get(self.next).is_some_and(|range| {
            range.start <= pos && !(self.reveal.0 <= pos && pos < self.reveal.1)
        })
    }
}

static NO_FOLDS: Folds = Folds::new();

impl Flow<'static> {
    /// Rows that break wherever the pane ends, of text with no line hidden.
    pub fn chars(tab_width: usize) -> Self {
        Self {
            tab_width,
            words: false,
            folds: &NO_FOLDS,
            images: &[],
            hidden: &[],
            reveal: (0, 0),
            headings: &[],
            indent_step: DEFAULT_INDENT_STEP,
        }
    }
}

/// One screen row of a wrapped line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub start: usize,
    /// Just after the last char drawn on this row.
    pub end: usize,
    /// Where the following row starts, or `None` at the end of the text.
    pub next: Option<usize>,
    /// The line continues on the next row.
    pub continued: bool,
    /// Lines are hidden after this row, which ends its line.
    pub folded: bool,
    /// The rows of pictures that follow this row, which ends its line:
    /// those of the pictures that are anchored in the line.
    pub extra: usize,
}

impl Row {
    /// Whether the cursor is drawn on this row when point is at `pos`. A
    /// position at a wrap boundary belongs to the following row.
    pub fn contains(&self, pos: usize) -> bool {
        pos >= self.start && (pos < self.end || (pos == self.end && !self.continued))
    }
}

/// Lays out the row starting at `start` in a pane `width` cells wide, calling
/// `visit(pos, char, column, cells)` for each char drawn on it.
///
/// The last column is reserved for the continuation marker, so a row holds at
/// most `width - 1` cells of text and the end of a line always has a cell for
/// the cursor. Columns are counted from the row start, so a tab on a
/// continuation row may be narrower than on an unwrapped line.
///
/// A char wider than the whole row is still visited, alone on its row, so
/// that layout always makes progress. The caller must not draw it.
///
/// When the flow breaks between words, a line that does not fit ends its
/// row after the last whitespace that does, which stays on that row; a
/// word longer than the row is broken where the pane ends.
pub fn walk_row(
    text: &Text,
    start: usize,
    width: usize,
    flow: Flow,
    mut visit: impl FnMut(usize, char, usize, usize),
) -> Row {
    let budget = width.saturating_sub(1).max(1);
    let tab_width = flow.tab_width;
    let word_break = if flow.words {
        word_break(text, start, budget, &flow)
    } else {
        None
    };
    let mut hidden = Hidden::from(&flow, start);
    // The cells of the indentation come before those of the text.
    let indent = flow.indent(text, start, width);
    let mut col = indent;
    let mut pos = start;
    for c in text.chars_from(start) {
        // A row that is broken between words ends before the word, and
        // so before markup that the word starts with and that is left out.
        if c != '\n' && word_break == Some(pos) {
            return Row {
                start,
                end: pos,
                next: Some(pos),
                continued: true,
                folded: false,
                extra: 0,
            };
        }
        if c != '\n' && hidden.at(pos) {
            pos += 1;
            continue;
        }
        if c == '\n' {
            // The row after this one is that of the next line that is
            // shown, if there is one.
            let hidden_to = flow.folds.after(pos);
            return Row {
                start,
                end: pos,
                next: match hidden_to {
                    Some(end) => (end < text.len()).then_some(end + 1),
                    None => Some(pos + 1),
                },
                continued: false,
                folded: hidden_to.is_some(),
                extra: picture_rows(text, pos, &flow),
            };
        }
        // A tab never needs more than a whole row.
        let cells = match c {
            '\t' => char_cells(c, col - indent, tab_width).min(budget),
            _ => char_cells(c, col, tab_width),
        };
        if word_break == Some(pos) || (col + cells > budget && col > indent) {
            return Row {
                start,
                end: pos,
                next: Some(pos),
                continued: true,
                folded: false,
                extra: 0,
            };
        }
        visit(pos, c, col, cells);
        col += cells;
        pos += 1;
    }
    Row {
        start,
        end: pos,
        next: None,
        continued: false,
        folded: false,
        extra: picture_rows(text, pos, &flow),
    }
}

/// The rows that the pictures take which are anchored in the line that
/// ends at `line_end`.
fn picture_rows(text: &Text, line_end: usize, flow: &Flow) -> usize {
    if flow.images.is_empty() {
        return 0;
    }
    line_images(text, line_end, flow.images)
        .iter()
        .map(crate::image::BufferImage::rows)
        .sum()
}

/// The pictures that are anchored in the line that ends at `line_end`.
pub fn line_images<'a>(
    text: &Text,
    line_end: usize,
    images: &'a [crate::image::BufferImage],
) -> &'a [crate::image::BufferImage] {
    let line_start = text.line_start(text.line_of(line_end));
    let first = images.partition_point(|image| image.pos < line_start);
    let after = images.partition_point(|image| image.pos <= line_end);
    &images[first..after.max(first)]
}

/// Where the row that starts at `start` is broken between words: just
/// after the last whitespace that fits in `budget` cells, if the line does
/// not fit and has whitespace to break at.
fn word_break(text: &Text, start: usize, budget: usize, flow: &Flow) -> Option<usize> {
    let tab_width = flow.tab_width;
    let mut hidden = Hidden::from(flow, start);
    let indent = flow.indent(text, start, budget + 1);
    let mut col = indent;
    let mut pos = start;
    let mut last = None;
    for c in text.chars_from(start) {
        if c == '\n' {
            return None;
        }
        if hidden.at(pos) {
            pos += 1;
            continue;
        }
        let cells = match c {
            '\t' => char_cells(c, col - indent, tab_width).min(budget),
            _ => char_cells(c, col, tab_width),
        };
        if col + cells > budget && col > indent {
            return last;
        }
        col += cells;
        pos += 1;
        if c.is_whitespace() {
            last = Some(pos);
        }
    }
    None
}

pub(crate) fn row_at(text: &Text, start: usize, width: usize, flow: Flow) -> Row {
    walk_row(text, start, width, flow, |_, _, _, _| {})
}

/// Start of the screen row on which `pos` is drawn, and that row's index
/// within its line. A position on a hidden line is drawn at the end of the
/// line before the hidden ones.
pub fn row_containing(text: &Text, pos: usize, width: usize, flow: Flow) -> (usize, usize) {
    let pos = flow.folds.visible(pos);
    let mut start = text.line_start(text.line_of(pos));
    let mut index = 0;
    loop {
        let row = row_at(text, start, width, flow);
        match row.next {
            Some(next) if row.continued && !row.contains(pos) => {
                start = next;
                index += 1;
            }
            _ => return (start, index),
        }
    }
}

/// The first row of a window: the start of a row of text, and how many
/// rows of that row and the pictures under it are above the window. With
/// 0 the window starts with the text of the row.
pub type Top = (usize, usize);

/// The row `n` rows after `from`, stopping at the last row. The rows of
/// pictures count as those of text do.
pub fn rows_forward(text: &Text, from: Top, n: usize, width: usize, flow: Flow) -> Top {
    let (mut start, skip) = from;
    let mut row = row_at(text, start, width, flow);
    let mut skip = skip.min(row.extra);
    for _ in 0..n {
        if skip < row.extra {
            skip += 1;
            continue;
        }
        match row.next {
            Some(next) => {
                start = next;
                skip = 0;
                row = row_at(text, start, width, flow);
            }
            None => break,
        }
    }
    (start, skip)
}

/// The row `n` rows before `from`, stopping at the first row.
pub fn rows_back(text: &Text, from: Top, n: usize, width: usize, flow: Flow) -> Top {
    let row_start = flow.folds.visible(from.0);
    if from.1 > 0 {
        let skip = from.1.min(row_at(text, row_start, width, flow).extra);
        if skip >= n {
            return (row_start, skip - n);
        }
        return rows_back(text, (row_start, 0), n - skip, width, flow);
    }
    let mut line = text.line_of(row_start);
    let (_, index) = row_containing(text, row_start, width, flow);
    if index >= n {
        return rows_forward(text, (text.line_start(line), 0), index - n, width, flow);
    }
    let mut remaining = n - index;
    while let Some(before) = flow.folds.previous_line(text, line) {
        line = before;
        let line_start = text.line_start(line);
        let (last, last_index) = row_containing(text, text.line_end(line), width, flow);
        // The pictures of a line are under the last of its rows.
        let rows = last_index + 1 + row_at(text, last, width, flow).extra;
        if rows >= remaining {
            let index = rows - remaining;
            return if index <= last_index {
                rows_forward(text, (line_start, 0), index, width, flow)
            } else {
                (last, index - last_index)
            };
        }
        remaining -= rows;
    }
    (0, 0)
}

/// The first position whose row of text a window that starts at `top`
/// shows: the row after the first, where the window starts among that
/// one's pictures and there is a row after it.
pub fn first_text(text: &Text, top: Top, width: usize, flow: Flow) -> usize {
    if top.1 == 0 {
        return top.0;
    }
    let row = row_at(text, top.0, width, flow);
    match row.next {
        Some(next) if row.extra > 0 => next,
        _ => top.0,
    }
}

/// A row of text with what a window shows of it and of its pictures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    pub row: Row,
    /// The row of the window that the text is on, unless it is above the
    /// window.
    pub at: Option<usize>,
    /// The rows of the window that its pictures are on.
    pub pictures: Range<usize>,
    /// Which of the rows of its pictures the first of those is.
    pub first_picture: usize,
}

/// What a window of `rows` rows shows from `top`: the rows of text in
/// order, each with where it and its pictures are.
pub fn shown_rows(text: &Text, top: Top, rows: usize, width: usize, flow: Flow) -> Vec<Shown> {
    let mut shown = Vec::new();
    let (mut start, mut skip) = top;
    let mut at = 0;
    while at < rows {
        let row = row_at(text, start, width, flow);
        let skip_here = skip.min(row.extra);
        let text_at = (skip_here == 0).then_some(at);
        if skip_here == 0 {
            at += 1;
        }
        let first_picture = skip_here.saturating_sub(1);
        let pictures = at..(at + row.extra - first_picture).min(rows);
        at = pictures.end.max(at);
        let next = row.next;
        shown.push(Shown {
            row,
            at: text_at,
            pictures,
            first_picture,
        });
        match next {
            Some(next) => start = next,
            None => break,
        }
        skip = 0;
    }
    shown
}

/// Display column of `pos`, counted from the start of its line.
pub fn column_of(text: &Text, pos: usize, tab_width: usize) -> usize {
    let start = text.line_start(text.line_of(pos));
    text.chars_from(start)
        .take(pos - start)
        .fold(0, |col, c| col + char_cells(c, col, tab_width))
}

/// The position on `line` closest to display column `goal` without passing it.
pub fn pos_at_column(text: &Text, line: usize, goal: usize, tab_width: usize) -> usize {
    let mut pos = text.line_start(line);
    let mut col = 0;
    for c in text.chars_from(pos) {
        if c == '\n' {
            break;
        }
        let next = col + char_cells(c, col, tab_width);
        if next > goal {
            break;
        }
        col = next;
        pos += 1;
    }
    pos
}

/// Replaces `width` cells of a line from column `col` with `content`,
/// padded with spaces in `fill`. The rest of the line stays as it is.
pub(crate) fn overlay_cells(
    line: &mut DisplayLine,
    col: usize,
    width: usize,
    content: &[(char, FaceId)],
    fill: FaceId,
) {
    // The line as cells: a wide char is its char and then a filler.
    let mut cells: Vec<(Option<char>, FaceId)> = Vec::new();
    for (segment, face) in line.segments() {
        for c in segment.chars() {
            match amacs_proto::char_cells(c) {
                0 => {}
                cells_wide => {
                    cells.push((Some(c), face));
                    cells.extend(std::iter::repeat_n((None, face), cells_wide - 1));
                }
            }
        }
    }
    let line_fill = line.fill;
    if cells.len() < col + width {
        cells.resize(col + width, (Some(' '), line_fill));
    }
    // A wide char cut in half by either edge becomes spaces.
    if col > 0 && cells[col].0.is_none() {
        let mut head = col - 1;
        while cells[head].0.is_none() && head > 0 {
            head -= 1;
        }
        for cell in &mut cells[head..col] {
            cell.0 = Some(' ');
        }
    }
    let mut after = col + width;
    while after < cells.len() && cells[after].0.is_none() {
        cells[after].0 = Some(' ');
        after += 1;
    }

    let mut boxed: Vec<(Option<char>, FaceId)> = Vec::with_capacity(width);
    for (c, face) in content {
        let wide = amacs_proto::char_cells(*c);
        if wide == 0 || boxed.len() + wide > width {
            break;
        }
        boxed.push((Some(*c), *face));
        boxed.extend(std::iter::repeat_n((None, *face), wide - 1));
    }
    boxed.resize(width, (Some(' '), fill));
    cells.splice(col..col + width, boxed);

    let mut rebuilt = DisplayLine {
        fill: line_fill,
        ..DisplayLine::default()
    };
    let mut buffer = [0u8; 4];
    for (c, face) in cells {
        if let Some(c) = c {
            rebuilt.push(c.encode_utf8(&mut buffer), face);
        }
    }
    *line = rebuilt;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_wrap_one_cell_short_of_the_width() {
        let text = Text::from("abcdefgh\nxy");
        let first = row_at(&text, 0, 5, Flow::chars(8));
        assert_eq!((first.end, first.next, first.continued), (4, Some(4), true));
        let second = row_at(&text, 4, 5, Flow::chars(8));
        assert_eq!(
            (second.end, second.next, second.continued),
            (8, Some(9), false)
        );
        let last = row_at(&text, 9, 5, Flow::chars(8));
        assert_eq!((last.end, last.next), (11, None));
    }

    #[test]
    fn wrap_boundary_belongs_to_the_next_row() {
        let text = Text::from("abcdefgh");
        assert_eq!(row_containing(&text, 3, 5, Flow::chars(8)), (0, 0));
        assert_eq!(row_containing(&text, 4, 5, Flow::chars(8)), (4, 1));
        assert_eq!(row_containing(&text, 8, 5, Flow::chars(8)), (4, 1));
    }

    #[test]
    fn rows_back_crosses_lines() {
        // Rows at width 5: "abcd" "efgh" | "xy" | "" (after the last newline).
        let text = Text::from("abcdefgh\nxy\n");
        let last = text.len();
        assert_eq!(rows_back(&text, (last, 0), 1, 5, Flow::chars(8)).0, 9);
        assert_eq!(rows_back(&text, (last, 0), 2, 5, Flow::chars(8)).0, 4);
        assert_eq!(rows_back(&text, (last, 0), 3, 5, Flow::chars(8)).0, 0);
        assert_eq!(rows_back(&text, (last, 0), 99, 5, Flow::chars(8)).0, 0);
        assert_eq!(rows_back(&text, (4, 0), 1, 5, Flow::chars(8)).0, 0);
        assert_eq!(rows_forward(&text, (0, 0), 2, 5, Flow::chars(8)).0, 9);
        assert_eq!(rows_forward(&text, (0, 0), 99, 5, Flow::chars(8)).0, last);
    }

    #[test]
    fn rows_break_between_words_when_the_flow_says_so() {
        let words = Flow {
            words: true,
            ..Flow::chars(8)
        };
        // Nine cells of text to a row: "the quick " "brown fox".
        let text = Text::from("the quick brown fox\nend");
        let first = row_at(&text, 0, 10, words);
        assert_eq!((first.end, first.next, first.continued), (4, Some(4), true));
        let text = Text::from("the quick brown fox\nend");
        let first = row_at(&text, 0, 12, words);
        assert_eq!(
            (first.end, first.next, first.continued),
            (10, Some(10), true)
        );
        let second = row_at(&text, 10, 12, words);
        assert_eq!(
            (second.end, second.next, second.continued),
            (19, Some(20), false)
        );
        // A line that fits is left alone, and so is the last of the text.
        assert_eq!(row_at(&text, 20, 12, words).next, None);
        // A word longer than the row is broken where the pane ends.
        let text = Text::from("abcdefghijkl mn");
        let first = row_at(&text, 0, 6, words);
        assert_eq!((first.end, first.continued), (5, true));
        // Rows are counted and walked by the same breaks.
        let text = Text::from("the quick brown fox");
        assert_eq!(row_containing(&text, 12, 12, words), (10, 1));
        assert_eq!(rows_forward(&text, (0, 0), 1, 12, words).0, 10);
        assert_eq!(rows_back(&text, (10, 0), 1, 12, words).0, 0);
    }

    #[test]
    fn columns_account_for_tabs_and_wide_chars() {
        let text = Text::from("a\tb\n日本x");
        assert_eq!(column_of(&text, 2, 8), 8);
        assert_eq!(column_of(&text, 3, 8), 9);
        assert_eq!(pos_at_column(&text, 0, 5, 8), 1);
        assert_eq!(pos_at_column(&text, 0, 8, 8), 2);
        assert_eq!(pos_at_column(&text, 1, 3, 8), 5);
        assert_eq!(pos_at_column(&text, 1, 99, 8), 7);
    }

    #[test]
    fn control_chars_use_caret_notation() {
        let mut out = String::new();
        push_glyph(&mut out, '\u{1}', 2);
        push_glyph(&mut out, '\u{7f}', 2);
        push_glyph(&mut out, '\t', 3);
        assert_eq!(out, "^A^?   ");
    }

    fn line(text: &str) -> DisplayLine {
        DisplayLine::plain(text, FaceId(0))
    }

    fn content(text: &str) -> Vec<(char, FaceId)> {
        text.chars().map(|c| (c, FaceId(9))).collect()
    }

    #[test]
    fn a_box_replaces_cells_of_a_line() {
        let mut row = line("hello world");
        overlay_cells(&mut row, 2, 5, &content("abc"), FaceId(9));
        assert_eq!(row.text, "heabc  orld");
        let faces: Vec<_> = row.runs.iter().map(|run| (run.len, run.face.0)).collect();
        assert_eq!(faces, [(2, 0), (5, 9), (4, 0)]);

        // Past the end of the text the line is padded.
        let mut row = line("ab");
        overlay_cells(&mut row, 4, 3, &content("xyz!"), FaceId(9));
        assert_eq!(row.text, "ab  xyz");
    }

    #[test]
    fn wide_chars_cut_by_a_box_become_spaces() {
        let mut row = line("日本語日本語");
        overlay_cells(&mut row, 3, 4, &content("ab"), FaceId(9));
        assert_eq!(row.text, "日 ab   本語");
        let mut row = line("a日b");
        overlay_cells(&mut row, 0, 2, &content("語x"), FaceId(9));
        assert_eq!(row.text, "語 b", "content that does not fit is cut");
    }
}
