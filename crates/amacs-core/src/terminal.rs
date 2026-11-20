//! A terminal in a buffer.
//!
//! A program on a pseudo-terminal (`process.rs`) writes to a screen
//! (`amacs-term`), and the screen is kept in a buffer: the lines that have
//! scrolled off the top, as text that is never touched again, and then the
//! live screen, the last rows of the buffer, rewritten as the program
//! draws. Colours are spans. Point follows the program's cursor while the
//! user lets it. Being an ordinary buffer, it can be searched, copied from
//! and shown in any window; the mode in `runtime/lib/term.scm` decides
//! which keys go to the program.

use std::collections::HashMap;

use amacs_term::{HISTORY, Palette, Row, TermEvent, Terminal};

use crate::buffer::BufferId;
use crate::editor::Editor;
use crate::error::{CmdError, CmdResult};
use crate::script::{ScriptCall, ScriptFn, ScriptTarget, Value};

/// Feed at most this many line breaks before taking what scrolled off, so
/// that nothing is lost: see [`amacs_term::HISTORY`].
const LINES_PER_FEED: usize = HISTORY / 2;

pub(crate) struct TerminalState {
    process: u64,
    term: Terminal,
    /// Called with the buffer's name, an event and its argument: `title`,
    /// `bell`, `clipboard`, `exit`.
    handler: Option<ScriptFn>,
    /// The rows of the live screen as last written into the buffer.
    rows: Vec<Row>,
    /// How many lines of scrollback the buffer holds, and may hold.
    scrollback: usize,
    scrollback_limit: usize,
    /// Whether point follows the program's cursor.
    follow: bool,
    /// The version of the faces that the palette was made from.
    palette_version: u64,
}

#[derive(Default)]
pub(crate) struct Terminals {
    by_buffer: HashMap<BufferId, TerminalState>,
    by_process: HashMap<u64, BufferId>,
}

