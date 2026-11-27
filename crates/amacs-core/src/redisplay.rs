//! Turning editor state into draw operations.
//!
//! Each call builds the lines every pane should show and sends only the rows
//! that differ from what the frontend already has.

use std::collections::HashMap;

use amacs_proto::{CursorShape, DisplayLine, DrawOp, FaceId, PaneId, PaneRect};

use crate::buffer::{Buffer, LineNumbers};
use crate::decorate::RowContext;
use crate::display::{
    char_cells, column_of, overlay_cells, push_glyph, row_containing, rows_back, str_cells,
    walk_row,
};
use crate::editor::Editor;
use crate::faces;
use crate::window::{Rect, Window, WindowId};

const ECHO_PANE: PaneId = PaneId(0);

/// What the frontend currently shows.
#[derive(Default)]
pub(crate) struct RedisplayState {
    /// Version and length of the face table that the frontend has.
    faces: (u64, usize),
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
        // Relative numbers are counts for moving by lines, which steps
        // over hidden lines.
        LineNumbers::Relative => {
            line.abs_diff(point_line) - buffer.folds().hidden_lines_between(text, line, point_line)
        }
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
        }
        // Point can have been put among hidden lines outside any command.
        self.settle_points();
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
        let faces = (self.faces.version(), self.faces.table_len());
        let new_faces = state.faces.0 != faces.0;
        if state.faces != faces {
            // More faces of the same version are combinations made since:
            // the table grew, and what is drawn still looks right.
            ops.push(DrawOp::DefineFaces(self.faces.table()));
            state.faces = faces;
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
                    color: self.faces.color("cursor"),
                });
            }
            state.cursor = cursor;
            ops.push(DrawOp::Flush);
        }
        self.spell_flush();
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
        let (text, tab) = (buffer.text(), self.window_flow(id));
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
        // In a frame that a program has for itself, a secret is not drawn.
        if self.frame().program && self.is_secret(window.buffer) {
            let mut lines = vec![DisplayLine::plain(
                "This buffer is secret. It is not shown to a program.",
                faces::DEFAULT,
            )];
            lines.resize(rows, DisplayLine::default());
            return (lines, Some((0, 0)));
        }
        let (gutter, width) = text_area(window, buffer);
        let (text, tab) = (buffer.text(), self.window_flow(id));
        let point = buffer.marker(window.point);
        let point_line = text.line_of(point);
        let window_start = buffer.marker(window.start);
        // What the window can show at most: a char per cell, unless lines
        // are hidden between those that it shows.
        let shown = if buffer.folds().is_empty() && tab.hidden.is_empty() {
            window_start..(window_start + rows * width).min(text.len())
        } else {
            self.visible_range(id)
        };
        let highlights =
            buffer.highlights(text.char_to_byte(shown.start)..text.char_to_byte(shown.end));
        let mut decorations = self.decorations(id, shown, highlights.as_ref());

        let mut lines = Vec::with_capacity(rows);
        let mut cursor = None;
        let mut pos = Some(window_start);
        let mut glyph = String::new();
        // Labels to draw on the row: column and keys.
        let mut labels: Vec<(usize, &str)> = Vec::new();
        for row_index in 0..rows {
            let Some(start) = pos else {
                lines.push(DisplayLine::default());
                continue;
            };
            let mut line = DisplayLine::default();
            if gutter > 0 {
                let (number, face) = line_number(buffer, start, point_line, gutter);
                // A line's mark takes the place of the space after its
                // number, on the row that starts the line.
                let row_line = text.line_of(start);
                let sign = buffer
                    .line_sign(row_line)
                    .filter(|_| text.line_start(row_line) == start)
                    .and_then(|name| self.faces.face_id(name));
                match sign {
                    Some(sign) => {
                        line.push(&number[..number.len() - 1], face);
                        line.push("▎", sign);
                    }
                    None => line.push(&number, face),
                }
            }
            let line_index = text.line_of(start);
            let line_end = text.line_end(line_index);
            let trailing = text
                .chars_before(line_end)
                .take(line_end - text.line_start(line_index))
                .take_while(|c| matches!(c, ' ' | '\t'))
                .count();
            let context = RowContext {
                line: line_index,
                base_column: column_of(text, start, tab.tab_width),
                trailing_from: line_end - trailing,
                line_end,
            };
            let mut end_col = 0;
            let mut point_col = None;
            let mut byte = text.char_to_byte(start);
            // The char that the walk comes to next, unless chars before it
            // are left out.
            let mut expected = start;
            labels.clear();
            let row = walk_row(text, start, width, tab, |pos, c, col, cells| {
                if pos != expected {
                    byte = text.char_to_byte(pos);
                }
                expected = pos + 1;
                let highlight = highlights.as_ref().and_then(|found| found.get(byte));
                byte += c.len_utf8();
                let base = highlight.map_or(faces::DEFAULT, faces::syntax);
                let face = decorations.face(base, pos, &context, col);
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
                for (label_pos, keys) in &decorations.labels {
                    if *label_pos == Some(pos) {
                        labels.push((col, keys));
                    }
                }
                end_col = col + cells;
            });
            if row.contains(point) {
                let col = point_col.unwrap_or(end_col).min(width - 1);
                cursor = Some((row_index as u16, (gutter + col) as u16));
            }
            if row.folded {
                // The mark that lines are hidden: after the text, or over
                // its last cell on a row that is full.
                let mark = [('…', faces::FOLD)];
                if end_col < width.saturating_sub(1).max(1) {
                    line.push("…", faces::FOLD);
                } else {
                    overlay_cells(&mut line, gutter + end_col - 1, 1, &mark, faces::FOLD);
                }
            }
            let fill = decorations.fill(line_index);
            // Rows that break between words have no mark of it: the
            // break says so.
            if row.continued && width >= 2 && !tab.words {
                let padding = (width - 1).saturating_sub(end_col.min(width));
                line.push(&" ".repeat(padding), fill);
                line.push("\\", fill);
            } else if row.next.is_some()
                && decorations.rectangle.is_none()
                && decorations.region.contains(&row.end)
            {
                // The newline is selected: highlight to the window's edge.
                line.fill = faces::REGION;
            } else {
                line.fill = fill;
            }
            // Labels at the end of the line, as on an empty one, and the
            // label of the window itself in its corner.
            if !row.continued {
                for (label_pos, keys) in &decorations.labels {
                    if *label_pos == Some(row.end) {
                        labels.push((end_col, keys));
                    }
                }
            }
            if row_index == 0 {
                for (label_pos, keys) in &decorations.labels {
                    if label_pos.is_none() {
                        labels.push((0, keys));
                    }
                }
            }
            for (col, keys) in &labels {
                let cells: Vec<(char, FaceId)> =
                    keys.chars().map(|c| (c, faces::JUMP_LABEL)).collect();
                let room = (gutter + width).saturating_sub(gutter + col);
                let wanted = cells.len().min(room);
                if wanted > 0 {
                    overlay_cells(&mut line, gutter + col, wanted, &cells, faces::JUMP_LABEL);
                }
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
        if let Some(more) = buffer.mode_line.as_deref().filter(|more| !more.is_empty()) {
            text.push(' ');
            text.push_str(more);
        }
        let servers = self.lsp_mode_line(window.buffer);
        if !servers.is_empty() {
            text.push(' ');
            text.push_str(&servers);
        }
        if self.defining_macro() && id == self.selected() {
            text.push_str(" Def");
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
        // A secret is drawn as a star for each char of it.
        buffer
            .text()
            .chars_from(0)
            .for_each(|c| add(if state.secret { '*' } else { c }, faces::DEFAULT));
        if let Some(hint) = &self.minibuffer_hint {
            hint.chars().for_each(|c| add(c, faces::DEFAULT));
        }

        let cursor_cell: usize = glyphs[..prompt_len + point].iter().map(|g| g.1).sum();
        // Scroll horizontally just enough to keep the cursor in view.
        let hidden = (cursor_cell + 1).saturating_sub(width);
        let mut line = DisplayLine::default();
        // Where no candidate is selected, RET takes the input as it is
        // typed, and the row of the input has the highlight of the
        // selection.
        let on_input = state
            .session
            .as_ref()
            .is_some_and(|session| session.selected().is_none());
        if on_input {
            line.fill = faces::CURRENT_CANDIDATE;
            for (_, _, face) in &mut glyphs {
                *face = self.faces.overlay(*face, &[faces::CURRENT_CANDIDATE]);
            }
        }
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
            // A prompt without candidates may still have a hint.
            if let Some(hint) = self.prompt_hint() {
                return hint
                    .lines()
                    .map(|line| DisplayLine::plain(fit(line, width), faces::ANNOTATION))
                    .collect();
            }
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
        // What the prompt says about its input comes before what can
        // follow it.
        let hint = self.prompt_hint().into_iter().flat_map(|hint| {
            hint.lines().map(|text| {
                let mut line = DisplayLine::default();
                line.push(fit(text, width), faces::ANNOTATION);
                line
            })
        });
        hint.chain(visible.iter().map(|(index, candidate)| {
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
        }))
        .collect()
    }
}
