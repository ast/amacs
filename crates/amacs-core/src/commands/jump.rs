//! Jumping to a place on screen by naming it: every candidate place gets a
//! label of a key or two, and pressing the label goes there. This is how
//! avy and ace-window work in Emacs.

use amacs_proto::Key;

use crate::commands::edit::kill;
use crate::decorate::{Label, Labels};
use crate::display::walk_row;
use crate::editor::Editor;
use crate::error::{CmdError, CmdResult};
use crate::kill_ring::KillDirection;
use crate::window::WindowId;

/// The keys that labels are made of: the home row.
const LABEL_KEYS: &str = "asdfghjkl";

/// What happens at the place that is picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Goto,
    /// Kill from point through the place.
    ZapTo,
    /// Kill from point up to the place.
    ZapUpTo,
    SelectWindow,
}

/// Labels for `count` places, the shortest first. With more places than
/// keys, some keys lead on to a second key, and so on.
fn make_labels(count: usize) -> Vec<String> {
    let keys: Vec<char> = LABEL_KEYS.chars().collect();
    if count <= keys.len() {
        return keys.iter().take(count).map(|key| key.to_string()).collect();
    }
    // Each key that leads on stands for up to as many places as it takes
    // to cover the rest, spread evenly.
    let leading = (count - keys.len())
        .div_ceil(keys.len() - 1)
        .min(keys.len());
    let single = keys.len() - leading;
    let mut labels: Vec<String> = keys[..single].iter().map(|key| key.to_string()).collect();
    let rest = count - single;
    for (index, key) in keys[single..].iter().enumerate() {
        let share = rest / leading + usize::from(index < rest % leading);
        labels.extend(
            make_labels(share)
                .into_iter()
                .map(|tail| format!("{key}{tail}")),
        );
    }
    labels
}

impl Editor {
    /// The chars a window shows: from its start to the end of its last row.
    pub(crate) fn visible_range(&self, id: WindowId) -> std::ops::Range<usize> {
        let window = &self.windows[id];
        let buffer = &self.buffers[window.buffer];
        let width = self.text_width(id);
        let start = buffer.marker(window.start);
        let mut pos = start;
        for _ in 0..window.text_rows() {
            let row = walk_row(buffer.text(), pos, width, buffer.tab_width, |_, _, _, _| {});
            match row.next {
                Some(next) => pos = next,
                None => return start..buffer.len(),
            }
        }
        start..pos
    }

    /// The places on screen where `matches` holds, nearest to point first.
    /// `matches` is given a window's text and a position in it.
    fn jump_candidates(
        &self,
        matches: impl Fn(&amacs_text::Text, usize) -> bool,
    ) -> Vec<(WindowId, usize)> {
        let selected = self.selected();
        let mut windows = self.frame().layout.leaves();
        windows.sort_by_key(|id| *id != selected);
        let mut found = Vec::new();
        for id in windows {
            let window = &self.windows[id];
            let buffer = &self.buffers[window.buffer];
            let point = buffer.marker(window.point);
            let range = self.visible_range(id);
            let mut here: Vec<usize> = (range.start..=range.end.min(buffer.len()))
                .filter(|pos| matches(buffer.text(), *pos))
                // Where point already is, is nowhere to go.
                .filter(|pos| id != selected || *pos != point)
                .collect();
            here.sort_by_key(|pos| pos.abs_diff(point));
            found.extend(here.into_iter().map(|pos| (id, pos)));
        }
        found
    }

    /// Labels the candidates and waits for one to be picked; goes straight
    /// to a single candidate.
    fn start_jump(
        &mut self,
        candidates: Vec<(WindowId, Option<usize>)>,
        action: Action,
    ) -> CmdResult {
        match candidates.as_slice() {
            [] => Err(CmdError::msg("Nothing on screen matches")),
            [(window, pos)] => self.finish_jump(*window, *pos, action),
            _ => {
                let labels = make_labels(candidates.len());
                let entries = candidates
                    .into_iter()
                    .zip(labels)
                    .map(|((window, pos), keys)| Label { window, pos, keys })
                    .collect();
                self.labels = Some(Labels {
                    entries,
                    dim: action != Action::SelectWindow,
                });
                self.read_label(action);
                Ok(())
            }
        }
    }

    fn finish_jump(&mut self, window: WindowId, pos: Option<usize>, action: Action) -> CmdResult {
        self.labels = None;
        if !self.windows.contains_key(window) {
            return Ok(());
        }
        let point = self.point();
        match (action, pos) {
            (Action::SelectWindow, _) | (_, None) => {
                self.frame_mut().selected = window;
                Ok(())
            }
            (Action::Goto, Some(pos)) => {
                if window == self.selected() {
                    let buffer = self.buffer_mut();
                    if !buffer.mark_active {
                        buffer.set_mark(point);
                    }
                }
                self.frame_mut().selected = window;
                self.set_point(pos);
                Ok(())
            }
            (Action::ZapTo | Action::ZapUpTo, Some(pos)) => {
                if window != self.selected() {
                    return Err(CmdError::msg("That is in another window"));
                }
                let through = usize::from(action == Action::ZapTo);
                if pos >= point {
                    kill(self, point, pos + through, KillDirection::Forward)
                } else {
                    kill(self, pos + 1 - through, point, KillDirection::Backward)
                }
            }
        }
    }

