//! Rectangles: the columns between point and mark, on the lines between
//! them.

use std::ops::Range;

use crate::completion::Completion;
use crate::display::{column_of, pos_at_column};
use crate::editor::Editor;
use crate::error::{CmdError, CmdResult};

/// The lines and columns of the rectangle whose corners are point and mark.
fn bounds(ed: &Editor) -> CmdResult<(Range<usize>, Range<usize>)> {
    let region = ed.region()?;
    let (text, tab) = (ed.text(), ed.buffer().tab_width);
    let (a, b) = (
        column_of(text, region.start, tab),
        column_of(text, region.end, tab),
    );
    let lines = text.line_of(region.start)..text.line_of(region.end) + 1;
    Ok((lines, a.min(b)..a.max(b)))
}

/// The part of a line that lies in a range of columns.
fn segment(ed: &Editor, line: usize, columns: &Range<usize>) -> Range<usize> {
    let (text, tab) = (ed.text(), ed.buffer().tab_width);
    pos_at_column(text, line, columns.start, tab)..pos_at_column(text, line, columns.end, tab)
}

/// The text of the rectangle, a string per line.
fn extract(ed: &Editor) -> CmdResult<Vec<String>> {
    let (lines, columns) = bounds(ed)?;
    Ok(lines
        .map(|line| ed.text().slice(segment(ed, line, &columns)))
        .collect())
}

/// Removes the rectangle and leaves point at its top left corner.
fn delete(ed: &mut Editor) -> CmdResult {
    let (lines, columns) = bounds(ed)?;
    let first = lines.start;
    // From the last line up, so that earlier positions stay valid.
    for line in lines.rev() {
        let range = segment(ed, line, &columns);
        ed.delete(range)?;
    }
    let tab = ed.buffer().tab_width;
    ed.set_point(pos_at_column(ed.text(), first, columns.start, tab));
    finish(ed);
    Ok(())
}

fn finish(ed: &mut Editor) {
    let buffer = ed.buffer_mut();
    buffer.mark_active = false;
    buffer.rectangle_mark = false;
}

/// Pads a line with spaces so that it reaches a column, and returns the
/// position at that column.
fn reach_column(ed: &mut Editor, line: usize, column: usize) -> CmdResult<usize> {
    let tab = ed.buffer().tab_width;
    let end = ed.text().line_end(line);
    let width = column_of(ed.text(), end, tab);
    if width < column {
        ed.buffer_mut().insert(end, &" ".repeat(column - width))?;
    }
    Ok(pos_at_column(ed.text(), line, column, tab))
}

pub fn kill_rectangle(ed: &mut Editor) -> CmdResult {
    let killed = extract(ed)?;
    ed.kill_new(killed.join("\n"));
    ed.killed_rectangle = killed;
    delete(ed)
}

pub fn copy_rectangle(ed: &mut Editor) -> CmdResult {
    let copied = extract(ed)?;
    ed.kill_new(copied.join("\n"));
    ed.killed_rectangle = copied;
    finish(ed);
    Ok(())
}

pub fn delete_rectangle(ed: &mut Editor) -> CmdResult {
    delete(ed)
}

/// Inserts the last killed rectangle with its top left corner at point.
pub fn yank_rectangle(ed: &mut Editor) -> CmdResult {
    let rectangle = ed.killed_rectangle.clone();
    if rectangle.is_empty() {
        return Err(CmdError::msg("No rectangle has been killed"));
    }
    let tab = ed.buffer().tab_width;
    let first = ed.text().line_of(ed.point());
    let column = column_of(ed.text(), ed.point(), tab);
    let mut end = ed.point();
    for (offset, row) in rectangle.iter().enumerate() {
        let line = first + offset;
        if line >= ed.text().len_lines() {
            let len = ed.buffer().len();
            ed.buffer_mut().insert(len, "\n")?;
        }
        let pos = reach_column(ed, line, column)?;
        ed.buffer_mut().insert(pos, row)?;
        end = pos + row.chars().count();
    }
    ed.set_point(end);
    Ok(())
}

/// Blanks out the rectangle by inserting spaces, pushing its text right.
pub fn open_rectangle(ed: &mut Editor) -> CmdResult {
    let (lines, columns) = bounds(ed)?;
    let first = lines.start;
    for line in lines.rev() {
        let pos = reach_column(ed, line, columns.start)?;
        ed.buffer_mut().insert(pos, &" ".repeat(columns.len()))?;
    }
    let tab = ed.buffer().tab_width;
    ed.set_point(pos_at_column(ed.text(), first, columns.start, tab));
    finish(ed);
    Ok(())
}

/// Replaces the rectangle's text on every line with a string.
pub fn string_rectangle(ed: &mut Editor) -> CmdResult {
    let (lines, columns) = bounds(ed)?;
    ed.read_string(
        "String rectangle: ",
        "",
        Completion::None,
        move |ed, string| {
            let last = lines.end - 1;
            for line in lines.rev() {
                let range = segment(ed, line, &columns);
                ed.delete(range)?;
                let pos = reach_column(ed, line, columns.start)?;
                ed.buffer_mut().insert(pos, &string)?;
            }
            let tab = ed.buffer().tab_width;
            let end = pos_at_column(ed.text(), last, columns.start, tab) + string.chars().count();
            ed.set_point(end);
            finish(ed);
            Ok(())
        },
    )
}

/// Starts marking a rectangle: the region is shown and killed as the
/// columns between point and mark.
pub fn rectangle_mark_mode(ed: &mut Editor) -> CmdResult {
    let point = ed.point();
    let buffer = ed.buffer_mut();
    if buffer.rectangle_mark && buffer.mark_active {
        buffer.rectangle_mark = false;
        buffer.mark_active = false;
        return Ok(());
    }
    if !buffer.mark_active {
        buffer.set_mark(point);
    }
    buffer.mark_active = true;
    buffer.rectangle_mark = true;
    ed.message("Rectangle mark set");
    Ok(())
}

/// Whether the active region is a rectangle.
pub(crate) fn rectangle_is_marked(ed: &Editor) -> bool {
    ed.buffer().rectangle_mark && ed.buffer().mark_active
}
