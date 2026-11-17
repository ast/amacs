//! Snippets: templates with fields to fill in.
//!
//! A template is expanded at point. Its fields are then visited in order
//! with TAB; typing in a field that still holds its placeholder replaces
//! the placeholder. Templates come from Scheme, where they are lists in the
//! manner of tempel, and from language servers, in the protocol's own
//! notation.

use std::collections::HashMap;

use amacs_text::MarkerId;

use crate::buffer::BufferId;
use crate::commands::indent;
use crate::editor::Editor;
use crate::error::{CmdError, CmdResult};
use crate::script::Value;

/// One piece of a template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    Text(String),
    /// A place to fill in. Fields are visited in order of their index, and
    /// the field with index 0 last: it is where point ends up. Fields with
    /// the same index hold the same text.
    Field {
        index: u32,
        default: String,
    },
    /// A line break. With `indent`, the new line is indented as its
    /// language says; otherwise like the line the snippet starts on.
    Newline {
        indent: bool,
    },
    /// Indent the current line as its language says.
    Indent,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Template {
    pub parts: Vec<Part>,
}

impl Template {
    fn push_text(&mut self, text: &str) {
        // Line breaks are parts of their own, so that lines can be indented.
        for (index, line) in text.split('\n').enumerate() {
            if index > 0 {
                self.parts.push(Part::Newline { indent: false });
            }
            if line.is_empty() {
                continue;
            }
            match self.parts.last_mut() {
                Some(Part::Text(last)) => last.push_str(line),
                _ => self.parts.push(Part::Text(line.to_owned())),
            }
        }
    }

    /// Reads a snippet in the notation of the Language Server Protocol:
    /// `$1`, `${1:default}`, `${1|one,two|}`, `$0` for where point ends up.
    /// Variables such as `$TM_FILENAME` are not known and give their
    /// default, or nothing.
    pub fn parse_lsp(source: &str) -> Self {
        let chars: Vec<char> = source.chars().collect();
        let mut template = Template::default();
        let mut text = String::new();
        let mut at = 0;
        while at < chars.len() {
            match chars[at] {
                '\\' if at + 1 < chars.len() => {
                    text.push(chars[at + 1]);
                    at += 2;
                }
                '$' => match parse_placeholder(&chars, at + 1) {
                    Some((field, next)) => {
                        template.push_text(&std::mem::take(&mut text));
                        match field {
                            (Some(index), default) => {
                                template.parts.push(Part::Field { index, default });
                            }
                            (None, default) => template.push_text(&default),
                        }
                        at = next;
                    }
                    None => {
                        text.push('$');
                        at += 1;
                    }
                },
                c => {
                    text.push(c);
                    at += 1;
                }
            }
        }
        template.push_text(&text);
        template
    }

    /// Reads a template written in Scheme: a list of strings and of
    ///
    /// - `p` a field, `(p "default")` a field with a placeholder,
    /// - `(s name)` a field that every `(s name)` of the same name mirrors,
    /// - `q` where point ends up,
    /// - `n` a line break, `n>` a line break and indentation, `>` indentation,
    /// - `r` and `r>`, which here are fields like `p`.
    pub fn from_elements(elements: &[Value]) -> Result<Self, String> {
        let mut template = Template::default();
        let mut next_index = 1;
        let mut named: HashMap<String, u32> = HashMap::new();
        let mut field = |default: &str| {
            next_index += 1;
            Part::Field {
                index: next_index - 1,
                default: default.to_owned(),
            }
        };
        for element in elements {
            match element {
                Value::Str(text) => template.push_text(text),
                Value::Int(number) => template.push_text(&number.to_string()),
                Value::Sym(symbol) => match symbol.as_str() {
                    "p" | "r" => template.parts.push(field("")),
                    "r>" => {
                        template.parts.push(Part::Indent);
                        template.parts.push(field(""));
                    }
                    "q" => template.parts.push(Part::Field {
                        index: 0,
                        default: String::new(),
                    }),
                    "n" => template.parts.push(Part::Newline { indent: false }),
                    "n>" => template.parts.push(Part::Newline { indent: true }),
                    ">" => template.parts.push(Part::Indent),
                    other => return Err(format!("unknown snippet element: {other}")),
                },
                Value::List(list) => match list.as_slice() {
                    [Value::Sym(kind), rest @ ..] if kind == "p" || kind == "r" => {
                        let default = match rest.first() {
                            Some(Value::Str(default)) => default.as_str(),
                            _ => "",
                        };
                        template.parts.push(field(default));
                    }
                    [Value::Sym(kind), name] if kind == "s" => {
                        let name = name.as_name().ok_or("(s NAME) needs a name")?;
                        let index = match named.get(name) {
                            Some(index) => *index,
                            None => {
                                let Part::Field { index, .. } = field("") else {
                                    unreachable!("field makes a field");
                                };
                                named.insert(name.to_owned(), index);
                                index
                            }
                        };
                        template.parts.push(Part::Field {
                            index,
                            default: String::new(),
                        });
                    }
                    _ => return Err(format!("unknown snippet element: {element:?}")),
                },
                other => return Err(format!("unknown snippet element: {other:?}")),
            }
        }
        Ok(template)
    }
}

