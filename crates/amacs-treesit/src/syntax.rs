//! The syntax tree of one buffer, kept up to date as the text changes.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;

use amacs_text::Text;
use tree_sitter::{InputEdit, Node, Parser, Point, QueryCursor, StreamingIterator, Tree};

use crate::highlight::{Capture, Highlight};
use crate::languages::{Grammar, Injections, Language};
use crate::org::Org;

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
    /// The highlights of a range within these.
    fn part(&self, range: Range<usize>) -> Highlights {
        Highlights {
            start: range.start,
            bytes: self.bytes[range.start - self.start..range.end - self.start].to_vec(),
        }
    }

    /// The highlight of the char that starts at byte `byte`.
    pub fn get(&self, byte: usize) -> Option<Highlight> {
        let index = *self.bytes.get(byte.checked_sub(self.start)?)?;
        Highlight::from_index(index.checked_sub(1)?)
    }

    /// The bytes that these are the highlights of.
    pub(crate) fn range(&self) -> Range<usize> {
        self.start..self.start + self.bytes.len()
    }

    /// Gives the part of a range of bytes that is among these a highlight,
    /// or none.
    pub(crate) fn fill(&mut self, range: Range<usize>, highlight: Option<Highlight>) {
        let shown = self.range();
        let from = range.start.clamp(shown.start, shown.end) - shown.start;
        let to = range.end.clamp(shown.start, shown.end) - shown.start;
        if from < to {
            self.bytes[from..to].fill(highlight.map_or(0, |highlight| highlight as u8 + 1));
        }
    }
}

/// A buffer's syntax tree. A language that is read line by line, as Org
/// is, has no tree, and what is known of its text instead.
pub struct Syntax {
    language: &'static Language,
    grammar: Option<&'static Grammar>,
    parser: Parser,
    tree: Option<Tree>,
    lines: Option<Org>,
    /// The chars that are markup, as of the last [`Syntax::update_markup`],
    /// and whether the text has changed since.
    markup: Vec<Range<usize>>,
    /// The headings, likewise: the char that each starts at, and its level.
    headings: Vec<(usize, usize)>,
    markup_stale: bool,
    /// The markup found in each piece of text in another language, by a
    /// hash of the piece and as bytes from its start, so that a piece that
    /// did not change is not parsed again.
    concealed: HashMap<u64, Vec<Range<usize>>>,
    /// Whether the text has changed since the tree was parsed.
    stale: bool,
    /// Whether the tree has syntax errors.
    broken: bool,
    /// The highlights that were asked for since the tree was parsed: the
    /// same are asked for each time the text is drawn, and most of the
    /// time the text has not changed in between.
    painted: RefCell<Vec<Highlights>>,
}

/// The ranges of highlights that are remembered: one for each of a few
/// windows on the same text.
const PAINTED_KEPT: usize = 4;

impl std::fmt::Debug for Syntax {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Syntax({})", self.language.name)
    }
}

