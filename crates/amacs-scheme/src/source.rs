//! Source text and positions in it, so that every error can say where.

use std::fmt;
use std::rc::Rc;

/// A stretch of a source file, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub file: u32,
    pub start: u32,
    pub end: u32,
}

/// A position as people give it: lines and columns count from one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub file: Rc<str>,
    pub line: usize,
    pub column: usize,
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.file, self.line, self.column)
    }
}

struct SourceFile {
    name: Rc<str>,
    text: Rc<str>,
    /// Byte offset of the start of each line.
    lines: Vec<u32>,
}

/// Every piece of source text that has been read.
#[derive(Default)]
pub struct Sources {
    files: Vec<SourceFile>,
}

impl Sources {
    /// Registers a text under a name and returns its number.
    pub fn add(&mut self, name: &str, text: &str) -> u32 {
        let mut lines = vec![0];
        lines.extend(
            text.bytes()
                .enumerate()
                .filter(|(_, byte)| *byte == b'\n')
                .map(|(index, _)| index as u32 + 1),
        );
        self.files.push(SourceFile {
            name: name.into(),
            text: text.into(),
            lines,
        });
        self.files.len() as u32 - 1
    }

    pub fn location(&self, span: Span) -> Option<Location> {
        let file = self.files.get(span.file as usize)?;
        let line = file.lines.partition_point(|start| *start <= span.start) - 1;
        let line_start = file.lines[line] as usize;
        let upto = (span.start as usize).min(file.text.len());
        let column = file.text.get(line_start..upto)?.chars().count();
        Some(Location {
            file: file.name.clone(),
            line: line + 1,
            column: column + 1,
        })
    }

    /// The text a span covers.
    pub fn text(&self, span: Span) -> Option<&str> {
        let file = self.files.get(span.file as usize)?;
        file.text.get(span.start as usize..span.end as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_become_lines_and_columns() {
        let mut sources = Sources::default();
        let file = sources.add("a.scm", "(a\n  (bö c)\n)");
        let at = |start: u32| {
            let span = Span {
                file,
                start,
                end: start + 1,
            };
            let location = sources.location(span).unwrap();
            (location.line, location.column)
        };
        assert_eq!(at(0), (1, 1));
        assert_eq!(at(5), (2, 3));
        // The ö is two bytes but one column.
        assert_eq!(at(10), (2, 7));
        assert_eq!(at(13), (3, 1));
        let span = Span {
            file,
            start: 5,
            end: 12,
        };
        assert_eq!(sources.text(span), Some("(bö c)"));
        assert_eq!(sources.location(span).unwrap().to_string(), "a.scm:2:3");
    }
}
