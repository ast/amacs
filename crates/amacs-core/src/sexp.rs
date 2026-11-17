//! Balanced expressions: lists in brackets, strings, and the atoms between
//! them. This is what the commands that move over and edit whole
//! expressions work on, and what Lisp indentation is computed from.
//!
//! The text around a position is read into a small tree. Reading starts at
//! the top-level form that holds the position, which is found by a rule of
//! thumb: an opening bracket in the first column starts a top-level form.

use amacs_text::Text;

use crate::buffer::Buffer;
use crate::display::column_of;

/// How far back, in lines, the start of the enclosing top-level form is
/// looked for.
const MAX_LINES_BACK: usize = 5000;

/// The lexical conventions of a buffer's language.
#[derive(Debug, Clone, Copy)]
pub struct SexpSyntax {
    pub line_comment: Option<&'static str>,
    /// Lisp: symbols contain punctuation, `'` and friends are prefixes.
    pub lisp: bool,
    /// Whether `'` delimits strings.
    pub single_quotes: bool,
}

impl SexpSyntax {
    pub fn of(buffer: &Buffer) -> Self {
        match buffer.language() {
            Some(language) => Self {
                line_comment: language.line_comment,
                lisp: language.indent == amacs_treesit::IndentStyle::Lisp,
                single_quotes: language.pairs_single_quote,
            },
            None => Self {
                line_comment: None,
                lisp: false,
                single_quotes: false,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A bracketed list. `closed` is false if its closing bracket is missing.
    List {
        open: char,
        closed: bool,
    },
    Atom,
    /// A string. `closed` is false if its closing quote is missing.
    Str {
        closed: bool,
    },
    Comment,
    /// A closing bracket with no opening one.
    Stray,
}

/// One expression.
#[derive(Debug, Clone)]
pub struct Node {
    pub kind: Kind,
    /// Start of the expression, including quote prefixes such as `'`.
    pub prefix_start: usize,
    /// For a list, the position of its opening bracket.
    pub start: usize,
    /// Just after the expression's last char.
    pub end: usize,
    pub children: Vec<Node>,
}

impl Node {
    pub fn is_list(&self) -> bool {
        matches!(self.kind, Kind::List { .. })
    }

    pub fn is_closed(&self) -> bool {
        matches!(
            self.kind,
            Kind::List { closed: true, .. } | Kind::Str { closed: true }
        )
    }

    /// Whether `pos` is between the delimiters of this list or string.
    pub fn contains(&self, pos: usize) -> bool {
        self.start < pos && (pos < self.end || !self.is_closed())
    }

    /// The children that are expressions, leaving out comments.
    pub fn sexps(&self) -> impl DoubleEndedIterator<Item = &Node> {
        self.children
            .iter()
            .filter(|child| !matches!(child.kind, Kind::Comment | Kind::Stray))
    }

    /// Position of the closing bracket or quote, if there is one.
    pub fn close_pos(&self) -> Option<usize> {
        self.is_closed().then(|| self.end - 1)
    }
}

pub fn closer_of(open: char) -> char {
    match open {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        other => other,
    }
}

fn is_opener(c: char) -> bool {
    matches!(c, '(' | '[' | '{')
}

fn is_closer(c: char) -> bool {
    matches!(c, ')' | ']' | '}')
}

struct Reader<'a> {
    chars: Vec<char>,
    /// Position in the text of `chars[0]`.
    base: usize,
    index: usize,
    syntax: &'a SexpSyntax,
}

impl Reader<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.index).copied()
    }

    fn pos(&self) -> usize {
        self.base + self.index
    }

    fn at(&self, prefix: &str) -> bool {
        let mut index = self.index;
        prefix.chars().all(|c| {
            let found = self.chars.get(index) == Some(&c);
            index += 1;
            found
        })
    }

    fn is_atom_char(&self, c: char) -> bool {
        if c.is_whitespace() || is_opener(c) || is_closer(c) || c == '"' {
            return false;
        }
        if self.syntax.lisp {
            c != ';'
        } else {
            c.is_alphanumeric() || c == '_'
        }
    }

    fn string(&mut self, quote: char) -> Node {
        let start = self.pos();
        self.index += 1;
        let mut closed = false;
        while let Some(c) = self.peek() {
            self.index += 1;
            if c == '\\' {
                self.index += 1;
            } else if c == quote {
                closed = true;
                break;
            }
        }
        self.index = self.index.min(self.chars.len());
        Node {
            kind: Kind::Str { closed },
            prefix_start: start,
            start,
            end: self.pos(),
            children: Vec::new(),
        }
    }

    fn atom(&self, prefix_start: usize, start: usize) -> Node {
        Node {
            kind: Kind::Atom,
            prefix_start,
            start,
            end: self.pos(),
            children: Vec::new(),
        }
    }

    /// Reads expressions up to the closing bracket of the list being read,
    /// or to the end. Returns them and whether the bracket was found.
    fn list_body(&mut self, top_level: bool) -> (Vec<Node>, bool) {
        let mut nodes = Vec::new();
        // Where a run of quote prefixes before the next expression began.
        let mut prefix: Option<usize> = None;
        loop {
            let c = self.peek();
            let start = self.pos();
            // A prefix with nothing to prefix is an atom of its own.
            if let Some(prefix_start) = prefix
                && c.is_none_or(|c| c.is_whitespace() || is_closer(c))
            {
                nodes.push(self.atom(prefix_start, prefix_start));
                prefix = None;
            }
            let Some(c) = c else {
                return (nodes, false);
            };
            if c.is_whitespace() {
                self.index += 1;
                continue;
            }
            if self.syntax.line_comment.is_some_and(|mark| self.at(mark)) {
                while self.peek().is_some_and(|c| c != '\n') {
                    self.index += 1;
                }
                nodes.push(Node {
                    kind: Kind::Comment,
                    prefix_start: start,
                    start,
                    end: self.pos(),
                    children: Vec::new(),
                });
                continue;
            }
            if self.syntax.lisp && !self.at("#\\") {
                let prefix_len = match c {
                    ',' if self.at(",@") => 2,
                    '\'' | '`' | ',' => 1,
                    '#' if self.at("#(") || self.at("#'") => 1,
                    _ => 0,
                };
                if prefix_len > 0 {
                    prefix.get_or_insert(start);
                    self.index += prefix_len;
                    continue;
                }
            }
            let prefix_start = prefix.take().unwrap_or(start);
            if is_opener(c) {
                self.index += 1;
                let (children, closed) = self.list_body(false);
                // A closing bracket of another shape still closes the list.
                nodes.push(Node {
                    kind: Kind::List { open: c, closed },
                    prefix_start,
                    start,
                    end: self.pos(),
                    children,
                });
            } else if is_closer(c) {
                self.index += 1;
                if !top_level {
                    return (nodes, true);
                }
                nodes.push(Node {
                    kind: Kind::Stray,
                    prefix_start: start,
                    start,
                    end: self.pos(),
                    children: Vec::new(),
                });
            } else if c == '"' || (c == '\'' && self.syntax.single_quotes) {
                let mut node = self.string(c);
                node.prefix_start = prefix_start;
                nodes.push(node);
            } else if self.syntax.lisp && self.at("#\\") {
                // A character literal, whatever the character.
                self.index = (self.index + 3).min(self.chars.len());
                while self.peek().is_some_and(|c| self.is_atom_char(c)) {
                    self.index += 1;
                }
                nodes.push(self.atom(prefix_start, start));
            } else if self.is_atom_char(c) {
                while self.peek().is_some_and(|c| self.is_atom_char(c)) {
                    self.index += 1;
                }
                nodes.push(self.atom(prefix_start, start));
            } else {
                // Punctuation in a language that is not Lisp separates
                // expressions, as whitespace does.
                self.index += 1;
            }
        }
    }
}

