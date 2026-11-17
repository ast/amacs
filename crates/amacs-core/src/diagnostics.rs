//! Commands for the diagnostics of a buffer: moving between them and
//! listing them.

use crate::completion::{Candidate, Completion};
use crate::editor::Editor;
use crate::error::{CmdError, CmdResult};
use crate::faces::Severity;
use crate::lsp::first_line;
use crate::minibuffer::Accepted;

fn describe(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Note => "note",
    }
}

/// Moves to the nearest diagnostic after or before point and says what it is.
fn move_to_diagnostic(ed: &mut Editor, forward: bool) -> CmdResult {
    let point = ed.point();
    let mut starts: Vec<(usize, String)> = ed
        .buffer()
        .diagnostics()
        .map(|(range, diagnostic)| (range.start, first_line(&diagnostic.message).to_owned()))
        .collect();
    starts.sort();
    let target = if forward {
        starts.iter().find(|(start, _)| *start > point)
    } else {
        starts.iter().rev().find(|(start, _)| *start < point)
    };
    match target {
        Some((start, message)) => {
            let (start, message) = (*start, message.clone());
            ed.set_point(start);
            ed.message(message);
            Ok(())
        }
        None if starts.is_empty() => Err(CmdError::msg("This buffer has no diagnostics")),
        None if forward => Err(CmdError::msg("No more diagnostics after point")),
        None => Err(CmdError::msg("No more diagnostics before point")),
    }
}

pub fn next_error(ed: &mut Editor) -> CmdResult {
    move_to_diagnostic(ed, true)
}

pub fn previous_error(ed: &mut Editor) -> CmdResult {
    move_to_diagnostic(ed, false)
}

/// Lists the buffer's diagnostics and goes to the one chosen. The window
/// follows the selection while choosing.
pub fn list_diagnostics(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    let text = ed.text();
    let mut found: Vec<(usize, Candidate)> = ed
        .buffer()
        .diagnostics()
        .map(|(range, diagnostic)| {
            let line = text.line_of(range.start) + 1;
            let candidate = Candidate {
                text: format!(
                    "{line}: {}: {}",
                    describe(diagnostic.severity),
                    first_line(&diagnostic.message)
                ),
                annotation: diagnostic.source.clone(),
                value: Some(range.start.to_string()),
            };
            (range.start, candidate)
        })
        .collect();
    if found.is_empty() {
        return Err(CmdError::msg("This buffer has no diagnostics"));
    }
    found.sort_by_key(|(start, _)| *start);
    let point = ed.point();
    let nearest = found
        .iter()
        .rposition(|(start, _)| *start <= point)
        .unwrap_or(0);
    let completion = Completion::Items {
        candidates: found.into_iter().map(|(_, candidate)| candidate).collect(),
        category: "diagnostic".to_owned(),
    };
    let origin = (point, ed.buffer().marker(ed.window().start));
    let restore = move |ed: &mut Editor| {
        ed.set_point(origin.0);
        let start = ed.window().start;
        ed.buffer_mut().set_marker(start, origin.1);
    };
    let go_to = |ed: &mut Editor, candidate: &Candidate| {
        if let Ok(pos) = candidate.value().parse::<usize>() {
            ed.set_point(pos);
        }
    };
    ed.read_candidate(
        "Diagnostic: ",
        "",
        completion,
        move |ed, accepted| match accepted {
            Accepted::Candidate(candidate) => {
                ed.buffer_mut().set_mark(origin.0);
                go_to(ed, &candidate);
                Ok(())
            }
            Accepted::Input(_) => {
                restore(ed);
                Err(CmdError::msg("No such diagnostic"))
            }
        },
    )?;
    ed.set_prompt_preview(
        move |ed, candidate| {
            if let Some(candidate) = candidate {
                go_to(ed, candidate);
            }
        },
        restore,
    );
    ed.select_candidate(nearest);
    Ok(())
}
