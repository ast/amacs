//! Reading a line of input in the echo area.
//!
//! This is the minimal reader for the first milestone: a prompt, an editable
//! line and prefix completion with TAB. The vertico-style completion UI
//! replaces the completion part later.

use std::path::{Path, PathBuf};

use amacs_text::Text;

use crate::editor::Editor;
use crate::error::{CmdError, CmdResult};
use crate::files::{abbreviate_path, expand_path};
use crate::script::Value;
use crate::window::WindowId;

/// What TAB completes in the minibuffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completion {
    None,
    File,
    Buffer,
    Command,
}

type OnAccept = Box<dyn FnOnce(&mut Editor, String) -> CmdResult>;

pub(crate) struct MinibufferState {
    pub prompt: String,
    completion: Completion,
    on_accept: OnAccept,
    /// Window to return to when the minibuffer exits.
    previous: WindowId,
    /// Directory that relative file names in the input are resolved against.
    directory: PathBuf,
    /// Promise to reject if the user quits, when Scheme code is waiting.
    promise: Option<u64>,
}

impl Editor {
    /// Reads a string in the minibuffer and passes it to `on_accept` when the
    /// user presses RET. Returns immediately; the command loop keeps running.
    pub fn read_string(
        &mut self,
        prompt: impl Into<String>,
        initial: &str,
        completion: Completion,
        on_accept: impl FnOnce(&mut Editor, String) -> CmdResult + 'static,
    ) -> CmdResult {
        if self.minibuffer.is_some() {
            return Err(CmdError::msg(
                "Command attempted to use minibuffer while in minibuffer",
            ));
        }
        self.minibuffer = Some(MinibufferState {
            prompt: prompt.into(),
            completion,
            on_accept: Box::new(on_accept),
            previous: self.selected(),
            directory: self.working_directory().to_owned(),
            promise: None,
        });
        self.prompt_frame = Some(self.current_frame);
        let window = self.minibuffer_window();
        let buffer = self.windows[window].buffer;
        self.buffers[buffer].reset(Text::from(initial));
        self.frame_mut().selected = window;
        self.set_point(usize::MAX);
        Ok(())
    }

    /// Like [`Editor::read_string`] for Scheme code: returns a promise that
    /// resolves to the input, or is rejected if the user quits.
    pub fn read_string_promise(
        &mut self,
        prompt: impl Into<String>,
        initial: &str,
        completion: Completion,
    ) -> CmdResult<u64> {
        let id = self.new_promise();
        self.read_string(prompt, initial, completion, move |ed, input| {
            ed.resolve_promise(id, Value::Str(input));
            Ok(())
        })?;
        if let Some(state) = &mut self.minibuffer {
            state.promise = Some(id);
        }
        Ok(id)
    }

    fn leave_minibuffer(&mut self) -> Option<MinibufferState> {
        let state = self.minibuffer.take()?;
        self.prompt_frame = None;
        self.frame_mut().selected = if self.windows.contains_key(state.previous) {
            state.previous
        } else {
            self.frame().layout.leaves()[0]
        };
        Some(state)
    }
}

pub fn exit_minibuffer(ed: &mut Editor) -> CmdResult {
    let buffer = ed.windows[ed.minibuffer_window()].buffer;
    let input = ed.buffers[buffer].text().to_string();
    match ed.leave_minibuffer() {
        Some(state) => (state.on_accept)(ed, input),
        None => Ok(()),
    }
}

pub fn abort_minibuffer(ed: &mut Editor) -> CmdResult {
    if let Some(state) = ed.leave_minibuffer()
        && let Some(id) = state.promise
    {
        ed.reject_promise(id);
    }
    Err(CmdError::Quit)
}

/// Extends the input to the longest prefix shared by all candidates and
/// shows the candidates if that does not settle it.
pub fn minibuffer_complete(ed: &mut Editor) -> CmdResult {
    let Some(state) = &ed.minibuffer else {
        return Ok(());
    };
    let completion = state.completion;
    let input = ed.text().to_string();
    // The part of the input that candidates replace starts at `base`.
    let (base, candidates) = match completion {
        Completion::None => return Ok(()),
        Completion::File => file_candidates(&input, &state.directory),
        Completion::Buffer => {
            let names = ed.buffer_ids().map(|id| ed.buffers[id].name.clone());
            (0, matching(names, &input))
        }
        Completion::Command => (0, matching(ed.command_names().map(str::to_owned), &input)),
    };

    let Some(common) = common_prefix(&candidates) else {
        ed.minibuffer_hint = Some(" [No match]".to_owned());
        return Ok(());
    };
    let base_chars = input[..base].chars().count();
    let end = ed.buffer().len();
    ed.delete(base_chars..end)?;
    ed.set_point(base_chars);
    ed.insert(&common)?;

    ed.minibuffer_hint = match candidates.as_slice() {
        [_] => None,
        many => {
            const SHOWN: usize = 8;
            let mut list = many
                .iter()
                .take(SHOWN)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            if many.len() > SHOWN {
                list.push_str(", …");
            }
            Some(format!(" [{list}]"))
        }
    };
    Ok(())
}

fn matching(names: impl Iterator<Item = String>, prefix: &str) -> Vec<String> {
    let mut matches: Vec<String> = names.filter(|name| name.starts_with(prefix)).collect();
    matches.sort();
    matches
}

/// File names completing the last path component of `input`. Directories get
/// a trailing slash.
fn file_candidates(input: &str, directory: &Path) -> (usize, Vec<String>) {
    let base = input.rfind('/').map_or(0, |slash| slash + 1);
    let (dir, partial) = input.split_at(base);
    let dir = expand_path(dir, directory);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (base, Vec::new());
    };
    let names = entries.filter_map(Result::ok).filter_map(|entry| {
        let mut name = entry.file_name().into_string().ok()?;
        // `metadata` follows symlinks, so a link to a directory completes as one.
        if entry.path().metadata().is_ok_and(|meta| meta.is_dir()) {
            name.push('/');
        }
        Some(name)
    });
    (base, matching(names, partial))
}

fn common_prefix(candidates: &[String]) -> Option<String> {
    let (first, rest) = candidates.split_first()?;
    let mut len = first.len();
    for other in rest {
        len = first
            .char_indices()
            .zip(other.chars())
            .find(|((_, a), b)| a != b)
            .map_or(len.min(other.len()), |((i, _), _)| i.min(len));
    }
    Some(first[..len].to_owned())
}

/// The working directory as file prompts show it, with a trailing slash.
pub fn default_directory(ed: &Editor) -> String {
    let mut shown = abbreviate_path(ed.working_directory());
    if !shown.ends_with('/') {
        shown.push('/');
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn common_prefix_respects_char_boundaries() {
        assert_eq!(
            common_prefix(&strings(&["föö", "föx", "fö"])).unwrap(),
            "fö"
        );
        assert_eq!(common_prefix(&strings(&["abc"])).unwrap(), "abc");
        assert_eq!(common_prefix(&strings(&["abc", "xyz"])).unwrap(), "");
        assert_eq!(common_prefix(&strings(&["ab", "abc"])).unwrap(), "ab");
        assert!(common_prefix(&[]).is_none());
    }
}
