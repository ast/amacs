//! Org, which has no grammar here: what a line is shows at its start, and
//! what is within a line is found by reading it. Only the blocks whose
//! text is not Org, such as source code, take knowing more than one line,
//! and those are found anew whenever the text has changed.

use std::ops::Range;

use amacs_text::Text;
use tree_sitter::Parser;

use crate::highlight::Highlight;
use crate::languages::{Language, language_called};
use crate::syntax::{Highlights, paint_ranges, point};

/// What a block holds whose text is not Org.
#[derive(Debug, Clone, Copy)]
enum Contents {
    /// Code in a language, or in one that the editor does not know.
    Source(Option<&'static Language>),
    /// Text to show as it is: an example, or text for an exporter.
    Literal,
    Comment,
}

/// A block from its `#+begin_` line to its `#+end_` line, in bytes.
#[derive(Debug, Clone)]
struct Block {
    start: usize,
    body: Range<usize>,
    end: usize,
    contents: Contents,
}

/// What is known of an Org text beyond its single lines.
#[derive(Debug)]
pub(crate) struct Org {
    blocks: Vec<Block>,
    /// The words that say a heading is still to do, and that it is done.
    not_done: Vec<String>,
    done: Vec<String>,
}

impl Default for Org {
    fn default() -> Self {
        Self {
            blocks: Vec::new(),
            not_done: vec!["TODO".to_owned()],
            done: vec!["DONE".to_owned()],
        }
    }
}

/// The bytes of a line, with the newline that ends it.
fn line_bytes(text: &Text, line: usize) -> Range<usize> {
    let end = if line + 1 < text.len_lines() {
        text.line_to_byte(line + 1)
    } else {
        text.len_bytes()
    };
    text.line_to_byte(line)..end
}

/// Reads a line into `buffer`, without the newline that ends it.
fn read_line(text: &Text, bytes: Range<usize>, buffer: &mut String) {
    buffer.clear();
    buffer.extend(text.byte_chunks(bytes));
    if buffer.ends_with('\n') {
        buffer.pop();
    }
}

/// The level of the heading that a line is, by its stars.
fn heading_level(line: &str) -> Option<usize> {
    let stars = line.bytes().take_while(|byte| *byte == b'*').count();
    (stars > 0 && line.as_bytes().get(stars) == Some(&b' ')).then_some(stars)
}

/// The rest of a line after a word that starts it, whatever the case of
/// the letters of either.
fn after_word<'a>(line: &'a str, word: &str) -> Option<&'a str> {
    let start = line.get(..word.len())?;
    start
        .eq_ignore_ascii_case(word)
        .then(|| &line[word.len()..])
}

/// The name of the block that a line begins, as in `#+begin_src sh`, with
/// what follows the name.
fn block_start(line: &str) -> Option<(&str, &str)> {
    let rest = after_word(line.trim_start(), "#+begin_")?;
    let name = rest.split_whitespace().next()?;
    Some((name, rest[name.len()..].trim()))
}

fn block_end(line: &str, name: &str) -> bool {
    after_word(line.trim_start(), "#+end_")
        .is_some_and(|rest| rest.trim().eq_ignore_ascii_case(name))
}

impl Org {
    /// Takes the words of the states of a heading.
    pub(crate) fn set_keywords(&mut self, not_done: Vec<String>, done: Vec<String>) {
        self.not_done = not_done;
        self.done = done;
    }