/// Reads what follows a `$`. Returns the field's index, or `None` for a
/// variable, with the default text, and where reading stopped.
fn parse_placeholder(chars: &[char], at: usize) -> Option<((Option<u32>, String), usize)> {
    let word_end = |from: usize, accept: fn(char) -> bool| {
        from + chars[from..].iter().take_while(|c| accept(**c)).count()
    };
    let name = |from: usize| {
        let digits = word_end(from, |c| c.is_ascii_digit());
        if digits > from {
            let index: String = chars[from..digits].iter().collect();
            return Some((index.parse::<u32>().ok(), digits));
        }
        let word = word_end(from, |c| c.is_ascii_alphanumeric() || c == '_');
        (word > from).then_some((None, word))
    };
    if chars.get(at) != Some(&'{') {
        let (index, end) = name(at)?;
        return Some(((index, String::new()), end));
    }
    let (index, mut pos) = name(at + 1)?;
    let mut default = String::new();
    match chars.get(pos)? {
        '}' => Some(((index, default), pos + 1)),
        ':' => {
            // The default may hold placeholders of its own; they count for
            // their text only.
            pos += 1;
            let mut depth = 0;
            loop {
                match *chars.get(pos)? {
                    '\\' => {
                        default.push(*chars.get(pos + 1)?);
                        pos += 1;
                    }
                    '$' if chars.get(pos + 1) == Some(&'{') => {
                        let ((_, inner), next) = parse_placeholder(chars, pos + 1)?;
                        default.push_str(&inner);
                        pos = next - 1;
                    }
                    '$' => {
                        let ((_, inner), next) = parse_placeholder(chars, pos + 1)?;
                        default.push_str(&inner);
                        pos = next - 1;
                    }
                    '{' => {
                        depth += 1;
                        default.push('{');
                    }
                    '}' if depth == 0 => return Some(((index, default), pos + 1)),
                    '}' => {
                        depth -= 1;
                        default.push('}');
                    }
                    c => default.push(c),
                }
                pos += 1;
            }
        }
        '|' => {
            // A choice: the first alternative stands in.
            let end = pos + 1 + chars[pos + 1..].iter().position(|c| *c == '|')?;
            let choices: String = chars[pos + 1..end].iter().collect();
            default = choices.split(',').next().unwrap_or("").to_owned();
            (chars.get(end + 1) == Some(&'}')).then_some(((index, default), end + 2))
        }
        _ => None,
    }
}

struct Field {
    index: u32,
    /// The placeholder the field started with.
    default: String,
    /// Start and end of every place the field's text appears.
    spans: Vec<(MarkerId, MarkerId)>,
}

/// A snippet whose fields are being filled in.
pub(crate) struct ActiveSnippet {
    buffer: BufferId,
    start: MarkerId,
    end: MarkerId,
    /// In the order they are visited.
    fields: Vec<Field>,
    current: usize,
    /// The current field still holds its placeholder, which the next char
    /// typed replaces.
    fresh: bool,
    /// The current field was moved to by the command that is running.
    just_selected: bool,
}

pub const SNIPPET_MAP: &str = "snippet";

impl Editor {
    // Templates by name

    /// Defines a snippet for a mode and the modes derived from it.
    pub fn define_snippet(&mut self, mode: &str, name: &str, elements: Vec<Value>) {
        let snippets = self.snippets.entry(mode.to_owned()).or_default();
        snippets.retain(|(existing, _)| existing != name);
        snippets.push((name.to_owned(), elements));
    }

