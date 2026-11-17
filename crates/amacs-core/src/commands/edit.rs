use crate::commands::indent;
use crate::commands::motion::{backward_word_pos, forward_word_pos};
use crate::commands::sexp::{self, PAREDIT_MODE};
use crate::editor::Editor;
use crate::error::{CmdError, CmdResult};
use crate::kill_ring::KillDirection;

/// The prefix argument as a repeat count. Negative counts are not supported
/// by the commands that use this.
fn count(ed: &Editor) -> usize {
    usize::try_from(ed.arg()).unwrap_or(0)
}

pub const AUTO_PAIR_MODE: &str = "auto-pair-mode";

/// The char that the last key typed.
fn typed_char(ed: &Editor) -> CmdResult<char> {
    ed.last_key
        .and_then(|key| key.self_insert_char())
        .ok_or_else(|| CmdError::msg("self-insert-command needs a character key"))
}

/// Inserts the typed char and nothing else.
pub(crate) fn self_insert_plain(ed: &mut Editor) -> CmdResult {
    let c = typed_char(ed)?;
    let text: String = std::iter::repeat_n(c, count(ed)).collect();
    ed.insert(&text)
}

/// Whether delimiters are inserted in pairs here. Paredit does its own
/// pairing, and the minibuffer is left alone.
fn pairing(ed: &Editor) -> bool {
    ed.minor_mode_enabled(AUTO_PAIR_MODE)
        && !ed.minor_mode_enabled(PAREDIT_MODE)
        && !ed.in_minibuffer()
}

/// The delimiter that closes `open`, if typing `open` here inserts both.
fn partner(ed: &Editor, open: char) -> Option<char> {
    let language = ed.buffer().language();
    match open {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '"' => Some('"'),
        '\'' if language.is_some_and(|language| language.pairs_single_quote) => Some('\''),
        '`' if language.is_some_and(|language| language.pairs_backtick) => Some('`'),
        _ => None,
    }
}

fn char_before_point(ed: &Editor) -> Option<char> {
    ed.point()
        .checked_sub(1)
        .and_then(|pos| ed.text().char_at(pos))
}

/// Handles a typed delimiter when delimiters come in pairs. Returns false
/// if the char should be inserted as usual.
fn insert_paired(ed: &mut Editor, c: char) -> CmdResult<bool> {
    let point = ed.point();
    let next = ed.text().char_at(point);
    let close = partner(ed, c);

    // Around the region.
    if let Some(close) = close
        && ed.buffer().mark_active
        && let Ok(region) = ed.region()
        && !region.is_empty()
    {
        ed.buffer_mut().insert(region.end, &close.to_string())?;
        ed.buffer_mut().insert(region.start, &c.to_string())?;
        ed.set_point(if point == region.start {
            region.start + 1
        } else {
            region.end + 2
        });
        return Ok(true);
    }
    // Typing a closing delimiter that is already there steps over it.
    let closes = matches!(c, ')' | ']' | '}') || close == Some(c);
    if closes && next == Some(c) {
        ed.set_point(point + 1);
        return Ok(true);
    }
    let Some(close) = close else {
        return Ok(false);
    };
    // Not in front of text, which the delimiter is more likely meant to
    // enclose by hand.
    let room = next.is_none_or(|c| c.is_whitespace() || matches!(c, ')' | ']' | '}' | ',' | ';'));
    // A quote after a word is an apostrophe or the end of a string.
    let quote_in_word = close == c
        && char_before_point(ed)
            .is_some_and(|before| before.is_alphanumeric() || before == '\\' || before == c);
    if !room || quote_in_word {
        return Ok(false);
    }
    ed.insert(&format!("{c}{close}"))?;
    ed.set_point(point + 1);
    Ok(true)
}

pub fn self_insert(ed: &mut Editor) -> CmdResult {
    let c = typed_char(ed)?;
    ed.snippet_before_insert()?;
    if count(ed) == 1 && pairing(ed) && insert_paired(ed, c)? {
        return Ok(());
    }
    self_insert_plain(ed)?;
    if matches!(c, ')' | ']' | '}') && !ed.in_minibuffer() {
        indent::after_closing_bracket(ed)?;
    }
    Ok(())
}

