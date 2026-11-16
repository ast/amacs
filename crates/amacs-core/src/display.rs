//! How text maps to screen cells: char widths, columns and wrapped rows.

use amacs_text::Text;
use unicode_width::UnicodeWidthChar;

/// Number of cells `c` occupies when drawn starting at column `col`.
pub fn char_cells(c: char, col: usize, tab_width: usize) -> usize {
    match c {
        '\t' => tab_width - col % tab_width,
        c if c.is_control() => 2,
        c => c.width().unwrap_or(1),
    }
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
pub fn walk_row(
    text: &Text,
    start: usize,
    width: usize,
    tab_width: usize,
    mut visit: impl FnMut(usize, char, usize, usize),
) -> Row {
    let budget = width.saturating_sub(1).max(1);
    let mut col = 0;
    let mut pos = start;
    for c in text.chars_from(start) {
        if c == '\n' {
            return Row {
                start,
                end: pos,
                next: Some(pos + 1),
                continued: false,
            };
        }
        // A tab never needs more than a whole row.
        let cells = match c {
            '\t' => char_cells(c, col, tab_width).min(budget),
            _ => char_cells(c, col, tab_width),
        };
        if col + cells > budget && col > 0 {
            return Row {
                start,
                end: pos,
                next: Some(pos),
                continued: true,
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
    }
}

fn row_at(text: &Text, start: usize, width: usize, tab_width: usize) -> Row {
    walk_row(text, start, width, tab_width, |_, _, _, _| {})
}

/// Start of the screen row on which `pos` is drawn, and that row's index
/// within its line.
pub fn row_containing(text: &Text, pos: usize, width: usize, tab_width: usize) -> (usize, usize) {
    let mut start = text.line_start(text.line_of(pos));
    let mut index = 0;
    loop {
        let row = row_at(text, start, width, tab_width);
        match row.next {
            Some(next) if row.continued && !row.contains(pos) => {
                start = next;
                index += 1;
            }
            _ => return (start, index),
        }
    }
}

/// Start of the row `n` rows after `row_start`, stopping at the last row.
pub fn rows_forward(
    text: &Text,
    row_start: usize,
    n: usize,
    width: usize,
    tab_width: usize,
) -> usize {
    let mut start = row_start;
    for _ in 0..n {
        match row_at(text, start, width, tab_width).next {
            Some(next) => start = next,
            None => break,
        }
    }
    start
}

/// Start of the row `n` rows before `row_start`, stopping at the first row.
pub fn rows_back(text: &Text, row_start: usize, n: usize, width: usize, tab_width: usize) -> usize {
    let mut line = text.line_of(row_start);
    let (_, index) = row_containing(text, row_start, width, tab_width);
    if index >= n {
        return rows_forward(text, text.line_start(line), index - n, width, tab_width);
    }
    let mut remaining = n - index;
    while line > 0 {
        line -= 1;
        let line_start = text.line_start(line);
        let (_, last_index) = row_containing(text, text.line_end(line), width, tab_width);
        let rows = last_index + 1;
        if rows >= remaining {
            return rows_forward(text, line_start, rows - remaining, width, tab_width);
        }
        remaining -= rows;
    }
    0
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_wrap_one_cell_short_of_the_width() {
        let text = Text::from("abcdefgh\nxy");
        let first = row_at(&text, 0, 5, 8);
        assert_eq!((first.end, first.next, first.continued), (4, Some(4), true));
        let second = row_at(&text, 4, 5, 8);
        assert_eq!(
            (second.end, second.next, second.continued),
            (8, Some(9), false)
        );
        let last = row_at(&text, 9, 5, 8);
        assert_eq!((last.end, last.next), (11, None));
    }

    #[test]
    fn wrap_boundary_belongs_to_the_next_row() {
        let text = Text::from("abcdefgh");
        assert_eq!(row_containing(&text, 3, 5, 8), (0, 0));
        assert_eq!(row_containing(&text, 4, 5, 8), (4, 1));
        assert_eq!(row_containing(&text, 8, 5, 8), (4, 1));
    }

    #[test]
    fn rows_back_crosses_lines() {
        // Rows at width 5: "abcd" "efgh" | "xy" | "" (after the last newline).
        let text = Text::from("abcdefgh\nxy\n");
        let last = text.len();
        assert_eq!(rows_back(&text, last, 1, 5, 8), 9);
        assert_eq!(rows_back(&text, last, 2, 5, 8), 4);
        assert_eq!(rows_back(&text, last, 3, 5, 8), 0);
        assert_eq!(rows_back(&text, last, 99, 5, 8), 0);
        assert_eq!(rows_back(&text, 4, 1, 5, 8), 0);
        assert_eq!(rows_forward(&text, 0, 2, 5, 8), 9);
        assert_eq!(rows_forward(&text, 0, 99, 5, 8), last);
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
}
