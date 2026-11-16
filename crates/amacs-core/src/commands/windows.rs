use crate::editor::Editor;
use crate::error::{CmdError, CmdResult};
use crate::window::SplitDir;

/// Smallest window that may be split: each half needs a text row and a mode
/// line, or a few columns.
const MIN_SPLIT_ROWS: u16 = 4;
const MIN_SPLIT_COLS: u16 = 10;

fn split(ed: &mut Editor, dir: SplitDir) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    let rect = ed.window().rect;
    let too_small = match dir {
        SplitDir::Below => rect.h < MIN_SPLIT_ROWS,
        SplitDir::Right => rect.w < MIN_SPLIT_COLS,
    };
    if too_small {
        return Err(CmdError::msg("Window too small for splitting"));
    }
    // The new window starts out showing the same text at the same place.
    let (point, start) = (ed.point(), ed.buffer().marker(ed.window().start));
    let buffer = ed.window().buffer;
    ed.buffers[buffer].save_view(point, start);
    let new = ed.make_window(buffer);
    ed.layout.split(ed.selected, dir, new);
    ed.relayout();
    Ok(())
}

pub fn split_below(ed: &mut Editor) -> CmdResult {
    split(ed, SplitDir::Below)
}

pub fn split_right(ed: &mut Editor) -> CmdResult {
    split(ed, SplitDir::Right)
}

pub fn delete_window(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    let leaves = ed.layout.leaves();
    if leaves.len() == 1 {
        return Err(CmdError::msg("Attempt to delete sole window"));
    }
    let index = leaves
        .iter()
        .position(|id| *id == ed.selected)
        .expect("the selected window is in the layout");
    let target = ed.selected;
    ed.layout.remove(target);
    ed.remove_window(target);
    let remaining = ed.layout.leaves();
    ed.selected = remaining[index.min(remaining.len() - 1)];
    ed.relayout();
    Ok(())
}

pub fn delete_other_windows(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    let keep = ed.selected;
    for id in ed.layout.leaves() {
        if id != keep {
            ed.layout.remove(id);
            ed.remove_window(id);
        }
    }
    ed.relayout();
    Ok(())
}

pub fn other_window(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    let leaves = ed.layout.leaves();
    let index = leaves
        .iter()
        .position(|id| *id == ed.selected)
        .expect("the selected window is in the layout");
    let count = leaves.len() as i64;
    let next = (index as i64 + ed.arg()).rem_euclid(count);
    ed.selected = leaves[next as usize];
    Ok(())
}