    /// Waits for the next key of a label.
    fn read_label(&mut self, action: Action) {
        self.ask_key("Jump to: ", move |ed, key| {
            let Some(labels) = ed.labels.take() else {
                return Ok(());
            };
            if key == Key::ctrl('g') {
                return Err(CmdError::Quit);
            }
            let typed = key.self_insert_char();
            let entries: Vec<Label> = labels
                .entries
                .into_iter()
                .filter_map(|label| {
                    let mut keys = label.keys.chars();
                    (keys.next() == typed && typed.is_some()).then(|| Label {
                        keys: keys.collect(),
                        ..label
                    })
                })
                .collect();
            match entries.as_slice() {
                [] => Err(CmdError::msg("No such label")),
                [only] if only.keys.is_empty() => ed.finish_jump(only.window, only.pos, action),
                _ => {
                    ed.labels = Some(Labels {
                        entries,
                        dim: labels.dim,
                    });
                    ed.read_label(action);
                    Ok(())
                }
            }
        });
    }
}

/// Reads a char and passes it on.
fn read_char(
    ed: &mut Editor,
    prompt: &str,
    then: impl FnOnce(&mut Editor, char) -> CmdResult + 'static,
) {
    ed.ask_key(prompt, move |ed, key| match key.self_insert_char() {
        Some(c) => then(ed, c),
        None if key == Key::ctrl('g') => Err(CmdError::Quit),
        None => Err(CmdError::msg("That is not a character")),
    });
}

/// Whether the text has `wanted` at `pos`. A lowercase letter also matches
/// its capital.
fn char_matches(text: &amacs_text::Text, pos: usize, wanted: char) -> bool {
    text.char_at(pos).is_some_and(|c| {
        c == wanted || (wanted.is_lowercase() && c.to_lowercase().eq(std::iter::once(wanted)))
    })
}

fn jump_to_matches(
    ed: &mut Editor,
    action: Action,
    matches: impl Fn(&amacs_text::Text, usize) -> bool,
) -> CmdResult {
    let candidates = ed
        .jump_candidates(matches)
        .into_iter()
        .map(|(window, pos)| (window, Some(pos)))
        .collect();
    ed.start_jump(candidates, action)
}

pub fn goto_char(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    read_char(ed, "Jump to char: ", |ed, c| {
        jump_to_matches(ed, Action::Goto, move |text, pos| {
            char_matches(text, pos, c)
        })
    });
    Ok(())
}

pub fn goto_char_2(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    read_char(ed, "Jump to chars: ", |ed, first| {
        read_char(ed, &format!("Jump to chars: {first}"), move |ed, second| {
            jump_to_matches(ed, Action::Goto, move |text, pos| {
                char_matches(text, pos, first) && char_matches(text, pos + 1, second)
            })
        });
        Ok(())
    });
    Ok(())
}

pub fn goto_word(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    read_char(ed, "Jump to word starting with: ", |ed, c| {
        jump_to_matches(ed, Action::Goto, move |text, pos| {
            let starts_word = pos
                .checked_sub(1)
                .and_then(|before| text.char_at(before))
                .is_none_or(|before| !before.is_alphanumeric() && before != '_');
            starts_word && char_matches(text, pos, c)
        })
    });
    Ok(())
}

pub fn goto_line(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    jump_to_matches(ed, Action::Goto, |text, pos| {
        text.line_start(text.line_of(pos)) == pos
    })
}

fn zap(ed: &mut Editor, action: Action) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    read_char(ed, "Zap to char: ", move |ed, c| {
        let selected = ed.selected();
        let candidates = ed
            .jump_candidates(move |text, pos| char_matches(text, pos, c))
            .into_iter()
            .filter(|(window, _)| *window == selected)
            .map(|(window, pos)| (window, Some(pos)))
            .collect();
        ed.start_jump(candidates, action)
    });
    Ok(())
}

pub fn zap_to_char(ed: &mut Editor) -> CmdResult {
    zap(ed, Action::ZapTo)
}

pub fn zap_up_to_char(ed: &mut Editor) -> CmdResult {
    zap(ed, Action::ZapUpTo)
}

/// Selects another window: the other one of two, or one picked by label.
pub fn ace_window(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    let selected = ed.selected();
    let others: Vec<(WindowId, Option<usize>)> = ed
        .frame()
        .layout
        .leaves()
        .into_iter()
        .filter(|id| *id != selected)
        .map(|id| (id, None))
        .collect();
    if others.is_empty() {
        return Err(CmdError::msg("There is no other window"));
    }
    ed.start_jump(others, Action::SelectWindow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_distinct_and_short_first() {
        assert_eq!(make_labels(3), ["a", "s", "d"]);
        assert_eq!(make_labels(9).len(), 9);
        let ten = make_labels(10);
        assert_eq!(ten[..8], ["a", "s", "d", "f", "g", "h", "j", "k"]);
        assert_eq!(ten[8..], ["la", "ls"]);
        for count in [10, 17, 18, 50, 81, 82, 300, 1000] {
            let labels = make_labels(count);
            assert_eq!(labels.len(), count);
            // No label is the start of another, so each can be typed.
            for (index, label) in labels.iter().enumerate() {
                for other in &labels[index + 1..] {
                    assert!(
                        !other.starts_with(label.as_str()) && !label.starts_with(other.as_str()),
                        "{label} and {other} of {count}"
                    );
                }
            }
        }
    }
}
