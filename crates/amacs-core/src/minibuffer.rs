//! Reading input in the minibuffer, with a list of candidates to pick from.
//!
//! The candidates are shown under the prompt and narrow as the user types.
//! RET accepts the selected candidate, or the input itself when nothing
//! matches or the user has moved the selection up onto the input.

use std::time::Instant;

use amacs_text::Text;

use crate::completion::{Candidate, Completion, Session, VISIBLE_CANDIDATES, split_path};
use crate::editor::Editor;
use crate::error::{CmdError, CmdResult};
use crate::files::abbreviate_path;
use crate::script::Value;
use crate::window::WindowId;

/// What the user accepted in the minibuffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Accepted {
    Candidate(Candidate),
    /// The text as typed, when it is not one of the candidates.
    Input(String),
}

impl Accepted {
    /// The candidate's value or the typed text.
    pub fn into_value(self) -> String {
        match self {
            Accepted::Candidate(candidate) => candidate.value().to_owned(),
            Accepted::Input(input) => input,
        }
    }
}

/// How long accepting waits for the matcher to catch up with the input.
const SETTLE_LIMIT: std::time::Duration = std::time::Duration::from_millis(500);

type OnAccept = Box<dyn FnOnce(&mut Editor, Accepted) -> CmdResult>;
type OnSelect = Box<dyn FnMut(&mut Editor, Option<&Candidate>)>;
type OnAbort = Box<dyn FnOnce(&mut Editor)>;

pub(crate) struct MinibufferState {
    pub prompt: String,
    pub session: Option<Session>,
    on_accept: OnAccept,
    /// Called in the original window when the selected candidate changes,
    /// to preview it.
    on_select: Option<OnSelect>,
    /// Called in the original window if the user quits, to undo previews.
    on_abort: Option<OnAbort>,
    /// Window to return to when the minibuffer exits.
    previous: WindowId,
    /// Promise to reject if the user quits, when Scheme code is waiting.
    promise: Option<u64>,
    /// Which input history this prompt reads and adds to.
    history: String,
    /// Position in that history while the user steps through it.
    history_index: Option<usize>,
    /// The minibuffer text's tick when the candidates were last narrowed.
    synced_tick: Option<u64>,
    /// The candidate last passed to `on_select`.
    previewed: Option<Candidate>,
    /// Where each window started when the prompt opened. The candidate
    /// list takes rows from the windows, which scrolls them to keep point
    /// in view; they go back when the prompt closes.
    window_starts: Vec<(WindowId, crate::buffer::BufferId, usize)>,
}

impl Editor {
    /// Reads from the minibuffer and passes what the user accepts to
    /// `on_accept`. Returns immediately; the command loop keeps running.
    pub fn read_candidate(
        &mut self,
        prompt: impl Into<String>,
        initial: &str,
        completion: Completion,
        on_accept: impl FnOnce(&mut Editor, Accepted) -> CmdResult + 'static,
    ) -> CmdResult {
        if self.minibuffer.is_some() {
            return Err(CmdError::msg(
                "Command attempted to use minibuffer while in minibuffer",
            ));
        }
        let completion = self.resolve_completion(completion);
        let session = Session::new(completion, self.working_directory(), self.waker.clone());
        let window_starts = self
            .frame()
            .layout
            .leaves()
            .into_iter()
            .map(|id| {
                let window = &self.windows[id];
                let start = self.buffers[window.buffer].marker(window.start);
                (id, window.buffer, start)
            })
            .collect();
        let history = session
            .as_ref()
            .map_or("string", |session| session.category.as_str())
            .to_owned();
        self.minibuffer = Some(MinibufferState {
            prompt: prompt.into(),
            session,
            on_accept: Box::new(on_accept),
            on_select: None,
            on_abort: None,
            previous: self.selected(),
            promise: None,
            history,
            history_index: None,
            synced_tick: None,
            previewed: None,
            window_starts,
        });
        self.prompt_frame = Some(self.current_frame);
        let window = self.minibuffer_window();
        let buffer = self.windows[window].buffer;
        self.buffers[buffer].reset(Text::from(initial));
        self.frame_mut().selected = window;
        self.set_point(usize::MAX);
        self.sync_minibuffer(Instant::now());
        Ok(())
    }