/// The expressions around a position.
#[derive(Debug)]
pub struct Tree {
    /// A list without brackets that holds the top-level expressions read.
    root: Node,
}

/// Where the top-level form that contains `pos` starts, by rule of thumb.
fn top_level_start(text: &Text, pos: usize) -> usize {
    let line = text.line_of(pos);
    let first = line.saturating_sub(MAX_LINES_BACK);
    (first..=line)
        .rev()
        .map(|line| text.line_start(line))
        .find(|start| *start <= pos && text.char_at(*start).is_some_and(is_opener))
        .unwrap_or_else(|| text.line_start(first))
}

impl Tree {
    /// Reads the expressions around `pos`: in Lisp the top-level form that
    /// holds it and the one after, elsewhere a bounded stretch of lines.
    pub fn around(text: &Text, syntax: &SexpSyntax, pos: usize) -> Self {
        let line = text.line_of(pos);
        let last = (line + MAX_LINES_BACK).min(text.len_lines() - 1);
        let (base, limit) = if syntax.lisp {
            // Up to where the second top-level form after `pos` starts.
            let limit = (line + 1..=last)
                .map(|line| text.line_start(line))
                .filter(|start| text.char_at(*start).is_some_and(is_opener))
                .nth(1)
                .unwrap_or_else(|| text.line_end(last));
            (top_level_start(text, pos), limit)
        } else {
            let first = line.saturating_sub(MAX_LINES_BACK);
            (text.line_start(first), text.line_end(last))
        };
        let mut reader = Reader {
            chars: text.chars_from(base).take(limit - base).collect(),
            base,
            index: 0,
            syntax,
        };
        let (children, _) = reader.list_body(true);
        Self {
            root: Node {
                kind: Kind::List {
                    open: ' ',
                    closed: false,
                },
                prefix_start: base,
                start: base,
                end: reader.pos(),
                children,
            },
        }
    }

