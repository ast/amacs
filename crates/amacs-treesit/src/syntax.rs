//! The syntax tree of one buffer, kept up to date as the text changes.

use std::ops::Range;

use amacs_text::Text;
use tree_sitter::{InputEdit, Node, Parser, Point, QueryCursor, StreamingIterator, Tree};

use crate::highlight::{Capture, Highlight};
use crate::languages::{Grammar, Injections, Language};

/// Texts longer than this many bytes are not parsed, since the first parse
/// would hold up the editor for too long.
pub const MAX_PARSE_BYTES: usize = 8 << 20;

/// Texts up to this size are parsed afresh while they have syntax errors.
const REPARSE_BYTES: usize = 256 << 10;

/// The highlights of a range of text, one entry per byte.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Highlights {
    start: usize,
    /// Zero for no highlight, else the highlight's index plus one.
    bytes: Vec<u8>,
}

impl Highlights {
    /// The highlight of the char that starts at byte `byte`.
    pub fn get(&self, byte: usize) -> Option<Highlight> {
        let index = *self.bytes.get(byte.checked_sub(self.start)?)?;
        Highlight::from_index(index.checked_sub(1)?)
    }
}

/// A buffer's syntax tree.
pub struct Syntax {
    language: &'static Language,
    grammar: &'static Grammar,
    parser: Parser,
    tree: Option<Tree>,
    /// Whether the text has changed since the tree was parsed.
    stale: bool,
}

impl std::fmt::Debug for Syntax {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Syntax({})", self.language.name)
    }
}

fn point(text: &Text, byte: usize) -> Point {
    let row = text.byte_to_line(byte);
    Point::new(row, byte - text.line_to_byte(row))
}

/// Where a point ends up after `inserted` is put at it.
fn advance(start: Point, inserted: &str) -> Point {
    match inserted.rfind('\n') {
        Some(last) => Point::new(
            start.row + inserted.matches('\n').count(),
            inserted.len() - last - 1,
        ),
        None => Point::new(start.row, start.column + inserted.len()),
    }
}

impl Syntax {
    /// A syntax tree for a language, or `None` if the language has no grammar.
    pub fn new(language: &'static Language) -> Option<Self> {
        let grammar = language.compiled()?;
        let mut parser = Parser::new();
        parser.set_language(&grammar.language).ok()?;
        Some(Self {
            language,
            grammar,
            parser,
            tree: None,
            stale: true,
        })
    }

