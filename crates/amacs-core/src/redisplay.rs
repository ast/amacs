//! Turning editor state into draw operations.
//!
//! Each call builds the lines every pane should show and sends only the rows
//! that differ from what the frontend already has.

use std::collections::HashMap;

use amacs_proto::{CursorShape, DisplayLine, DrawOp, FaceId, PaneId, PaneRect};

use crate::display::{
    char_cells, column_of, push_glyph, row_containing, rows_back, str_cells, walk_row,
};
use crate::editor::Editor;
use crate::faces;
use crate::window::{Rect, WindowId};

const ECHO_PANE: PaneId = PaneId(0);

/// What the frontend currently shows.
#[derive(Default)]
pub(crate) struct RedisplayState {
    faces_sent: bool,
    size: (u16, u16),
    layout: Vec<PaneRect>,
    lines: HashMap<PaneId, Vec<DisplayLine>>,
    cursor: Option<(PaneId, u16, u16)>,
}

impl RedisplayState {
    /// Forgets what the frontend shows, so the next redisplay sends everything.
    pub fn invalidate(&mut self) {
        *self = Self::default();
    }
}

fn pane_rect(id: PaneId, rect: Rect) -> PaneRect {
    PaneRect {
        id,
        x: rect.x,
        y: rect.y,
        w: rect.w,
        h: rect.h,
    }
}

/// Cuts `s` to at most `width` cells.
fn fit(s: &str, width: usize) -> &str {
    let mut cells = 0;
    for (index, c) in s.char_indices() {
        cells += char_cells(c, 0, 1);
        if cells > width {
            return &s[..index];
        }
    }
    s
}

impl Editor {
    /// Returns the operations that bring the frontend up to date. Empty if
    /// nothing changed.
    pub fn redisplay(&mut self) -> Vec<DrawOp> {
        self.relayout();
        let leaves = self.frame().layout.leaves();
        for id in &leaves {
            self.scroll_to_point(*id);
        }

        let mut panes: Vec<(PaneRect, Vec<DisplayLine>)> = Vec::new();
        let mut cursor = None;
        for id in &leaves {
            let window = &self.windows[*id];
            let rect = window.rect;
            let text_pane = PaneId(window.serial * 3);
            let mode_pane = PaneId(window.serial * 3 + 1);
            let divider_pane = PaneId(window.serial * 3 + 2);
            let border = u16::from(window.border);
            let text_rect = Rect {
                h: rect.h.saturating_sub(1),
                ..rect
            };
            let (lines, window_cursor) = self.window_lines(*id);
            if *id == self.selected()
                && let Some((row, col)) = window_cursor
            {
                cursor = Some((text_pane, row, col));
            }
            panes.push((pane_rect(text_pane, text_rect), lines));
            if rect.h > 0 {
                let mode_rect = Rect {
                    y: rect.y + rect.h - 1,
                    w: rect.w + border,
                    h: 1,
                    ..rect
                };
                panes.push((pane_rect(mode_pane, mode_rect), vec![self.mode_line(*id)]));
            }
            if window.border {
                let divider_rect = Rect {
                    x: rect.x + rect.w,
                    w: 1,
                    ..text_rect
                };
                let line = DisplayLine::plain("│", faces::VERTICAL_BORDER);
                panes.push((
                    pane_rect(divider_pane, divider_rect),
                    vec![line; usize::from(divider_rect.h)],
                ));
            }
        }
        let echo_rect = self.windows[self.minibuffer_window()].rect;
        let (echo, echo_cursor) = self.echo_line();
        if let Some(col) = echo_cursor {
            cursor = Some((ECHO_PANE, 0, col));
        }
        panes.push((pane_rect(ECHO_PANE, echo_rect), vec![echo]));
        panes.retain(|(rect, _)| rect.w > 0 && rect.h > 0);

        self.diff(panes, cursor)
    }