/// The sixteen colours of the theme, for programs.
fn theme_palette(faces: &crate::faces::Faces) -> Palette {
    let mut palette = Palette::default();
    let names = [
        "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    ];
    for (index, name) in names.iter().enumerate() {
        if let Some(color) = faces.color(&format!("fg-term-{name}")) {
            palette.base[index] = color;
        }
        if let Some(color) = faces.color(&format!("fg-term-{name}-bright")) {
            palette.base[index + 8] = color;
        }
    }
    palette
}

impl Editor {
    /// Makes a buffer the screen of a process that runs on a
    /// pseudo-terminal, of a size in columns and rows. The buffer's text is
    /// taken over. `handler` hears what the program asks of its terminal.
    pub fn make_terminal(
        &mut self,
        buffer: BufferId,
        process: u64,
        cols: usize,
        rows: usize,
        scrollback_limit: usize,
        handler: Option<ScriptFn>,
    ) -> CmdResult {
        if !self.process_on_pty(process)? {
            return Err(CmdError::msg(
                "a terminal needs a process on a pseudo-terminal: make-process with 'pty",
            ));
        }
        if self.terminals.by_buffer.contains_key(&buffer) {
            return Err(CmdError::msg("the buffer is a terminal already"));
        }
        let rows = rows.max(1);
        let palette = theme_palette(&self.faces);
        let term = Terminal::new(cols.max(1), rows, palette);
        let text = &mut self.buffers[buffer];
        text.splice_raw(0..text.len(), &"\n".repeat(rows - 1));
        text.clear_spans(None);
        self.terminals.by_buffer.insert(
            buffer,
            TerminalState {
                process,
                term,
                handler,
                rows: vec![Row::default(); rows],
                scrollback: 0,
                scrollback_limit,
                follow: true,
                palette_version: self.faces.version(),
            },
        );
        self.terminals.by_process.insert(process, buffer);
        self.terminal_sync(buffer);
        Ok(())
    }

    pub(crate) fn terminal_of_process(&self, process: u64) -> Option<BufferId> {
        self.terminals.by_process.get(&process).copied()
    }

    pub fn is_terminal(&self, buffer: BufferId) -> bool {
        self.terminals.by_buffer.contains_key(&buffer)
    }

    /// The process of a terminal buffer.
    pub fn terminal_process(&self, buffer: BufferId) -> Option<u64> {
        self.terminals.by_buffer.get(&buffer).map(|t| t.process)
    }

    /// Forgets the terminal of a buffer: the buffer is text from then on.
    pub(crate) fn terminal_detach(&mut self, buffer: BufferId) {
        if let Some(state) = self.terminals.by_buffer.remove(&buffer) {
            self.terminals.by_process.remove(&state.process);
        }
    }

    /// What the program wrote: onto the screen, and then into the buffer.
    pub(crate) fn terminal_feed(&mut self, buffer: BufferId, bytes: &[u8]) {
        let mut rest = bytes;
        while !rest.is_empty() {
            // A piece with few enough line breaks that none scroll out of
            // reach before they are taken.
            let cut = rest
                .iter()
                .enumerate()
                .filter(|(_, byte)| **byte == b'\n')
                .nth(LINES_PER_FEED)
                .map_or(rest.len(), |(index, _)| index);
            let (piece, after) = rest.split_at(cut);
            let Some(state) = self.terminals.by_buffer.get_mut(&buffer) else {
                return;
            };
            state.term.feed(piece);
            self.terminal_take_scrolled(buffer);
            rest = after;
        }
        self.terminal_sync(buffer);
    }

    /// Moves the lines that scrolled off the screen into the buffer, above
    /// the live screen.
    fn terminal_take_scrolled(&mut self, buffer: BufferId) {
        let Some(state) = self.terminals.by_buffer.get_mut(&buffer) else {
            return;
        };
        let scrolled = state.term.take_scrolled();
        if scrolled.is_empty() {
            return;
        }
        let text = &mut self.buffers[buffer];
        let live_line = text.text().len_lines() - state.rows.len();
        let at = text.text().line_start(live_line);
        let (joined, spans) = join_rows(&scrolled, at, true);
        let end = at + joined.chars().count();
        text.splice_raw(at..at, &joined);
        text.replace_spans(at..end, spans);
        state.scrollback += scrolled.len();
    }

    /// Brings the buffer up to date with the screen, trims the scrollback,
    /// moves point after the cursor and acts on what the program asked.
    fn terminal_sync(&mut self, buffer: BufferId) {
        let Some(state) = self.terminals.by_buffer.get_mut(&buffer) else {
            return;
        };
        if state.palette_version != self.faces.version() {
            state.term.set_palette(theme_palette(&self.faces));
            state.palette_version = self.faces.version();
        }
        let mut rows = state.term.screen();
        // The cursor's row keeps blanks up to the cursor column, so that
        // point can rest where the cursor is even past the written text.
        if let Some((crow, ccol)) = state.term.cursor()
            && let Some(row) = rows.get_mut(crow)
        {
            let chars = row.text.chars().count();
            if ccol > chars {
                row.text.extend(std::iter::repeat_n(' ', ccol - chars));
            }
        }
        let rows = rows;
        let text = &mut self.buffers[buffer];
        let total = text.text().len_lines();
        if rows.len() != state.rows.len() {
            // The screen changed size: the live part is written anew.
            let live_line = total.saturating_sub(state.rows.len());
            let at = text.text().line_start(live_line);
            let (joined, spans) = join_rows(&rows, at, false);
            let end = at + joined.chars().count();
            text.splice_raw(at..text.len(), &joined);
            text.replace_spans(at..end, spans);
        } else {
            let live_line = total - rows.len();
            for (index, row) in rows.iter().enumerate() {
                if state.rows[index] == *row {
                    continue;
                }
                let line = live_line + index;
                let start = text.text().line_start(line);
                let end = text.text().line_end(line);
                // Only what differs is replaced, so that marks in the rest of
                // the row stay where they are.
                let old: Vec<char> = state.rows[index].text.chars().collect();
                let new: Vec<char> = row.text.chars().collect();
                let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
                let suffix = old[prefix..]
                    .iter()
                    .rev()
                    .zip(new[prefix..].iter().rev())
                    .take_while(|(a, b)| a == b)
                    .count();
                let middle: String = new[prefix..new.len() - suffix].iter().collect();
                text.splice_raw(start + prefix..end - suffix, &middle);
                let spans = row
                    .spans
                    .iter()
                    .map(|(range, attrs)| (start + range.start..start + range.end, *attrs));
                text.replace_spans(start..start + new.len(), spans);
            }
        }
        state.rows = rows;

        if state.scrollback > state.scrollback_limit {
            let extra = state.scrollback - state.scrollback_limit;
            let at = text.text().line_start(extra);
            text.splice_raw(0..at, "");
            state.scrollback = state.scrollback_limit;
        }

        if state.follow {
            let total = text.text().len_lines();
            let live_line = total - state.rows.len();
            let live_start = text.text().line_start(live_line);
            let point = state.term.cursor().map(|(row, col)| {
                let line = live_line + row;
                (text.text().line_start(line) + col).min(text.text().line_end(line))
            });
            let showing: Vec<_> = self
                .windows
                .iter()
                .filter(|(_, window)| window.buffer == buffer)
                .map(|(_, window)| (window.point, window.start))
                .collect();
            for (point_marker, start_marker) in showing {
                if let Some(point) = point {
                    text.set_marker(point_marker, point);
                }
                text.set_marker(start_marker, live_start);
            }
        }

        let events = state.term.take_events();
        let (process, handler) = (state.process, state.handler.clone());
        let name = self.buffers[buffer].name.clone();
        for event in events {
            let (kind, argument) = match event {
                TermEvent::Write(bytes) => {
                    let _ = self.process_send_bytes(process, bytes);
                    continue;
                }
                TermEvent::Title(title) => ("title", Value::Str(title)),
                TermEvent::ResetTitle => ("title", Value::Bool(false)),
                TermEvent::Bell => ("bell", Value::Void),
                TermEvent::Clipboard(text) => ("clipboard", Value::Str(text)),
            };
            self.terminal_notify(&handler, &name, kind, argument);
        }
    }

    fn terminal_notify(
        &mut self,
        handler: &Option<ScriptFn>,
        name: &str,
        kind: &str,
        argument: Value,
    ) {
        let Some(handler) = handler else {
            return;
        };
        let _ = self.queue_script(ScriptCall {
            target: ScriptTarget::Func(handler.clone()),
            args: vec![Value::str(name), Value::sym(kind), argument],
            command: None,
            echo_result: false,
            frame: None,
        });
    }

    /// The program of a terminal has ended: the buffer is text from now on.
    pub(crate) fn terminal_ended(&mut self, buffer: BufferId, status: Option<i32>) {
        self.terminal_sync(buffer);
        let Some(state) = self.terminals.by_buffer.remove(&buffer) else {
            return;
        };
        self.terminals.by_process.remove(&state.process);
        let name = self.buffers[buffer].name.clone();
        let status = status.map(i64::from);
        self.terminal_notify(&state.handler, &name, "exit", status.into());
    }

    fn terminal_mut(&mut self, buffer: BufferId) -> CmdResult<&mut TerminalState> {
        self.terminals
            .by_buffer
            .get_mut(&buffer)
            .ok_or_else(|| CmdError::msg("the buffer is not a terminal"))
    }

    /// Sends a key to the program, encoded as a terminal would. `false` if
    /// the key is one that sends nothing.
    pub fn terminal_send_key(
        &mut self,
        buffer: BufferId,
        key: &amacs_proto::Key,
    ) -> CmdResult<bool> {
        let state = self.terminal_mut(buffer)?;
        let process = state.process;
        let Some(bytes) = state.term.encode_key(key) else {
            return Ok(false);
        };
        state.follow = true;
        self.process_send_bytes(process, bytes)?;
        self.terminal_sync(buffer);
        Ok(true)
    }

    /// Sends pasted text to the program, as a paste.
    pub fn terminal_paste(&mut self, buffer: BufferId, text: &str) -> CmdResult {
        let state = self.terminal_mut(buffer)?;
        let process = state.process;
        let bytes = state.term.encode_paste(text);
        state.follow = true;
        self.process_send_bytes(process, bytes)?;
        self.terminal_sync(buffer);
        Ok(())
    }

    /// Gives the terminal, and the program, a new size.
    pub fn terminal_resize(&mut self, buffer: BufferId, cols: usize, rows: usize) -> CmdResult {
        let state = self.terminal_mut(buffer)?;
        if state.term.cols() == cols && state.term.rows() == rows {
            return Ok(());
        }
        let process = state.process;
        state.term.resize(cols, rows);
        self.terminal_take_scrolled(buffer);
        self.terminal_sync(buffer);
        self.set_process_window_size(process, cols as u16, rows as u16)
    }

    pub fn terminal_size(&self, buffer: BufferId) -> Option<(usize, usize)> {
        self.terminals
            .by_buffer
            .get(&buffer)
            .map(|state| (state.term.cols(), state.term.rows()))
    }

    /// Whether point follows the program's cursor; set again by any key
    /// sent to the program.
    pub fn terminal_follows(&self, buffer: BufferId) -> bool {
        self.terminals
            .by_buffer
            .get(&buffer)
            .is_some_and(|state| state.follow)
    }

    pub fn set_terminal_follow(&mut self, buffer: BufferId, follow: bool) -> CmdResult {
        self.terminal_mut(buffer)?.follow = follow;
        if follow {
            self.terminal_sync(buffer);
        }
        Ok(())
    }

    /// Where the program's cursor is, as a position in the buffer.
    pub fn terminal_cursor(&self, buffer: BufferId) -> Option<usize> {
        let state = self.terminals.by_buffer.get(&buffer)?;
        let (row, col) = state.term.cursor()?;
        let text = self.buffers[buffer].text();
        let line = text.len_lines() - state.rows.len() + row;
        Some((text.line_start(line) + col).min(text.line_end(line)))
    }

    /// Whether the program has the alternate screen, as full screen
    /// programs do, which have no scrollback worth keeping.
    pub fn terminal_alt_screen(&self, buffer: BufferId) -> bool {
        self.terminals
            .by_buffer
            .get(&buffer)
            .is_some_and(|state| state.term.alt_screen())
    }
}

/// Rows as one text, each on its own line, with their spans moved to start
/// at `at`. With `trailing_newline`, every row ends with a line break;
/// otherwise the rows are between line breaks.
fn join_rows(
    rows: &[Row],
    at: usize,
    trailing_newline: bool,
) -> (
    String,
    Vec<(std::ops::Range<usize>, amacs_proto::FaceAttrs)>,
) {
    let mut text = String::new();
    let mut spans = Vec::new();
    let mut offset = at;
    for (index, row) in rows.iter().enumerate() {
        for (range, attrs) in &row.spans {
            spans.push((offset + range.start..offset + range.end, *attrs));
        }
        text.push_str(&row.text);
        offset += row.text.chars().count();
        if trailing_newline || index + 1 < rows.len() {
            text.push('\n');
            offset += 1;
        }
    }
    (text, spans)
}