    /// The innermost list that `pos` is inside, or `None` at top level.
    pub fn enclosing_list(&self, pos: usize) -> Option<&Node> {
        let mut found = None;
        let mut node = &self.root;
        while let Some(child) = node
            .children
            .iter()
            .find(|child| child.is_list() && child.contains(pos))
        {
            found = Some(child);
            node = child;
        }
        found
    }

    /// The list whose children are at the level of `pos`: the enclosing
    /// list, or the top level.
    pub fn level(&self, pos: usize) -> &Node {
        self.enclosing_list(pos).unwrap_or(&self.root)
    }

    /// The list that holds `list` itself.
    pub fn parent_of(&self, list: &Node) -> &Node {
        self.level(list.start)
    }

    /// The string that `pos` is inside, between its quotes.
    pub fn string_at(&self, pos: usize) -> Option<&Node> {
        self.level(pos)
            .children
            .iter()
            .find(|child| matches!(child.kind, Kind::Str { .. }) && child.contains(pos))
    }

    pub fn in_comment(&self, pos: usize) -> bool {
        self.level(pos)
            .children
            .iter()
            .any(|child| child.kind == Kind::Comment && child.start < pos && pos <= child.end)
    }

    /// Whether `pos` is in a string or comment, where brackets are text.
    pub fn in_text(&self, pos: usize) -> bool {
        self.string_at(pos).is_some() || self.in_comment(pos)
    }

    /// The expression that starts at or after `pos`, at its level.
    pub fn next(&self, pos: usize) -> Option<&Node> {
        self.level(pos).sexps().find(|child| child.end > pos)
    }

    /// The expression that ends at or before `pos`, at its level; or the
    /// one `pos` is in the middle of.
    pub fn previous(&self, pos: usize) -> Option<&Node> {
        self.level(pos)
            .sexps()
            .rev()
            .find(|child| child.prefix_start < pos)
    }
}