    fn diff(
        &mut self,
        panes: Vec<(PaneRect, Vec<DisplayLine>)>,
        cursor: Option<(PaneId, u16, u16)>,
    ) -> Vec<DrawOp> {
        let size = self.frame().size;
        let state = &mut self.frames[self.current_frame].redisplay;
        let mut ops = Vec::new();
        if !state.faces_sent {
            ops.push(DrawOp::DefineFaces(faces::table()));
            state.faces_sent = true;
        }
        let layout: Vec<PaneRect> = panes.iter().map(|(rect, _)| *rect).collect();
        if layout != state.layout || size != state.size {
            ops.push(DrawOp::Layout {
                cols: size.0,
                rows: size.1,
                panes: layout.clone(),
            });
            state.lines.clear();
            state.layout = layout;
            state.size = size;
        }
        for (rect, lines) in panes {
            let old = state.lines.get(&rect.id);
            let unchanged = |row: usize| old.and_then(|old| old.get(row)) == Some(&lines[row]);
            let mut row = 0;
            while row < lines.len() {
                if unchanged(row) {
                    row += 1;
                    continue;
                }
                let first = row;
                while row < lines.len() && !unchanged(row) {
                    row += 1;
                }
                ops.push(DrawOp::Lines {
                    pane: rect.id,
                    first_row: first as u16,
                    lines: lines[first..row].to_vec(),
                });
            }
            state.lines.insert(rect.id, lines);
        }
        // A pane too small to exist cannot hold the cursor.
        let cursor = cursor.filter(|(pane, row, col)| {
            state
                .layout
                .iter()
                .any(|rect| rect.id == *pane && *row < rect.h && *col < rect.w)
        });
        if cursor != state.cursor || !ops.is_empty() {
            if let Some((pane, row, col)) = cursor {
                ops.push(DrawOp::Cursor {
                    pane,
                    row,
                    col,
                    shape: CursorShape::Block,
                });
            }
            state.cursor = cursor;
            ops.push(DrawOp::Flush);
        }
        ops
    }

    /// Moves the window start, by as little as possible, so that point is on
    /// one of the window's rows.
    pub(crate) fn scroll_to_point(&mut self, id: WindowId) {
        let window = &self.windows[id];
        let buffer = &self.buffers[window.buffer];
        let (rows, width) = (window.text_rows(), window.width());
        if rows == 0 || width == 0 {
            return;
        }
        let (text, tab) = (buffer.text(), buffer.tab_width);
        let point = buffer.marker(window.point);
        // Edits can leave the start marker in the middle of a row.
        let (mut start, _) = row_containing(text, buffer.marker(window.start), width, tab);

        let mut visible = false;
        let mut pos = start;
        for _ in 0..rows {
            let row = walk_row(text, pos, width, tab, |_, _, _, _| {});
            if row.contains(point) {
                visible = true;
                break;
            }
            match row.next {
                Some(next) => pos = next,
                None => break,
            }
        }
        if !visible {
            let (point_row, _) = row_containing(text, point, width, tab);
            start = if point < start {
                point_row
            } else {
                rows_back(text, point_row, rows - 1, width, tab)
            };
        }
        let (marker, buffer) = (window.start, window.buffer);
        self.buffers[buffer].set_marker(marker, start);
    }

    /// The rows of a window's text area and the cell that point is on.
    fn window_lines(&self, id: WindowId) -> (Vec<DisplayLine>, Option<(u16, u16)>) {
        let window = &self.windows[id];
        let buffer = &self.buffers[window.buffer];
        let (rows, width) = (window.text_rows(), window.width());
        let (text, tab) = (buffer.text(), buffer.tab_width);
        let point = buffer.marker(window.point);
        let region = match buffer.mark() {
            Some(mark) if buffer.mark_active && id == self.selected() => {
                mark.min(point)..mark.max(point)
            }
            _ => 0..0,
        };

        let mut lines = Vec::with_capacity(rows);
        let mut cursor = None;
        let mut pos = Some(buffer.marker(window.start));
        let mut glyph = String::new();
        for row_index in 0..rows {
            let Some(start) = pos else {
                lines.push(DisplayLine::default());
                continue;
            };
            let mut line = DisplayLine::default();
            let mut end_col = 0;
            let mut point_col = None;
            let row = walk_row(text, start, width, tab, |pos, c, col, cells| {
                let face = if region.contains(&pos) {
                    faces::REGION
                } else {
                    faces::DEFAULT
                };
                // In a pane one or two cells wide, a wide char may not fit in
                // the columns left of the continuation marker.
                if col + cells <= width.saturating_sub(1).max(1) {
                    glyph.clear();
                    push_glyph(&mut glyph, c, cells);
                    line.push(&glyph, face);
                }
                if pos == point {
                    point_col = Some(col);
                }
                end_col = col + cells;
            });
            if row.contains(point) {
                let col = point_col.unwrap_or(end_col).min(width - 1);
                cursor = Some((row_index as u16, col as u16));
            }
            if row.continued && width >= 2 {
                let padding = (width - 1).saturating_sub(end_col.min(width));
                line.push(&" ".repeat(padding), faces::DEFAULT);
                line.push("\\", faces::DEFAULT);
            } else if row.next.is_some() && region.contains(&row.end) {
                // The newline is selected: highlight to the window's edge.
                line.fill = faces::REGION;
            }
            lines.push(line);
            pos = row.next;
        }
        (lines, cursor)
    }

