/// One replacement: at `pos`, `deleted` was removed and `inserted` put in its place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub pos: usize,
    pub deleted: String,
    pub inserted: String,
}

/// The edits made by one command, undone and redone as a unit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Transaction {
    /// In the order they were applied.
    pub edits: Vec<Edit>,
    pub point_before: usize,
    pub point_after: usize,
}

#[derive(Debug)]
struct Node {
    parent: Option<usize>,
    children: Vec<usize>,
    /// The child that redo follows: the one most recently created or visited.
    redo_child: Option<usize>,
    /// Takes the text from the parent's state to this node's state.
    txn: Transaction,
}

/// Edit history as a tree: undoing and then editing starts a new branch rather
/// than discarding the old one.
///
/// The tree only records transactions. The caller applies them to the text.
#[derive(Debug)]
pub struct UndoTree {
    nodes: Vec<Node>,
    current: usize,
    /// The node whose state matches the file on disk, if any.
    saved: Option<usize>,
}

impl Default for UndoTree {
    fn default() -> Self {
        Self::new()
    }
}

impl UndoTree {
    const ROOT: usize = 0;

    pub fn new() -> Self {
        Self {
            nodes: vec![Node {
                parent: None,
                children: Vec::new(),
                redo_child: None,
                txn: Transaction::default(),
            }],
            current: Self::ROOT,
            saved: Some(Self::ROOT),
        }
    }

    /// Records a transaction that has just been applied to the text.
    pub fn push(&mut self, txn: Transaction) {
        let id = self.nodes.len();
        self.nodes.push(Node {
            parent: Some(self.current),
            children: Vec::new(),
            redo_child: None,
            txn,
        });
        let parent = &mut self.nodes[self.current];
        parent.children.push(id);
        parent.redo_child = Some(id);
        self.current = id;
    }

    /// The current transaction, if further edits may be merged into it. That
    /// is the case when it is a leaf and is not the saved state.
    pub fn amendable(&mut self) -> Option<&mut Transaction> {
        let node = &mut self.nodes[self.current];
        let amendable =
            node.parent.is_some() && node.children.is_empty() && self.saved != Some(self.current);
        amendable.then_some(&mut node.txn)
    }

    /// Steps to the parent state. The caller must revert the returned
    /// transaction: undo its edits last to first.
    pub fn undo(&mut self) -> Option<&Transaction> {
        let node = self.current;
        let parent = self.nodes[node].parent?;
        self.nodes[parent].redo_child = Some(node);
        self.current = parent;
        Some(&self.nodes[node].txn)
    }

    /// Steps to the most recently visited child state. The caller must apply
    /// the returned transaction's edits first to last.
    pub fn redo(&mut self) -> Option<&Transaction> {
        let child = self.nodes[self.current].redo_child?;
        self.current = child;
        Some(&self.nodes[child].txn)
    }

    // The shape of the tree, for showing it and moving about in it. States
    // are numbered; the first, with nothing done yet, is state 0.

    /// The state the text is in.
    pub fn current(&self) -> usize {
        self.current
    }

    /// The state that matches the file on disk, if any does.
    pub fn saved(&self) -> Option<usize> {
        self.saved
    }

    pub fn parent(&self, state: usize) -> Option<usize> {
        self.nodes.get(state)?.parent
    }

    /// The states reached from `state` by one transaction, oldest first.
    pub fn children(&self, state: usize) -> &[usize] {
        self.nodes
            .get(state)
            .map_or(&[], |node| node.children.as_slice())
    }

    /// The child that redo goes to from `state`.
    pub fn redo_child(&self, state: usize) -> Option<usize> {
        self.nodes.get(state)?.redo_child
    }

    /// The states from the first to `state`, inclusive.
    pub fn path_to(&self, state: usize) -> Vec<usize> {
        let mut path = vec![state];
        while let Some(parent) = self.parent(path[path.len() - 1]) {
            path.push(parent);
        }
        path.reverse();
        path
    }

    /// Makes `child` the state that redo goes to from its parent.
    pub fn choose_branch(&mut self, child: usize) {
        if let Some(parent) = self.parent(child) {
            self.nodes[parent].redo_child = Some(child);
        }
    }

    /// Marks the current state as the one on disk.
    pub fn mark_saved(&mut self) {
        self.saved = Some(self.current);
    }

    /// Forgets the saved state, so every state counts as modified.
    pub fn clear_saved(&mut self) {
        self.saved = None;
    }

    pub fn is_at_saved(&self) -> bool {
        self.saved == Some(self.current)
    }

    /// Number of recorded transactions, across all branches.
    pub fn len(&self) -> usize {
        self.nodes.len() - 1
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn txn(pos: usize, inserted: &str) -> Transaction {
        Transaction {
            edits: vec![Edit {
                pos,
                deleted: String::new(),
                inserted: inserted.to_owned(),
            }],
            point_before: pos,
            point_after: pos + inserted.len(),
        }
    }

    #[test]
    fn undo_then_edit_keeps_the_old_branch() {
        let mut tree = UndoTree::new();
        tree.push(txn(0, "a"));
        tree.push(txn(1, "b"));
        assert_eq!(tree.undo().unwrap().edits[0].inserted, "b");
        tree.push(txn(1, "c"));
        assert_eq!(tree.len(), 3);

        // Redo follows the newest branch.
        assert!(tree.redo().is_none());
        assert_eq!(tree.undo().unwrap().edits[0].inserted, "c");
        assert_eq!(tree.redo().unwrap().edits[0].inserted, "c");
    }

    #[test]
    fn redo_follows_the_branch_last_undone() {
        let mut tree = UndoTree::new();
        tree.push(txn(0, "a"));
        tree.undo();
        tree.push(txn(0, "b"));
        tree.undo();
        assert_eq!(tree.redo().unwrap().edits[0].inserted, "b");
    }

    #[test]
    fn saved_state_tracks_position_in_tree() {
        let mut tree = UndoTree::new();
        assert!(tree.is_at_saved());
        tree.push(txn(0, "a"));
        assert!(!tree.is_at_saved());
        tree.undo();
        assert!(tree.is_at_saved());
        tree.redo();
        tree.mark_saved();
        assert!(tree.is_at_saved());
        assert!(
            tree.amendable().is_none(),
            "saved state must not be amended"
        );
        tree.push(txn(1, "b"));
        assert!(tree.amendable().is_some());
    }

    #[test]
    fn the_shape_of_the_tree_can_be_read() {
        let mut tree = UndoTree::new();
        tree.push(txn(0, "a"));
        tree.push(txn(1, "b"));
        tree.undo();
        tree.push(txn(1, "c"));
        assert_eq!(tree.current(), 3);
        assert_eq!(tree.children(1), [2, 3]);
        assert_eq!(tree.parent(3), Some(1));
        assert_eq!(tree.path_to(2), [0, 1, 2]);
        assert_eq!(tree.redo_child(1), Some(3));
        tree.undo();
        tree.choose_branch(2);
        assert_eq!(tree.redo().unwrap().edits[0].inserted, "b");
        assert_eq!(tree.current(), 2);
    }

    #[test]
    fn root_is_not_amendable_or_undoable() {
        let mut tree = UndoTree::new();
        tree.clear_saved();
        assert!(tree.amendable().is_none());
        assert!(tree.undo().is_none());
        assert!(tree.redo().is_none());
    }
}