pub(crate) fn point(text: &Text, byte: usize) -> Point {
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
    /// A syntax tree for a language, or `None` if the language has no
    /// grammar and is not read line by line either.
    pub fn new(language: &'static Language) -> Option<Self> {
        let mut parser = Parser::new();
        let (grammar, lines) = if language.by_lines {
            (None, Some(Org::default()))
        } else {
            let grammar = language.compiled()?;
            parser.set_language(&grammar.language).ok()?;
            (Some(grammar), None)
        };
        Some(Self {
            language,
            grammar,
            parser,
            tree: None,
            lines,
            markup: Vec::new(),
            headings: Vec::new(),
            markup_stale: true,
            concealed: HashMap::new(),
            stale: true,
            broken: false,
            painted: RefCell::new(Vec::new()),
        })
    }

    /// Says which words mark a heading as still to do and which as done, in
    /// a language that has headings with such words.
    pub fn set_keywords(&mut self, not_done: Vec<String>, done: Vec<String>) {
        if let Some(lines) = &mut self.lines {
            lines.set_keywords(not_done, done);
            self.stale = true;
            self.markup_stale = true;
        }
    }

    pub fn language(&self) -> &'static Language {
        self.language
    }

    /// Tells the tree that `inserted` is about to be put at char `pos` of
    /// `text`. Must be called before the text changes.
    pub fn before_insert(&mut self, text: &Text, pos: usize, inserted: &str) {
        self.stale = true;
        self.markup_stale = true;
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
        self.markup_stale = true;
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
        self.markup_stale = true;
    }

    /// Brings the tree up to date with the text. Cheap when nothing changed,
    /// and after small edits only the changed part is parsed again.
    pub fn update(&mut self, text: &Text) {
        if !self.stale {
            return;
        }
        self.stale = false;
        self.painted.get_mut().clear();
        if let Some(lines) = &mut self.lines {
            lines.scan(text);
            return;
        }
        if text.len_bytes() > MAX_PARSE_BYTES {
            self.tree = None;
            return;
        }
        let mut read = |byte: usize, _: Point| text.chunk_at_byte(byte);
        // Reusing a tree across edits can recover from a syntax error in
        // another way than parsing afresh does, in some grammars. Where
        // parsing afresh is cheap, text with errors gets that, so that the
        // tree depends on the text alone. A tree that had errors is not
        // tried first: text that is being typed has them for a while, and
        // each key would have the text parsed twice.
        let afresh = text.len_bytes() <= REPARSE_BYTES;
        let old = self.tree.take().filter(|_| !(self.broken && afresh));
        let broken = |tree: &Option<Tree>| {
            tree.as_ref()
                .is_some_and(|tree| tree.root_node().has_error())
        };
        self.tree = self
            .parser
            .parse_with_options(&mut read, old.as_ref(), None);
        if old.is_some() && afresh && broken(&self.tree) {
            self.tree = self.parser.parse_with_options(&mut read, None, None);
        }
        self.broken = broken(&self.tree);
    }

    /// Finds anew the chars of the text that are markup: what stands
    /// around text to say how it is meant, such as the stars of `*strong*`,
    /// and can be left out of what is drawn. Call after [`Syntax::update`];
    /// cheap when nothing changed. A language with a grammar says what its
    /// markup is with the captures `conceal` and `conceal.rest` of its
    /// query. The headings of the text are found with it, in Org.
    pub fn update_markup(&mut self, text: &Text) {
        if !std::mem::take(&mut self.markup_stale) {
            return;
        }
        if self.lines.is_none() {
            self.markup = self.captured_markup(text);
            return;
        }
        self.headings = match self.lines.as_ref().filter(|_| !self.stale) {
            Some(lines) => lines
                .headings()
                .iter()
                .map(|(byte, level)| (text.byte_to_char(*byte), *level))
                .collect(),
            None => Vec::new(),
        };
        self.markup = match self.lines.as_ref().filter(|_| !self.stale) {
            Some(lines) => lines
                .markup(text)
                .into_iter()
                .map(|bytes| text.byte_to_char(bytes.start)..text.byte_to_char(bytes.end))
                .collect(),
            None => Vec::new(),
        };
    }

    /// The chars that the queries of the language and of the languages
    /// within it say are markup, in order and apart. Markup that goes
    /// across the end of a line is not among them: it stays as written.
    fn captured_markup(&mut self, text: &Text) -> Vec<Range<usize>> {
        let known = std::mem::take(&mut self.concealed);
        let (Some(tree), Some(grammar)) = (self.tree(), self.grammar) else {
            return Vec::new();
        };
        let mut found = Vec::new();
        if grammar.conceals {
            conceal(grammar, tree.root_node(), text, 0, &mut found);
        }
        let mut next = HashMap::new();
        if let Some(injections) = self.language.injections {
            let mut parser = Parser::new();
            each_injection(injections, tree, text, None, |language, content, _| {
                let grammar = language.compiled().filter(|grammar| grammar.conceals);
                let bytes = content.byte_range();
                let (Some(grammar), false, false) = (
                    grammar,
                    injections.as_written.contains(&content.kind()),
                    bytes.len() > MAX_INJECTED_BYTES,
                ) else {
                    return;
                };
                let pieces = pieces(content);
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                language.name.hash(&mut hasher);
                for chunk in text.byte_chunks(bytes.clone()) {
                    hasher.write(chunk.as_bytes());
                }
                for piece in &pieces {
                    (piece.start_byte - bytes.start, piece.end_byte - bytes.start)
                        .hash(&mut hasher);
                }
                let key = hasher.finish();
                let within = match known.get(&key).or_else(|| next.get(&key)) {
                    Some(within) => within.clone(),
                    None => {
                        let mut within = Vec::new();
                        if let Some(tree) = parse_pieces(&mut parser, grammar, &pieces, text) {
                            conceal(grammar, tree.root_node(), text, bytes.start, &mut within);
                        }
                        within
                    }
                };
                found.extend(
                    within
                        .iter()
                        .map(|range| range.start + bytes.start..range.end + bytes.start),
                );
                next.insert(key, within);
            });
        }
        self.concealed = next;
        found.retain(|range: &Range<usize>| {
            range.start < range.end
                && text.byte_to_line(range.start) == text.byte_to_line(range.end)
        });
        found.sort_by_key(|range| (range.start, range.end));
        let mut markup: Vec<Range<usize>> = Vec::with_capacity(found.len());
        for range in found {
            match markup.last_mut() {
                Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
                _ => markup.push(range),
            }
        }
        for range in &mut markup {
            *range = text.byte_to_char(range.start)..text.byte_to_char(range.end);
        }
        markup
    }

    /// The chars that are markup, as ranges in order and apart, as of the
    /// last [`Syntax::update_markup`].
    pub fn markup(&self) -> &[Range<usize>] {
        &self.markup
    }

    /// The headings of the text, in order, each as the char that its line
    /// starts at and its level from 1, as of the last
    /// [`Syntax::update_markup`].
    pub fn headings(&self) -> &[(usize, usize)] {
        &self.headings
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
        if self.stale {
            return self.paint(text, range);
        }
        // What was found for the tree as it is holds until the text
        // changes.
        let mut painted = self.painted.borrow_mut();
        let ends = |found: &Highlights| found.start..found.start + found.bytes.len();
        let holds = |found: &Highlights| {
            let known = ends(found);
            known.start <= range.start && range.end <= known.end
        };
        if let Some(found) = painted.iter().find(|found| holds(found)) {
            return found.part(range);
        }
        // A window that scrolls asks for a range next to the one before:
        // more is painted then, on both sides, so that the next rows it
        // scrolls to are known. After a change to the text only what is
        // asked for is painted, since typing would pay for the rest with
        // each key.
        let scrolled = painted.iter().position(|found| {
            let known = ends(found);
            known.start < range.end && range.start < known.end
        });
        let wanted = match scrolled {
            Some(at) => {
                painted.remove(at);
                let snap = |byte: usize| text.char_to_byte(text.byte_to_char(byte));
                snap(range.start.saturating_sub(range.len()))
                    ..snap((range.end + range.len()).min(text.len_bytes()))
            }
            None => range.clone(),
        };
        let out = self.paint(text, wanted);
        let part = out.part(range);
        if painted.len() == PAINTED_KEPT {
            painted.remove(0);
        }
        painted.push(out);
        part
    }

    fn paint(&self, text: &Text, range: Range<usize>) -> Highlights {
        let mut out = Highlights {
            start: range.start,
            bytes: vec![0; range.len()],
        };
        if let Some(lines) = self.lines.as_ref().filter(|_| !self.stale) {
            lines.paint(text, &mut out);
            return out;
        }
        let (Some(tree), Some(grammar)) = (self.tree(), self.grammar) else {
            return out;
        };

        paint(
            grammar,
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
                Capture::Ignore | Capture::Conceal | Capture::ConcealRest => continue,
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

/// Adds what a grammar's query says is markup under `root` to `out`, as
/// ranges of bytes counted from `origin`.
fn conceal(grammar: &Grammar, root: Node, text: &Text, origin: usize, out: &mut Vec<Range<usize>>) {
    let mut cursor = QueryCursor::new();
    let provider = |node: Node| text.byte_chunks(node.byte_range()).map(str::as_bytes);
    let mut matches = cursor.matches(&grammar.query, root, provider);
    while let Some(found) = matches.next() {
        for capture in found.captures() {
            let node = capture.node;
            let end = match grammar.captures[capture.index as usize] {
                Capture::Conceal => node.end_byte(),
                Capture::ConcealRest => node.parent().unwrap_or(node).end_byte(),
                _ => continue,
            };
            out.push(node.start_byte().saturating_sub(origin)..end.saturating_sub(origin));
        }
    }
}

/// Text in another language that is longer than this many bytes is left
/// as the outer language shows it: it is parsed anew whenever it is shown.
const MAX_INJECTED_BYTES: usize = 128 << 10;

/// Paints the text in other languages that is among the highlights asked
/// for, each piece parsed by itself with the grammar of its language.
fn paint_injections(injections: &Injections, tree: &Tree, text: &Text, out: &mut Highlights) {
    let mut parser = Parser::new();
    let range = out.range();
    each_injection(
        injections,
        tree,
        text,
        Some(range),
        |language, content, whole| {
            paint_injected(&mut parser, language, content, whole, text, out);
        },
    );
}

/// Calls `found` for each piece of the text that is in another language,
/// or for those that reach into a range of bytes: with the language, the
/// node that holds the text, and whether the language owns all of it.
fn each_injection<'tree>(
    injections: &Injections,
    tree: &'tree Tree,
    text: &Text,
    range: Option<Range<usize>>,
    mut found: impl FnMut(&'static Language, Node<'tree>, bool),
) {
    let mut cursor = tree.walk();
    'walk: loop {
        let node = cursor.node();
        let shown = range
            .as_ref()
            .is_none_or(|range| node.end_byte() > range.start && node.start_byte() < range.end);
        let injected = if shown {
            injection(injections, node, text)
        } else {
            None
        };
        if let Some((language, content, whole)) = injected {
            found(language, content, whole);
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
    if content.byte_range().len() > MAX_INJECTED_BYTES {
        return;
    }
    paint_ranges(parser, language, &pieces(content), whole, text, out);
}

/// The pieces of a node that are in the language of its text. The node's
/// named children belong to the outer language: the markers of a quote or
/// a list at the start of each line within it.
fn pieces(content: Node) -> Vec<tree_sitter::Range> {
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
    ranges
}

/// Parses pieces of the text as one text in a language.
fn parse_pieces(
    parser: &mut Parser,
    grammar: &Grammar,
    ranges: &[tree_sitter::Range],
    text: &Text,
) -> Option<Tree> {
    let bytes: usize = ranges
        .iter()
        .map(|range| range.end_byte - range.start_byte)
        .sum();
    if bytes == 0
        || bytes > MAX_INJECTED_BYTES
        || parser.set_language(&grammar.language).is_err()
        || parser.set_included_ranges(ranges).is_err()
    {
        return None;
    }
    let mut read = |byte: usize, _: Point| text.chunk_at_byte(byte);
    parser.parse_with_options(&mut read, None, None)
}

/// Paints pieces of the text that are in another language, parsed as one
/// text in that language. If the language owns all of the pieces, what it
/// does not highlight in them is plain.
pub(crate) fn paint_ranges(
    parser: &mut Parser,
    language: &'static Language,
    ranges: &[tree_sitter::Range],
    whole: bool,
    text: &Text,
    out: &mut Highlights,
) {
    let Some(grammar) = language.compiled() else {
        return;
    };
    let Some(tree) = parse_pieces(parser, grammar, ranges, text) else {
        return;
    };
    let shown = out.start..out.start + out.bytes.len();
    let within = |range: &tree_sitter::Range| {
        // The part of it that is among the highlights asked for, which may
        // be none.
        let from = range.start_byte.clamp(shown.start, shown.end) - shown.start;
        let to = range.end_byte.clamp(shown.start, shown.end) - shown.start;
        from..to.max(from)
    };
    // A node of the injected language can span what was left out of it,
    // which is to stay as it is.
    let before = out.bytes.clone();
    if whole {
        for range in ranges {
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
