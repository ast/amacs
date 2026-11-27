//! The syntax tree of one buffer, kept up to date as the text changes.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::{ControlFlow, Range};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use amacs_text::Text;
use tree_sitter::{
    InputEdit, Node, ParseOptions, ParseState, Parser, Point, QueryCursor, StreamingIterator, Tree,
};

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

    /// Follows a change to the text: the bytes from `start` to `old_end`
    /// became `new_len` bytes, which have no highlight yet. Returns
    /// whether these still are the highlights of one stretch of text; they
    /// are not when the change went across their start.
    fn follow(&mut self, start: usize, old_end: usize, new_len: usize) -> bool {
        let known = self.range();
        if start >= known.end {
            return true;
        }
        if old_end <= known.start {
            self.start = known.start - (old_end - start) + new_len;
            return true;
        }
        if start < known.start {
            return false;
        }
        let from = start - known.start;
        let to = old_end.min(known.end) - known.start;
        self.bytes.splice(from..to, std::iter::repeat_n(0, new_len));
        true
    }

    /// Takes over the highlights of another, where they are among these.
    fn take(&mut self, other: &Highlights) {
        let (known, given) = (self.range(), other.range());
        let from = given.start.max(known.start);
        let to = given.end.min(known.end);
        if from < to {
            self.bytes[from - known.start..to - known.start]
                .copy_from_slice(&other.bytes[from - given.start..to - given.start]);
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
    /// The bytes whose remembered highlights a change to the text has made
    /// doubtful, as they are after the change: they are painted again at
    /// the next [`Syntax::update`], and the rest is kept. `None` when
    /// nothing changed, or when what is remembered was dropped instead.
    doubtful: Option<Range<usize>>,
    /// Counts the changes to the text.
    generation: u64,
    /// The count that the thread that parses is asked about last.
    newest: Arc<AtomicU64>,
    /// Where the tree of a parse on that thread comes.
    pending: Option<Receiver<(u64, Option<Tree>)>>,
    /// Whether the tree is not that of the text: the text was changed
    /// since the tree was parsed, and the tree only told where. That is
    /// what is drawn from while another thread parses the text.
    rough: bool,
}

/// How long [`Syntax::update_soon`] parses before it leaves the rest to
/// another thread: more than a change to text without errors takes, and
/// little of the time that a key press may take.
const PARSE_BUDGET: Duration = Duration::from_millis(1);

/// Called from the thread that parses when a tree is ready, to wake the
/// thread that owns the text.
pub type Notify = Arc<dyn Fn() + Send + Sync>;

/// A text to parse on the thread for that.
struct Job {
    language: tree_sitter::Language,
    text: Text,
    /// Which state of the text this is, and the newest that is asked
    /// about: a job for an older one is not worth doing.
    generation: u64,
    newest: Arc<AtomicU64>,
    reply: Sender<(u64, Option<Tree>)>,
    notify: Notify,
}

/// The thread that parses texts whole, away from the one that draws.
/// `None` if it could not be started.
fn parse_thread() -> Option<&'static Sender<Job>> {
    static THREAD: OnceLock<Option<Sender<Job>>> = OnceLock::new();
    THREAD
        .get_or_init(|| {
            let (sender, jobs) = mpsc::channel::<Job>();
            std::thread::Builder::new()
                .name("parse".to_owned())
                .spawn(move || {
                    let mut parser = Parser::new();
                    for job in jobs {
                        let wanted = || job.newest.load(Ordering::Relaxed) == job.generation;
                        if !wanted() || parser.set_language(&job.language).is_err() {
                            continue;
                        }
                        let mut read = |byte: usize, _: Point| job.text.chunk_at_byte(byte);
                        let mut go_on = |_: &ParseState| {
                            if wanted() {
                                ControlFlow::Continue(())
                            } else {
                                ControlFlow::Break(())
                            }
                        };
                        let options = ParseOptions::new().progress_callback(&mut go_on);
                        let tree = parser.parse_with_options(&mut read, None, Some(options));
                        parser.reset();
                        if tree.is_some() && job.reply.send((job.generation, tree)).is_ok() {
                            (job.notify)();
                        }
                    }
                })
                .ok()
                .map(|_| sender)
        })
        .as_ref()
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
            doubtful: None,
            generation: 0,
            newest: Arc::new(AtomicU64::new(0)),
            pending: None,
            rough: false,
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
        let start_byte = text.char_to_byte(pos);
        self.before_change(text, start_byte..start_byte, inserted.len());
        self.stale = true;
        self.markup_stale = true;
        let Some(tree) = &mut self.tree else { return };
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
        let start_byte = text.char_to_byte(range.start);
        let old_end_byte = text.char_to_byte(range.end);
        self.before_change(text, start_byte..old_end_byte, 0);
        self.stale = true;
        self.markup_stale = true;
        let Some(tree) = &mut self.tree else { return };
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

    fn forget_painted(&mut self) {
        self.painted.get_mut().clear();
        self.doubtful = None;
    }

    /// Prepares the remembered highlights for a change to the text, which
    /// is still as it was: the bytes of `old` are about to become
    /// `new_len` bytes.
    ///
    /// Most of what is remembered holds after one change: a highlight
    /// comes from a match of the query, and a match away from the change
    /// and from where the tree comes out different is the same match
    /// after. So the highlights follow the change, and what is doubtful
    /// is noted for [`Syntax::update`] to paint again. Here that is what
    /// the matches at the change cover, which the text may no longer
    /// match after: the tree does not tell that.
    ///
    /// The second change before an update has no tree to ask, and text
    /// in other languages is parsed as a whole each time, so in both cases
    /// what is remembered is dropped.
    fn before_change(&mut self, text: &Text, old: Range<usize>, new_len: usize) {
        self.generation += 1;
        if self.painted.get_mut().is_empty() {
            self.doubtful = None;
            return;
        }
        let asked = match (&self.tree, self.grammar) {
            (Some(tree), Some(grammar))
                if !self.stale
                    && !self.rough
                    && self.lines.is_none()
                    && self.language.injections.is_none() =>
            {
                Some((tree, grammar))
            }
            _ => None,
        };
        let Some((tree, grammar)) = asked else {
            self.forget_painted();
            return;
        };
        let around = old.start.saturating_sub(1)..(old.end + 1).min(text.len_bytes());
        let covered = covered(grammar, tree.root_node(), text, around);
        // Where a byte of before the change is after it.
        let after = |byte: usize| {
            if byte >= old.end {
                byte - old.end + old.start + new_len
            } else {
                byte.min(old.start)
            }
        };
        self.doubtful = Some(after(covered.start)..after(covered.end).max(old.start + new_len));
        self.painted
            .get_mut()
            .retain_mut(|found| found.follow(old.start, old.end, new_len));
    }

    /// Forgets the tree, for when the text was replaced wholesale.
    pub fn reset(&mut self) {
        self.generation += 1;
        self.forget_painted();
        self.pending = None;
        self.rough = false;
        self.tree = None;
        self.stale = true;
        self.markup_stale = true;
    }

    /// Brings the tree up to date with the text. Cheap when nothing changed,
    /// and after small edits only the changed part is parsed again.
    pub fn update(&mut self, text: &Text) {
        if !self.stale && !self.rough {
            return;
        }
        self.stale = false;
        self.rough = false;
        self.pending = None;
        let doubtful = self.doubtful.take();
        if doubtful.is_none() {
            self.painted.get_mut().clear();
        }
        if let Some(lines) = &mut self.lines {
            lines.scan(text);
            return;
        }
        if text.len_bytes() > MAX_PARSE_BYTES {
            self.tree = None;
            self.forget_painted();
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
        let mut grown = old.is_some();
        if old.is_some() && afresh && broken(&self.tree) {
            self.tree = self.parser.parse_with_options(&mut read, None, None);
            grown = false;
        }
        self.broken = broken(&self.tree);
        // What is remembered of the highlights is brought up to date, if
        // the new tree was made from the old one: only then do the two
        // say where they differ.
        match (doubtful, old, grown) {
            (Some(doubtful), Some(old), true) => self.paint_again(text, &old, doubtful),
            _ => self.forget_painted(),
        }
    }

    /// Whether another thread is parsing the text: what is drawn now is
    /// drawn from a tree of before the last changes, and will be drawn
    /// again when that thread is done.
    pub fn is_parsing(&self) -> bool {
        self.pending.is_some()
    }

    /// Brings the tree up to date with the text as [`Syntax::update`]
    /// does, but without making a key press wait for it: for drawing.
    ///
    /// Text without syntax errors is parsed here, which takes little
    /// after a change. Text with errors is parsed afresh, all of it, and
    /// that is left to another thread: `notify` is called from it when
    /// the tree is there, and the next call of this takes the tree. Until
    /// then the tree is the one of before, told of the changes, so that
    /// what is drawn from it is right nearly everywhere.
    pub fn update_soon(&mut self, text: &Text, notify: &Notify) {
        self.take_parsed();
        if !self.stale {
            return;
        }
        let Some(grammar) = self.grammar else {
            return self.update(text);
        };
        // A long text is parsed here once it has a tree, since a change
        // to it is quick to parse and it is not parsed afresh. Its first
        // parse takes long, and is left to the other thread: the text is
        // drawn without highlights until then.
        let long = text.len_bytes() > REPARSE_BYTES;
        let first = self.tree.is_none();
        if self.language.injections.is_some() || long != first || text.len_bytes() > MAX_PARSE_BYTES
        {
            return self.update(text);
        }
        let Some(thread) = parse_thread() else {
            return self.update(text);
        };
        self.stale = false;
        let doubtful = self.doubtful.take();
        if !first && !self.broken && !self.rough {
            let mut read = |byte: usize, _: Point| text.chunk_at_byte(byte);
            let started = Instant::now();
            let mut in_time = |_: &ParseState| {
                if started.elapsed() < PARSE_BUDGET {
                    ControlFlow::Continue(())
                } else {
                    ControlFlow::Break(())
                }
            };
            let options = ParseOptions::new().progress_callback(&mut in_time);
            let parsed =
                self.parser
                    .parse_with_options(&mut read, self.tree.as_ref(), Some(options));
            match parsed {
                Some(tree) if !tree.root_node().has_error() => {
                    let old = self.tree.replace(tree);
                    match (doubtful, old) {
                        (Some(doubtful), Some(old)) => self.paint_again(text, &old, doubtful),
                        _ => self.forget_painted(),
                    }
                    return;
                }
                // With errors, which a parse afresh may see otherwise:
                // good to draw from until that is there.
                Some(tree) => self.tree = Some(tree),
                // Out of time. The parser would go on from there if asked
                // again, but the text will not be the same then.
                None => self.parser.reset(),
            }
        }
        self.forget_painted();
        self.rough = true;
        let (reply, pending) = mpsc::channel();
        self.newest.store(self.generation, Ordering::Relaxed);
        let job = Job {
            language: grammar.language.clone(),
            text: text.clone(),
            generation: self.generation,
            newest: self.newest.clone(),
            reply,
            notify: notify.clone(),
        };
        if thread.send(job).is_ok() {
            self.pending = Some(pending);
        } else {
            self.update(text);
        }
    }

    /// Takes the tree that another thread has parsed, if it is there and
    /// the text is still the one it was parsed from.
    fn take_parsed(&mut self) {
        let Some(pending) = &self.pending else { return };
        let Some((_, tree)) = pending
            .try_iter()
            .find(|(generation, _)| *generation == self.generation)
        else {
            return;
        };
        self.pending = None;
        if self.stale {
            return;
        }
        self.broken = tree
            .as_ref()
            .is_some_and(|tree| tree.root_node().has_error());
        self.tree = tree;
        self.rough = false;
        self.markup_stale = true;
        self.forget_painted();
    }

    /// Paints the remembered highlights again where a change to the text
    /// can have changed them: `doubtful`, and where the tree is not what
    /// `old` was, with what the matches of the query there cover.
    fn paint_again(&mut self, text: &Text, old: &Tree, doubtful: Range<usize>) {
        let (Some(tree), Some(grammar)) = (&self.tree, self.grammar) else {
            self.forget_painted();
            return;
        };
        let mut again = doubtful;
        for changed in old.changed_ranges(tree) {
            again.start = again.start.min(changed.start_byte);
            again.end = again.end.max(changed.end_byte);
        }
        // Whole lines: what a line shows is mostly decided within it.
        let len = text.len_bytes();
        again.end = again.end.min(len);
        again.start = text.line_to_byte(text.byte_to_line(again.start.min(len)));
        let last_line = text.byte_to_line(again.end);
        again.end = if last_line + 1 < text.len_lines() {
            text.line_to_byte(last_line + 1)
        } else {
            len
        };
        let again = covered(grammar, tree.root_node(), text, again);
        let remembered = std::mem::take(self.painted.get_mut());
        let mut kept = Vec::with_capacity(remembered.len());
        for mut found in remembered {
            let known = found.range();
            let from = again.start.max(known.start);
            let to = again.end.min(known.end);
            if from < to {
                found.take(&self.paint(text, from..to));
            }
            kept.push(found);
        }
        *self.painted.get_mut() = kept;
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
        // A window that scrolls asks for a range next to the one before,
        // and one whose text changed for one that ends elsewhere. What is
        // missing is painted and joined to what is known, and when it is
        // more than a line, as much again on that side, so that the rows
        // that the window scrolls to next are known too.
        let near = painted.iter().position(|found| {
            let known = ends(found);
            known.start <= range.end && range.start <= known.end
        });
        let Some(near) = near else {
            let out = self.paint(text, range);
            if painted.len() == PAINTED_KEPT {
                painted.remove(0);
            }
            painted.push(out.clone());
            return out;
        };
        let mut found = painted.remove(near);
        let known = ends(&found);
        let snap = |byte: usize| text.char_to_byte(text.byte_to_char(byte));
        let more = |missing: usize| if missing > 256 { range.len() } else { 0 };
        if range.start < known.start {
            let from = snap(range.start.saturating_sub(more(known.start - range.start)));
            let before = self.paint(text, from..known.start);
            found.bytes.splice(0..0, before.bytes);
            found.start = from;
        }
        if range.end > known.end {
            let to = snap((range.end + more(range.end - known.end)).min(text.len_bytes()));
            let after = self.paint(text, known.end..to);
            found.bytes.extend(after.bytes);
        }
        // What is far from the window is let go, so that scrolling through
        // a text does not come to remember all of it.
        let known = ends(&found);
        let keep = range.start.saturating_sub(2 * range.len()).max(known.start)
            ..(range.end + 2 * range.len()).min(known.end);
        if keep != known {
            found = found.part(keep);
        }
        let part = found.part(range);
        painted.push(found);
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
    let provider = |node: Node| node_text(text, node.byte_range());
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

/// The text of a node, for the query to look at. A tree that was told of
/// changes but not parsed since can have a node where no text is, or one
/// that ends within a char: such a node gets what there is.
fn node_text(text: &Text, node: Range<usize>) -> impl Iterator<Item = &[u8]> + '_ {
    let whole = |byte: usize| text.char_to_byte(text.byte_to_char(byte.min(text.len_bytes())));
    let end = whole(node.end);
    let start = whole(node.start).min(end);
    text.byte_chunks(start..end).map(str::as_bytes)
}

/// A range of bytes with what the matches of a grammar's query at it
/// cover: a match gives its captures their highlights together, so when
/// one of them is in doubt, all are.
fn covered(grammar: &Grammar, root: Node, text: &Text, range: Range<usize>) -> Range<usize> {
    let mut cursor = QueryCursor::new();
    cursor.set_byte_range(range.clone());
    let provider = |node: Node| node_text(text, node.byte_range());
    let mut matches = cursor.matches(&grammar.query, root, provider);
    let mut covered = range;
    while let Some(found) = matches.next() {
        for capture in found.captures() {
            let node = capture.node.byte_range();
            covered.start = covered.start.min(node.start);
            covered.end = covered.end.max(node.end);
        }
    }
    covered
}

/// Adds what a grammar's query says is markup under `root` to `out`, as
/// ranges of bytes counted from `origin`.
fn conceal(grammar: &Grammar, root: Node, text: &Text, origin: usize, out: &mut Vec<Range<usize>>) {
    let mut cursor = QueryCursor::new();
    let provider = |node: Node| node_text(text, node.byte_range());
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
