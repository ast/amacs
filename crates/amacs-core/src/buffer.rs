use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::time::SystemTime;

use amacs_proto::FaceAttrs;
use amacs_text::{Edit, MarkerId, MarkerSet, Text, Transaction, UndoTree};
use amacs_treesit::{Highlights, IndentParams, Language, Syntax};
use slotmap::new_key_type;

use crate::error::{CmdError, CmdResult};
use crate::faces::Severity;
use crate::lsp::{Document, TextChange, lsp_position};
use crate::script::{FUNDAMENTAL_MODE, Value};

new_key_type! {
    pub struct BufferId;
}

/// Consecutive single-char insertions are undone in groups of this size.
const AMALGAMATION_LIMIT: usize = 20;

/// Whether and how line numbers are shown beside the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineNumbers {
    #[default]
    Off,
    Absolute,
    /// Distances from the line that point is on, which shows its own number.
    Relative,
}

/// Something a language server has to say about a stretch of text.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    start: MarkerId,
    end: MarkerId,
    pub severity: Severity,
    pub message: String,
    /// Who says so, such as `rustc`.
    pub source: String,
    /// The diagnostic as the server sent it, which is how it wants to be
    /// told about it again.
    pub raw: serde_json::Value,
}

/// A diagnostic to put on a buffer, with its range in chars.
pub struct NewDiagnostic {
    pub range: Range<usize>,
    pub severity: Severity,
    pub message: String,
    pub source: String,
    pub raw: serde_json::Value,
}

/// Text being edited, with the state that belongs to the text rather than to
/// any window showing it.
#[derive(Debug)]
pub struct Buffer {
    pub name: String,
    pub file: Option<PathBuf>,
    /// Modification time of `file` when it was last read or written, to
    /// notice changes made by other programs.
    pub disk_mtime: Option<SystemTime>,
    /// The buffer was read from its file without being shown, so the hooks
    /// that choose its mode have yet to run.
    pub(crate) needs_file_hook: bool,
    /// Tokens of the clients waiting for this buffer to be finished.
    pub waiters: Vec<u64>,
    /// The text's tick when it was last auto-saved, if it has been.
    pub auto_saved_tick: Option<u64>,
    pub read_only: bool,
    /// The directory that relative file names are taken from in this
    /// buffer, when that is not the directory of its file: for buffers
    /// that show a directory or what a program wrote about one.
    pub directory: Option<PathBuf>,
    /// Text that the mode line shows after the modes, for code that has
    /// something to say about the buffer: the state of an agent, the title
    /// of a terminal.
    pub mode_line: Option<String>,
    /// A face whose colours are the buffer's default, in place of the
    /// theme's: the background of a terminal, say. `None` is the theme.
    default_face: Option<String>,
    /// Faces laid over stretches of the text by code, with their names.
    text_faces: Vec<(Range<usize>, String)>,
    /// Colours and attributes laid over stretches of the text without a
    /// face of a name: sorted by start and never overlapping, so that the
    /// one at a position is found by bisection. A terminal's screen is
    /// drawn with these.
    spans: Vec<(Range<usize>, FaceAttrs)>,
    /// Marks beside lines, by line counted from 0, each the name of the
    /// face it is drawn in: what differs from the last commit, for one.
    line_signs: HashMap<usize, String>,
    /// Copied from the `tab-width` option whenever options change.
    pub tab_width: usize,
    /// Copied from the `line-numbers` option.
    pub line_numbers: LineNumbers,
    /// Copied from the `indent-offset` option: columns per level.
    pub indent_offset: usize,
    /// Copied from the `indent-tabs-mode` option: indent with tabs.
    pub indent_tabs: bool,
    /// The language of the text, which decides highlighting and indentation.
    language: Option<&'static Language>,
    syntax: Option<Syntax>,
    /// The buffer's link to a language server, if it has one.
    pub(crate) document: Option<Document>,
    diagnostics: Vec<Diagnostic>,
    pub major_mode: String,
    /// Buffer-local minor modes, in the order they were enabled.
    pub minor_modes: Vec<String>,
    /// Buffer-local option values.
    pub locals: HashMap<String, Value>,
    text: Text,
    markers: MarkerSet,
    undo: UndoTree,
    /// Edits made by the command that is currently running.
    pending: Option<Transaction>,
    /// Point when the running command started, for the undo record.
    undo_point: Option<usize>,
    amalgamated: usize,
    /// Point and window start to use when the buffer is next shown in a window.
    saved_point: MarkerId,
    saved_start: MarkerId,
    mark: MarkerId,
    mark_set: bool,
    pub mark_active: bool,
    /// The active region is the rectangle whose corners are point and mark.
    pub rectangle_mark: bool,
    /// Incremented on every change to the text.
    tick: u64,
}

