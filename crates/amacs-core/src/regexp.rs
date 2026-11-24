//! Regular expressions, in the syntax of the `regex` crate, with `^` and
//! `$` at the starts and ends of lines as in Emacs.
//!
//! Positions are counted in chars, as everywhere in the editor. A
//! replacement is written as in Emacs: `\&` is what matched, `\1` to `\9`
//! what a group matched, `\\` a backslash.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use regex::{Captures, Regex, RegexBuilder};

use crate::error::{CmdError, CmdResult};

thread_local! {
    /// The expressions compiled lately: a command asks about the same one
    /// for each line it looks at.
    static COMPILED: RefCell<HashMap<String, Rc<Regex>>> = RefCell::new(HashMap::new());
}

/// How many compiled expressions are kept.
const KEPT: usize = 32;

pub fn compile(pattern: &str) -> CmdResult<Rc<Regex>> {
    COMPILED.with_borrow_mut(|compiled| {
        if let Some(regex) = compiled.get(pattern) {
            return Ok(Rc::clone(regex));
        }
        let regex = RegexBuilder::new(pattern)
            .multi_line(true)
            .build()
            .map_err(|error| {
                // The last line of the report says what is wrong.
                let report = error.to_string();
                let reason = report.lines().last().unwrap_or("").trim();
                CmdError::msg(format!("Bad regular expression {pattern}: {reason}"))
            })?;
        if compiled.len() >= KEPT {
            compiled.clear();
        }
        let regex = Rc::new(regex);
        compiled.insert(pattern.to_owned(), Rc::clone(&regex));
        Ok(regex)
    })
}

fn char_to_byte(text: &str, pos: usize) -> usize {
    text.char_indices()
        .nth(pos)
        .map_or(text.len(), |(byte, _)| byte)
}

/// A match in a text: where it and each of its groups are, in chars, and
/// what a replacement becomes for it.
pub struct Found<'t> {
    text: &'t str,
    captures: Captures<'t>,
}

impl Found<'_> {
    /// Where the whole match is, or with `Some(n)` the `n`th group, if it
    /// took part.
    pub fn range(&self, group: usize) -> Option<std::ops::Range<usize>> {
        let found = self.captures.get(group)?;
        let start = self.text[..found.start()].chars().count();
        Some(start..start + found.as_str().chars().count())
    }

    pub fn groups(&self) -> usize {
        self.captures.len()
    }

    /// A replacement with its `\&`, `\1` and so on filled in.
    pub fn expand(&self, replacement: &str) -> String {
        let mut out = String::new();
        let mut chars = replacement.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('&') => out.push_str(&self.captures[0]),
                Some(digit @ '0'..='9') => {
                    let group = digit as usize - '0' as usize;
                    if let Some(found) = self.captures.get(group) {
                        out.push_str(found.as_str());
                    }
                }
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        }
        out
    }
}

/// The first match of an expression in a text at or after a position.
pub fn search<'t>(regex: &Regex, text: &'t str, from: usize) -> Option<Found<'t>> {
    let start = char_to_byte(text, from);
    if from > 0 && start == text.len() && text.chars().count() < from {
        return None;
    }
    let captures = regex.captures_at(text, start)?;
    Some(Found { text, captures })
}

/// A text with every match of an expression replaced, and how many there
/// were.
pub fn replace_all(regex: &Regex, text: &str, replacement: &str) -> (String, usize) {
    let mut out = String::new();
    let mut last = 0;
    let mut count = 0;
    for captures in regex.captures_iter(text) {
        let Some(found) = captures.get(0) else {
            continue;
        };
        out.push_str(&text[last..found.start()]);
        out.push_str(&Found { text, captures }.expand(replacement));
        last = found.end();
        count += 1;
    }
    out.push_str(&text[last..]);
    (out, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_are_found_by_chars_and_lines() {
        let regex = compile(r"^(\w+)é(\d*)$").unwrap();
        let text = "åä\nnaé12\nend";
        let found = search(&regex, text, 0).unwrap();
        assert_eq!(found.range(0), Some(3..8));
        assert_eq!(found.range(1), Some(3..5));
        assert_eq!(found.range(2), Some(6..8));
        assert_eq!(found.groups(), 3);
        assert!(search(&regex, text, 4).is_none());
        // A match cannot start in what is before the position, but the
        // expression sees it: this is no start of a line.
        assert!(search(&compile("^a").unwrap(), "ba", 1).is_none());
    }

    #[test]
    fn a_replacement_takes_what_matched() {
        let regex = compile(r"(\w+)@(\w+)").unwrap();
        let (text, count) = replace_all(&regex, "ann@home bo@work", r"\2:\1 (\&) \\");
        assert_eq!(text, r"home:ann (ann@home) \ work:bo (bo@work) \");
        assert_eq!(count, 2);
        // An expression that matches nothing at all, at each place once.
        let (text, count) = replace_all(&compile("^").unwrap(), "a\nb", "> ");
        assert_eq!((text.as_str(), count), ("> a\n> b", 2));
    }

    #[test]
    fn a_bad_expression_is_said_to_be_so() {
        let error = compile("a(b").unwrap_err().to_string();
        assert!(error.contains("Bad regular expression a(b"), "{error}");
    }
}