    /// The snippets that apply in the current buffer: those of its major
    /// mode and that mode's ancestors, and those of `fundamental-mode`,
    /// which apply everywhere. Nearer modes come first and win.
    pub fn snippets_here(&self) -> Vec<(&str, &[Value])> {
        let mut modes: Vec<&str> = Vec::new();
        let mut mode = Some(self.buffer().major_mode.as_str());
        for _ in 0..16 {
            let Some(name) = mode else { break };
            modes.push(name);
            mode = self.mode_parent(name);
        }
        if !modes.contains(&crate::script::FUNDAMENTAL_MODE) {
            modes.push(crate::script::FUNDAMENTAL_MODE);
        }
        let mut found: Vec<(&str, &[Value])> = Vec::new();
        for mode in modes {
            for (name, elements) in self.snippets.get(mode).into_iter().flatten() {
                if !found.iter().any(|(seen, _)| seen == name) {
                    found.push((name, elements));
                }
            }
        }
        found
    }

    /// Expands the snippet with the given name at point.
    pub fn expand_snippet_named(&mut self, name: &str) -> CmdResult {
        let elements = self
            .snippets_here()
            .into_iter()
            .find(|(candidate, _)| *candidate == name)
            .map(|(_, elements)| elements.to_vec())
            .ok_or_else(|| CmdError::msg(format!("No snippet named {name}")))?;
        self.expand_snippet_elements(elements)
    }

    /// Expands a template given as Scheme data. Procedures in it are called
    /// for their text, which only Scheme can do, so then it goes the long
    /// way round.
    pub fn expand_snippet_elements(&mut self, elements: Vec<Value>) -> CmdResult {
        if elements
            .iter()
            .any(|element| matches!(element, Value::Func(_)))
        {
            self.queue_named("%expand-snippet", vec![Value::List(elements)]);
            return Ok(());
        }
        let template = Template::from_elements(&elements).map_err(CmdError::msg)?;
        self.expand_snippet(&template)
    }

    // Expansion

    /// Inserts a template at point and starts filling in its fields.
    pub fn expand_snippet(&mut self, template: &Template) -> CmdResult {
        self.snippet_finish();
        let origin = self.point();
        let line = self.text().line_of(origin);
        let line_start = self.text().line_start(line);
        let base: String = self
            .text()
            .chars_from(line_start)
            .take_while(|c| matches!(c, ' ' | '\t'))
            .collect();
        let unit = if self.buffer().indent_tabs {
            "\t".to_owned()
        } else {
            " ".repeat(self.buffer().indent_offset)
        };

        // The text, with where each field is in it and which of its lines
        // are to be indented, counting lines from the first.
        let mut out = String::new();
        let mut spans: Vec<(u32, usize, usize)> = Vec::new();
        let mut to_indent: Vec<usize> = Vec::new();
        let mut lines = 0;
        for part in &template.parts {
            match part {
                Part::Text(text) => out.push_str(&text.replace('\t', &unit)),
                Part::Field { index, default } => {
                    let start = out.chars().count();
                    out.push_str(default);
                    spans.push((*index, start, out.chars().count()));
                }
                Part::Newline { indent } => {
                    out.push('\n');
                    lines += 1;
                    if *indent {
                        to_indent.push(lines);
                    } else {
                        out.push_str(&base);
                    }
                }
                Part::Indent => to_indent.push(lines),
            }
        }
        self.insert(&out)?;
        let mut end = origin + out.chars().count();

        // Indenting moves what follows; the fields are still plain offsets,
        // so they are shifted by hand.
        to_indent.dedup();
        for relative in to_indent {
            let target = line + relative;
            let at = self.text().line_start(target);
            let before = self.buffer().len();
            indent::indent_line(self, target)?;
            let after = self.buffer().len();
            let offset = at - origin;
            for span in &mut spans {
                for edge in [&mut span.1, &mut span.2] {
                    if *edge >= offset {
                        *edge = (*edge + after).saturating_sub(before);
                    }
                }
            }
            end = (end + after).saturating_sub(before);
        }

        let mut indices: Vec<u32> = spans.iter().map(|span| span.0).collect();
        indices.sort_unstable_by_key(|index| (*index == 0, *index));
        indices.dedup();
        let (last_span, only_final) = (spans.last().copied(), indices == [0]);
        if indices.is_empty() || only_final {
            // Nothing to fill in: point goes where the snippet says, or to
            // its end.
            let target = last_span.map_or(end, |(_, start, _)| origin + start);
            self.set_point(target);
            return Ok(());
        }
        let buffer = self.buffer_mut();
        let fields = indices
            .into_iter()
            .map(|index| Field {
                index,
                default: template
                    .parts
                    .iter()
                    .find_map(|part| match part {
                        Part::Field {
                            index: other,
                            default,
                        } if *other == index => Some(default.clone()),
                        _ => None,
                    })
                    .unwrap_or_default(),
                spans: spans
                    .iter()
                    .filter(|span| span.0 == index)
                    .map(|(_, start, end)| {
                        let start = buffer.create_marker_with(origin + start, false);
                        (start, buffer.create_marker_with(origin + end, true))
                    })
                    .collect(),
            })
            .collect();
        let snippet = ActiveSnippet {
            buffer: self.window().buffer,
            start: self.buffer_mut().create_marker_with(origin, false),
            end: self.buffer_mut().create_marker_with(end, true),
            fields,
            current: 0,
            fresh: false,
            just_selected: false,
        };
        self.snippet = Some(snippet);
        self.snippet_select(0);
        Ok(())
    }