impl Buffer {
    pub fn new(name: impl Into<String>) -> Self {
        Self::with_text(name, Text::new())
    }

    pub fn with_text(name: impl Into<String>, text: Text) -> Self {
        let mut markers = MarkerSet::new();
        Self {
            name: name.into(),
            file: None,
            disk_mtime: None,
            needs_file_hook: false,
            waiters: Vec::new(),
            auto_saved_tick: None,
            read_only: false,
            directory: None,
            mode_line: None,
            default_face: None,
            text_faces: Vec::new(),
            spans: Vec::new(),
            line_signs: HashMap::new(),
            tab_width: 8,
            line_numbers: LineNumbers::Off,
            indent_offset: 4,
            indent_tabs: false,
            language: None,
            syntax: None,
            document: None,
            diagnostics: Vec::new(),
            major_mode: FUNDAMENTAL_MODE.to_owned(),
            minor_modes: Vec::new(),
            locals: HashMap::new(),
            saved_point: markers.create(0, false),
            saved_start: markers.create(0, false),
            mark: markers.create(0, false),
            mark_set: false,
            mark_active: false,
            rectangle_mark: false,
            text,
            markers,
            undo: UndoTree::new(),
            pending: None,
            undo_point: None,
            amalgamated: 0,
            tick: 0,
        }
    }

    pub fn text(&self) -> &Text {
        &self.text
    }

    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn tick(&self) -> u64 {
        self.tick
    }

    pub fn is_modified(&self) -> bool {
        self.pending.is_some() || !self.undo.is_at_saved()
    }

    pub fn mark_saved(&mut self) {
        self.undo.mark_saved();
    }

    /// Makes the buffer count as modified even with no edits, for text that
    /// does not match any file on disk.
    pub fn mark_unsaved(&mut self) {
        self.undo.clear_saved();
    }

    // Syntax

