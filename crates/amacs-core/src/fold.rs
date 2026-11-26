//! Lines of a buffer that are not shown: what is folded away under a
//! heading, say. Code says which lines to hide; here they are kept right
//! as the text changes, and display and motion ask here what is hidden.
//!
//! A fold is a range of chars, `start..end`. `start` is the newline that
//! ends the line before the hidden lines, which stays in view and shows a
//! mark that something is hidden. `end` is the end of the last hidden line:
//! its newline, or the end of the text. A position `p` is hidden when
//! `start < p <= end`. The first line of a text can therefore not be
//! hidden.
//!
//! Folds can lie within one another, and never cross: opening an outer
//! fold leaves the ones within it folded.

use std::ops::Range;

use amacs_text::Text;

/// The hidden lines of a buffer.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Folds {
    /// Every fold, sorted by start. No two start at the same place, and
    /// any two are apart or one is within the other.
    all: Vec<Range<usize>>,
    /// The folds that are within no other: apart and in order, so that the
    /// one around a position is found by bisection.
    outer: Vec<Range<usize>>,
}

/// Whether one fold lies within another, or is the same.
fn within(inner: &Range<usize>, outer: &Range<usize>) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

/// Whether two folds have nothing to do with one another. A fold that
/// starts at the end of another starts on the other's last hidden line, so
/// those two are not apart.
fn apart(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start > b.end || b.start > a.end
}

impl Folds {
    pub const fn new() -> Self {
        Self {
            all: Vec::new(),
            outer: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.all.is_empty()
    }

    /// Every fold, sorted by start: a fold within another comes after it.
    pub fn all(&self) -> &[Range<usize>] {
        &self.all
    }

    fn rebuild(&mut self) {
        self.outer.clear();
        for fold in &self.all {
            if self.outer.last().is_none_or(|last| fold.start > last.end) {
                self.outer.push(fold.clone());
            }
        }
    }

    /// Forgets every fold.
    pub fn clear(&mut self) {
        self.all.clear();
        self.outer.clear();
    }

    /// Hides the lines that the text between two positions touches; an end
    /// at the start of a line leaves that line shown. Returns the fold, or
    /// `None` if there is nothing to hide: the first line is never hidden.
    /// A fold that starts where this one does is replaced, and folds that
    /// would cross it give way.
    pub fn hide(&mut self, text: &Text, a: usize, b: usize) -> Option<Range<usize>> {
        let (a, b) = (a.min(b).min(text.len()), a.max(b).min(text.len()));
        let first = text.line_of(a).max(1);
        let mut last = text.line_of(b);
        if b > a && text.line_start(last) == b {
            last -= 1;
        }
        if last < first {
            return None;
        }
        let fold = text.line_end(first - 1)..text.line_end(last);
        self.all.retain(|other| {
            other.start != fold.start
                && (apart(other, &fold) || within(other, &fold) || within(&fold, other))
        });
        let index = self.all.partition_point(|other| other.start < fold.start);
        self.all.insert(index, fold.clone());
        self.rebuild();
        Some(fold)
    }

    /// Shows the lines of the folds whose first hidden line starts in a
    /// range of positions, or at its end if the range is empty.
    pub fn show(&mut self, range: Range<usize>) {
        self.all.retain(|fold| {
            let first = fold.start + 1;
            !(range.start <= first && (first < range.end || first == range.start))
        });
        self.rebuild();
    }

    /// Shows a position: opens every fold around it and none beside it.
    /// Returns whether there was one.
    pub fn reveal(&mut self, pos: usize) -> bool {
        let before = self.all.len();
        self.all
            .retain(|fold| !(fold.start < pos && pos <= fold.end));
        self.rebuild();
        self.all.len() != before
    }

    /// Where a motion that came to a position ends: the position, or if it
    /// is hidden, the start of the first line that is shown after it when
    /// going forward, and the end of the line that is shown before it when
    /// going back. Forward, `None` if all lines to the end of the text are
    /// hidden.
    pub fn step(&self, pos: usize, forward: bool, len: usize) -> Option<usize> {
        match self.around(pos) {
            None => Some(pos),
            Some(fold) if !forward => Some(fold.start),
            Some(fold) => (fold.end < len).then_some(fold.end + 1),
        }
    }

    /// The outermost fold that hides a position.
    fn around(&self, pos: usize) -> Option<&Range<usize>> {
        let index = self.outer.partition_point(|fold| fold.end < pos);
        self.outer.get(index).filter(|fold| fold.start < pos)
    }

    /// Whether a position is on a hidden line.
    pub fn hidden(&self, pos: usize) -> bool {
        self.around(pos).is_some()
    }

    /// A position, or if it is hidden the end of the line that is shown
    /// before it.
    pub fn visible(&self, pos: usize) -> usize {
        self.around(pos).map_or(pos, |fold| fold.start)
    }

    /// The end of the hidden lines that follow the line that ends at a
    /// position, if lines are hidden there.
    pub fn after(&self, line_end: usize) -> Option<usize> {
        let index = self.outer.partition_point(|fold| fold.start < line_end);
        self.outer
            .get(index)
            .filter(|fold| fold.start == line_end)
            .map(|fold| fold.end)
    }

    /// The line that is shown after a line that is shown, if there is one.
    pub fn next_line(&self, text: &Text, line: usize) -> Option<usize> {
        let end = self
            .after(text.line_end(line))
            .unwrap_or(text.line_end(line));
        (end < text.len()).then(|| text.line_of(end + 1))
    }

    /// The line that is shown before a line that is shown, if there is one.
    pub fn previous_line(&self, text: &Text, line: usize) -> Option<usize> {
        let before = line.checked_sub(1)?;
        Some(text.line_of(self.visible(text.line_start(before))))
    }

    /// How many of the lines between two lines are hidden.
    pub fn hidden_lines_between(&self, text: &Text, a: usize, b: usize) -> usize {
        let (from, to) = (text.line_start(a.min(b)), text.line_start(a.max(b)));
        let first = self.outer.partition_point(|fold| fold.end < from);
        self.outer[first..]
            .iter()
            .take_while(|fold| fold.start < to)
            .map(|fold| text.line_of(fold.end) - text.line_of(fold.start))
            .sum()
    }

    /// The parts of a range of positions that are shown, in order.
    pub fn shown_in(&self, range: Range<usize>) -> Vec<Range<usize>> {
        let mut parts = Vec::new();
        let mut from = range.start;
        let first = self.outer.partition_point(|fold| fold.end < range.start);
        for fold in &self.outer[first..] {
            if fold.start >= range.end {
                break;
            }
            // The newline at the start of a fold is shown.
            if fold.start + 1 > from {
                parts.push(from..(fold.start + 1).min(range.end));
            }
            from = from.max(fold.end + 1);
        }
        if from < range.end {
            parts.push(from..range.end);
        }
        parts
    }

    /// Follows the insertion of `len` chars at a position. Text put at the
    /// end of the line before a fold stays in view; text put among hidden
    /// lines is hidden.
    pub fn on_insert(&mut self, pos: usize, len: usize) {
        if self.all.is_empty() {
            return;
        }
        for fold in &mut self.all {
            if pos <= fold.start {
                fold.start += len;
                fold.end += len;
            } else if pos <= fold.end {
                fold.end += len;
            }
        }
        self.rebuild();
    }

    /// Follows the deletion of a range of chars. A fold that loses the
    /// newline at either of its ends is opened: what it hid would else be
    /// part of a line that is shown.
    pub fn on_delete(&mut self, range: Range<usize>) {
        if self.all.is_empty() {
            return;
        }
        let moved = |pos: usize| {
            if pos >= range.end {
                pos - range.len()
            } else {
                pos.min(range.start)
            }
        };
        self.all
            .retain(|fold| !range.contains(&fold.start) && !range.contains(&fold.end));
        for fold in &mut self.all {
            *fold = moved(fold.start)..moved(fold.end);
        }
        self.rebuild();
    }
}

#[cfg(test)]
// A list of folds can hold one fold, which is a range.
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    /// Six lines of two chars and a newline: line `n` starts at `3 * n`.
    fn text() -> Text {
        Text::from("l0\nl1\nl2\nl3\nl4\nl5\n")
    }