    /// Finds the blocks of the text whose text is not Org.
    pub(crate) fn scan(&mut self, text: &Text) {
        self.blocks.clear();
        let mut line = String::new();
        // The block that is open: where it started, where its text starts,
        // its name and what it holds.
        let mut open: Option<(usize, usize, String, Contents)> = None;
        for number in 0..text.len_lines() {
            let bytes = line_bytes(text, number);
            read_line(text, bytes.clone(), &mut line);
            if let Some((start, body, name, contents)) = &open {
                if block_end(&line, name) {
                    self.blocks.push(Block {
                        start: *start,
                        body: *body..bytes.start,
                        end: bytes.end,
                        contents: *contents,
                    });
                    open = None;
                } else if heading_level(&line).is_some() {
                    // A block does not reach past a heading: its first
                    // line began nothing.
                    open = None;
                }
                continue;
            }
            let Some((name, rest)) = block_start(&line) else {
                continue;
            };
            let contents = match name.to_ascii_lowercase().as_str() {
                "src" => Contents::Source(rest.split_whitespace().next().and_then(language_called)),
                "example" | "export" => Contents::Literal,
                "comment" => Contents::Comment,
                // A quote, a verse and the like hold Org.
                _ => continue,
            };
            open = Some((bytes.start, bytes.end, name.to_owned(), contents));
        }
    }

    /// The block that a byte is in, if it is in one.
    fn block_at(&self, byte: usize) -> Option<&Block> {
        let index = self.blocks.partition_point(|block| block.end <= byte);
        self.blocks.get(index).filter(|block| block.start <= byte)
    }

    /// Paints the highlights of the lines that hold the bytes asked for.
    pub(crate) fn paint(&self, text: &Text, out: &mut Highlights) {
        let shown = out.range();
        if shown.is_empty() {
            return;
        }
        let mut line = String::new();
        let mut number = text.byte_to_line(shown.start);
        while number < text.len_lines() {
            let bytes = line_bytes(text, number);
            if bytes.start >= shown.end {
                break;
            }
            number += 1;
            match self.block_at(bytes.start) {
                Some(block) if !block.body.contains(&bytes.start) => {
                    out.fill(bytes, Some(Highlight::Metadata));
                }
                Some(block) => {
                    if matches!(block.contents, Contents::Comment) {
                        out.fill(bytes, Some(Highlight::Comment));
                    }
                }
                None => {
                    read_line(text, bytes.clone(), &mut line);
                    Line {
                        org: self,
                        text: &line,
                        start: bytes.start,
                        out: &mut *out,
                    }
                    .paint();
                }
            }
        }
        let mut parser = Parser::new();
        let first = self
            .blocks
            .partition_point(|block| block.end <= shown.start);
        for block in &self.blocks[first..] {
            if block.start >= shown.end {
                break;
            }
            let Contents::Source(Some(language)) = block.contents else {
                continue;
            };
            let range = tree_sitter::Range {
                start_byte: block.body.start,
                end_byte: block.body.end,
                start_point: point(text, block.body.start),
                end_point: point(text, block.body.end),
            };
            paint_ranges(&mut parser, language, &[range], true, text, out);
        }
    }
}

/// A line of Org that is being painted.
struct Line<'a> {
    org: &'a Org,
    text: &'a str,
    /// The byte of the whole text that the line starts at.
    start: usize,
    out: &'a mut Highlights,
}

/// What may stand before the mark that opens emphasis, and after the one
/// that closes it.
fn opens_emphasis(before: Option<u8>) -> bool {
    before.is_none_or(|byte| byte.is_ascii_whitespace() || b"-({'\"".contains(&byte))
}

fn closes_emphasis(after: Option<u8>) -> bool {
    after.is_none_or(|byte| byte.is_ascii_whitespace() || b"-.,;:!?')}[\"\\".contains(&byte))
}

