//! Keyboard macros: the keys of some work, kept as they are typed, to be
//! typed again.
//!
//! While a macro is defined, every key is noted before it is handled. To
//! run a macro its keys are handed to the host one at a time, as what was
//! typed while a command waited is: each is handled when the commands of
//! the one before, those written in Scheme too, are done. An error stops
//! the macro, which is how one that runs "until the end" ends.

use std::collections::VecDeque;

use amacs_proto::Key;

use crate::editor::Editor;
use crate::error::{CmdError, CmdResult};

/// How many times a macro that is to run until an error runs at most, so
/// that one that never fails comes to an end.
const UNTIL_ERROR: usize = 10_000;

#[derive(Default)]
pub(crate) struct Macros {
    /// The keys typed since the definition of a macro began, if one is
    /// being defined.
    recording: Option<Vec<Key>>,
    /// Where in those the key sequence that is being typed began: the keys
    /// of the command that ends the definition are not part of the macro.
    sequence_start: usize,
    /// The macro last defined.
    last: Vec<Key>,
    /// The keys of the run in progress that are yet to be handled.
    running: VecDeque<Key>,
    /// The macro that runs and how many runs are to follow this one.
    again: Option<(Vec<Key>, usize)>,
    /// The key being handled came from a macro, not from the user.
    feeding: bool,
}

impl Editor {
    /// Whether a keyboard macro is being defined.
    pub fn defining_macro(&self) -> bool {
        self.macros.recording.is_some()
    }

    /// The keys of the macro last defined.
    pub fn last_macro(&self) -> &[Key] {
        &self.macros.last
    }

    /// Notes a key that is about to be handled, while a macro is defined.
    /// `starts_sequence` says that no key sequence is in progress.
    pub(crate) fn macro_note_key(&mut self, key: Key, starts_sequence: bool) {
        if std::mem::take(&mut self.macros.feeding) {
            return;
        }
        if let Some(keys) = &mut self.macros.recording {
            if starts_sequence {
                self.macros.sequence_start = keys.len();
            }
            keys.push(key);
        }
    }

    pub(crate) fn macro_start(&mut self) -> CmdResult {
        if self.defining_macro() {
            return Err(CmdError::msg("Already defining a keyboard macro"));
        }
        self.macros.recording = Some(Vec::new());
        self.macros.sequence_start = 0;
        self.message("Defining a keyboard macro…");
        Ok(())
    }

    pub(crate) fn macro_end(&mut self) -> CmdResult {
        let Some(mut keys) = self.macros.recording.take() else {
            return Err(CmdError::msg("Not defining a keyboard macro"));
        };
        keys.truncate(self.macros.sequence_start);
        if keys.is_empty() {
            self.message("Nothing was typed; the last macro is kept");
        } else {
            self.macros.last = keys;
            self.message("Keyboard macro defined");
        }
        Ok(())
    }

    /// Runs a macro a number of times; 0 is until a command fails.
    pub fn macro_run(&mut self, keys: Vec<Key>, times: usize) -> CmdResult {
        if keys.is_empty() {
            return Err(CmdError::msg("No keyboard macro is defined"));
        }
        if self.defining_macro() {
            return Err(CmdError::msg(
                "A keyboard macro is being defined; end it first",
            ));
        }
        if self.macro_running() {
            return Err(CmdError::msg("A keyboard macro is running"));
        }
        let times = if times == 0 { UNTIL_ERROR } else { times };
        self.macros.running = keys.iter().copied().collect();
        self.macros.again = Some((keys, times - 1));
        Ok(())
    }

    pub(crate) fn macro_running(&self) -> bool {
        self.macros.again.is_some()
    }

    /// Stops the macro that runs, as an error in one of its commands does.
    pub(crate) fn macro_stop(&mut self) {
        self.macros.running.clear();
        self.macros.again = None;
    }

    /// The next key of the macro that runs, for the host to handle as if
    /// it had been typed.
    pub(crate) fn macro_take_key(&mut self) -> Option<Key> {
        if self.macros.running.is_empty() {
            match &mut self.macros.again {
                Some((keys, left)) if *left > 0 => {
                    *left -= 1;
                    self.macros.running = keys.iter().copied().collect();
                }
                _ => {
                    self.macros.again = None;
                    return None;
                }
            }
        }
        let key = self.macros.running.pop_front();
        self.macros.feeding = key.is_some();
        key
    }
}

fn times(ed: &Editor) -> usize {
    usize::try_from(ed.arg()).unwrap_or(1)
}

pub fn start_macro(ed: &mut Editor) -> CmdResult {
    ed.macro_start()
}

pub fn end_macro(ed: &mut Editor) -> CmdResult {
    ed.macro_end()
}

pub fn call_macro(ed: &mut Editor) -> CmdResult {
    let keys = ed.last_macro().to_vec();
    ed.macro_run(keys, times(ed))
}

/// Ends the definition of a macro, or runs the last one: what one key
/// does for both.
pub fn end_or_call_macro(ed: &mut Editor) -> CmdResult {
    if ed.defining_macro() {
        ed.macro_end()
    } else {
        call_macro(ed)
    }
}
