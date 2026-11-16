use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::time::SystemTime;

use amacs_text::{Edit, MarkerId, MarkerSet, Text, Transaction, UndoTree};
use slotmap::new_key_type;

use crate::error::{CmdError, CmdResult};
use crate::script::{FUNDAMENTAL_MODE, Value};

new_key_type! {
    pub struct BufferId;
}

/// Consecutive single-char insertions are undone in groups of this size.
const AMALGAMATION_LIMIT: usize = 20;

/// Text being edited, with the state that belongs to the text rather than to
/// any window showing it.
#[derive(Debug)]
pub struct Buffer {
    pub name: String,
    pub file: Option<PathBuf>,
    /// Modification time of `file` when it was last read or written, to
    /// notice changes made by other programs.
    pub disk_mtime: Option<SystemTime>,
    /// Tokens of the clients waiting for this buffer to be finished.
    pub waiters: Vec<u64>,
    /// The text's tick when it was last auto-saved, if it has been.
    pub auto_saved_tick: Option<u64>,
    pub read_only: bool,
    /// Copied from the `tab-width` option whenever options change.
    pub tab_width: usize,
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
            waiters: Vec::new(),
            auto_saved_tick: None,
            read_only: false,
            tab_width: 8,
            major_mode: FUNDAMENTAL_MODE.to_owned(),
            minor_modes: Vec::new(),
            locals: HashMap::new(),
            saved_point: markers.create(0, false),
            saved_start: markers.create(0, false),
            mark: markers.create(0, false),
            mark_set: false,
            mark_active: false,
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

    // Markers

    pub fn create_marker(&mut self, pos: usize) -> MarkerId {
        self.markers.create(pos.min(self.len()), false)
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
        self.undo = UndoTree::new();
        self.pending = None;
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

    fn raw_insert(&mut self, pos: usize, s: &str) {
        self.text.insert(pos, s);
        self.markers.on_insert(pos, s.chars().count());
        self.tick += 1;
    }

    fn raw_delete(&mut self, range: Range<usize>) -> String {
        let deleted = self.text.remove(range.clone());
        self.markers.on_delete(range);
        self.tick += 1;
        deleted
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

    /// Undoes one transaction. Returns where point should go.
    pub fn undo(&mut self) -> CmdResult<Option<usize>> {
        self.check_writable()?;
        let Some(txn) = self.undo.undo() else {
            return Ok(None);
        };
        for edit in txn.edits.iter().rev() {
            let inserted = edit.inserted.chars().count();
            self.text.remove(edit.pos..edit.pos + inserted);
            self.markers.on_delete(edit.pos..edit.pos + inserted);
            self.text.insert(edit.pos, &edit.deleted);
            self.markers
                .on_insert(edit.pos, edit.deleted.chars().count());
        }
        self.tick += 1;
        self.amalgamated = 0;
        Ok(Some(txn.point_before))
    }

    /// Redoes one transaction. Returns where point should go.
    pub fn redo(&mut self) -> CmdResult<Option<usize>> {
        self.check_writable()?;
        let Some(txn) = self.undo.redo() else {
            return Ok(None);
        };
        for edit in &txn.edits {
            let deleted = edit.deleted.chars().count();
            self.text.remove(edit.pos..edit.pos + deleted);
            self.markers.on_delete(edit.pos..edit.pos + deleted);
            self.text.insert(edit.pos, &edit.inserted);
            self.markers
                .on_insert(edit.pos, edit.inserted.chars().count());
        }
        self.tick += 1;
        self.amalgamated = 0;
        Ok(Some(txn.point_after))
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
}