    pub fn language(&self) -> Option<&'static Language> {
        self.language
    }

    /// Sets the language of the text. Its syntax tree is built when next
    /// needed.
    pub fn set_language(&mut self, language: Option<&'static Language>) {
        self.language = language;
        self.syntax = language.and_then(Syntax::new);
    }

    /// Whether the text has a syntax tree, which takes a language with a
    /// grammar.
    pub fn has_syntax(&self) -> bool {
        self.syntax.is_some()
    }

    /// Brings the syntax tree up to date with the text.
    pub fn update_syntax(&mut self) {
        if let Some(syntax) = &mut self.syntax {
            syntax.update(&self.text);
        }
    }

    /// Syntax highlights for a range of bytes. The tree must be up to date.
    pub fn highlights(&self, range: Range<usize>) -> Option<Highlights> {
        Some(self.syntax.as_ref()?.highlights(&self.text, range))
    }

    pub fn indent_params(&self) -> IndentParams {
        IndentParams {
            offset: self.indent_offset,
            tab_width: self.tab_width,
        }
    }

    /// The column that the syntax tree says a line should be indented to.
    /// The tree must be up to date.
    pub fn syntax_indent(&self, line: usize) -> Option<usize> {
        self.syntax
            .as_ref()?
            .indent_column(&self.text, line, self.indent_params())
    }

    // Diagnostics

    /// Replaces the buffer's diagnostics.
    pub fn set_diagnostics(&mut self, diagnostics: Vec<NewDiagnostic>) {
        for old in std::mem::take(&mut self.diagnostics) {
            self.markers.remove(old.start);
            self.markers.remove(old.end);
        }
        let len = self.text.len();
        self.diagnostics = diagnostics
            .into_iter()
            .map(|new| Diagnostic {
                start: self.markers.create(new.range.start.min(len), true),
                end: self.markers.create(new.range.end.min(len), false),
                severity: new.severity,
                message: new.message,
                source: new.source,
                raw: new.raw,
            })
            .collect();
    }

    /// The diagnostics with where they are now, in the order they came.
    pub fn diagnostics(&self) -> impl Iterator<Item = (Range<usize>, &Diagnostic)> {
        self.diagnostics.iter().map(|diagnostic| {
            let start = self.markers.get(diagnostic.start);
            // Text deleted around a diagnostic can leave its end before
            // its start.
            let end = self.markers.get(diagnostic.end).max(start);
            (start..end, diagnostic)
        })
    }

    /// The diagnostics that cover `pos`, most serious first. One with no
    /// extent covers the char at its position.
    pub fn diagnostics_at(&self, pos: usize) -> Vec<&Diagnostic> {
        let mut found: Vec<&Diagnostic> = self
            .diagnostics()
            .filter(|(range, _)| range.start <= pos && pos < range.end.max(range.start + 1))
            .map(|(_, diagnostic)| diagnostic)
            .collect();
        found.sort_by_key(|diagnostic| diagnostic.severity);
        found
    }

    // Markers

    pub fn create_marker(&mut self, pos: usize) -> MarkerId {
        self.markers.create(pos.min(self.len()), false)
    }

    /// Creates a marker that, with `advances`, stays after text inserted
    /// exactly at it.
    pub fn create_marker_with(&mut self, pos: usize, advances: bool) -> MarkerId {
        self.markers.create(pos.min(self.len()), advances)
    }

    pub fn remove_marker(&mut self, id: MarkerId) {
        self.markers.remove(id);
    }

    pub fn marker(&self, id: MarkerId) -> usize {
        self.markers.get(id)
    }

    pub fn set_marker(&mut self, id: MarkerId, pos: usize) {
        self.markers.set(id, pos.min(self.text.len()));
    }

    pub fn saved_point(&self) -> usize {
        self.marker(self.saved_point)
    }

    pub fn saved_start(&self) -> usize {
        self.marker(self.saved_start)
    }

    pub fn save_view(&mut self, point: usize, start: usize) {
        self.set_marker(self.saved_point, point);
        self.set_marker(self.saved_start, start);
    }

    pub fn mark(&self) -> Option<usize> {
        self.mark_set.then(|| self.marker(self.mark))
    }

    pub fn set_mark(&mut self, pos: usize) {
        self.set_marker(self.mark, pos);
        self.mark_set = true;
    }

    // Editing

    fn check_writable(&self) -> CmdResult {
        if self.read_only {
            Err(CmdError::msg(format!("Buffer is read-only: {}", self.name)))
        } else {
            Ok(())
        }
    }

    pub fn insert(&mut self, pos: usize, s: &str) -> CmdResult {
        if s.is_empty() {
            return Ok(());
        }
        self.check_writable()?;
        self.raw_insert(pos, s);
        self.record(Edit {
            pos,
            deleted: String::new(),
            inserted: s.to_owned(),
        });
        Ok(())
    }

    pub fn delete(&mut self, range: Range<usize>) -> CmdResult<String> {
        if range.is_empty() {
            return Ok(String::new());
        }
        self.check_writable()?;
        let deleted = self.raw_delete(range.clone());
        self.record(Edit {
            pos: range.start,
            deleted: deleted.clone(),
            inserted: String::new(),
        });
        Ok(deleted)
    }

    /// Replaces the whole text and forgets the undo history.
    pub fn reset(&mut self, text: Text) {
        self.markers.on_delete(0..self.text.len());
        self.text = text;
        if let Some(syntax) = &mut self.syntax {
            syntax.reset();
        }
        if let Some(document) = &mut self.document {
            document.resend = true;
        }
        self.set_diagnostics(Vec::new());
        self.undo = UndoTree::new();
        self.pending = None;
        self.text_faces.clear();
        self.spans.clear();
        self.line_signs.clear();
        self.mark_set = false;
        self.mark_active = false;
        self.tick += 1;
    }

    /// Appends a line of text regardless of the read-only flag and without
    /// recording undo, for buffers that the editor itself writes to.
    pub fn append_log(&mut self, text: &str) {
        let end = self.len();
        self.raw_insert(end, text);
        if !text.ends_with('\n') {
            self.raw_insert(self.len(), "\n");
        }
    }

    fn parts(&mut self) -> Parts<'_> {
        Parts {
            text: &mut self.text,
            markers: &mut self.markers,
            syntax: &mut self.syntax,
            document: &mut self.document,
        }
    }

    fn raw_insert(&mut self, pos: usize, s: &str) {
        if !self.line_signs.is_empty() {
            let added = s.matches('\n').count();
            if added > 0 {
                // A line that is pushed down takes its mark with it.
                let line = self.text.line_of(pos);
                let from = if self.text.line_start(line) == pos {
                    line
                } else {
                    line + 1
                };
                self.line_signs = std::mem::take(&mut self.line_signs)
                    .into_iter()
                    .map(|(at, face)| (if at >= from { at + added } else { at }, face))
                    .collect();
            }
        }
        self.parts().insert(pos, s);
        self.tick += 1;
        if !self.text_faces.is_empty() || !self.spans.is_empty() {
            let added = s.chars().count();
            for (range, _) in &mut self.text_faces {
                if range.start >= pos {
                    range.start += added;
                }
                if range.end > pos {
                    range.end += added;
                }
            }
            // The spans are in order and apart, so their ends are in order
            // too: those that end at or before the insertion stay.
            let first = self.spans.partition_point(|(range, _)| range.end <= pos);
            for (range, _) in &mut self.spans[first..] {
                if range.start >= pos {
                    range.start += added;
                }
                range.end += added;
            }
        }
    }

    fn raw_delete(&mut self, range: Range<usize>) -> String {
        self.tick += 1;
        if !self.line_signs.is_empty() {
            let (first, last) = (self.text.line_of(range.start), self.text.line_of(range.end));
            if last > first {
                // The lines that are joined into the first lose their marks.
                self.line_signs = std::mem::take(&mut self.line_signs)
                    .into_iter()
                    .filter(|(at, _)| *at <= first || *at > last)
                    .map(|(at, face)| (if at > last { at - (last - first) } else { at }, face))
                    .collect();
            }
        }
        if !self.text_faces.is_empty() {
            // A position in what is deleted ends up where that started.
            let moved = |pos: usize| {
                if pos >= range.end {
                    pos - range.len()
                } else {
                    pos.min(range.start)
                }
            };
            for (face, _) in &mut self.text_faces {
                *face = moved(face.start)..moved(face.end);
            }
            self.text_faces.retain(|(face, _)| !face.is_empty());
        }
        if !self.spans.is_empty() {
            let moved = |pos: usize| {
                if pos >= range.end {
                    pos - range.len()
                } else {
                    pos.min(range.start)
                }
            };
            let first = self
                .spans
                .partition_point(|(span, _)| span.end <= range.start);
            for (span, _) in &mut self.spans[first..] {
                *span = moved(span.start)..moved(span.end);
            }
            self.spans.retain(|(span, _)| !span.is_empty());
        }
        self.parts().delete(range)
    }

    // Faces laid over the text

    /// Lays a face, by name, over a stretch of the text. It stays on that
    /// text as the text around it changes.
    pub fn add_text_face(&mut self, range: Range<usize>, face: &str) {
        let range = range.start.min(self.len())..range.end.min(self.len());
        if !range.is_empty() {
            self.text_faces.push((range, face.to_owned()));
        }
    }

    /// Takes away the faces laid over the text that touch a range, or all.
    pub fn clear_text_faces(&mut self, range: Option<Range<usize>>) {
        match range {
            Some(range) => self
                .text_faces
                .retain(|(face, _)| face.end <= range.start || face.start >= range.end),
            None => self.text_faces.clear(),
        }
    }

    pub fn text_faces(&self) -> &[(Range<usize>, String)] {
        &self.text_faces
    }

    /// The name of the buffer's default face, if it has one of its own.
    pub fn default_face(&self) -> Option<&str> {
        self.default_face.as_deref()
    }

    /// Gives the buffer a default face of its own, or `None` for the
    /// theme's default.
    pub fn set_default_face(&mut self, face: Option<String>) {
        self.default_face = face;
    }

    /// Replaces a stretch of the text regardless of the read-only flag and
    /// without recording undo, for buffers that the editor itself writes,
    /// such as a terminal's. Markers and spans move as for any edit.
    pub fn splice_raw(&mut self, range: Range<usize>, text: &str) {
        let len = self.len();
        let range = range.start.min(len)..range.end.min(len);
        if !range.is_empty() {
            self.raw_delete(range.clone());
        }
        if !text.is_empty() {
            self.raw_insert(range.start, text);
        }
    }

    // Spans: colours without a face

    /// Lays colours and attributes over a stretch of the text, in place of
    /// whatever spans covered it. `None` colours leave those under them.
    pub fn add_span(&mut self, range: Range<usize>, attrs: FaceAttrs) {
        self.replace_spans(range.clone(), [(range, attrs)]);
    }

    /// Replaces the spans within `range` with `spans`, which must be in
    /// order, apart, and within `range`. Spans that reach into `range`
    /// from either side are cut at its edge.
    pub fn replace_spans(
        &mut self,
        range: Range<usize>,
        spans: impl IntoIterator<Item = (Range<usize>, FaceAttrs)>,
    ) {
        let len = self.len();
        let range = range.start.min(len)..range.end.min(len);
        let first = self
            .spans
            .partition_point(|(span, _)| span.end <= range.start);
        let last = self
            .spans
            .partition_point(|(span, _)| span.start < range.end);
        let mut replaced: Vec<(Range<usize>, FaceAttrs)> = Vec::new();
        if let Some((before, attrs)) = self.spans.get(first)
            && before.start < range.start
        {
            replaced.push((before.start..range.start, *attrs));
        }
        let after = (first < last)
            .then(|| self.spans[last - 1].clone())
            .filter(|(span, _)| span.end > range.end)
            .map(|(span, attrs)| (range.end..span.end, attrs));
        replaced.extend(
            spans
                .into_iter()
                .map(|(span, attrs)| (span.start.max(range.start)..span.end.min(range.end), attrs))
                .filter(|(span, _)| !span.is_empty()),
        );
        replaced.extend(after);
        self.spans.splice(first..last.max(first), replaced);
    }

    /// Takes away the spans that touch a range, or all of them.
    pub fn clear_spans(&mut self, range: Option<Range<usize>>) {
        match range {
            Some(range) => self.replace_spans(range, []),
            None => self.spans.clear(),
        }
    }

    /// The spans, in order.
    pub fn spans(&self) -> &[(Range<usize>, FaceAttrs)] {
        &self.spans
    }

    /// Replaces the marks beside lines: the line, counted from 0, and the
    /// name of the face to draw the mark in. They are shown where line
    /// numbers are.
    pub fn set_line_signs(&mut self, signs: impl IntoIterator<Item = (usize, String)>) {
        self.line_signs = signs.into_iter().collect();
    }

    pub fn line_sign(&self, line: usize) -> Option<&str> {
        self.line_signs.get(&line).map(String::as_str)
    }

    // Undo

    fn record(&mut self, edit: Edit) {
        let point_before = self.undo_point.unwrap_or(edit.pos);
        self.pending
            .get_or_insert_with(|| Transaction {
                point_before,
                ..Transaction::default()
            })
            .edits
            .push(edit);
    }

    /// Called before a command runs with point in this buffer.
    pub fn begin_command(&mut self, point: usize) {
        self.undo_point = Some(point);
    }

    /// Called after every command. Moves the command's edits into the undo
    /// tree. With `amalgamate`, they join the previous transaction, so that a
    /// run of typed chars is undone together.
    pub fn end_command(&mut self, point: usize, amalgamate: bool) {
        self.undo_point = None;
        let Some(mut txn) = self.pending.take() else {
            if !amalgamate {
                self.amalgamated = 0;
            }
            return;
        };
        txn.point_after = point;
        if amalgamate
            && self.amalgamated < AMALGAMATION_LIMIT
            && let Some(last) = self.undo.amendable()
        {
            last.edits.append(&mut txn.edits);
            last.point_after = point;
            self.amalgamated += 1;
            return;
        }
        self.amalgamated = 1;
        self.undo.push(txn);
    }

    /// Moves the edits that the running command has made so far into the
    /// undo tree, as a step of their own, so that the state they lead to
    /// can be marked as saved.
    pub fn commit_edits(&mut self, point: usize) {
        if self.pending.is_some() {
            let in_command = self.undo_point.is_some();
            self.end_command(point, false);
            if in_command {
                self.undo_point = Some(point);
            }
        }
    }

    /// Undoes one transaction. Returns where point should go.
    pub fn undo(&mut self) -> CmdResult<Option<usize>> {
        self.check_writable()?;
        let Some(txn) = self.undo.undo() else {
            return Ok(None);
        };
        for edit in txn.edits.iter().rev() {
            let inserted = edit.inserted.chars().count();
            let mut parts = Parts {
                text: &mut self.text,
                markers: &mut self.markers,
                syntax: &mut self.syntax,
                document: &mut self.document,
            };
            parts.delete(edit.pos..edit.pos + inserted);
            parts.insert(edit.pos, &edit.deleted);
        }
        self.tick += 1;
        self.amalgamated = 0;
        Ok(Some(txn.point_before))
    }

    /// The history of the text.
    pub fn undo_tree(&self) -> &UndoTree {
        &self.undo
    }

    /// Takes the text to any state of its history, undoing and redoing as
    /// needed. Returns where point should go, if anything changed.
    pub fn undo_goto(&mut self, state: usize) -> CmdResult<Option<usize>> {
        let path = self.undo.path_to(state);
        let mut point = None;
        // Back to where the way to the state branches off.
        while !path.contains(&self.undo.current()) {
            point = self.undo()?;
        }
        let reached = path
            .iter()
            .position(|on_path| *on_path == self.undo.current())
            .unwrap_or(0);
        for next in &path[reached + 1..] {
            self.undo.choose_branch(*next);
            point = self.redo()?;
        }
        Ok(point)
    }

    /// Redoes one transaction. Returns where point should go.
    pub fn redo(&mut self) -> CmdResult<Option<usize>> {
        self.check_writable()?;
        let Some(txn) = self.undo.redo() else {
            return Ok(None);
        };
        for edit in &txn.edits {
            let deleted = edit.deleted.chars().count();
            let mut parts = Parts {
                text: &mut self.text,
                markers: &mut self.markers,
                syntax: &mut self.syntax,
                document: &mut self.document,
            };
            parts.delete(edit.pos..edit.pos + deleted);
            parts.insert(edit.pos, &edit.inserted);
        }
        self.tick += 1;
        self.amalgamated = 0;
        Ok(Some(txn.point_after))
    }
}

