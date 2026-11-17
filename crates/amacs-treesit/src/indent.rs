//! How far a line should be indented.
//!
//! Bracket languages are indented from the syntax tree: every bracket pair
//! or other indenting construct that is open across the start of a line
//! adds one level. Where the tree is broken by a syntax error, and in
//! languages indented by layout, the previous line decides instead.

use std::collections::BTreeSet;

use amacs_text::Text;
use tree_sitter::Node;

use crate::languages::{BracketRules, IndentStyle};
use crate::syntax::Syntax;

/// The widths that indentation is measured in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndentParams {
    /// Columns per level.
    pub offset: usize,
    pub tab_width: usize,
}

const OPENERS: &[&str] = &["{", "(", "["];
const CLOSERS: &[&str] = &["}", ")", "]"];

/// Column of the first char of `line` that is not a space or tab, and
/// whether the line has such a char.
pub fn line_indentation(text: &Text, line: usize, tab_width: usize) -> (usize, bool) {
    let mut col = 0;
    for c in text.chars_from(text.line_start(line)) {
        match c {
            ' ' => col += 1,
            '\t' => col += tab_width - col % tab_width,
            '\n' => return (col, false),
            _ => return (col, true),
        }
    }
    (col, false)
}

/// The text of a line without its indentation and trailing whitespace.
fn line_content(text: &Text, line: usize) -> String {
    let (start, end) = (text.line_start(line), text.line_end(line));
    text.slice(start..end).trim().to_owned()
}

/// Indentation for `line` judged by the nearest line above with text on it:
/// the same, one level more if that line ends by opening something, and one
/// less if this line starts by closing something. With `colon`, a line that
/// ends in a colon opens a block, as in Python.
pub fn indent_from_previous_line(
    text: &Text,
    line: usize,
    params: IndentParams,
    colon: bool,
) -> usize {
    let previous = (0..line)
        .rev()
        .find(|line| line_indentation(text, *line, params.tab_width).1);
    let Some(previous) = previous else { return 0 };
    let mut indent = line_indentation(text, previous, params.tab_width).0;
    let above = line_content(text, previous);
    if above.ends_with(['{', '(', '[']) || (colon && above.ends_with(':')) {
        indent += params.offset;
    }
    if line_content(text, line).starts_with(['}', ')', ']']) {
        indent = indent.saturating_sub(params.offset);
    }
    indent
}

/// The opening bracket among a node's own children.
fn opener<'tree>(node: Node<'tree>, rules: &Brackets) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| rules.is_opener(child.kind()))
}

/// The closing bracket among a node's own children.
fn closer<'tree>(node: Node<'tree>, rules: &Brackets) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|child| rules.is_closer(child.kind()))
        .last()
}

/// What counts as a bracket. Some grammars name their bracket tokens.
struct Brackets;

impl Brackets {
    fn is_opener(&self, kind: &str) -> bool {
        OPENERS.contains(&kind) || (kind.ends_with("_start") && !kind.contains("heredoc"))
    }

    fn is_closer(&self, kind: &str) -> bool {
        CLOSERS.contains(&kind) || kind.ends_with("_end")
    }
}

/// The dot among a node's own children, if it is the first thing on its
/// line, as in a chain of method calls with one call per line.
fn leading_dot<'tree>(text: &Text, node: Node<'tree>) -> Option<Node<'tree>> {
    // In a call, the dot belongs to the callee, which is the first child.
    let find = |node: Node<'tree>| {
        let mut cursor = node.walk();
        node.children(&mut cursor)
            .find(|child| matches!(child.kind(), "." | "?."))
    };
    let dot = find(node).or_else(|| node.child(0).and_then(find))?;
    let pos = text.byte_to_char(dot.start_byte());
    let line_start = text.line_start(text.line_of(pos));
    text.chars_from(line_start)
        .take(pos - line_start)
        .all(|c| c == ' ' || c == '\t')
        .then_some(dot)
}

fn is_text_node(kind: &str) -> bool {
    kind.contains("string") || kind.contains("comment") || kind.contains("heredoc")
}

enum TreeIndent {
    /// The line is inside a string or comment and is left alone.
    Keep,
    Levels(usize),
    /// The tree is broken around the line.
    Unknown,
}

impl Syntax {
    /// The column that `line` should be indented to, or `None` to leave it
    /// as it is. The tree must be up to date.
    pub fn indent_column(&self, text: &Text, line: usize, params: IndentParams) -> Option<usize> {
        match self.language().indent {
            IndentStyle::Brackets(rules) => match self.tree_indent(text, line, rules) {
                TreeIndent::Keep => None,
                TreeIndent::Levels(levels) => Some(levels * params.offset),
                TreeIndent::Unknown => Some(indent_from_previous_line(text, line, params, false)),
            },
            IndentStyle::Offside => Some(indent_from_previous_line(text, line, params, true)),
            IndentStyle::None | IndentStyle::Lisp => None,
        }
    }

