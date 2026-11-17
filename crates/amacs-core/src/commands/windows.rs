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
    let selected = ed.selected();
    ed.frame_mut().layout.split(selected, dir, new);
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
    let leaves = ed.frame().layout.leaves();
    if leaves.len() == 1 {
        return Err(CmdError::msg("Attempt to delete sole window"));
    }
    let index = leaves
        .iter()
        .position(|id| *id == ed.selected())
        .expect("the selected window is in the layout");
    let target = ed.selected();
    ed.frame_mut().layout.remove(target);
    ed.remove_window(target);
    let remaining = ed.frame().layout.leaves();
    ed.frame_mut().selected = remaining[index.min(remaining.len() - 1)];
    ed.relayout();
    Ok(())
}

pub fn delete_other_windows(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    let keep = ed.selected();
    for id in ed.frame().layout.leaves() {
        if id != keep {
            ed.frame_mut().layout.remove(id);
            ed.remove_window(id);
        }
    }
    ed.relayout();
    Ok(())
}

pub fn other_window(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    let leaves = ed.frame().layout.leaves();
    let index = leaves
        .iter()
        .position(|id| *id == ed.selected())
        .expect("the selected window is in the layout");
    let count = leaves.len() as i64;
    let next = (index as i64 + ed.arg()).rem_euclid(count);
    ed.frame_mut().selected = leaves[next as usize];
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// Selects the window that lies in a direction from the selected one, at
/// the height or column of the cursor.
fn windmove(ed: &mut Editor, direction: Direction) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    let here = ed.window().rect;
    // The cell to look from: where point is drawn, near enough.
    let text = ed.text();
    let point = ed.point();
    let start = ed.buffer().marker(ed.window().start);
    let row = text.line_of(point).saturating_sub(text.line_of(start)) as u16;
    let (x, y) = (here.x, here.y + row.min(here.h.saturating_sub(1)));
    let selected = ed.selected();
    let found = ed
        .frame()
        .layout
        .leaves()
        .into_iter()
        .filter(|id| *id != selected)
        .filter_map(|id| {
            let rect = ed.windows[id].rect;
            let across = (rect.y..rect.y + rect.h).contains(&y);
            let along = (rect.x..=rect.x + rect.w).contains(&x);
            // How far away, for windows on the right side of the selected.
            let distance = match direction {
                Direction::Left if across && rect.x < here.x => here.x - rect.x,
                Direction::Right if across && rect.x > here.x => rect.x - here.x,
                Direction::Up if along && rect.y < here.y => here.y - rect.y,
                Direction::Down if along && rect.y > here.y => rect.y - here.y,
                _ => return None,
            };
            Some((distance, id))
        })
        .min_by_key(|(distance, _)| *distance);
    match found {
        Some((_, id)) => {
            ed.frame_mut().selected = id;
            Ok(())
        }
        None => Err(CmdError::msg(match direction {
            Direction::Left => "No window to the left",
            Direction::Right => "No window to the right",
            Direction::Up => "No window above",
            Direction::Down => "No window below",
        })),
    }
}

pub fn windmove_left(ed: &mut Editor) -> CmdResult {
    windmove(ed, Direction::Left)
}

pub fn windmove_right(ed: &mut Editor) -> CmdResult {
    windmove(ed, Direction::Right)
}

pub fn windmove_up(ed: &mut Editor) -> CmdResult {
    windmove(ed, Direction::Up)
}

pub fn windmove_down(ed: &mut Editor) -> CmdResult {
    windmove(ed, Direction::Down)
}

/// Makes the selected window taller or wider by `delta` cells, taking them
/// from a neighbour.
fn resize(ed: &mut Editor, dir: SplitDir, delta: i64) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    let selected = ed.selected();
    let rects: std::collections::HashMap<_, _> = ed
        .frame()
        .layout
        .leaves()
        .into_iter()
        .map(|id| (id, ed.windows[id].rect))
        .collect();
    let rect_of = |id| rects.get(&id).copied().unwrap_or_default();
    if !ed.frame_mut().layout.resize(selected, dir, delta, &rect_of) {
        return Err(CmdError::msg(match dir {
            SplitDir::Below => "No window above or below to take rows from or give them to",
            SplitDir::Right => "No window beside this one to take columns from or give them to",
        }));
    }
    ed.relayout();
    Ok(())
}

pub fn enlarge_window(ed: &mut Editor) -> CmdResult {
    resize(ed, SplitDir::Below, ed.arg())
}

pub fn shrink_window(ed: &mut Editor) -> CmdResult {
    resize(ed, SplitDir::Below, -ed.arg())
}

pub fn enlarge_window_horizontally(ed: &mut Editor) -> CmdResult {
    resize(ed, SplitDir::Right, ed.arg())
}

pub fn shrink_window_horizontally(ed: &mut Editor) -> CmdResult {
    resize(ed, SplitDir::Right, -ed.arg())
}

/// Gives every window of a split the same share of the space.
pub fn balance_windows(ed: &mut Editor) -> CmdResult {
    ed.frame_mut().layout.balance();
    ed.relayout();
    Ok(())
}

/// Selects another window, making one beside this if there is none, to
/// show something in.
pub(crate) fn select_other_window(ed: &mut Editor) -> CmdResult {
    if ed.frame().layout.leaves().len() == 1 {
        split_right(ed).or_else(|_| split_below(ed))?;
    }
    other_window(ed)
}