    pub fn language(&self) -> &'static Language {
        self.language
    }

    /// Tells the tree that `inserted` is about to be put at char `pos` of
    /// `text`. Must be called before the text changes.
    pub fn before_insert(&mut self, text: &Text, pos: usize, inserted: &str) {
        self.stale = true;
        let Some(tree) = &mut self.tree else { return };
        let start_byte = text.char_to_byte(pos);
        let start_position = point(text, start_byte);
        tree.edit(&InputEdit {
            start_byte,
            old_end_byte: start_byte,
            new_end_byte: start_byte + inserted.len(),
            start_position,
            old_end_position: start_position,
            new_end_position: advance(start_position, inserted),
        });
    }

    /// Tells the tree that a range of chars of `text` is about to be
    /// removed. Must be called before the text changes.
    pub fn before_delete(&mut self, text: &Text, range: Range<usize>) {
        self.stale = true;
        let Some(tree) = &mut self.tree else { return };
        let start_byte = text.char_to_byte(range.start);
        let old_end_byte = text.char_to_byte(range.end);
        let start_position = point(text, start_byte);
        tree.edit(&InputEdit {
            start_byte,
            old_end_byte,
            new_end_byte: start_byte,
            start_position,
            old_end_position: point(text, old_end_byte),
            new_end_position: start_position,
        });
    }

    /// Forgets the tree, for when the text was replaced wholesale.
    pub fn reset(&mut self) {
        self.tree = None;
        self.stale = true;
    }

    /// Brings the tree up to date with the text. Cheap when nothing changed,
    /// and after small edits only the changed part is parsed again.
    pub fn update(&mut self, text: &Text) {
        if !self.stale {
            return;
        }
        self.stale = false;
        if text.len_bytes() > MAX_PARSE_BYTES {
            self.tree = None;
            return;
        }
        let mut read = |byte: usize, _: Point| text.chunk_at_byte(byte);
        let reused = self.tree.is_some();
        self.tree = self
            .parser
            .parse_with_options(&mut read, self.tree.as_ref(), None);
        // Reusing a tree across edits can recover from a syntax error in
        // another way than parsing afresh does, in some grammars. Where
        // parsing afresh is cheap, text with errors gets that, so that the
        // tree depends on the text alone.
        let broken = self
            .tree
            .as_ref()
            .is_some_and(|tree| tree.root_node().has_error());
        if reused && broken && text.len_bytes() <= REPARSE_BYTES {
            self.tree = self.parser.parse_with_options(&mut read, None, None);
        }
    }

    /// The tree as of the last [`Syntax::update`].
    pub(crate) fn tree(&self) -> Option<&Tree> {
        self.tree.as_ref().filter(|_| !self.stale)
    }

    /// The tree in the notation of tree-sitter's tests, for debugging.
    pub fn sexp(&self) -> Option<String> {
        self.tree().map(|tree| tree.root_node().to_sexp())
    }

    /// Highlights for a range of bytes, which must lie on char boundaries.
    pub fn highlights(&self, text: &Text, range: Range<usize>) -> Highlights {
        let mut out = Highlights {
            start: range.start,
            bytes: vec![0; range.len()],
        };
        let Some(tree) = self.tree() else { return out };

        paint(
            self.grammar,
            self.language.first_pattern_wins,
            tree.root_node(),
            text,
            &mut out,
        );
        if let Some(injections) = self.language.injections {
            paint_injections(injections, tree, text, &mut out);
        }
        out
    }
}

/// Paints what a grammar's query captures under `root` over the highlights.
fn paint(grammar: &Grammar, first_wins: bool, root: Node, text: &Text, out: &mut Highlights) {
    let range = out.start..out.start + out.bytes.len();
    let mut cursor = QueryCursor::new();
    cursor.set_byte_range(range.clone());
    let provider = |node: Node| text.byte_chunks(node.byte_range()).map(str::as_bytes);
    let mut matches = cursor.matches(&grammar.query, root, provider);
    // (start, end, pattern, highlight) of everything captured.
    let mut found: Vec<(usize, usize, usize, u8)> = Vec::new();
    while let Some(found_match) = matches.next() {
        for capture in found_match.captures() {
            let highlight = match grammar.captures[capture.index as usize] {
                Capture::Ignore => continue,
                Capture::Plain => 0,
                Capture::Is(highlight) => highlight as u8 + 1,
            };
            let node = capture.node.byte_range();
            found.push((node.start, node.end, found_match.pattern_index, highlight));
        }
    }
    // Paint outer nodes before the nodes within them, and for one node
    // the losing pattern before the winning one, so that what should
    // show is painted last.
    found.sort_by(|a, b| {
        let patterns = if first_wins {
            b.2.cmp(&a.2)
        } else {
            a.2.cmp(&b.2)
        };
        a.0.cmp(&b.0).then(b.1.cmp(&a.1)).then(patterns)
    });
    for (start, end, _, highlight) in found {
        let from = start.max(range.start) - range.start;
        let to = end.min(range.end).saturating_sub(range.start);
        if from < to {
            out.bytes[from..to].fill(highlight);
        }
    }
}

/// Text in another language that is longer than this many bytes is left
/// as the outer language shows it: it is parsed anew whenever it is shown.
const MAX_INJECTED_BYTES: usize = 128 << 10;

