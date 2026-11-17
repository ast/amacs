//! The undo history as a tree to walk about in.
//!
//! Undoing and then editing starts a new branch of history rather than
//! losing the old one. This shows the branches under the echo area and
//! moves the buffer's text from state to state as the user moves.

use amacs_text::UndoTree;

use crate::buffer::BufferId;
use crate::editor::Editor;
use crate::error::{CmdError, CmdResult};

pub const UNDO_MAP: &str = "undo-tree";
const HELP: &str = "Undo tree: b f move, p n branch, q done, C-g cancel";
/// Columns between a state and the next.
const STEP: usize = 3;

/// The tree being shown.
pub(crate) struct UndoView {
    buffer: BufferId,
    /// The state to return to if the user cancels.
    origin: usize,
}

/// Where each state is drawn: its depth, its row and its parent's row.
/// The first state reached from a state continues its row; each other one
/// starts a row below everything drawn so far.
fn layout(tree: &UndoTree) -> Vec<(usize, usize, usize, usize)> {
    let mut placed = Vec::new();
    let mut rows: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    let mut next_row = 0;
    // Depth first without recursion, since histories get long: state,
    // depth, and whether it continues its parent's row.
    let mut stack = vec![(0, 0, true)];
    while let Some((state, depth, continues)) = stack.pop() {
        let parent_row = tree
            .parent(state)
            .and_then(|parent| rows.get(&parent).copied())
            .unwrap_or(0);
        let row = if continues {
            parent_row
        } else {
            next_row += 1;
            next_row
        };
        rows.insert(state, row);
        placed.push((state, depth, row, parent_row));
        // Pushed last to first, so that the first and all that follows
        // from it are placed before the second.
        for (index, child) in tree.children(state).iter().enumerate().rev() {
            stack.push((*child, depth + 1, index == 0));
        }
    }
    placed
}

/// The tree as lines of text, and the column and row of the current state.
pub(crate) fn render(tree: &UndoTree) -> (Vec<String>, usize, usize) {
    let placed = layout(tree);
    let rows = placed
        .iter()
        .map(|(_, _, row, _)| row + 1)
        .max()
        .unwrap_or(1);
    let cols = placed
        .iter()
        .map(|(_, depth, _, _)| depth * STEP + 1)
        .max()
        .unwrap_or(1);
    let mut grid = vec![vec![' '; cols]; rows];
    let mut current = (0, 0);
    for (state, depth, row, parent_row) in placed {
        let x = depth * STEP;
        if depth > 0 {
            let parent_x = x - STEP;
            if row == parent_row {
                grid[row][parent_x + 1] = '─';
                grid[row][parent_x + 2] = '─';
            } else {
                // Down from the parent, then across.
                for between in &mut grid[parent_row + 1..row] {
                    between[parent_x] = match between[parent_x] {
                        '└' | '├' => '├',
                        _ => '│',
                    };
                }
                grid[row][parent_x] = '└';
                grid[row][parent_x + 1] = '─';
                grid[row][parent_x + 2] = '─';
            }
        }
        grid[row][x] = if state == tree.current() {
            current = (x, row);
            '●'
        } else if tree.saved() == Some(state) {
            '◎'
        } else {
            '○'
        };
    }
    let lines = grid
        .into_iter()
        .map(|row| row.into_iter().collect::<String>().trim_end().to_owned())
        .collect();
    (lines, current.0, current.1)
}

impl Editor {
    pub(crate) fn undo_view_active(&self) -> bool {
        self.undo_view
            .as_ref()
            .is_some_and(|view| view.buffer == self.window().buffer)
    }

    /// Draws the tree under the echo area, scrolled so that the current
    /// state is in view.
    fn show_undo_tree(&mut self) {
        let Some(view) = &self.undo_view else { return };
        let (lines, x, y) = render(self.buffers[view.buffer].undo_tree());
        let (cols, rows) = self.frame().size;
        let width = usize::from(cols).max(1);
        let height = usize::from(rows).saturating_sub(4).clamp(1, 8);
        let first_row = y
            .saturating_sub(height / 2)
            .min(lines.len().saturating_sub(height));
        let first_col = (x + STEP + 1).saturating_sub(width);
        let shown = lines
            .iter()
            .skip(first_row)
            .take(height)
            .map(|line| line.chars().skip(first_col).take(width).collect())
            .collect();
        self.frame_mut().hint = shown;
        self.message(HELP);
    }

    /// Takes the buffer to a state of its history and shows the tree again.
    fn undo_view_goto(&mut self, state: usize) -> CmdResult {
        let Some(view) = &self.undo_view else {
            return Ok(());
        };
        let id = view.buffer;
        if let Some(point) = self.buffers[id].undo_goto(state)? {
            self.set_point(point);
        }
        self.show_undo_tree();
        Ok(())
    }