/// How far a web address reaches that starts at a byte of a line, if one
/// does.
fn url_end(line: &str, start: usize) -> Option<usize> {
    let rest = &line[start..];
    let scheme = ["https://", "http://", "ftp://", "mailto:"]
        .into_iter()
        .find(|scheme| rest.starts_with(scheme))?;
    let before = start.checked_sub(1).map(|index| line.as_bytes()[index]);
    if before.is_some_and(|byte| byte.is_ascii_alphanumeric()) {
        return None;
    }
    let mut open = 0usize;
    let mut end = rest.len();
    for (index, byte) in rest.bytes().enumerate() {
        match byte {
            b'(' => open += 1,
            // A bracket that closes what was opened before the address is
            // not part of it.
            b')' if open > 0 => open -= 1,
            b')' | b'<' | b'>' | b'[' | b']' | b'"' => {
                end = index;
                break;
            }
            _ if byte.is_ascii_whitespace() => {
                end = index;
                break;
            }
            _ => {}
        }
    }
    let address = rest[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', '\'']);
    (address.len() > scheme.len()).then_some(start + address.len())
}

impl Line<'_> {
    fn fill(&mut self, range: Range<usize>, highlight: Highlight) {
        self.out.fill(
            self.start + range.start..self.start + range.end,
            Some(highlight),
        );
    }

    fn paint(&mut self) {
        let line = self.text;
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if let Some(level) = heading_level(line) {
            self.heading(level);
        } else if trimmed.starts_with("#+") {
            self.keyword(indent);
        } else if trimmed == "#" || trimmed.starts_with("# ") {
            self.fill(0..line.len(), Highlight::Comment);
        } else if trimmed.starts_with('|') {
            self.fill(indent..line.len(), Highlight::Table);
            self.inline(indent, false);
        } else if let Some(end) = property_end(trimmed) {
            self.fill(indent..indent + end, Highlight::Metadata);
        } else if let Some(marker) = item_marker(trimmed, indent) {
            self.item(indent, marker);
        } else {
            let planned = ["SCHEDULED:", "DEADLINE:", "CLOSED:"]
                .into_iter()
                .find(|word| trimmed.starts_with(word))
                .map_or(0, str::len);
            self.fill(indent..indent + planned, Highlight::Metadata);
            self.inline(indent + planned, true);
        }
    }

    fn heading(&mut self, level: usize) {
        let line = self.text;
        self.fill(0..line.len(), Highlight::heading(level));
        let title = level + 1;
        let word = line[title..].split(' ').next().unwrap_or_default();
        let is = |words: &[String]| words.iter().any(|known| known == word);
        if is(&self.org.not_done) {
            self.fill(title..title + word.len(), Highlight::Todo);
        } else if is(&self.org.done) {
            self.fill(title..title + word.len(), Highlight::Done);
        }
        // Tags end the line: `:work:urgent:` after a blank.
        let text = line.trim_end();
        let tags = text
            .rfind([' ', '\t'])
            .map_or(text.len(), |blank| blank + 1);
        let tag = |c: char| c.is_alphanumeric() || "_@#%:".contains(c);
        let found = &text[tags..];
        if tags > title
            && found.len() > 2
            && found.starts_with(':')
            && found.ends_with(':')
            && found.chars().all(tag)
        {
            self.fill(tags..text.len(), Highlight::Tag);
        }
        self.inline(title, false);
    }

    /// A line that starts with `#+`: a setting such as `#+title: Notes`, or
    /// the first or last line of a block that holds Org.
    fn keyword(&mut self, indent: usize) {
        let line = self.text;
        let rest = &line[indent + 2..];
        let name_end = rest
            .find(|c: char| c == ':' || c.is_whitespace())
            .unwrap_or(rest.len());
        let name = rest[..name_end].to_ascii_lowercase();
        let value = indent + 2 + name_end + usize::from(rest[name_end..].starts_with(':'));
        let named = !name.is_empty() && rest[name_end..].starts_with(':');
        match name.as_str() {
            "title" if named => {
                self.fill(0..value, Highlight::Metadata);
                self.fill(value..line.len(), Highlight::Heading);
            }
            "author" | "date" | "email" | "subtitle" if named => {
                self.fill(0..value, Highlight::Metadata);
                self.inline(value, false);
            }
            _ if named || name.starts_with("begin_") || name.starts_with("end_") => {
                self.fill(0..line.len(), Highlight::Metadata);
            }
            // `#+` and other text is text.
            _ => self.inline(indent, true),
        }
    }

    fn item(&mut self, indent: usize, marker: usize) {
        let line = self.text;
        let mut text = indent + marker;
        self.fill(indent..text.min(line.len()), Highlight::Delimiter);
        let rest = line.get(text..).unwrap_or_default();
        let boxed = rest
            .get(..3)
            .filter(|_| rest.len() == 3 || rest.as_bytes()[3] == b' ');
        let state = match boxed {
            Some("[ ]" | "[-]") => Some(Highlight::Todo),
            Some("[X]" | "[x]") => Some(Highlight::Done),
            _ => None,
        };
        if let Some(state) = state {
            self.fill(text..text + 3, state);
            text += 3;
        }
        self.inline(text.min(line.len()), true);
    }

    /// Paints what is within a line from a byte on: links, addresses,
    /// dates, code, and where `emphasis` says so, strong and stressed text.
    /// In a heading or a table those would take the colour of the line.
    fn inline(&mut self, from: usize, emphasis: bool) {
        let line = self.text;
        let bytes = line.as_bytes();
        let mut at = from;
        while at < bytes.len() {
            let before = at.checked_sub(1).map(|index| bytes[index]);
            let next = match bytes[at] {
                b'[' if bytes.get(at + 1) == Some(&b'[') => self.link(at),
                b'[' | b'<' => self.date(at),
                b'h' | b'f' | b'm' => url_end(line, at).inspect(|end| {
                    self.fill(at..*end, Highlight::Link);
                }),
                mark @ (b'=' | b'~') if opens_emphasis(before) => self.emphasis(at, mark),
                mark @ (b'*' | b'/' | b'_' | b'+') if emphasis && opens_emphasis(before) => {
                    self.emphasis(at, mark)
                }
                _ => None,
            };
            at = next.unwrap_or(at + 1);
        }
    }

    /// Paints the link that starts at a byte, `[[target]]` or
    /// `[[target][text]]`, and returns where it ends.
    fn link(&mut self, at: usize) -> Option<usize> {
        let line = self.text;
        let close = at + 2 + line[at + 2..].find("]]")?;
        if close == at + 2 {
            return None;
        }
        match line[at + 2..close].find("][") {
            Some(middle) => {
                let middle = at + 2 + middle;
                self.fill(at..middle + 2, Highlight::Delimiter);
                self.fill(middle + 2..close, Highlight::Link);
            }
            None => {
                self.fill(at..at + 2, Highlight::Delimiter);
                self.fill(at + 2..close, Highlight::Link);
            }
        }
        self.fill(close..close + 2, Highlight::Delimiter);
        Some(close + 2)
    }

    /// Paints the date that starts at a byte, `<2026-02-16 Mon>` or the
    /// same in square brackets, and returns where it ends.
    fn date(&mut self, at: usize) -> Option<usize> {
        let line = self.text;
        let bytes = line.as_bytes();
        let day = bytes.get(at + 1..at + 11)?;
        let digits = |range: Range<usize>| day[range].iter().all(u8::is_ascii_digit);
        if !(digits(0..4) && day[4] == b'-' && digits(5..7) && day[7] == b'-' && digits(8..10)) {
            return None;
        }
        let close = if bytes[at] == b'<' { '>' } else { ']' };
        let end = at + 11 + line[at + 11..].find(close)? + 1;
        self.fill(at..end, Highlight::Date);
        Some(end)
    }

    /// Paints the text between a mark at a byte and the next mark of its
    /// kind that can close it, as `*strong*` and `~code~`, and returns
    /// where it ends.
    fn emphasis(&mut self, at: usize, mark: u8) -> Option<usize> {
        let bytes = self.text.as_bytes();
        if bytes.get(at + 1).is_none_or(u8::is_ascii_whitespace) {
            return None;
        }
        let close = (at + 2..bytes.len()).find(|index| {
            bytes[*index] == mark
                && !bytes[index - 1].is_ascii_whitespace()
                && closes_emphasis(bytes.get(index + 1).copied())
        })?;
        let highlight = match mark {
            b'*' => Some(Highlight::Strong),
            b'/' => Some(Highlight::Emphasis),
            b'=' => Some(Highlight::Verbatim),
            b'~' => Some(Highlight::Code),
            // Underlined and struck text have no face of their own.
            _ => None,
        };
        if let Some(highlight) = highlight {
            self.fill(at..at + 1, Highlight::Delimiter);
            self.fill(at + 1..close, highlight);
            self.fill(close..close + 1, Highlight::Delimiter);
        }
        Some(close + 1)
    }
}