    fn tree_indent(&self, text: &Text, line: usize, rules: &BracketRules) -> TreeIndent {
        let Some(tree) = self.tree() else {
            return TreeIndent::Unknown;
        };
        let root = tree.root_node();
        let brackets = Brackets;

        // The first char of the line that is not whitespace, if it has one.
        let line_start = text.line_start(line);
        let mut first = line_start;
        let mut blank = true;
        for c in text.chars_from(line_start) {
            match c {
                ' ' | '\t' => first += 1,
                '\n' => break,
                _ => {
                    blank = false;
                    break;
                }
            }
        }
        let line_byte = text.char_to_byte(line_start);
        let first_byte = text.char_to_byte(first);
        let end_byte = if blank {
            first_byte
        } else {
            text.char_to_byte(first + 1)
        };
        let Some(leaf) = root.descendant_for_byte_range(first_byte, end_byte) else {
            return TreeIndent::Unknown;
        };

        // Inside a string or comment that began on an earlier line.
        let mut node = Some(leaf);
        for _ in 0..3 {
            let Some(inner) = node else { break };
            if is_text_node(inner.kind()) && inner.start_byte() < line_byte {
                return TreeIndent::Keep;
            }
            node = inner.parent();
        }

        if root.has_error() && self.broken_near(text, root, leaf, first, first_byte) {
            return TreeIndent::Unknown;
        }

        // The rows on which constructs that enclose this line were opened.
        // Several opened on one row indent their contents only once.
        let mut rows = BTreeSet::new();
        let mut child: Option<Node> = None;
        let mut node = Some(leaf);
        while let Some(current) = node {
            let kind = current.kind();
            let flat = rules.flat.contains(&kind)
                || current
                    .parent()
                    .is_some_and(|parent| rules.flat.contains(&parent.kind()));
            if !flat && let Some(open) = opener(current, &brackets) {
                let inside = open.start_byte() < first_byte
                    && closer(current, &brackets)
                        .is_none_or(|close| close.start_byte() >= first_byte);
                if inside && open.start_position().row < line {
                    rows.insert(open.start_position().row);
                }
            }
            // A chain of method calls broken before a dot: the lines from
            // the dot on are indented relative to where the chain starts.
            if let Some(dot) = leading_dot(text, current)
                && dot.start_byte() <= first_byte
                && current.start_position().row < line
            {
                rows.insert(current.start_position().row);
            }
            let within = child.is_some_and(|child| {
                rules.indent_within.contains(&(kind, child.kind()))
                    && child.start_byte() <= first_byte
            });
            if (rules.indent.contains(&kind) || within) && current.start_position().row < line {
                rows.insert(current.start_position().row);
            }
            child = Some(current);
            node = current.parent();
        }

        let mut levels = rows.len();
        if !blank {
            // Tokens at the start of the line that close what they are in.
            let mut closed = BTreeSet::new();
            let mut token = Some(leaf);
            while let Some(current) = token {
                let kind = current.kind();
                let row = if brackets.is_closer(kind) {
                    // Some grammars wrap the bracket in a node of its own.
                    let mut bracket = current;
                    while let Some(parent) = bracket.parent()
                        && parent.byte_range() == bracket.byte_range()
                    {
                        bracket = parent;
                    }
                    bracket
                        .parent()
                        .and_then(|parent| opener(parent, &brackets))
                        .map(|open| open.start_position().row)
                } else if rules.outdent_tokens.contains(&kind) {
                    let mut ancestor = current.parent();
                    loop {
                        match ancestor {
                            Some(node)
                                if rules.indent.contains(&node.kind())
                                    && node.start_position().row < line =>
                            {
                                break Some(node.start_position().row);
                            }
                            Some(node) => ancestor = node.parent(),
                            None => break None,
                        }
                    }
                } else {
                    break;
                };
                if let Some(row) = row {
                    closed.insert(row);
                }
                // The next token, if it follows on this line.
                let next = current.end_byte();
                token = root
                    .descendant_for_byte_range(next, next + 1)
                    .filter(|node| {
                        node.start_byte() == next
                            && node.child_count() == 0
                            && node.start_position().row == line
                    });
            }
            levels = rows.difference(&closed).count();

            // A node that starts here and belongs one level out.
            let mut node = Some(leaf);
            while let Some(current) = node {
                if current.start_byte() != first_byte {
                    break;
                }
                if rules.outdent_start.contains(&current.kind()) {
                    levels = levels.saturating_sub(1);
                    break;
                }
                node = current.parent();
            }
        }
        TreeIndent::Levels(levels)
    }

    /// Whether a syntax error makes the tree around a line untrustworthy.
    fn broken_near(
        &self,
        text: &Text,
        root: Node,
        leaf: Node,
        first: usize,
        first_byte: usize,
    ) -> bool {
        let mut node = Some(leaf);
        while let Some(current) = node {
            if current.is_error() || current.is_missing() {
                return true;
            }
            node = current.parent();
        }
        // The last token before the line: an error there usually means the
        // construct that this line belongs to is not finished.
        let before = text
            .chars_before(first)
            .position(|c| !c.is_whitespace())
            .map(|skipped| first - skipped - 1);
        let Some(before) = before else { return false };
        let (start, end) = (text.char_to_byte(before), text.char_to_byte(before + 1));
        let mut node = root.descendant_for_byte_range(start, end);
        while let Some(current) = node {
            if current.id() == root.id() {
                break;
            }
            if current.is_error()
                || current.is_missing()
                || (current.has_error() && current.end_byte() <= first_byte)
            {
                return true;
            }
            node = current.parent();
        }
        false
    }
}
