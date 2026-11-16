use std::fmt;
use std::io;
use std::ops::Range;

use ropey::Rope;

/// A rope of UTF-8 text addressed by char offset. Lines are separated by LF.
///
/// Cloning is O(1) and shares storage, so a clone is a cheap snapshot.
#[derive(Clone, Default)]
pub struct Text {
    rope: Rope,
}

impl Text {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads UTF-8 text. Fails with `InvalidData` if the input is not UTF-8.
    pub fn from_reader<R: io::Read>(reader: R) -> io::Result<Self> {
        Ok(Self {
            rope: Rope::from_reader(reader)?,
        })
    }

    pub fn write_to<W: io::Write>(&self, writer: W) -> io::Result<()> {
        self.rope.write_to(writer)
    }

    /// Length in chars.
    pub fn len(&self) -> usize {
        self.rope.len_chars()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Number of lines. Text ending in a newline has an empty last line, so
    /// this is always at least one.
    pub fn len_lines(&self) -> usize {
        self.rope.len_lines()
    }

    pub fn char_at(&self, pos: usize) -> Option<char> {
        self.rope.get_char(pos)
    }

    /// Inserts `s` so that its first char ends up at `pos`.
    ///
    /// # Panics
    /// If `pos > self.len()`.
    pub fn insert(&mut self, pos: usize, s: &str) {
        self.rope.insert(pos, s);
    }

    /// Removes `range` and returns the removed text.
    ///
    /// # Panics
    /// If the range is out of bounds.
    pub fn remove(&mut self, range: Range<usize>) -> String {
        let removed = self.slice(range.clone());
        self.rope.remove(range);
        removed
    }

    pub fn slice(&self, range: Range<usize>) -> String {
        self.rope.slice(range).to_string()
    }

    /// Zero-based line containing `pos`. `pos == self.len()` is valid.
    pub fn line_of(&self, pos: usize) -> usize {
        self.rope.char_to_line(pos)
    }

    /// Position of the first char of `line`.
    pub fn line_start(&self, line: usize) -> usize {
        self.rope.line_to_char(line)
    }

    /// Position just after the last char of `line`, before its newline.
    pub fn line_end(&self, line: usize) -> usize {
        if line + 1 < self.len_lines() {
            self.line_start(line + 1) - 1
        } else {
            self.len()
        }
    }

    /// Chars from `pos` to the end of the text.
    pub fn chars_from(&self, pos: usize) -> impl Iterator<Item = char> + '_ {
        self.rope.chars_at(pos)
    }

    /// Chars before `pos`, nearest first.
    pub fn chars_before(&self, pos: usize) -> impl Iterator<Item = char> + '_ {
        self.rope.chars_at(pos).reversed()
    }
}

impl From<&str> for Text {
    fn from(s: &str) -> Self {
        Self {
            rope: Rope::from_str(s),
        }
    }
}

impl fmt::Display for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for chunk in self.rope.chunks() {
            f.write_str(chunk)?;
        }
        Ok(())
    }
}

impl fmt::Debug for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Text({} chars)", self.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_lf_only() {
        let text = Text::from("a\r\nb\u{2028}c\n");
        assert_eq!(text.len_lines(), 3);
        assert_eq!(text.line_start(1), 3);
        assert_eq!(text.line_end(1), 6);
        assert_eq!(text.line_end(2), text.len());
    }

    #[test]
    fn line_end_excludes_newline() {
        let text = Text::from("ab\ncd");
        assert_eq!(text.line_end(0), 2);
        assert_eq!(text.line_end(1), 5);
        assert_eq!(text.line_of(2), 0);
        assert_eq!(text.line_of(3), 1);
    }

    #[test]
    fn remove_returns_removed_text() {
        let mut text = Text::from("hällo world");
        assert_eq!(text.remove(1..5), "ällo");
        assert_eq!(text.to_string(), "h world");
    }

    #[test]
    fn chars_before_runs_backwards() {
        let text = Text::from("abc");
        assert_eq!(text.chars_before(2).collect::<String>(), "ba");
        assert_eq!(text.chars_from(1).collect::<String>(), "bc");
    }
}