    /// Like [`Editor::read_candidate`] for callers that only want the text.
    pub fn read_string(
        &mut self,
        prompt: impl Into<String>,
        initial: &str,
        completion: Completion,
        on_accept: impl FnOnce(&mut Editor, String) -> CmdResult + 'static,
    ) -> CmdResult {
        self.read_candidate(prompt, initial, completion, |ed, accepted| {
            on_accept(ed, accepted.into_value())
        })
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

    /// Sets what happens as the selection moves and if the user quits, for
    /// the prompt just opened. Both run with the original window selected.
    pub fn set_prompt_preview(
        &mut self,
        on_select: impl FnMut(&mut Editor, Option<&Candidate>) + 'static,
        on_abort: impl FnOnce(&mut Editor) + 'static,
    ) {
        if let Some(state) = &mut self.minibuffer {
            state.on_select = Some(Box::new(on_select));
            state.on_abort = Some(Box::new(on_abort));
        }
    }

    /// Selects a candidate by its index among the matches, for prompts that
    /// should not start at the first one.
    pub fn select_candidate(&mut self, index: usize) {
        if let Some(session) = self
            .minibuffer
            .as_mut()
            .and_then(|state| state.session.as_mut())
        {
            session.select_first();
            session.move_selection(index as i64);
        }
        self.preview_selection();
    }

    /// The buffers as candidates, most recently selected first, annotated
    /// with their files. The current buffer comes first or last: last suits
    /// switching, since switching to it does nothing.
    pub(crate) fn buffer_candidates(&self, current_first: bool) -> Vec<Candidate> {
        let current = self.window().buffer;
        let others = self.buffer_ids().filter(|id| *id != current);
        let ids: Vec<_> = if current_first {
            std::iter::once(current).chain(others).collect()
        } else {
            others.chain(std::iter::once(current)).collect()
        };
        ids.into_iter()
            .map(|id| {
                let buffer = &self.buffers[id];
                let file = buffer.file.as_deref().map(abbreviate_path);
                Candidate::annotated(&buffer.name, file.unwrap_or_default())
            })
            .collect()
    }

    /// Turns the shorthands for buffers and commands into their candidates.
    fn resolve_completion(&self, completion: Completion) -> Completion {
        match completion {
            Completion::Buffer => Completion::Items {
                candidates: self.buffer_candidates(false),
                category: "buffer".to_owned(),
            },
            Completion::Command => {
                let mut names: Vec<&str> = self.command_names().collect();
                names.sort_unstable();
                // Commands used before come first, most recent first.
                let recent: Vec<&str> = self
                    .history
                    .entries("command")
                    .iter()
                    .rev()
                    .map(String::as_str)
                    .filter(|name| self.command(name).is_some())
                    .collect();
                let rest = names.iter().copied().filter(|name| !recent.contains(name));
                let candidates = recent
                    .iter()
                    .copied()
                    .chain(rest)
                    .map(|name| {
                        let doc = self
                            .command(name)
                            .map_or("", |command| command.doc.as_str());
                        Candidate::annotated(name, doc.lines().next().unwrap_or_default())
                    })
                    .collect();
                Completion::Items {
                    candidates,
                    category: "command".to_owned(),
                }
            }
            other => other,
        }
    }

    /// Rows of the echo area in the current frame: one, plus the candidate
    /// list while a prompt with candidates is open there, or the lines of a
    /// key hint.
    pub fn echo_rows(&self) -> u16 {
        let available = self.frame().size.1.saturating_sub(3);
        let prompting = self.prompt_frame == Some(self.current_frame);
        let extra = match &self.minibuffer {
            Some(state) if prompting && state.session.is_some() => VISIBLE_CANDIDATES,
            _ => self.frame().hint.len(),
        };
        1 + (extra as u16).min(available)
    }

    fn minibuffer_input(&self) -> String {
        let buffer = self.windows[self.minibuffer_window()].buffer;
        self.buffers[buffer].text().to_string()
    }

    /// Replaces the minibuffer text. Only valid while the minibuffer is selected.
    fn set_minibuffer_input(&mut self, text: &str) -> CmdResult {
        let len = self.buffer().len();
        self.delete(0..len)?;
        self.set_point(0);
        self.insert(text)
    }

    /// Narrows the candidates to the current input if it has changed, takes
    /// in results from the matcher, and previews the selection.
    pub(crate) fn sync_minibuffer(&mut self, now: Instant) {
        let buffer = self.frame().minibuffer_buffer;
        let tick = self.buffers[buffer].tick();
        let input = self.buffers[buffer].text().to_string();
        let Some(state) = &mut self.minibuffer else {
            return;
        };
        if let Some(session) = &mut state.session {
            if state.synced_tick != Some(tick) {
                session.update(&input, now);
            }
            session.poll(now);
            session.tick(0);
        }
        state.synced_tick = Some(tick);
        self.preview_selection();
    }

    /// Runs `f` with the window that was selected before the minibuffer.
    fn in_original_window(&mut self, f: impl FnOnce(&mut Editor)) {
        let Some(previous) = self.minibuffer.as_ref().map(|state| state.previous) else {
            return;
        };
        if !self.windows.contains_key(previous) {
            return;
        }
        let minibuffer = std::mem::replace(&mut self.frame_mut().selected, previous);
        f(self);
        self.frame_mut().selected = minibuffer;
    }

    fn preview_selection(&mut self) {
        let Some(state) = &mut self.minibuffer else {
            return;
        };
        let current = state.session.as_ref().and_then(Session::selected_candidate);
        if state.on_select.is_none() || current == state.previewed {
            return;
        }
        state.previewed.clone_from(&current);
        // The hook needs the editor, so it is taken out while it runs.
        let Some(mut on_select) = state.on_select.take() else {
            return;
        };
        self.in_original_window(|ed| on_select(ed, current.as_ref()));
        if let Some(state) = &mut self.minibuffer {
            state.on_select = Some(on_select);
        }
    }

    /// Whether worker threads are still narrowing the candidates. They wake
    /// the host when done; tests use this to wait for a settled state.
    pub fn is_matching(&self) -> bool {
        let session = self
            .minibuffer
            .as_ref()
            .and_then(|state| state.session.as_ref());
        session.is_some_and(Session::is_running)
    }

    /// When the minibuffer next needs [`Editor::poll`] to be called.
    pub(crate) fn minibuffer_deadline(&self) -> Option<Instant> {
        self.minibuffer.as_ref()?.session.as_ref()?.deadline()
    }

    fn leave_minibuffer(&mut self) -> Option<MinibufferState> {
        let state = self.minibuffer.take()?;
        self.prompt_frame = None;
        for (id, buffer, start) in &state.window_starts {
            if let Some(window) = self.windows.get(*id)
                && window.buffer == *buffer
            {
                let marker = window.start;
                self.buffers[*buffer].set_marker(marker, *start);
            }
        }
        self.frame_mut().selected = if self.windows.contains_key(state.previous) {
            state.previous
        } else {
            self.frame().layout.leaves()[0]
        };
        // The windows get back the rows the candidates took, before what
        // was asked for is done with them.
        self.relayout();
        Some(state)
    }

    fn accept(&mut self, accepted: Accepted) -> CmdResult {
        let Some(state) = self.leave_minibuffer() else {
            return Ok(());
        };
        let entry = match &accepted {
            Accepted::Candidate(candidate) => candidate.text.clone(),
            Accepted::Input(input) => input.clone(),
        };
        self.history.add(&state.history, entry);
        (state.on_accept)(self, accepted)
    }

    fn session_mut(&mut self) -> Option<&mut Session> {
        self.minibuffer.as_mut()?.session.as_mut()
    }

    /// Makes the candidates reflect the input as it is now. Typing ahead of
    /// the matcher must not make RET accept something from a stale list.
    pub(crate) fn settle_minibuffer(&mut self) {
        self.sync_minibuffer(Instant::now());
        if let Some(session) = self.session_mut() {
            session.settle(SETTLE_LIMIT);
        }
    }

    /// The candidate that an action would apply to, with its category.
    pub(crate) fn candidate_at_point(&self) -> Option<(String, Candidate)> {
        let session = self.minibuffer.as_ref()?.session.as_ref()?;
        let candidate = session.selected_candidate()?;
        // A file candidate is only the last component of the path.
        let candidate = match session.file_base() {
            Some(_) => {
                let input = self.minibuffer_input();
                let path = format!("{}{}", split_path(&input).0, candidate.text);
                Candidate {
                    value: Some(path),
                    ..candidate
                }
            }
            None => candidate,
        };
        Some((session.category.clone(), candidate))
    }
}

/// Accepts the selected candidate, or the input if none is selected. A
/// directory is entered instead, so that its files can be chosen from.
pub fn exit_minibuffer(ed: &mut Editor) -> CmdResult {
    ed.settle_minibuffer();
    let input = ed.minibuffer_input();
    let Some((category, candidate)) = ed.candidate_at_point() else {
        return ed.accept(Accepted::Input(input));
    };
    // Where to put something: a directory as typed is the answer, and a
    // name being typed is completed to a directory.
    if category == "directory" {
        if input.is_empty() || input.ends_with('/') {
            return ed.accept(Accepted::Input(input));
        }
        return ed.set_minibuffer_input(candidate.value());
    }
    if category == "file" && candidate.text.ends_with('/') {
        return ed.set_minibuffer_input(candidate.value());
    }
    ed.accept(Accepted::Candidate(candidate))
}

/// Accepts the text as typed, even if a candidate is selected.
pub fn exit_with_input(ed: &mut Editor) -> CmdResult {
    let input = ed.minibuffer_input();
    ed.accept(Accepted::Input(input))
}

pub fn abort_minibuffer(ed: &mut Editor) -> CmdResult {
    if let Some(on_abort) = ed
        .minibuffer
        .as_mut()
        .and_then(|state| state.on_abort.take())
    {
        ed.in_original_window(on_abort);
    }
    if let Some(state) = ed.leave_minibuffer()
        && let Some(id) = state.promise
    {
        ed.reject_promise(id);
    }
    Err(CmdError::Quit)
}

/// Leaves the minibuffer without accepting or counting as a quit, for
/// actions that take over from the prompt.
pub(crate) fn dismiss_minibuffer(ed: &mut Editor) {
    let _ = abort_minibuffer(ed);
}

fn move_selection(ed: &mut Editor, delta: i64) -> CmdResult {
    if let Some(session) = ed.session_mut() {
        session.move_selection(delta);
    }
    Ok(())
}

pub fn next_candidate(ed: &mut Editor) -> CmdResult {
    move_selection(ed, ed.arg())
}

pub fn previous_candidate(ed: &mut Editor) -> CmdResult {
    move_selection(ed, -ed.arg())
}

pub fn next_candidate_page(ed: &mut Editor) -> CmdResult {
    move_selection(ed, VISIBLE_CANDIDATES as i64)
}

pub fn previous_candidate_page(ed: &mut Editor) -> CmdResult {
    // Paging up stops at the first candidate rather than on the input.
    let selected = ed.session_mut().and_then(|session| session.selected());
    let delta = (selected.unwrap_or(0) as i64).min(VISIBLE_CANDIDATES as i64);
    move_selection(ed, -delta)
}

pub fn first_candidate(ed: &mut Editor) -> CmdResult {
    if let Some(session) = ed.session_mut() {
        session.select_first();
    }
    Ok(())
}

pub fn last_candidate(ed: &mut Editor) -> CmdResult {
    if let Some(session) = ed.session_mut() {
        session.select_last();
    }
    Ok(())
}

/// Replaces the input with the selected candidate, to edit it or, for a
/// directory, to continue into it.
pub fn insert_candidate(ed: &mut Editor) -> CmdResult {
    ed.settle_minibuffer();
    match ed.candidate_at_point() {
        // A file candidate's value is the whole path so far.
        Some((category, candidate)) if category == "file" => {
            ed.set_minibuffer_input(candidate.value())
        }
        Some((_, candidate)) => ed.set_minibuffer_input(&candidate.text),
        None => Ok(()),
    }
}

/// Deletes backward. After a slash in a file name, deletes the whole last
/// directory, which is how one goes up.
pub fn delete_backward(ed: &mut Editor) -> CmdResult {
    let is_file = ed
        .session_mut()
        .is_some_and(|session| session.file_base().is_some());
    let input = ed.minibuffer_input();
    let at_end = ed.point() == ed.buffer().len();
    if is_file && at_end && input.ends_with('/') && input.len() > 1 {
        let parent = split_path(&input[..input.len() - 1]).0.to_owned();
        return ed.set_minibuffer_input(&parent);
    }
    crate::commands::delete_backward_char(ed)
}

fn step_history(ed: &mut Editor, older: bool) -> CmdResult {
    let Some(state) = &mut ed.minibuffer else {
        return Ok(());
    };
    let entries = ed.history.entries(&state.history);
    let index = match (state.history_index, older) {
        (None, true) => entries.len().checked_sub(1),
        (Some(index), true) => index.checked_sub(1),
        (Some(index), false) => Some(index + 1).filter(|next| *next < entries.len()),
        (None, false) => None,
    };
    let Some(index) = index else {
        let end = if older { "Beginning" } else { "End" };
        return Err(CmdError::msg(format!("{end} of history")));
    };
    state.history_index = Some(index);
    let entry = entries[index].clone();
    ed.set_minibuffer_input(&entry)
}

pub fn previous_history(ed: &mut Editor) -> CmdResult {
    step_history(ed, true)
}

pub fn next_history(ed: &mut Editor) -> CmdResult {
    step_history(ed, false)
}

/// The working directory as file prompts show it, with a trailing slash.
pub fn default_directory(ed: &Editor) -> String {
    let mut shown = abbreviate_path(ed.working_directory());
    if !shown.ends_with('/') {
        shown.push('/');
    }
    shown
}