    /// Whether a snippet is being filled in in the current buffer.
    pub(crate) fn snippet_active(&self) -> bool {
        self.snippet
            .as_ref()
            .is_some_and(|snippet| snippet.buffer == self.window().buffer)
    }

    /// Start and end of a field's first appearance.
    fn snippet_span(&self, field: usize) -> Option<(usize, usize)> {
        let snippet = self.snippet.as_ref()?;
        let (start, end) = *snippet.fields.get(field)?.spans.first()?;
        let buffer = &self.buffers[snippet.buffer];
        Some((buffer.marker(start), buffer.marker(end)))
    }

    fn snippet_select(&mut self, field: usize) {
        let Some((start, end)) = self.snippet_span(field) else {
            return;
        };
        let is_final = self
            .snippet
            .as_ref()
            .is_some_and(|snippet| snippet.fields[field].index == 0);
        self.set_point(end);
        let buffer = self.buffer_mut();
        buffer.mark_active = false;
        if is_final {
            // Where point was meant to end up: the snippet is done.
            self.snippet_finish();
            return;
        }
        // A field that still holds its placeholder is shown as selected,
        // and typing replaces it. Text the user put there is theirs.
        let untouched = start != end
            && self.snippet.as_ref().is_some_and(|snippet| {
                snippet.fields[field].default == self.text().slice(start..end)
            });
        if untouched {
            let buffer = self.buffer_mut();
            buffer.set_mark(start);
            buffer.mark_active = true;
        }
        if let Some(snippet) = &mut self.snippet {
            snippet.current = field;
            snippet.fresh = untouched;
            snippet.just_selected = true;
        }
    }

    /// Copies the text of the current field to its other appearances.
    fn snippet_mirror(&mut self) -> CmdResult {
        let Some(snippet) = &self.snippet else {
            return Ok(());
        };
        let spans = snippet.fields[snippet.current].spans.clone();
        let id = snippet.buffer;
        let Some((first, rest)) = spans.split_first() else {
            return Ok(());
        };
        let buffer = &mut self.buffers[id];
        let text = buffer
            .text()
            .slice(buffer.marker(first.0)..buffer.marker(first.1));
        for (start, end) in rest {
            let (start, end) = (buffer.marker(*start), buffer.marker(*end));
            if buffer.text().slice(start..end) != text {
                buffer.delete(start..end)?;
                buffer.insert(start, &text)?;
            }
        }
        Ok(())
    }

    /// Stops filling in the snippet, leaving the text as it is.
    pub(crate) fn snippet_finish(&mut self) {
        let Some(snippet) = self.snippet.take() else {
            return;
        };
        let Some(buffer) = self.buffers.get_mut(snippet.buffer) else {
            return;
        };
        buffer.remove_marker(snippet.start);
        buffer.remove_marker(snippet.end);
        for (start, end) in snippet.fields.into_iter().flat_map(|field| field.spans) {
            buffer.remove_marker(start);
            buffer.remove_marker(end);
        }
    }

    /// Before a char is typed: a placeholder that is still there makes way.
    pub(crate) fn snippet_before_insert(&mut self) -> CmdResult {
        if !self.snippet_active() || !self.snippet.as_ref().is_some_and(|snippet| snippet.fresh) {
            return Ok(());
        }
        let current = self.snippet.as_ref().map_or(0, |snippet| snippet.current);
        if let Some((start, end)) = self.snippet_span(current)
            && self.point() == end
        {
            self.delete(start..end)?;
            self.set_point(start);
        }
        if let Some(snippet) = &mut self.snippet {
            snippet.fresh = false;
        }
        Ok(())
    }