/// Where the name of a property or a drawer ends that starts a line
/// without its indentation: `:ID:`, `:PROPERTIES:`, `:END:`.
fn property_end(trimmed: &str) -> Option<usize> {
    let rest = trimmed.strip_prefix(':')?;
    let name = rest.find(':')?;
    let named = name > 0
        && rest[..name]
            .chars()
            .all(|c| c.is_alphanumeric() || "_-+".contains(c));
    let ends = rest[name + 1..]
        .chars()
        .next()
        .is_none_or(char::is_whitespace);
    (named && ends).then_some(name + 2)
}

/// The length of the marker of the item of a list that a line is, with
/// the blank after it: `- `, `+ `, `1. `, `1) `, and `* ` where the line is
/// indented, since a star at the start of a line is a heading.
fn item_marker(trimmed: &str, indent: usize) -> Option<usize> {
    let bytes = trimmed.as_bytes();
    let digits = bytes
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    let marker = match bytes.first()? {
        b'-' | b'+' => 1,
        b'*' if indent > 0 => 1,
        _ if digits > 0 && matches!(bytes.get(digits), Some(b'.' | b')')) => digits + 1,
        _ => return None,
    };
    match bytes.get(marker) {
        None => Some(marker),
        Some(b' ') => Some(marker + 1),
        Some(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_heading_is_stars_and_a_blank() {
        assert_eq!(heading_level("* One"), Some(1));
        assert_eq!(heading_level("*** Three"), Some(3));
        assert_eq!(heading_level("*strong* text"), None);
        assert_eq!(heading_level(" * an item"), None);
        assert_eq!(heading_level("**"), None);
    }

    #[test]
    fn markers_of_items_are_told_from_other_starts_of_lines() {
        assert_eq!(item_marker("- one", 0), Some(2));
        assert_eq!(item_marker("-", 0), Some(1));
        assert_eq!(item_marker("12) one", 0), Some(4));
        assert_eq!(item_marker("* one", 2), Some(2));
        assert_eq!(item_marker("* one", 0), None);
        assert_eq!(item_marker("-one", 0), None);
        assert_eq!(item_marker("1.5 is a number", 0), None);
    }

    #[test]
    fn a_property_is_a_name_between_colons() {
        assert_eq!(property_end(":ID:       247c"), Some(4));
        assert_eq!(property_end(":PROPERTIES:"), Some(12));
        assert_eq!(property_end(": fixed width"), None);
        assert_eq!(property_end(":a:b:"), None);
    }

    #[test]
    fn an_address_ends_before_the_punctuation_of_its_sentence() {
        fn end(line: &str) -> Option<&str> {
            url_end(line, 0).map(|end| &line[..end])
        }
        assert_eq!(end("https://example.org/a."), Some("https://example.org/a"));
        assert_eq!(
            end("https://example.org/a_(b)) x"),
            Some("https://example.org/a_(b)")
        );
        assert_eq!(end("https://"), None);
        assert_eq!(end("help"), None);
        assert_eq!(url_end("xhttps://example.org", 1), None);
    }
}
