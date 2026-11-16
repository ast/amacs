use std::collections::VecDeque;
use std::ops::Range;

const KILL_RING_MAX: usize = 120;

/// Which end of the latest entry a consecutive kill extends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillDirection {
    Forward,
    Backward,
}

/// Killed text, newest first. Consecutive kills accumulate into one entry.
#[derive(Debug, Default)]
pub struct KillRing {
    entries: VecDeque<String>,
    /// Entry that the last yank inserted.
    yank_index: usize,
    /// Text inserted by the previous command if it was a yank, for `yank-pop`.
    last_yank: Option<Range<usize>>,
    killed_last_command: bool,
    killed_this_command: bool,
    yanked_this_command: bool,
}

impl KillRing {
    pub fn kill(&mut self, text: String, direction: KillDirection) {
        if text.is_empty() {
            return;
        }
        let appending = self.killed_last_command || self.killed_this_command;
        self.killed_this_command = true;
        match self.entries.front_mut() {
            Some(front) if appending => match direction {
                KillDirection::Forward => front.push_str(&text),
                KillDirection::Backward => front.insert_str(0, &text),
            },
            _ => {
                self.entries.push_front(text);
                self.entries.truncate(KILL_RING_MAX);
            }
        }
    }

    /// Adds an entry without making the next kill append to it.
    pub fn push(&mut self, text: String) {
        if !text.is_empty() {
            self.entries.push_front(text);
            self.entries.truncate(KILL_RING_MAX);
        }
    }

    /// All entries, newest first.
    pub fn entries(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(String::as_str)
    }

    /// The newest entry, without affecting what `yank-pop` does next.
    pub fn head(&self) -> Option<&str> {
        self.entries.front().map(String::as_str)
    }

    /// The newest entry, for `yank`.
    pub fn current(&mut self) -> Option<&str> {
        self.yank_index = 0;
        self.entries.front().map(String::as_str)
    }

    /// The entry before the one last yanked, wrapping around, for `yank-pop`.
    pub fn rotate(&mut self) -> Option<&str> {
        if self.entries.is_empty() {
            return None;
        }
        self.yank_index = (self.yank_index + 1) % self.entries.len();
        self.entries.get(self.yank_index).map(String::as_str)
    }

    pub fn set_yanked(&mut self, range: Range<usize>) {
        self.last_yank = Some(range);
        self.yanked_this_command = true;
    }

    pub fn last_yank(&self) -> Option<Range<usize>> {
        self.last_yank.clone()
    }

    /// Called after every command to age the per-command flags.
    pub fn end_command(&mut self) {
        self.killed_last_command = std::mem::take(&mut self.killed_this_command);
        if !std::mem::take(&mut self.yanked_this_command) {
            self.last_yank = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consecutive_kills_accumulate() {
        let mut ring = KillRing::default();
        ring.kill("b".into(), KillDirection::Forward);
        ring.end_command();
        ring.kill("c".into(), KillDirection::Forward);
        ring.end_command();
        ring.kill("a".into(), KillDirection::Backward);
        ring.end_command();
        assert_eq!(ring.current(), Some("abc"));

        ring.end_command(); // some other command
        ring.kill("x".into(), KillDirection::Forward);
        assert_eq!(ring.current(), Some("x"));
        assert_eq!(ring.rotate(), Some("abc"));
        assert_eq!(ring.rotate(), Some("x"));
    }
}