    /// The text as it is shown, with `…` where lines are hidden.
    fn shown(text: &Text, folds: &Folds) -> String {
        let mut out = String::new();
        let mut line = Some(0);
        while let Some(at) = line {
            out.push_str(&text.slice(text.line_start(at)..text.line_end(at)));
            if folds.after(text.line_end(at)).is_some() {
                out.push('…');
            }
            out.push('|');
            line = folds.next_line(text, at);
        }
        out
    }

    #[test]
    fn a_fold_runs_from_the_end_of_the_line_before_to_the_end_of_the_last_hidden_line() {
        let text = text();
        let mut folds = Folds::new();
        assert_eq!(folds.hide(&text, 3, 7), Some(2..8));
        assert_eq!(shown(&text, &folds), "l0…|l3|l4|l5||");
        assert!(!folds.hidden(2) && folds.hidden(3) && folds.hidden(8) && !folds.hidden(9));
        assert_eq!(folds.visible(5), 2);
        assert_eq!(folds.visible(9), 9);
        assert_eq!(folds.previous_line(&text, 3), Some(0));
        assert_eq!(folds.hidden_lines_between(&text, 0, 4), 2);
        assert_eq!(folds.hidden_lines_between(&text, 3, 5), 0);
    }

    #[test]
    fn the_first_line_is_never_hidden() {
        let text = text();
        let mut folds = Folds::new();
        assert_eq!(folds.hide(&text, 0, 1), None);
        assert_eq!(folds.hide(&text, 0, 4), Some(2..5));
        assert_eq!(shown(&text, &folds), "l0…|l2|l3|l4|l5||");
    }

