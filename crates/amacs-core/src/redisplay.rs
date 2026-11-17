//! Turning editor state into draw operations.
//!
//! Each call builds the lines every pane should show and sends only the rows
//! that differ from what the frontend already has.

use std::collections::HashMap;

use amacs_proto::{CursorShape, DisplayLine, DrawOp, FaceId, PaneId, PaneRect};

use crate::buffer::{Buffer, LineNumbers};
use crate::display::{
    char_cells, column_of, push_glyph, row_containing, rows_back, str_cells, walk_row,
};
use crate::editor::Editor;
use crate::faces;
use crate::window::{Rect, Window, WindowId};

const ECHO_PANE: PaneId = PaneId(0);

/// What the frontend currently shows.
#[derive(Default)]
pub(crate) struct RedisplayState {
    /// Version of the face table that the frontend has.
    faces: u64,
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

/// The line-number cell for the row that starts at `row_start`: the number
/// and a space, `gutter` columns in all. Rows that continue a line are blank.
fn line_number(
    buffer: &Buffer,
    row_start: usize,
    point_line: usize,
    gutter: usize,
) -> (String, FaceId) {
    let text = buffer.text();
    let line = text.line_of(row_start);
    let digits = gutter - 1;
    if text.line_start(line) != row_start {
        return (" ".repeat(gutter), faces::LINE_NUMBER);
    }
    if line == point_line {
        let number = if buffer.line_numbers == LineNumbers::Relative {
            // The current line shows its own number, set apart to the left.
            format!("{:<digits$} ", line + 1)
        } else {
            format!("{:>digits$} ", line + 1)
        };
        return (number, faces::LINE_NUMBER_CURRENT);
    }
    let number = match buffer.line_numbers {
        LineNumbers::Relative => line.abs_diff(point_line),
        _ => line + 1,
    };
    (format!("{number:>digits$} "), faces::LINE_NUMBER)
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

/// Columns that a window gives to line numbers, and those left for text.
pub(crate) fn text_area(window: &Window, buffer: &Buffer) -> (usize, usize) {
    let width = window.width();
    if buffer.line_numbers == LineNumbers::Off {
        return (0, width);
    }
    let digits = buffer.text().len_lines().to_string().len().max(3);
    // A window too narrow for both shows text only.
    if width < digits + 1 + 8 {
        (0, width)
    } else {
        (digits + 1, width - digits - 1)
    }
}

impl Editor {
    /// Columns for text in a window: its width less the line numbers.
    pub(crate) fn text_width(&self, id: WindowId) -> usize {
        let window = &self.windows[id];
        text_area(window, &self.buffers[window.buffer]).1
    }

    /// Returns the operations that bring the frontend up to date. Empty if
    /// nothing changed.
    pub fn redisplay(&mut self) -> Vec<DrawOp> {
        self.relayout();
        let leaves = self.frame().layout.leaves();
        for id in &leaves {
            let buffer = self.windows[*id].buffer;
            self.buffers[buffer].update_syntax();
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
            let (mut lines, window_cursor) = self.window_lines(*id);
            if *id == self.selected() && self.completion_active() {
                self.overlay_popup(&mut lines, window_cursor, window.width());
            }
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
        let mut echo_lines = vec![echo];
        echo_lines.extend(self.echo_extra_lines());
        echo_lines.resize(usize::from(echo_rect.h), DisplayLine::default());
        panes.push((pane_rect(ECHO_PANE, echo_rect), echo_lines));
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
        // New faces change the look of what is already drawn, so everything
        // is drawn again.
        let new_faces = state.faces != self.faces.version();
        if new_faces {
            ops.push(DrawOp::DefineFaces(self.faces.table()));
            state.faces = self.faces.version();
        }
        let layout: Vec<PaneRect> = panes.iter().map(|(rect, _)| *rect).collect();
        if layout != state.layout || size != state.size || new_faces {
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
        let (rows, width) = (window.text_rows(), text_area(window, buffer).1);
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
        let rows = window.text_rows();
        let (gutter, width) = text_area(window, buffer);
        let (text, tab) = (buffer.text(), buffer.tab_width);
        let point = buffer.marker(window.point);
        let point_line = text.line_of(point);
        let window_start = buffer.marker(window.start);
        // Highlights for what the window can show at most: a char per cell.
        let highlights = {
            let end = (window_start + rows * width).min(text.len());
            buffer.highlights(text.char_to_byte(window_start)..text.char_to_byte(end))
        };
        let region = match buffer.mark() {
            Some(mark) if buffer.mark_active && id == self.selected() => {
                mark.min(point)..mark.max(point)
            }
            _ => 0..0,
        };
        // The diagnostics that can be seen, each covering at least a char.
        let visible_end = (window_start + rows * width).min(text.len());
        let diagnostics: Vec<_> = buffer
            .diagnostics()
            .map(|(range, diagnostic)| {
                (
                    range.start..range.end.max(range.start + 1),
                    diagnostic.severity,
                )
            })
            .filter(|(range, _)| range.start <= visible_end && range.end > window_start)
            .collect();

        let mut lines = Vec::with_capacity(rows);
        let mut cursor = None;
        let mut pos = Some(window_start);
        let mut glyph = String::new();
        for row_index in 0..rows {
            let Some(start) = pos else {
                lines.push(DisplayLine::default());
                continue;
            };
            let mut line = DisplayLine::default();
            if gutter > 0 {
                let (number, face) = line_number(buffer, start, point_line, gutter);
                line.push(&number, face);
            }
            let mut end_col = 0;
            let mut point_col = None;
            let mut byte = text.char_to_byte(start);
            let row = walk_row(text, start, width, tab, |pos, c, col, cells| {
                let highlight = highlights.as_ref().and_then(|found| found.get(byte));
                byte += c.len_utf8();
                let face = if region.contains(&pos) {
                    faces::REGION
                } else {
                    let face = highlight.map_or(faces::DEFAULT, faces::syntax);
                    // The most serious diagnostic on a char is the one shown.
                    let severity = diagnostics
                        .iter()
                        .filter(|(range, _)| range.contains(&pos))
                        .map(|(_, severity)| *severity)
                        .min();
                    match severity {
                        Some(severity) => self.faces.diagnostic(face, severity),
                        None => face,
                    }
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
                cursor = Some((row_index as u16, (gutter + col) as u16));
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
        // The mode line spans the whole window, line numbers included.
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
        let mut text = format!(" {flags}  {}   L{line} C{col}  ({modes})", buffer.name);
        let servers = self.lsp_mode_line(window.buffer);
        if !servers.is_empty() {
            text.push(' ');
            text.push_str(&servers);
        }
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
        // With candidates, the prompt starts with which is selected of how many.
        let counter = state.session.as_ref().map(|session| {
            let selected = session
                .selected()
                .map_or("*".to_owned(), |index| (index + 1).to_string());
            format!("{selected}/{} ", session.count())
        });

        // One entry per char: what is drawn, its width and its face.
        let mut glyphs: Vec<(String, usize, FaceId)> = Vec::new();
        let mut add = |c: char, face: FaceId| {
            let cells = char_cells(c, 0, 2);
            let mut glyph = String::new();
            push_glyph(&mut glyph, c, cells);
            glyphs.push((glyph, cells, face));
        };
        let counter = counter.unwrap_or_default();
        counter.chars().for_each(|c| add(c, faces::SHADOW));
        state
            .prompt
            .chars()
            .for_each(|c| add(c, faces::MINIBUFFER_PROMPT));
        let prompt_len = counter.chars().count() + state.prompt.chars().count();
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

    /// The rows under the first row of the echo area: the candidates of the
    /// open prompt, or a key hint.
    fn echo_extra_lines(&self) -> Vec<DisplayLine> {
        let width = usize::from(self.frame().size.0);
        let prompting = self.prompt_frame == Some(self.current_frame);
        let session = self
            .minibuffer
            .as_ref()
            .filter(|_| prompting)
            .and_then(|state| state.session.as_ref());
        let Some(session) = session else {
            return self
                .frame()
                .hint
                .iter()
                .map(|line| DisplayLine::plain(fit(line, width), faces::DEFAULT))
                .collect();
        };

        let visible = session.visible();
        // Annotations line up in a column just right of the longest candidate,
        // but never further right than the middle.
        let longest = visible
            .iter()
            .map(|(_, c)| str_cells(&c.text))
            .max()
            .unwrap_or(0);
        let column = (longest + 2).min(width / 2);
        visible
            .iter()
            .map(|(index, candidate)| {
                let selected = session.selected() == Some(*index);
                let face = if selected {
                    faces::CURRENT_CANDIDATE
                } else {
                    faces::DEFAULT
                };
                let mut line = DisplayLine {
                    fill: face,
                    ..DisplayLine::default()
                };
                let text: String = candidate
                    .text
                    .chars()
                    .map(|c| if c.is_control() { ' ' } else { c })
                    .collect();
                let text = fit(&text, width);
                line.push(text, face);
                let used = str_cells(text);
                if !candidate.annotation.is_empty() && used + 2 < width {
                    let gap = column.saturating_sub(used).max(2);
                    line.push(&" ".repeat(gap), face);
                    let annotation_face = if selected { face } else { faces::ANNOTATION };
                    line.push(
                        fit(&candidate.annotation, width - used - gap),
                        annotation_face,
                    );
                }
                line
            })
            .collect()
    }
}
