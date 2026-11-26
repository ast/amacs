use amacs_text::Text;

use crate::display::{
    Flow, column_of, pos_at_column, row_containing, rows_back, rows_forward, walk_row,
};
use crate::editor::Editor;
use crate::error::{CmdError, CmdResult};

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric()
}

/// End of the next word at or after `pos`.
pub fn forward_word_pos(text: &Text, pos: usize) -> usize {
    let mut chars = text.chars_from(pos).peekable();
    let mut pos = pos;
    while chars.next_if(|c| !is_word_char(*c)).is_some() {
        pos += 1;
    }
    while chars.next_if(|c| is_word_char(*c)).is_some() {
        pos += 1;
    }
    pos
}

/// Start of the previous word at or before `pos`.
pub fn backward_word_pos(text: &Text, pos: usize) -> usize {
    let mut chars = text.chars_before(pos).peekable();
    let mut pos = pos;
    while chars.next_if(|c| !is_word_char(*c)).is_some() {
        pos -= 1;
    }
    while chars.next_if(|c| is_word_char(*c)).is_some() {
        pos -= 1;
    }
    pos
}

/// Where a motion ends that came to a position: hidden lines are stepped
/// over, forward to the line after them and back to the line before them.
/// Forward, `None` if all lines to the end of the text are hidden.
fn step_over_hidden(ed: &Editor, pos: usize, forward: bool) -> Option<usize> {
    let buffer = ed.buffer();
    buffer
        .folds()
        .step(pos.min(buffer.len()), forward, buffer.len())
}

/// Puts point where a motion ends that came to a position, outside hidden
/// lines: before them if there is no line after them.
fn set_point_shown(ed: &mut Editor, pos: usize, forward: bool) {
    let pos = step_over_hidden(ed, pos, forward)
        .or_else(|| step_over_hidden(ed, pos, false))
        .unwrap_or(pos);
    ed.set_point(pos);
}

fn move_chars(ed: &mut Editor, delta: i64) -> CmdResult {
    let len = ed.buffer().len() as i64;
    let target = ed.point() as i64 + delta;
    let clamped = target.clamp(0, len) as usize;
    match step_over_hidden(ed, clamped, delta > 0) {
        Some(pos) => ed.set_point(pos),
        // Lines are hidden from here to the end of the text.
        None => return Err(CmdError::msg("End of buffer")),
    }
    if target < 0 {
        Err(CmdError::msg("Beginning of buffer"))
    } else if target > len {
        Err(CmdError::msg("End of buffer"))
    } else {
        Ok(())
    }
}

pub fn forward_char(ed: &mut Editor) -> CmdResult {
    move_chars(ed, ed.arg())
}

pub fn backward_char(ed: &mut Editor) -> CmdResult {
    move_chars(ed, -ed.arg())
}

/// Moves by logical lines, keeping the display column where the line is long
/// enough.
fn move_lines(ed: &mut Editor, delta: i64) -> CmdResult {
    let (text, tab) = (ed.text(), ed.buffer().tab_width);
    let point = ed.point();
    let goal = ed
        .window()
        .goal_col
        .unwrap_or_else(|| column_of(text, point, tab));
    // Line by line, since lines that are hidden are stepped over.
    let folds = ed.buffer().folds();
    let mut target = text.line_of(point);
    let mut stopped = false;
    for _ in 0..delta.unsigned_abs() {
        let next = if delta > 0 {
            folds.next_line(text, target)
        } else {
            folds.previous_line(text, target)
        };
        match next {
            Some(line) => target = line,
            None => {
                stopped = true;
                break;
            }
        }
    }
    let pos = pos_at_column(text, target, goal, tab);

    ed.set_point(pos);
    ed.window_mut().goal_col = Some(goal);
    ed.keep_goal = true;
    match (stopped, delta > 0) {
        (false, _) => Ok(()),
        (true, true) => Err(CmdError::msg("End of buffer")),
        (true, false) => Err(CmdError::msg("Beginning of buffer")),
    }
}

pub fn next_line(ed: &mut Editor) -> CmdResult {
    move_lines(ed, ed.arg())
}

pub fn previous_line(ed: &mut Editor) -> CmdResult {
    move_lines(ed, -ed.arg())
}

pub fn beginning_of_line(ed: &mut Editor) -> CmdResult {
    let text = ed.text();
    let start = text.line_start(text.line_of(ed.point()));
    ed.set_point(start);
    Ok(())
}

pub fn end_of_line(ed: &mut Editor) -> CmdResult {
    let text = ed.text();
    let end = text.line_end(text.line_of(ed.point()));
    ed.set_point(end);
    Ok(())
}

pub fn forward_word(ed: &mut Editor) -> CmdResult {
    let count = ed.arg();
    if count < 0 {
        return backward_words(ed, count.unsigned_abs());
    }
    forward_words(ed, count.unsigned_abs())
}

pub fn backward_word(ed: &mut Editor) -> CmdResult {
    let count = ed.arg();
    if count < 0 {
        return forward_words(ed, count.unsigned_abs());
    }
    backward_words(ed, count.unsigned_abs())
}