    fn mode_line(&self, id: WindowId) -> DisplayLine {
        let window = &self.windows[id];
        let buffer = &self.buffers[window.buffer];
        let point = buffer.marker(window.point);
        let flags = if buffer.read_only {
            "%%"
        } else if buffer.is_modified() {
            "**"
        } else {
            "--"
        };
        let line = buffer.text().line_of(point) + 1;
        let col = column_of(buffer.text(), point, buffer.tab_width);
        let modes = self.mode_line_modes(window.buffer);
        let text = format!(" {flags}  {}   L{line} C{col}  ({modes})", buffer.name);
        let face = if id == self.selected() {
            faces::MODE_LINE
        } else {
            faces::MODE_LINE_INACTIVE
        };
        let width = window.width() + usize::from(window.border);
        DisplayLine::plain(fit(&text, width), face)
    }

    /// The echo area: a pending question, the minibuffer, or the last
    /// message. Also returns the cursor column if the cursor belongs there.
    fn echo_line(&self) -> (DisplayLine, Option<u16>) {
        let width = usize::from(self.frame().size.0);
        // A prompt is drawn only in the frame that is asking.
        let prompting = self.prompt_frame == Some(self.current_frame);
        if let Some(question) = self.key_prompt.as_ref().filter(|_| prompting) {
            let prompt = &question.prompt;
            let shown = fit(prompt, width.saturating_sub(1));
            let line = DisplayLine::plain(shown, faces::DEFAULT);
            return (line, Some(str_cells(shown) as u16));
        }
        let Some(state) = self.minibuffer.as_ref().filter(|_| prompting) else {
            let message = self.frame().echo.as_deref().unwrap_or("");
            let first_line = message.lines().next().unwrap_or("");
            return (
                DisplayLine::plain(fit(first_line, width), faces::DEFAULT),
                None,
            );
        };

        let window = &self.windows[self.minibuffer_window()];
        let buffer = &self.buffers[window.buffer];
        let point = buffer.marker(window.point);

        // One entry per char: what is drawn, its width and its face.
        let mut glyphs: Vec<(String, usize, FaceId)> = Vec::new();
        let mut add = |c: char, face: FaceId| {
            let cells = char_cells(c, 0, 2);
            let mut glyph = String::new();
            push_glyph(&mut glyph, c, cells);
            glyphs.push((glyph, cells, face));
        };
        state
            .prompt
            .chars()
            .for_each(|c| add(c, faces::MINIBUFFER_PROMPT));
        let prompt_len = state.prompt.chars().count();
        buffer
            .text()
            .chars_from(0)
            .for_each(|c| add(c, faces::DEFAULT));
        if let Some(hint) = &self.minibuffer_hint {
            hint.chars().for_each(|c| add(c, faces::DEFAULT));
        }

        let cursor_cell: usize = glyphs[..prompt_len + point].iter().map(|g| g.1).sum();
        // Scroll horizontally just enough to keep the cursor in view.
        let hidden = (cursor_cell + 1).saturating_sub(width);
        let mut line = DisplayLine::default();
        let (mut skipped, mut used) = (0, 0);
        for (glyph, cells, face) in &glyphs {
            if skipped < hidden {
                skipped += cells;
            } else if used + cells <= width {
                line.push(glyph, *face);
                used += cells;
            } else {
                break;
            }
        }
        (line, Some(cursor_cell.saturating_sub(skipped) as u16))
    }
}
