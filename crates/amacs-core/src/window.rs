use amacs_text::MarkerId;
use slotmap::new_key_type;

use crate::buffer::BufferId;

new_key_type! {
    pub struct WindowId;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

/// A view of a buffer. Point and the window start are markers in that buffer,
/// so edits made through another window keep them in place.
#[derive(Debug)]
pub struct Window {
    pub buffer: BufferId,
    pub point: MarkerId,
    /// Position of the first char shown.
    pub start: MarkerId,
    /// Column that vertical motion tries to keep.
    pub goal_col: Option<usize>,
    /// Area on the frame, including the mode line. Set by layout.
    pub rect: Rect,
    /// Whether a divider column follows the window on its right. The mode
    /// line extends under it. Set by layout.
    pub border: bool,
    /// Distinguishes this window's panes from every other window's.
    pub serial: u32,
}

impl Window {
    /// Rows available for text, excluding the mode line.
    pub fn text_rows(&self) -> usize {
        usize::from(self.rect.h.saturating_sub(1))
    }

    pub fn width(&self) -> usize {
        usize::from(self.rect.w)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDir {
    /// The new window goes below.
    Below,
    /// The new window goes to the right, separated by a divider column.
    Right,
}

/// How the frame is divided into windows.
#[derive(Debug)]
pub enum Layout {
    Leaf(WindowId),
    Split {
        dir: SplitDir,
        /// Each child with its share of the space, as a relative weight.
        children: Vec<(f32, Layout)>,
    },
}

impl Layout {
    /// Windows in display order: top to bottom, left to right.
    pub fn leaves(&self) -> Vec<WindowId> {
        let mut out = Vec::new();
        self.collect_leaves(&mut out);
        out
    }

    fn collect_leaves(&self, out: &mut Vec<WindowId>) {
        match self {
            Layout::Leaf(id) => out.push(*id),
            Layout::Split { children, .. } => {
                for (_, child) in children {
                    child.collect_leaves(out);
                }
            }
        }
    }

    /// Gives half of `target`'s space to `new`. Returns false if `target` is
    /// not in the tree.
    pub fn split(&mut self, target: WindowId, dir: SplitDir, new: WindowId) -> bool {
        match self {
            Layout::Leaf(id) if *id == target => {
                *self = Layout::Split {
                    dir,
                    children: vec![(1.0, Layout::Leaf(target)), (1.0, Layout::Leaf(new))],
                };
                true
            }
            Layout::Leaf(_) => false,
            Layout::Split {
                dir: own_dir,
                children,
            } => {
                let index = children
                    .iter()
                    .position(|(_, child)| matches!(child, Layout::Leaf(id) if *id == target));
                match index {
                    Some(index) if *own_dir == dir => {
                        let half = children[index].0 / 2.0;
                        children[index].0 = half;
                        children.insert(index + 1, (half, Layout::Leaf(new)));
                        true
                    }
                    _ => children
                        .iter_mut()
                        .any(|(_, child)| child.split(target, dir, new)),
                }
            }
        }
    }

    /// Removes `target`, giving its space to a neighbour. Returns false if it
    /// is not in the tree or is the only window.
    pub fn remove(&mut self, target: WindowId) -> bool {
        let Layout::Split { children, .. } = self else {
            return false;
        };
        let index = children
            .iter()
            .position(|(_, child)| matches!(child, Layout::Leaf(id) if *id == target));
        let Some(index) = index else {
            return children.iter_mut().any(|(_, child)| child.remove(target));
        };
        let (weight, _) = children.remove(index);
        children[index.saturating_sub(1)].0 += weight;
        if children.len() == 1 {
            let (_, only) = children.pop().expect("one child");
            *self = only;
        }
        true
    }

    /// Assigns each window its area within `rect`. With `border`, the last
    /// column of `rect` is a divider that belongs to the windows along its
    /// right edge, and is not part of their own areas.
    pub fn arrange(&self, rect: Rect, border: bool, out: &mut Vec<Placement>) {
        let (dir, children) = match self {
            Layout::Leaf(id) => {
                let rect = Rect {
                    w: rect.w.saturating_sub(u16::from(border)),
                    ..rect
                };
                out.push(Placement {
                    window: *id,
                    rect,
                    border,
                });
                return;
            }
            Layout::Split { dir, children } => (*dir, children),
        };
        let last_index = children.len() - 1;
        // Side by side, every child but the last has a divider of its own.
        let child_border = |index: usize| match dir {
            SplitDir::Right => index < last_index || border,
            SplitDir::Below => border,
        };
        let total = match dir {
            SplitDir::Right => {
                let borders = last_index as u16 + u16::from(border);
                rect.w.saturating_sub(borders)
            }
            SplitDir::Below => rect.h,
        };
        let weight_sum: f32 = children.iter().map(|(weight, _)| weight).sum();
        let (mut used, mut offset) = (0, 0);
        for (index, (weight, child)) in children.iter().enumerate() {
            let size = if index == last_index {
                total - used
            } else {
                ((f32::from(total) * weight / weight_sum).round() as u16).min(total - used)
            };
            used += size;
            let child_rect = match dir {
                SplitDir::Right => {
                    let w = (size + u16::from(child_border(index))).min(rect.w - offset);
                    let child_rect = Rect {
                        x: rect.x + offset,
                        w,
                        ..rect
                    };
                    offset += w;
                    child_rect
                }
                SplitDir::Below => {
                    let child_rect = Rect {
                        y: rect.y + offset,
                        h: size,
                        ..rect
                    };
                    offset += size;
                    child_rect
                }
            };
            child.arrange(child_rect, child_border(index), out);
        }
    }
}

/// Where layout put a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub window: WindowId,
    pub rect: Rect,
    /// Whether a divider column follows the window on its right.
    pub border: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use slotmap::SlotMap;

    fn ids(n: usize) -> Vec<WindowId> {
        let mut map: SlotMap<WindowId, ()> = SlotMap::with_key();
        (0..n).map(|_| map.insert(())).collect()
    }

    fn arrange(layout: &Layout, w: u16, h: u16) -> Vec<Placement> {
        let mut placed = Vec::new();
        layout.arrange(Rect { x: 0, y: 0, w, h }, false, &mut placed);
        placed
    }

    #[test]
    fn split_right_leaves_a_divider_column() {
        let w = ids(2);
        let mut layout = Layout::Leaf(w[0]);
        assert!(layout.split(w[0], SplitDir::Right, w[1]));
        let placed = arrange(&layout, 81, 24);
        assert_eq!(
            placed[0].rect,
            Rect {
                x: 0,
                y: 0,
                w: 40,
                h: 24
            }
        );
        assert!(placed[0].border);
        assert_eq!(
            placed[1].rect,
            Rect {
                x: 41,
                y: 0,
                w: 40,
                h: 24
            }
        );
        assert!(!placed[1].border);
    }

    #[test]
    fn splitting_again_halves_only_the_target() {
        let w = ids(3);
        let mut layout = Layout::Leaf(w[0]);
        layout.split(w[0], SplitDir::Below, w[1]);
        layout.split(w[0], SplitDir::Below, w[2]);
        assert_eq!(layout.leaves(), [w[0], w[2], w[1]]);
        let placed = arrange(&layout, 80, 40);
        let heights: Vec<u16> = placed.iter().map(|p| p.rect.h).collect();
        assert_eq!(heights, [10, 10, 20]);
    }

    #[test]
    fn windows_stacked_left_of_a_divider_all_border_it() {
        let w = ids(3);
        let mut layout = Layout::Leaf(w[0]);
        layout.split(w[0], SplitDir::Right, w[1]);
        layout.split(w[0], SplitDir::Below, w[2]);
        let placed = arrange(&layout, 21, 10);
        assert_eq!(
            placed[0],
            Placement {
                window: w[0],
                rect: Rect {
                    x: 0,
                    y: 0,
                    w: 10,
                    h: 5
                },
                border: true
            }
        );
        assert_eq!(
            placed[1],
            Placement {
                window: w[2],
                rect: Rect {
                    x: 0,
                    y: 5,
                    w: 10,
                    h: 5
                },
                border: true
            }
        );
        assert_eq!(
            placed[2],
            Placement {
                window: w[1],
                rect: Rect {
                    x: 11,
                    y: 0,
                    w: 10,
                    h: 10
                },
                border: false
            }
        );
    }

    #[test]
    fn tiny_frames_do_not_underflow() {
        let w = ids(3);
        let mut layout = Layout::Leaf(w[0]);
        layout.split(w[0], SplitDir::Right, w[1]);
        layout.split(w[1], SplitDir::Right, w[2]);
        for size in 0..4 {
            let placed = arrange(&layout, size, size);
            assert!(placed.iter().all(|p| p.rect.x + p.rect.w <= size));
        }
    }

    #[test]
    fn removing_collapses_single_child_splits() {
        let w = ids(3);
        let mut layout = Layout::Leaf(w[0]);
        layout.split(w[0], SplitDir::Below, w[1]);
        layout.split(w[1], SplitDir::Right, w[2]);
        assert!(layout.remove(w[2]));
        assert!(layout.remove(w[0]));
        assert!(matches!(layout, Layout::Leaf(id) if id == w[1]));
        assert!(!layout.remove(w[1]), "the last window cannot be removed");
    }
}