/// Paints the text in other languages that is among the highlights asked
/// for, each piece parsed by itself with the grammar of its language.
fn paint_injections(injections: &Injections, tree: &Tree, text: &Text, out: &mut Highlights) {
    let range = out.start..out.start + out.bytes.len();
    let mut parser = Parser::new();
    let mut cursor = tree.walk();
    'walk: loop {
        let node = cursor.node();
        let shown = node.end_byte() > range.start && node.start_byte() < range.end;
        let injected = if shown {
            injection(injections, node, text)
        } else {
            None
        };
        if let Some((language, content, whole)) = injected {
            paint_injected(&mut parser, language, content, whole, text, out);
        }
        // What is in another language has nothing of this one within it.
        if !shown || injected.is_some() || !cursor.goto_first_child() {
            while !cursor.goto_next_sibling() {
                if !cursor.goto_parent() {
                    break 'walk;
                }
            }
        }
    }
}

/// The language of the text in a node, if it is in another, with the node
/// that holds the text and whether the language owns all of the text, so
/// that what it does not highlight is plain.
fn injection<'tree>(
    injections: &Injections,
    node: Node<'tree>,
    text: &Text,
) -> Option<(&'static Language, Node<'tree>, bool)> {
    let kind = node.kind();
    if let Some((_, name)) = injections.kinds.iter().find(|(k, _)| *k == kind) {
        return Some((crate::languages::language(name)?, node, false));
    }
    let (_, name, content) = injections.named.iter().find(|(k, _, _)| *k == kind)?;
    let name = descendant(node, name)?;
    let name: String = text.byte_chunks(name.byte_range()).collect();
    let language = crate::languages::language_called(&name)?;
    Some((language, descendant(node, content)?, true))
}

/// The first node of a kind under a node, looking no deeper than two levels.
fn descendant<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            return Some(child);
        }
        let mut inner = child.walk();
        if let Some(found) = child.children(&mut inner).find(|n| n.kind() == kind) {
            return Some(found);
        }
    }
    None
}

fn paint_injected(
    parser: &mut Parser,
    language: &'static Language,
    content: Node,
    whole: bool,
    text: &Text,
    out: &mut Highlights,
) {
    let Some(grammar) = language.compiled() else {
        return;
    };
    if content.byte_range().len() > MAX_INJECTED_BYTES {
        return;
    }
    // The node's named children belong to the outer language: the markers
    // of a quote or a list at the start of each line within it.
    let mut ranges = Vec::new();
    let mut rest = content.range();
    let mut cursor = content.walk();
    for child in content.named_children(&mut cursor) {
        let child = child.range();
        if child.start_byte > rest.start_byte {
            ranges.push(tree_sitter::Range {
                end_byte: child.start_byte,
                end_point: child.start_point,
                ..rest
            });
        }
        rest.start_byte = child.end_byte;
        rest.start_point = child.end_point;
    }
    if rest.start_byte < rest.end_byte {
        ranges.push(rest);
    }
    if ranges.is_empty()
        || parser.set_language(&grammar.language).is_err()
        || parser.set_included_ranges(&ranges).is_err()
    {
        return;
    }
    let mut read = |byte: usize, _: Point| text.chunk_at_byte(byte);
    let Some(tree) = parser.parse_with_options(&mut read, None, None) else {
        return;
    };
    let shown = out.start..out.start + out.bytes.len();
    let within = |range: &tree_sitter::Range| {
        let from = range.start_byte.max(shown.start) - shown.start;
        let to = range.end_byte.min(shown.end).saturating_sub(shown.start);
        from..to.max(from)
    };
    // A node of the injected language can span what was left out of it,
    // which is to stay as it is.
    let before = out.bytes.clone();
    if whole {
        for range in &ranges {
            out.bytes[within(range)].fill(0);
        }
    }
    paint(
        grammar,
        language.first_pattern_wins,
        tree.root_node(),
        text,
        out,
    );
    for gap in ranges.windows(2) {
        let gap = within(&tree_sitter::Range {
            start_byte: gap[0].end_byte,
            end_byte: gap[1].start_byte,
            ..gap[0]
        });
        out.bytes[gap.clone()].copy_from_slice(&before[gap]);
    }
}