/// Whether point is between a pair of brackets with nothing in between.
fn between_brackets(ed: &Editor) -> bool {
    let next = ed.text().char_at(ed.point());
    matches!(
        (char_before_point(ed), next),
        (Some('('), Some(')')) | (Some('['), Some(']')) | (Some('{'), Some('}'))
    )
}

/// Removes spaces and tabs at the end of the line before point's line.
fn trim_previous_line(ed: &mut Editor) -> CmdResult {
    let text = ed.text();
    let line = text.line_of(ed.point());
    if line == 0 {
        return Ok(());
    }
    let end = text.line_end(line - 1);
    let blanks = text
        .chars_before(end)
        .take_while(|c| matches!(c, ' ' | '\t'))
        .count();
    ed.delete(end - blanks..end)?;
    Ok(())
}

/// Inserts a newline and indents the new line.
pub fn newline(ed: &mut Editor) -> CmdResult {
    if ed.in_minibuffer() {
        return ed.insert(&"\n".repeat(count(ed)));
    }
    if count(ed) == 1 && pairing(ed) && between_brackets(ed) {
        // Open the brackets: the closing one goes on a line of its own.
        let point = ed.point();
        ed.insert("\n\n")?;
        let line = ed.text().line_of(point);
        indent::indent_line(ed, line + 2)?;
        ed.set_point(ed.text().line_start(line + 1));
        return indent::indent_new_line(ed);
    }
    ed.insert(&"\n".repeat(count(ed)))?;
    trim_previous_line(ed)?;
    indent::indent_new_line(ed)
}

pub fn open_line(ed: &mut Editor) -> CmdResult {
    let point = ed.point();
    ed.insert(&"\n".repeat(count(ed)))?;
    ed.set_point(point);
    Ok(())
}

pub fn delete_char(ed: &mut Editor) -> CmdResult {
    if ed.minor_mode_enabled(PAREDIT_MODE) && ed.raw_prefix().is_none() {
        return sexp::paredit_forward_delete(ed);
    }
    let point = ed.point();
    let end = point + count(ed);
    if end > ed.buffer().len() {
        return Err(CmdError::msg("End of buffer"));
    }
    ed.delete(point..end)?;
    Ok(())
}

pub fn delete_backward_char(ed: &mut Editor) -> CmdResult {
    if ed.raw_prefix().is_none() {
        if ed.minor_mode_enabled(PAREDIT_MODE) && !ed.in_minibuffer() {
            return sexp::paredit_backward_delete(ed);
        }
        // A pair of delimiters with nothing in between goes together.
        let point = ed.point();
        let next = ed.text().char_at(point);
        if pairing(ed)
            && let Some(before) = char_before_point(ed)
            && next.is_some()
            && partner(ed, before) == next
        {
            ed.delete(point - 1..point + 1)?;
            ed.set_point(point - 1);
            return Ok(());
        }
    }
    let point = ed.point();
    let Some(start) = point.checked_sub(count(ed)) else {
        return Err(CmdError::msg("Beginning of buffer"));
    };
    ed.delete(start..point)?;
    ed.set_point(start);
    Ok(())
}

/// Deletes a range and puts the text on the kill ring.
pub(crate) fn kill(
    ed: &mut Editor,
    start: usize,
    end: usize,
    direction: KillDirection,
) -> CmdResult {
    let text = ed.delete(start..end)?;
    ed.kill_ring.kill(text, direction);
    ed.set_point(start);
    Ok(())
}

pub fn kill_line(ed: &mut Editor) -> CmdResult {
    if ed.minor_mode_enabled(PAREDIT_MODE) && ed.raw_prefix().is_none() && !ed.in_minibuffer() {
        return sexp::paredit_kill(ed);
    }
    kill_line_plain(ed)
}

/// Kills to the end of the line, whatever is there.
pub(crate) fn kill_line_plain(ed: &mut Editor) -> CmdResult {
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
    ed.import_clipboard();
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