    /// After every command: the snippet ends when point leaves it, and a
    /// placeholder stops being one when anything else is done with it.
    pub(crate) fn snippet_after_command(&mut self, command: Option<&str>) {
        let current = self.window().buffer;
        let point = self.point();
        let Some(snippet) = &mut self.snippet else {
            return;
        };
        if !std::mem::take(&mut snippet.just_selected) {
            snippet.fresh = false;
        }
        let Some(buffer) = self.buffers.get(snippet.buffer) else {
            self.snippet = None;
            return;
        };
        let (start, end) = (buffer.marker(snippet.start), buffer.marker(snippet.end));
        let here = current == snippet.buffer && (start..=end).contains(&point);
        if !here || command == Some("keyboard-quit") {
            self.snippet_finish();
        }
    }
}

pub fn next_field(ed: &mut Editor) -> CmdResult {
    ed.snippet_mirror()?;
    let Some(snippet) = &ed.snippet else {
        return Ok(());
    };
    let next = snippet.current + 1;
    if next >= snippet.fields.len() {
        // No final position was given: the end of the snippet is it.
        let end = ed.buffers[snippet.buffer].marker(snippet.end);
        ed.buffer_mut().mark_active = false;
        ed.set_point(end);
        ed.snippet_finish();
        return Ok(());
    }
    ed.snippet_select(next);
    Ok(())
}

pub fn previous_field(ed: &mut Editor) -> CmdResult {
    ed.snippet_mirror()?;
    let Some(snippet) = &ed.snippet else {
        return Ok(());
    };
    match snippet.current.checked_sub(1) {
        Some(previous) => {
            ed.snippet_select(previous);
            Ok(())
        }
        None => Err(CmdError::msg("This is the first field")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(text: &str) -> Part {
        Part::Text(text.to_owned())
    }

    fn field(index: u32, default: &str) -> Part {
        Part::Field {
            index,
            default: default.to_owned(),
        }
    }

    #[test]
    fn protocol_snippets_are_read() {
        assert_eq!(
            Template::parse_lsp("push(${1:value})$0").parts,
            [text("push("), field(1, "value"), text(")"), field(0, "")]
        );
        assert_eq!(
            Template::parse_lsp("for $1 in ${2} {\n\t$0\n}").parts,
            [
                text("for "),
                field(1, ""),
                text(" in "),
                field(2, ""),
                text(" {"),
                Part::Newline { indent: false },
                text("\t"),
                field(0, ""),
                Part::Newline { indent: false },
                text("}"),
            ]
        );
        // Choices, nested placeholders, variables and escapes.
        assert_eq!(
            Template::parse_lsp(
                "${1|pub,priv|} ${2:a${3:b}c} $TM_FILENAME${UNKNOWN:dflt} \\$5 \\}"
            )
            .parts,
            [
                field(1, "pub"),
                text(" "),
                field(2, "abc"),
                text(" dflt $5 }"),
            ]
        );
        // A lone dollar sign is text.
        assert_eq!(Template::parse_lsp("cost: $ 5").parts, [text("cost: $ 5")]);
        assert_eq!(
            Template::parse_lsp("${1:unclosed").parts,
            [text("${1:unclosed")]
        );
    }

    #[test]
    fn scheme_templates_are_read() {
        let sym = Value::sym;
        let elements = [
            Value::str("fn "),
            sym("p"),
            Value::str("("),
            Value::List(vec![sym("p"), Value::str("args")]),
            Value::str(") {"),
            sym("n>"),
            sym("r>"),
            sym("n"),
            Value::str("}"),
            sym("q"),
        ];
        assert_eq!(
            Template::from_elements(&elements).unwrap().parts,
            [
                text("fn "),
                field(1, ""),
                text("("),
                field(2, "args"),
                text(") {"),
                Part::Newline { indent: true },
                Part::Indent,
                field(3, ""),
                Part::Newline { indent: false },
                text("}"),
                field(0, ""),
            ]
        );
        let mirrored = [
            Value::List(vec![sym("s"), sym("name")]),
            Value::str(" = "),
            Value::List(vec![sym("s"), sym("name")]),
        ];
        assert_eq!(
            Template::from_elements(&mirrored).unwrap().parts,
            [field(1, ""), text(" = "), field(1, "")]
        );
        assert!(Template::from_elements(&[sym("zz")]).is_err());
    }
}
