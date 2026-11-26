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

/// One change to an [`UndoTree`]. A tree that keeps a log writes down each
/// change that is made to it, and the records, applied in order, make the
/// same tree again: that is how the history of a file outlives the editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Record {
    /// The first record: a tree with only its first state, which is the
    /// state on disk. `hash` is of the text in that state.
    Base { hash: u64 },
    /// A new state under `parent`, which becomes the current one.
    Push { parent: usize, txn: Transaction },
    /// More edits for the transaction that led to the current state.
    Amend {
        edits: Vec<Edit>,
        point_after: usize,
    },
    /// Redo goes to `child` from its parent.
    Choose { child: usize },
    /// The text is in `state`.
    Goto { state: usize },
    /// The current state is on disk. `hash` is of the text in it.
    Saved { hash: u64 },
    /// No state is on disk.
    Unsaved,
}

#[derive(Debug, PartialEq, Eq)]
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
    /// The hash of the text in the first state and in the saved one, as
    /// they were given. They mean something only in a tree that keeps a log.
    base_hash: u64,
    saved_hash: u64,
    /// The changes made since the log was last taken, if one is kept.
    log: Option<Vec<Record>>,
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
            base_hash: 0,
            saved_hash: 0,
            log: None,
        }
    }

    /// A new tree that keeps a log, for a text with this hash that is the
    /// one on disk. The log starts with the record of that.
    pub fn logged(hash: u64) -> Self {
        Self {
            base_hash: hash,
            saved_hash: hash,
            log: Some(vec![Record::Base { hash }]),
            ..Self::new()
        }
    }

    fn note(&mut self, record: impl FnOnce() -> Record) {
        if let Some(log) = &mut self.log {
            log.push(record());
        }
    }

    /// Whether a log is kept.
    pub fn is_logged(&self) -> bool {
        self.log.is_some()
    }

    /// Starts to keep a log, of the changes from now on. What the tree is
    /// now is not in it: [`UndoTree::dump`] gives that.
    pub fn start_log(&mut self) {
        self.log.get_or_insert_default();
    }

    /// Whether changes were made since the log was last taken.
    pub fn has_log(&self) -> bool {
        self.log.as_ref().is_some_and(|log| !log.is_empty())
    }

    /// The changes made since this was last called, oldest first.
    pub fn take_log(&mut self) -> Vec<Record> {
        self.log.as_mut().map(std::mem::take).unwrap_or_default()
    }

    /// Records that make this tree from nothing.
    pub fn dump(&self) -> Vec<Record> {
        let mut records = vec![Record::Base {
            hash: self.base_hash,
        }];
        for node in &self.nodes[1..] {
            records.push(Record::Push {
                parent: node.parent.unwrap_or(Self::ROOT),
                txn: node.txn.clone(),
            });
        }
        for node in &self.nodes {
            if let Some(child) = node.redo_child {
                records.push(Record::Choose { child });
            }
        }
        match self.saved {
            Some(state) => {
                records.push(Record::Goto { state });
                records.push(Record::Saved {
                    hash: self.saved_hash,
                });
            }
            None => records.push(Record::Unsaved),
        }
        records.push(Record::Goto {
            state: self.current,
        });
        records
    }

    /// The tree that records make, keeping a log from then on, or `None`
    /// if they do not start with a base or do not fit each other.
    pub fn from_records(records: impl IntoIterator<Item = Record>) -> Option<Self> {
        let mut records = records.into_iter();
        let Some(Record::Base { hash }) = records.next() else {
            return None;
        };
        let mut tree = Self::logged(hash);
        for record in records {
            match record {
                Record::Base { .. } => return None,
                Record::Push { parent, txn } => {
                    tree.nodes.get(parent)?;
                    tree.current = parent;
                    tree.push(txn);
                }
                Record::Amend {
                    mut edits,
                    point_after,
                } => {
                    if !tree.amend(&mut edits, point_after) {
                        return None;
                    }
                }
                Record::Choose { child } => {
                    tree.nodes.get(child)?.parent?;
                    tree.choose_branch(child);
                }
                Record::Goto { state } => {
                    tree.nodes.get(state)?;
                    tree.current = state;
                }
                Record::Saved { hash } => tree.mark_saved(|| hash),
                Record::Unsaved => tree.clear_saved(),
            }
        }
        tree.log = Some(Vec::new());
        Some(tree)
    }

    /// The hash that was given for the text of the saved state.
    pub fn saved_hash(&self) -> Option<u64> {
        self.saved.map(|_| self.saved_hash)
    }

    /// Says that the text is in the saved state, for a tree that was made
    /// from records and a text that was read from disk. Returns the state
    /// that the records ended in if it is another: there the text was when
    /// the records stopped, with changes that were not saved.
    pub fn rewind_to_saved(&mut self) -> Option<usize> {
        let saved = self.saved?;
        let was = self.current;
        if was == saved {
            return None;
        }
        self.current = saved;
        self.note(|| Record::Goto { state: saved });
        Some(was)
    }

    /// Records a transaction that has just been applied to the text.
    pub fn push(&mut self, txn: Transaction) {
        let parent = self.current;
        self.note(|| Record::Push {
            parent,
            txn: txn.clone(),
        });
        let id = self.nodes.len();
        self.nodes.push(Node {
            parent: Some(parent),
            children: Vec::new(),
            redo_child: None,
            txn,
        });
        let parent = &mut self.nodes[parent];
        parent.children.push(id);
        parent.redo_child = Some(id);
        self.current = id;
    }

    /// Whether further edits may be merged into the current transaction.
    /// That is the case when it is a leaf and is not the saved state.
    pub fn is_amendable(&self) -> bool {
        let node = &self.nodes[self.current];
        node.parent.is_some() && node.children.is_empty() && self.saved != Some(self.current)
    }

    /// Merges edits that have just been applied to the text into the
    /// current transaction, if that may be done: see
    /// [`UndoTree::is_amendable`]. Returns whether it was, and takes the
    /// edits only then.
    pub fn amend(&mut self, edits: &mut Vec<Edit>, point_after: usize) -> bool {
        if !self.is_amendable() {
            return false;
        }
        self.note(|| Record::Amend {
            edits: edits.clone(),
            point_after,
        });
        let txn = &mut self.nodes[self.current].txn;
        txn.edits.append(edits);
        txn.point_after = point_after;
        true
    }

    /// Steps to the parent state. The caller must revert the returned
    /// transaction: undo its edits last to first.
    pub fn undo(&mut self) -> Option<&Transaction> {
        let node = self.current;
        let parent = self.nodes[node].parent?;
        self.note(|| Record::Choose { child: node });
        self.note(|| Record::Goto { state: parent });
        self.nodes[parent].redo_child = Some(node);
        self.current = parent;
        Some(&self.nodes[node].txn)
    }

    /// Steps to the most recently visited child state. The caller must apply
    /// the returned transaction's edits first to last.
    pub fn redo(&mut self) -> Option<&Transaction> {
        let child = self.nodes[self.current].redo_child?;
        self.note(|| Record::Goto { state: child });
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
            self.note(|| Record::Choose { child });
            self.nodes[parent].redo_child = Some(child);
        }
    }

    /// Marks the current state as the one on disk. `hash` gives the hash
    /// of the text, and is asked only if a log is kept.
    pub fn mark_saved(&mut self, hash: impl FnOnce() -> u64) {
        self.saved = Some(self.current);
        if self.log.is_some() {
            let hash = hash();
            self.saved_hash = hash;
            self.note(|| Record::Saved { hash });
        }
    }

    /// Forgets the saved state, so every state counts as modified.
    pub fn clear_saved(&mut self) {
        self.saved = None;
        self.note(|| Record::Unsaved);
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
        tree.mark_saved(|| 0);
        assert!(tree.is_at_saved());
        assert!(!tree.is_amendable(), "saved state must not be amended");
        tree.push(txn(1, "b"));
        assert!(tree.is_amendable());
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

    /// A tree with branches, an amended transaction and a saved state that
    /// is not the current one, and the records of how it came to be.
    fn logged_tree() -> (UndoTree, Vec<Record>) {
        let mut tree = UndoTree::logged(7);
        tree.push(txn(0, "a"));
        let mut more = txn(1, "b").edits;
        assert!(tree.amend(&mut more, 2));
        assert!(more.is_empty());
        tree.mark_saved(|| 8);
        tree.push(txn(2, "c"));
        tree.undo();
        tree.push(txn(2, "d"));
        tree.undo();
        tree.choose_branch(2);
        tree.redo();
        let records = tree.take_log();
        (tree, records)
    }

    #[test]
    fn the_log_makes_the_same_tree_again() {
        let (tree, records) = logged_tree();
        assert!(matches!(records[0], Record::Base { hash: 7 }));
        let again = UndoTree::from_records(records).unwrap();
        assert_eq!(again.nodes, tree.nodes);
        assert_eq!(again.current(), 2);
        assert_eq!(again.saved(), Some(1));
        assert_eq!(again.saved_hash(), Some(8));
        assert!(!again.has_log(), "what was read is not written again");

        // So do the records that say what the tree is.
        let dumped = UndoTree::from_records(tree.dump()).unwrap();
        assert_eq!(dumped.nodes, tree.nodes);
        assert_eq!(dumped.current(), tree.current());
        assert_eq!(dumped.saved(), tree.saved());
        assert_eq!(dumped.dump(), tree.dump());
    }

    #[test]
    fn a_tree_without_a_log_writes_nothing_down() {
        let mut tree = UndoTree::new();
        tree.push(txn(0, "a"));
        tree.mark_saved(|| unreachable!("no hash is asked for"));
        assert!(!tree.has_log());
        assert!(tree.take_log().is_empty());
        tree.start_log();
        tree.undo();
        assert_eq!(
            tree.take_log(),
            [Record::Choose { child: 1 }, Record::Goto { state: 0 }]
        );
    }

    #[test]
    fn records_that_do_not_fit_make_no_tree() {
        assert!(UndoTree::from_records([]).is_none());
        assert!(UndoTree::from_records([Record::Unsaved]).is_none());
        let base = Record::Base { hash: 0 };
        for bad in [
            Record::Goto { state: 1 },
            Record::Choose { child: 0 },
            Record::Choose { child: 4 },
            Record::Push {
                parent: 3,
                txn: txn(0, "a"),
            },
            Record::Amend {
                edits: Vec::new(),
                point_after: 0,
            },
            Record::Base { hash: 0 },
        ] {
            assert!(
                UndoTree::from_records([base.clone(), bad.clone()]).is_none(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_tree_read_back_starts_at_the_saved_state() {
        let (_, records) = logged_tree();
        let mut tree = UndoTree::from_records(records).unwrap();
        assert_eq!(tree.rewind_to_saved(), Some(2), "where the text was");
        assert_eq!(tree.current(), 1);
        assert!(tree.is_at_saved());
        assert_eq!(tree.take_log(), [Record::Goto { state: 1 }]);
        assert_eq!(tree.rewind_to_saved(), None);
        // The changes that were not saved are a redo away.
        assert_eq!(tree.redo().unwrap().edits[0].inserted, "c");
    }

    #[test]
    fn root_is_not_amendable_or_undoable() {
        let mut tree = UndoTree::new();
        tree.clear_saved();
        assert!(!tree.is_amendable());
        assert!(tree.undo().is_none());
        assert!(tree.redo().is_none());
    }
}