    /// After every command: any command that is not one of the tree's
    /// leaves the tree.
    pub(crate) fn undo_view_after_command(&mut self, command: Option<&str>) {
        // No name means a command that ran another, as M-x does, and that
        // one has been seen already.
        if command.is_some_and(|name| !name.starts_with("undo-tree-")) {
            self.undo_view = None;
        }
    }

    fn undo_view_state(&self) -> Option<(usize, &UndoTree)> {
        let view = self.undo_view.as_ref()?;
        let tree = self.buffers.get(view.buffer)?.undo_tree();
        Some((tree.current(), tree))
    }
}

/// Shows the undo history of the buffer as a tree to move about in.
pub fn visualize(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    if ed.buffer().read_only {
        return Err(CmdError::msg("Buffer is read-only"));
    }
    let buffer = ed.window().buffer;
    let origin = ed.buffer().undo_tree().current();
    ed.undo_view = Some(UndoView { buffer, origin });
    ed.show_undo_tree();
    Ok(())
}

pub fn backward(ed: &mut Editor) -> CmdResult {
    let Some((current, tree)) = ed.undo_view_state() else {
        return Ok(());
    };
    match tree.parent(current) {
        Some(parent) => ed.undo_view_goto(parent),
        None => {
            ed.show_undo_tree();
            Err(CmdError::msg("This is the first state"))
        }
    }
}

pub fn forward(ed: &mut Editor) -> CmdResult {
    let Some((current, tree)) = ed.undo_view_state() else {
        return Ok(());
    };
    let next = tree
        .redo_child(current)
        .or_else(|| tree.children(current).first().copied());
    match next {
        Some(next) => ed.undo_view_goto(next),
        None => {
            ed.show_undo_tree();
            Err(CmdError::msg("This is the last state of the branch"))
        }
    }
}

/// Moves to the branch beside the current one.
fn sibling(ed: &mut Editor, delta: isize) -> CmdResult {
    let Some((current, tree)) = ed.undo_view_state() else {
        return Ok(());
    };
    let siblings = tree
        .parent(current)
        .map_or(&[][..], |parent| tree.children(parent));
    let index = siblings.iter().position(|state| *state == current);
    let target = index
        .and_then(|index| index.checked_add_signed(delta))
        .and_then(|index| siblings.get(index))
        .copied();
    match target {
        Some(target) => ed.undo_view_goto(target),
        None => {
            ed.show_undo_tree();
            Err(CmdError::msg("No other branch that way"))
        }
    }
}

pub fn next_branch(ed: &mut Editor) -> CmdResult {
    sibling(ed, 1)
}

pub fn previous_branch(ed: &mut Editor) -> CmdResult {
    sibling(ed, -1)
}

/// Leaves the tree with the text as it is.
pub fn quit(ed: &mut Editor) -> CmdResult {
    ed.undo_view = None;
    ed.frame_mut().hint.clear();
    Ok(())
}

/// Leaves the tree with the text as it was when the tree was opened.
pub fn abort(ed: &mut Editor) -> CmdResult {
    if let Some(origin) = ed.undo_view.as_ref().map(|view| view.origin) {
        ed.undo_view_goto(origin)?;
    }
    ed.undo_view = None;
    ed.frame_mut().hint.clear();
    Err(CmdError::Quit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use amacs_text::{Edit, Transaction};

    fn push(tree: &mut UndoTree, text: &str) {
        tree.push(Transaction {
            edits: vec![Edit {
                pos: 0,
                deleted: String::new(),
                inserted: text.to_owned(),
            }],
            point_before: 0,
            point_after: 0,
        });
    }

    #[test]
    fn a_straight_history_is_one_row() {
        let mut tree = UndoTree::new();
        push(&mut tree, "a");
        push(&mut tree, "b");
        let (lines, x, y) = render(&tree);
        assert_eq!(lines, ["◎──○──●"]);
        assert_eq!((x, y), (6, 0));
    }

    #[test]
    fn branches_go_on_rows_of_their_own() {
        let mut tree = UndoTree::new();
        push(&mut tree, "a");
        push(&mut tree, "b");
        push(&mut tree, "c");
        tree.undo();
        tree.undo();
        push(&mut tree, "d");
        tree.undo();
        push(&mut tree, "e");
        push(&mut tree, "f");
        tree.mark_saved();
        tree.undo();
        let (lines, x, y) = render(&tree);
        assert_eq!(lines, ["○──○──○──○", "   ├──○", "   └──●──◎",]);
        assert_eq!((x, y), (6, 2));
    }

    #[test]
    fn a_branch_of_a_branch_goes_below_its_own_branch() {
        let mut tree = UndoTree::new();
        push(&mut tree, "a");
        push(&mut tree, "b");
        tree.undo();
        tree.undo();
        push(&mut tree, "c");
        push(&mut tree, "d");
        tree.undo();
        push(&mut tree, "e");
        let (lines, _, _) = render(&tree);
        assert_eq!(lines, ["◎──○──○", "└──○──○", "   └──●"]);
    }
}
