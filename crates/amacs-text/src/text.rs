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

    // Byte offsets, for tools such as tree-sitter that count in UTF-8 bytes.

    /// Length in bytes.
    pub fn len_bytes(&self) -> usize {
        self.rope.len_bytes()
    }

    pub fn char_to_byte(&self, pos: usize) -> usize {
        self.rope.char_to_byte(pos)
    }

    /// The char that contains byte `byte`. `byte == self.len_bytes()` is valid.
    pub fn byte_to_char(&self, byte: usize) -> usize {
        self.rope.byte_to_char(byte)
    }

    /// Byte offset of the first char of `line`.
    pub fn line_to_byte(&self, line: usize) -> usize {
        self.rope.line_to_byte(line)
    }

    /// Zero-based line containing byte `byte`.
    pub fn byte_to_line(&self, byte: usize) -> usize {
        self.rope.byte_to_line(byte)
    }

    /// A piece of the text that starts at byte `byte`. It is empty only at
    /// the end of the text.
    pub fn chunk_at_byte(&self, byte: usize) -> &str {
        if byte >= self.rope.len_bytes() {
            return "";
        }
        let (chunk, chunk_start, _, _) = self.rope.chunk_at_byte(byte);
        &chunk[byte - chunk_start..]
    }

    /// The pieces that make up a byte range. The range must lie on char
    /// boundaries.
    pub fn byte_chunks(&self, range: Range<usize>) -> impl Iterator<Item = &str> + '_ {
        self.rope.byte_slice(range).chunks()
    }

    /// A number that two texts share if they are the same, and almost
    /// never otherwise: to tell whether a text is the one that something
    /// was written down about. It is the same from one run of the editor
    /// to the next, and guards against chance, not against intent.
    pub fn content_hash(&self) -> u64 {
        // FNV-1a.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for chunk in self.rope.chunks() {
            for byte in chunk.bytes() {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        hash
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
    fn the_hash_is_of_the_content_however_it_is_stored() {
        let whole = Text::from("hällo\nworld".repeat(2000).as_str());
        let mut built = Text::new();
        for _ in 0..2000 {
            let end = built.len();
            built.insert(end, "hällo\nworld");
        }
        assert_eq!(whole.content_hash(), built.content_hash());
        assert_ne!(whole.content_hash(), Text::from("hällo").content_hash());
        assert_eq!(Text::new().content_hash(), 0xcbf2_9ce4_8422_2325);
    }

    #[test]
    fn remove_returns_removed_text() {
        let mut text = Text::from("hällo world");
        assert_eq!(text.remove(1..5), "ällo");
        assert_eq!(text.to_string(), "h world");
    }

    #[test]
    fn byte_offsets_follow_utf8() {
        let text = Text::from("hä\nlö");
        assert_eq!(text.len_bytes(), 7);
        assert_eq!(text.char_to_byte(2), 3);
        assert_eq!(text.byte_to_char(3), 2);
        assert_eq!(text.line_to_byte(1), 4);
        assert_eq!(text.byte_to_line(5), 1);
        assert_eq!(text.chunk_at_byte(4), "lö");
        assert_eq!(text.chunk_at_byte(7), "");
        assert_eq!(text.byte_chunks(1..5).collect::<String>(), "ä\nl");
    }

    #[test]
    fn chars_before_runs_backwards() {
        let text = Text::from("abc");
        assert_eq!(text.chars_before(2).collect::<String>(), "ba");
        assert_eq!(text.chars_from(1).collect::<String>(), "bc");
    }
}
