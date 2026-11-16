use std::ops::Range;

use slotmap::{SlotMap, new_key_type};

new_key_type! {
    /// Handle to a marker in a [`MarkerSet`].
    pub struct MarkerId;
}

#[derive(Debug, Clone, Copy)]
struct Marker {
    pos: usize,
    /// Whether text inserted exactly at the marker pushes it forward.
    advances: bool,
}

/// Positions that follow the text they point at as it is edited.
///
/// Adjustment is linear in the number of markers, which stays small: syntax
/// highlighting does not use markers.
#[derive(Debug, Clone, Default)]
pub struct MarkerSet {
    markers: SlotMap<MarkerId, Marker>,
}

impl MarkerSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a marker at `pos`. If `advances`, text inserted at the marker
    /// ends up before it; otherwise after it.
    pub fn create(&mut self, pos: usize, advances: bool) -> MarkerId {
        self.markers.insert(Marker { pos, advances })
    }

    pub fn remove(&mut self, id: MarkerId) {
        self.markers.remove(id);
    }

    /// # Panics
    /// If the marker has been removed.
    pub fn get(&self, id: MarkerId) -> usize {
        self.markers[id].pos
    }

    /// # Panics
    /// If the marker has been removed.
    pub fn set(&mut self, id: MarkerId, pos: usize) {
        self.markers[id].pos = pos;
    }

    pub fn len(&self) -> usize {
        self.markers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.markers.is_empty()
    }

    /// Records that `len` chars were inserted at `pos`.
    pub fn on_insert(&mut self, pos: usize, len: usize) {
        for marker in self.markers.values_mut() {
            if marker.pos > pos || (marker.pos == pos && marker.advances) {
                marker.pos += len;
            }
        }
    }

    /// Records that `range` was deleted. Markers inside it collapse to its start.
    pub fn on_delete(&mut self, range: Range<usize>) {
        let len = range.end - range.start;
        for marker in self.markers.values_mut() {
            if marker.pos >= range.end {
                marker.pos -= len;
            } else if marker.pos > range.start {
                marker.pos = range.start;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insertion_at_marker_respects_type() {
        let mut set = MarkerSet::new();
        let stays = set.create(3, false);
        let advances = set.create(3, true);
        set.on_insert(3, 2);
        assert_eq!(set.get(stays), 3);
        assert_eq!(set.get(advances), 5);
    }

    #[test]
    fn deletion_collapses_markers_inside() {
        let mut set = MarkerSet::new();
        let before = set.create(1, false);
        let inside = set.create(4, false);
        let at_end = set.create(6, false);
        let after = set.create(9, false);
        set.on_delete(2..6);
        assert_eq!(set.get(before), 1);
        assert_eq!(set.get(inside), 2);
        assert_eq!(set.get(at_end), 2);
        assert_eq!(set.get(after), 5);
    }
}