fn forward_words(ed: &mut Editor, count: u64) -> CmdResult {
    let pos = (0..count).fold(ed.point(), |pos, _| forward_word_pos(ed.text(), pos));
    set_point_shown(ed, pos, true);
    Ok(())
}

fn backward_words(ed: &mut Editor, count: u64) -> CmdResult {
    let pos = (0..count).fold(ed.point(), |pos, _| backward_word_pos(ed.text(), pos));
    set_point_shown(ed, pos, false);
    Ok(())
}

fn line_is_blank(text: &Text, line: usize) -> bool {
    let start = text.line_start(line);
    text.chars_from(start)
        .take(text.line_end(line) - start)
        .all(char::is_whitespace)
}

/// Moves to the blank line that ends the current or next paragraph.
pub fn forward_paragraph(ed: &mut Editor) -> CmdResult {
    let text = ed.text();
    let last = text.len_lines() - 1;
    let mut line = text.line_of(ed.point());
    while line < last && line_is_blank(text, line) {
        line += 1;
    }
    while line < last && !line_is_blank(text, line) {
        line += 1;
    }
    let pos = if line_is_blank(text, line) {
        text.line_start(line)
    } else {
        text.len()
    };
    set_point_shown(ed, pos, true);
    Ok(())
}

/// Moves to the blank line that precedes the current or previous paragraph.
pub fn backward_paragraph(ed: &mut Editor) -> CmdResult {
    let text = ed.text();
    let point = ed.point();
    let mut line = text.line_of(point);
    // From the start of a line, the paragraph in question is the one above.
    if line > 0 && point == text.line_start(line) {
        line -= 1;
    }
    while line > 0 && line_is_blank(text, line) {
        line -= 1;
    }
    while line > 0 && !line_is_blank(text, line) {
        line -= 1;
    }
    let pos = text.line_start(line);
    set_point_shown(ed, pos, false);
    Ok(())
}

pub fn beginning_of_buffer(ed: &mut Editor) -> CmdResult {
    set_mark_silently(ed);
    ed.set_point(0);
    Ok(())
}

pub fn end_of_buffer(ed: &mut Editor) -> CmdResult {
    set_mark_silently(ed);
    set_point_shown(ed, usize::MAX, false);
    Ok(())
}

/// Leaves the mark where point was before a long jump, unless a region is
/// being extended.
fn set_mark_silently(ed: &mut Editor) {
    let point = ed.point();
    let buffer = ed.buffer_mut();
    if !buffer.mark_active {
        buffer.set_mark(point);
    }
}

/// Rows a full-window scroll moves by: the window height less two rows of
/// context.
fn page_rows(ed: &Editor) -> usize {
    ed.window().text_rows().saturating_sub(2).max(1)
}

fn window_geometry(ed: &Editor) -> (usize, usize, Flow<'_>) {
    let width = ed.text_width(ed.selected());
    (ed.window().text_rows(), width, ed.buffer().flow())
}

fn window_start(ed: &Editor) -> usize {
    ed.buffer().marker(ed.window().start)
}

fn set_window_start(ed: &mut Editor, pos: usize) {
    let marker = ed.window().start;
    ed.buffer_mut().set_marker(marker, pos);
}

/// Scrolls forward. Point moves only if it would otherwise leave the window.
pub fn scroll_up(ed: &mut Editor) -> CmdResult {
    let (_, width, tab) = window_geometry(ed);
    let start = window_start(ed);
    let new_start = rows_forward(ed.text(), start, page_rows(ed), width, tab);
    if new_start == start {
        return Err(CmdError::msg("End of buffer"));
    }
    set_window_start(ed, new_start);
    if ed.point() < new_start {
        ed.set_point(new_start);
    }
    Ok(())
}

/// Scrolls backward. Point moves only if it would otherwise leave the window.
pub fn scroll_down(ed: &mut Editor) -> CmdResult {
    let (rows, width, tab) = window_geometry(ed);
    let start = window_start(ed);
    let new_start = rows_back(ed.text(), start, page_rows(ed), width, tab);
    if new_start == start {
        return Err(CmdError::msg("Beginning of buffer"));
    }
    set_window_start(ed, new_start);

    // Find the last row now shown and pull point up to it if it is below.
    let tab = ed.buffer().flow();
    let text = ed.text();
    let mut last_row = new_start;
    let mut visible = false;
    let point = ed.point();
    for _ in 0..rows {
        let row = walk_row(text, last_row, width, tab, |_, _, _, _| {});
        if row.contains(point) {
            visible = true;
            break;
        }
        match row.next {
            Some(next) => last_row = next,
            None => break,
        }
    }
    if !visible {
        let last_shown = rows_back(text, last_row, 1, width, tab);
        ed.set_point(last_shown);
    }
    Ok(())
}

/// Scrolls so that point's row is in the middle of the window.
pub fn recenter(ed: &mut Editor) -> CmdResult {
    let (rows, width, tab) = window_geometry(ed);
    let text = ed.text();
    let (point_row, _) = row_containing(text, ed.point(), width, tab);
    let start = rows_back(text, point_row, rows / 2, width, tab);
    set_window_start(ed, start);
    Ok(())
}