/// The names of Lisp forms whose bodies are indented by two columns rather
/// than lined up with the first argument.
fn is_body_form(name: &str) -> bool {
    const FORMS: &[&str] = &[
        "lambda",
        "λ",
        "let",
        "let*",
        "letrec",
        "letrec*",
        "let-values",
        "let*-values",
        "let-syntax",
        "letrec-syntax",
        "named-lambda",
        "case-lambda",
        "when",
        "unless",
        "begin",
        "do",
        "case",
        "cond",
        "match",
        "syntax-rules",
        "syntax-case",
        "parameterize",
        "guard",
        "library",
        "module",
        "progn",
        "prog1",
        "save-excursion",
        "save-restriction",
        "save-match-data",
        "while",
        "dolist",
        "dotimes",
        "condition-case",
        "unwind-protect",
        "catch",
        "pcase",
        "use-package",
        "cl-defun",
        "add-hook!",
    ];
    FORMS.contains(&name)
        || name.starts_with("def")
        || name.starts_with("with-")
        || name.starts_with("call-with")
        || name.starts_with("cl-def")
}

/// The column that a line of Lisp should be indented to, or `None` to leave
/// it alone because it starts inside a string.
pub fn lisp_indent(
    text: &Text,
    syntax: &SexpSyntax,
    line: usize,
    tab_width: usize,
) -> Option<usize> {
    let pos = text.line_start(line);
    let tree = Tree::around(text, syntax, pos);
    if tree.string_at(pos).is_some() {
        return None;
    }
    let Some(list) = tree.enclosing_list(pos) else {
        return Some(0);
    };
    let column = |pos: usize| column_of(text, pos, tab_width);
    let open_column = column(list.start);
    let before: Vec<&Node> = list
        .sexps()
        .filter(|child| child.prefix_start < pos)
        .collect();
    let Some(first) = before.first() else {
        return Some(open_column + 1);
    };
    let Kind::List { open, .. } = list.kind else {
        return Some(0);
    };
    // Data, not a call: a quoted list, or one in square or curly brackets.
    let quoted = list.prefix_start < list.start
        && text
            .char_at(list.start - 1)
            .is_some_and(|c| matches!(c, '\'' | '`'));
    if open != '(' || quoted || first.kind != Kind::Atom {
        return Some(column(first.prefix_start));
    }
    let name = text.slice(first.start..first.end);
    if is_body_form(&name) {
        return Some(open_column + 2);
    }
    // A call: arguments line up under each other. The one to line up with
    // is the last that starts a line, else the first, if that is on the
    // line of the function's name.
    let starts_line = |node: &Node| {
        let line_start = text.line_start(text.line_of(node.prefix_start));
        text.chars_from(line_start)
            .take(node.prefix_start - line_start)
            .all(char::is_whitespace)
    };
    if let Some(above) = before[1..].iter().rev().find(|node| starts_line(node)) {
        return Some(column(above.prefix_start));
    }
    match before.get(1) {
        Some(second) if text.line_of(second.prefix_start) == text.line_of(first.start) => {
            Some(column(second.prefix_start))
        }
        _ => Some(open_column + 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISP: SexpSyntax = SexpSyntax {
        line_comment: Some(";"),
        lisp: true,
        single_quotes: false,
    };
    const C_LIKE: SexpSyntax = SexpSyntax {
        line_comment: Some("//"),
        lisp: false,
        single_quotes: true,
    };

    fn tree(source: &str, pos: usize) -> (Text, Tree) {
        let text = Text::from(source);
        let tree = Tree::around(&text, &LISP, pos);
        (text, tree)
    }

    #[test]
    fn lists_nest() {
        let (_, tree) = tree("(a (b c) d)", 5);
        let inner = tree.enclosing_list(5).unwrap();
        assert_eq!((inner.start, inner.end), (3, 8));
        let outer = tree.enclosing_list(3).unwrap();
        assert_eq!((outer.start, outer.end), (0, 11));
        assert!(tree.enclosing_list(0).is_none());
        assert!(tree.enclosing_list(11).is_none());
        assert_eq!(outer.sexps().count(), 3);
    }

    #[test]
    fn brackets_in_strings_comments_and_chars_are_text() {
        let source = "(a \"b ) c\" #\\( ; )\n d)";
        let (_, tree) = tree(source, 1);
        let list = tree.enclosing_list(1).unwrap();
        assert_eq!(list.end, source.chars().count());
        assert!(list.is_closed());
        assert!(tree.string_at(5).is_some());
        assert!(tree.string_at(3).is_none());
        assert!(tree.in_comment(17));
        assert!(!tree.in_comment(20));
        assert_eq!(list.sexps().count(), 4);
    }

    #[test]
    fn quote_prefixes_belong_to_their_expression() {
        let (_, tree) = tree("(f '(1 2) `(a ,b) #(v) 'sym x)", 1);
        let list = tree.enclosing_list(1).unwrap();
        let starts: Vec<_> = list.sexps().map(|n| (n.prefix_start, n.start)).collect();
        assert_eq!(starts[1], (3, 4));
        assert_eq!(starts[2], (10, 11));
        assert_eq!(starts[3], (18, 19));
        assert_eq!(starts[4], (23, 24));
        assert_eq!(list.sexps().count(), 6);
    }

    #[test]
    fn unbalanced_text_still_reads() {
        let (_, tree) = tree("(a (b \"c", 5);
        let inner = tree.enclosing_list(5).unwrap();
        assert!(!inner.is_closed());
        assert!(tree.string_at(8).is_some());
        let (_, tree) = super::tests::tree(") a", 2);
        assert!(tree.enclosing_list(2).is_none());
    }

    #[test]
    fn reading_starts_at_the_enclosing_top_level_form() {
        let source = "(a \"unterminated\n(b c)\n(d e)\n";
        let pos = source.find("c)").unwrap();
        let (_, tree) = tree(source, pos);
        let list = tree.enclosing_list(pos).unwrap();
        assert_eq!(list.start, source.find("(b").unwrap());
    }

    #[test]
    fn other_languages_split_atoms_at_punctuation() {
        let text = Text::from("foo.bar(x, 'c', y) // (\nz");
        let tree = Tree::around(&text, &C_LIKE, 0);
        let kinds: Vec<Kind> = tree.level(0).children.iter().map(|n| n.kind).collect();
        assert_eq!(kinds.len(), 5);
        assert_eq!(kinds[3], Kind::Comment);
        let list = tree.enclosing_list(8).unwrap();
        assert_eq!(list.sexps().count(), 3);
    }

    fn indent(source: &str) -> Option<usize> {
        let text = Text::from(source);
        lisp_indent(&text, &LISP, text.len_lines() - 1, 8)
    }

    #[test]
    fn lisp_lines_are_indented_by_their_form() {
        assert_eq!(indent("(define (f x)\n"), Some(2));
        assert_eq!(indent("(foo bar\n"), Some(5));
        assert_eq!(indent("(foo\n"), Some(1));
        assert_eq!(indent("(let ((a 1)\n"), Some(6));
        assert_eq!(indent("(let ((a 1))\n"), Some(2));
        assert_eq!(indent("'(1 2\n"), Some(2));
        assert_eq!(indent("(cond\n  [(null? x) 1]\n"), Some(2));
        assert_eq!(indent("(f)\n"), Some(0));
        assert_eq!(indent("(foo\n    a\n"), Some(4));
        assert_eq!(indent("(foo a\n     b\n"), Some(5));
        assert_eq!(indent("(f \"a\n"), None);
        assert_eq!(indent("  (when x\n"), Some(4));
    }
}
