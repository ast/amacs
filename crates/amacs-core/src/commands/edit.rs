use crate::commands::motion::{backward_word_pos, forward_word_pos};
use crate::editor::Editor;
use crate::error::{CmdError, CmdResult};
use crate::kill_ring::KillDirection;

/// The prefix argument as a repeat count. Negative counts are not supported
/// by the commands that use this.
fn count(ed: &Editor) -> usize {
    usize::try_from(ed.arg()).unwrap_or(0)
}

pub fn self_insert(ed: &mut Editor) -> CmdResult {
    let c = ed
        .last_key
        .and_then(|key| key.self_insert_char())
        .ok_or_else(|| CmdError::msg("self-insert-command needs a character key"))?;
    let text: String = std::iter::repeat_n(c, count(ed)).collect();
    ed.insert(&text)
}

pub fn newline(ed: &mut Editor) -> CmdResult {
    ed.insert(&"\n".repeat(count(ed)))
}

pub fn open_line(ed: &mut Editor) -> CmdResult {
    let point = ed.point();
    ed.insert(&"\n".repeat(count(ed)))?;
    ed.set_point(point);
    Ok(())
}

pub fn delete_char(ed: &mut Editor) -> CmdResult {
    let point = ed.point();
    let end = point + count(ed);
    if end > ed.buffer().len() {
        return Err(CmdError::msg("End of buffer"));
    }
    ed.delete(point..end)?;
    Ok(())
}

pub fn delete_backward_char(ed: &mut Editor) -> CmdResult {
    let point = ed.point();
    let Some(start) = point.checked_sub(count(ed)) else {
        return Err(CmdError::msg("Beginning of buffer"));
    };
    ed.delete(start..point)?;
    ed.set_point(start);
    Ok(())
}

/// Deletes a range and puts the text on the kill ring.
fn kill(ed: &mut Editor, start: usize, end: usize, direction: KillDirection) -> CmdResult {
    let text = ed.delete(start..end)?;
    ed.kill_ring.kill(text, direction);
    ed.set_point(start);
    Ok(())
}

pub fn kill_line(ed: &mut Editor) -> CmdResult {
    let point = ed.point();
    let text = ed.text();
    let line_end = text.line_end(text.line_of(point));
    let end = if point == line_end {
        // At the end of a line, kill the newline itself.
        (line_end + 1).min(text.len())
    } else {
        line_end
    };
    if end == point {
        return Err(CmdError::msg("End of buffer"));
    }
    kill(ed, point, end, KillDirection::Forward)
}

pub fn kill_word(ed: &mut Editor) -> CmdResult {
    let point = ed.point();
    let end = (0..count(ed)).fold(point, |pos, _| forward_word_pos(ed.text(), pos));
    kill(ed, point, end, KillDirection::Forward)
}

pub fn backward_kill_word(ed: &mut Editor) -> CmdResult {
    let point = ed.point();
    let start = (0..count(ed)).fold(point, |pos, _| backward_word_pos(ed.text(), pos));
    kill(ed, start, point, KillDirection::Backward)
}

pub fn kill_region(ed: &mut Editor) -> CmdResult {
    let region = ed.region()?;
    // Killing backwards from point prepends, so that the ring entry reads in
    // buffer order however the region was marked.
    let direction = if ed.point() == region.end {
        KillDirection::Backward
    } else {
        KillDirection::Forward
    };
    kill(ed, region.start, region.end, direction)
}

pub fn kill_ring_save(ed: &mut Editor) -> CmdResult {
    let region = ed.region()?;
    let text = ed.text().slice(region);
    ed.kill_ring.push(text);
    ed.buffer_mut().mark_active = false;
    Ok(())
}

pub fn yank(ed: &mut Editor) -> CmdResult {
    let Some(text) = ed.kill_ring.current().map(str::to_owned) else {
        return Err(CmdError::msg("Kill ring is empty"));
    };
    let start = ed.point();
    ed.insert(&text)?;
    ed.buffer_mut().set_mark(start);
    let end = ed.point();
    ed.kill_ring.set_yanked(start..end);
    Ok(())
}

pub fn yank_pop(ed: &mut Editor) -> CmdResult {
    let Some(previous) = ed.kill_ring.last_yank() else {
        return Err(CmdError::msg("Previous command was not a yank"));
    };
    let Some(text) = ed.kill_ring.rotate().map(str::to_owned) else {
        return Err(CmdError::msg("Kill ring is empty"));
    };
    ed.delete(previous.clone())?;
    ed.set_point(previous.start);
    ed.insert(&text)?;
    ed.buffer_mut().set_mark(previous.start);
    let end = ed.point();
    ed.kill_ring.set_yanked(previous.start..end);
    Ok(())
}

pub fn undo(ed: &mut Editor) -> CmdResult {
    match ed.buffer_mut().undo()? {
        Some(point) => {
            ed.set_point(point);
            ed.message("Undo");
            Ok(())
        }
        None => Err(CmdError::msg("No further undo information")),
    }
}

pub fn undo_redo(ed: &mut Editor) -> CmdResult {
    match ed.buffer_mut().redo()? {
        Some(point) => {
            ed.set_point(point);
            ed.message("Redo");
            Ok(())
        }
        None => Err(CmdError::msg("No further redo information")),
    }
}

pub fn set_mark(ed: &mut Editor) -> CmdResult {
    let point = ed.point();
    let buffer = ed.buffer_mut();
    buffer.set_mark(point);
    buffer.mark_active = true;
    ed.message("Mark set");
    Ok(())
}

pub fn exchange_point_and_mark(ed: &mut Editor) -> CmdResult {
    let point = ed.point();
    let mark = ed
        .buffer()
        .mark()
        .ok_or_else(|| CmdError::msg("No mark set in this buffer"))?;
    let buffer = ed.buffer_mut();
    buffer.set_mark(point);
    buffer.mark_active = true;
    ed.set_point(mark);
    Ok(())
}

pub fn mark_whole_buffer(ed: &mut Editor) -> CmdResult {
    let buffer = ed.buffer_mut();
    buffer.set_mark(buffer.len());
    buffer.mark_active = true;
    ed.set_point(0);
    Ok(())
}