/// The parts of a buffer that every change to its text touches. Borrowed
/// apart from the rest, so that the undo history can be read meanwhile.
struct Parts<'a> {
    text: &'a mut Text,
    markers: &'a mut MarkerSet,
    syntax: &'a mut Option<Syntax>,
    document: &'a mut Option<Document>,
}

impl Parts<'_> {
    /// Notes a change for the language server, as positions in the text
    /// before the change.
    fn note_change(&mut self, range: Range<usize>, inserted: &str) {
        if let Some(document) = self.document.as_mut().filter(|document| document.open) {
            document.changes.push(TextChange {
                start: lsp_position(self.text, range.start, document.encoding),
                end: lsp_position(self.text, range.end, document.encoding),
                text: inserted.to_owned(),
            });
        }
    }

    fn insert(&mut self, pos: usize, s: &str) {
        if s.is_empty() {
            return;
        }
        self.note_change(pos..pos, s);
        if let Some(syntax) = self.syntax {
            syntax.before_insert(self.text, pos, s);
        }
        self.text.insert(pos, s);
        self.markers.on_insert(pos, s.chars().count());
    }

    fn delete(&mut self, range: Range<usize>) -> String {
        if range.is_empty() {
            return String::new();
        }
        self.note_change(range.clone(), "");
        if let Some(syntax) = self.syntax {
            syntax.before_delete(self.text, range.clone());
        }
        let deleted = self.text.remove(range.clone());
        self.markers.on_delete(range);
        deleted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(buffer: &mut Buffer, amalgamate: bool, f: impl FnOnce(&mut Buffer)) {
        buffer.begin_command(0);
        f(buffer);
        buffer.end_command(0, amalgamate);
    }

    #[test]
    fn undo_and_redo_restore_text() {
        let mut buffer = Buffer::new("t");
        command(&mut buffer, false, |b| b.insert(0, "hello world").unwrap());
        command(&mut buffer, false, |b| {
            b.delete(0..6).unwrap();
            b.insert(5, "!").unwrap();
        });
        assert_eq!(buffer.text().to_string(), "world!");
        buffer.undo().unwrap();
        assert_eq!(buffer.text().to_string(), "hello world");
        buffer.undo().unwrap();
        assert_eq!(buffer.text().to_string(), "");
        assert!(!buffer.is_modified());
        assert_eq!(buffer.undo().unwrap(), None);
        buffer.redo().unwrap();
        buffer.redo().unwrap();
        assert_eq!(buffer.text().to_string(), "world!");
        assert!(buffer.is_modified());
    }

    #[test]
    fn typed_chars_are_undone_in_groups() {
        let mut buffer = Buffer::new("t");
        for i in 0..25 {
            command(&mut buffer, i > 0, |b| b.insert(i, "x").unwrap());
        }
        buffer.undo().unwrap();
        assert_eq!(buffer.len(), 20);
        buffer.undo().unwrap();
        assert_eq!(buffer.len(), 0);
    }

    #[test]
    fn read_only_buffers_reject_edits() {
        let mut buffer = Buffer::with_text("t", Text::from("abc"));
        buffer.read_only = true;
        assert!(buffer.insert(0, "x").is_err());
        assert!(buffer.delete(0..1).is_err());
        assert_eq!(buffer.text().to_string(), "abc");
    }

    #[test]
    fn markers_follow_edits_and_undo() {
        let mut buffer = Buffer::with_text("t", Text::from("abcdef"));
        let marker = buffer.create_marker(4);
        command(&mut buffer, false, |b| b.insert(1, "XY").unwrap());
        assert_eq!(buffer.marker(marker), 6);
        buffer.undo().unwrap();
        assert_eq!(buffer.marker(marker), 4);
    }

    fn red() -> FaceAttrs {
        FaceAttrs {
            fg: Some(amacs_proto::Color::rgb(255, 0, 0)),
            ..FaceAttrs::default()
        }
    }

    fn blue() -> FaceAttrs {
        FaceAttrs {
            bg: Some(amacs_proto::Color::rgb(0, 0, 255)),
            ..FaceAttrs::default()
        }
    }

    fn ranges(buffer: &Buffer) -> Vec<Range<usize>> {
        buffer
            .spans()
            .iter()
            .map(|(range, _)| range.clone())
            .collect()
    }

    #[test]
    fn a_span_takes_the_place_of_the_spans_under_it() {
        let mut buffer = Buffer::new("spans");
        buffer.reset("0123456789".into());
        buffer.add_span(0..4, red());
        buffer.add_span(6..10, red());
        // Over the middle: the first is cut short, the last starts later.
        buffer.add_span(2..8, blue());
        assert_eq!(ranges(&buffer), vec![0..2, 2..8, 8..10]);
        assert_eq!(buffer.spans()[1].1, blue());
        // Within one span: it is split around the new one.
        buffer.add_span(4..5, red());
        assert_eq!(ranges(&buffer), vec![0..2, 2..4, 4..5, 5..8, 8..10]);
        buffer.clear_spans(Some(3..9));
        assert_eq!(ranges(&buffer), vec![0..2, 2..3, 9..10]);
        buffer.clear_spans(None);
        assert!(buffer.spans().is_empty());
    }

    #[test]
    fn spans_stay_on_their_text_as_it_moves() {
        let mut buffer = Buffer::new("spans");
        buffer.reset("abcdef".into());
        buffer.add_span(1..3, red());
        buffer.add_span(4..6, blue());
        buffer.insert(0, "XX").unwrap();
        assert_eq!(ranges(&buffer), vec![3..5, 6..8]);
        // An insertion inside a span widens it.
        buffer.insert(4, "YY").unwrap();
        assert_eq!(ranges(&buffer), vec![3..7, 8..10]);
        // A deletion across spans shortens them, and one deleted whole goes.
        buffer.delete(5..9).unwrap();
        assert_eq!(ranges(&buffer), vec![3..5, 5..6]);
        buffer.delete(3..6).unwrap();
        assert!(buffer.spans().is_empty());
    }
}