    #[test]
    fn an_end_at_a_line_start_leaves_that_line_shown() {
        let text = text();
        let mut folds = Folds::new();
        assert_eq!(folds.hide(&text, 3, 9), Some(2..8));
        assert_eq!(folds.hide(&text, 12, 12), Some(11..14));
        // To the end of a text that ends without a newline.
        let text = Text::from("a\nb\nc");
        let mut folds = Folds::new();
        assert_eq!(folds.hide(&text, 2, 5), Some(1..5));
        assert_eq!(shown(&text, &folds), "a…|");
        assert_eq!(folds.next_line(&text, 0), None);
    }

    #[test]
    fn text_typed_at_the_end_of_a_folded_heading_stays_in_view() {
        let mut text = text();
        let mut folds = Folds::new();
        folds.hide(&text, 3, 7);
        text.insert(2, "!!");
        folds.on_insert(2, 2);
        assert_eq!(shown(&text, &folds), "l0!!…|l3|l4|l5||");
        // Text among hidden lines is hidden, and so is text before the
        // newline that ends them.
        text.insert(6, "x\ny");
        folds.on_insert(6, 3);
        text.insert(13, "z");
        folds.on_insert(13, 1);
        assert_eq!(shown(&text, &folds), "l0!!…|l3|l4|l5||");
        // Text after them is not.
        text.insert(15, "a");
        folds.on_insert(15, 1);
        assert_eq!(shown(&text, &folds), "l0!!…|al3|l4|l5||");
    }

    #[test]
    fn deleting_the_newline_at_either_end_of_hidden_lines_shows_them() {
        let (text, mut folds) = (text(), Folds::new());
        folds.hide(&text, 3, 7);
        let mut first = folds.clone();
        first.on_delete(2..3);
        assert!(first.is_empty());
        let mut last = folds.clone();
        last.on_delete(8..9);
        assert!(last.is_empty());
        // Other deletions move the fold or make it smaller.
        folds.on_delete(0..1);
        assert_eq!(folds.all(), [1..7]);
        folds.on_delete(3..5);
        assert_eq!(folds.all(), [1..5]);
        folds.on_delete(6..9);
        assert_eq!(folds.all(), [1..5]);
        folds.on_delete(0..9);
        assert!(folds.is_empty());
    }

    #[test]
    fn a_fold_within_another_survives_when_the_outer_is_opened() {
        let text = text();
        let mut folds = Folds::new();
        folds.hide(&text, 9, 13);
        folds.hide(&text, 3, 16);
        assert_eq!(folds.all(), [2..17, 8..14]);
        assert_eq!(shown(&text, &folds), "l0…||");
        folds.show(3..4);
        assert_eq!(shown(&text, &folds), "l0|l1|l2…|l5||");
        // Showing a range opens the folds that start in it.
        folds.hide(&text, 3, 16);
        folds.show(3..18);
        assert!(folds.is_empty());
    }

    #[test]
    fn revealing_a_position_opens_every_fold_around_it_and_none_beside_it() {
        let text = text();
        let mut folds = Folds::new();
        folds.hide(&text, 3, 16);
        folds.hide(&text, 6, 7);
        folds.hide(&text, 12, 13);
        assert_eq!(folds.all(), [2..17, 5..8, 11..14]);
        assert!(folds.reveal(13));
        assert_eq!(folds.all(), [5..8]);
        assert_eq!(shown(&text, &folds), "l0|l1…|l3|l4|l5||");
        assert!(!folds.reveal(13));
        // The end of the line before a fold is not hidden.
        assert!(!folds.reveal(5));
    }

    #[test]
    fn a_fold_that_crosses_a_new_one_gives_way() {
        let text = text();
        let mut folds = Folds::new();
        folds.hide(&text, 3, 7);
        folds.hide(&text, 6, 13);
        assert_eq!(folds.all(), [5..14]);
        // One that starts where another does takes its place.
        folds.hide(&text, 6, 7);
        assert_eq!(folds.all(), [5..8]);
        // One that starts on the last hidden line of another crosses it.
        folds.hide(&text, 9, 10);
        assert_eq!(folds.all(), [8..11]);
    }

    #[test]
    fn the_shown_parts_of_a_range_leave_out_what_is_hidden() {
        let text = text();
        let mut folds = Folds::new();
        folds.hide(&text, 3, 7);
        folds.hide(&text, 12, 13);
        assert_eq!(folds.shown_in(0..18), [0..3, 9..12, 15..18]);
        assert_eq!(folds.shown_in(4..10), [9..10]);
        assert_eq!(folds.shown_in(9..11), [9..11]);
        assert_eq!(Folds::new().shown_in(2..5), [2..5]);
    }
}
