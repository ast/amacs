//! Commands that pick from candidates: lines, search results, recent
//! files, earlier kills, and actions on a candidate.

use std::path::Path;
use std::process::{Command, Stdio};

use amacs_proto::Key;

use crate::buffer::Buffer;
use crate::completion::{Candidate, Completion, parse_search_value};
use crate::editor::Editor;
use crate::error::{CmdError, CmdResult};
use crate::files::{abbreviate_path, expand_path, project_root};
use crate::minibuffer::{Accepted, dismiss_minibuffer};

/// Picks a line of the buffer by its text and goes to it. The window follows
/// the selection while choosing, and returns if the choice is abandoned.
pub fn search_lines(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    let text = ed.text();
    let current = text.line_of(ed.point());
    let mut candidates = Vec::new();
    let mut nearest = 0;
    for line in 0..text.len_lines() {
        let content = text.slice(text.line_start(line)..text.line_end(line));
        if content.trim().is_empty() {
            continue;
        }
        if line <= current {
            nearest = candidates.len();
        }
        candidates.push(Candidate {
            text: content,
            annotation: (line + 1).to_string(),
            value: Some(line.to_string()),
        });
    }
    if candidates.is_empty() {
        return Err(CmdError::msg("The buffer has no text to search"));
    }

    let origin = (ed.point(), ed.buffer().marker(ed.window().start));
    let restore = move |ed: &mut Editor| {
        ed.set_point(origin.0);
        let start = ed.window().start;
        ed.buffer_mut().set_marker(start, origin.1);
    };
    let go_to = |ed: &mut Editor, candidate: &Candidate| {
        if let Ok(line) = candidate.value().parse::<usize>() {
            ed.goto_line(line + 1, 0);
        }
    };
    let completion = Completion::Items {
        candidates,
        category: "line".to_owned(),
    };
    ed.read_candidate(
        "Go to line: ",
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
                Err(CmdError::msg("No matching line"))
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

/// Searches the files of the project for a regular expression with ripgrep,
/// showing matches as the expression is typed, and visits the chosen one.
pub fn search_files(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    let installed = Command::new("rg")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !installed {
        return Err(CmdError::msg(
            "Searching files needs ripgrep (rg), which is not installed",
        ));
    }
    let directory = project_root(ed.working_directory());
    let prompt = format!("Search {}: ", abbreviate_path(&directory));
    let completion = Completion::Grep { directory };
    ed.read_candidate(prompt, "", completion, |ed, accepted| {
        let location = match &accepted {
            Accepted::Candidate(candidate) => parse_search_value(candidate.value()),
            Accepted::Input(_) => None,
        };
        let (path, line) = location.ok_or_else(|| CmdError::msg("No match selected"))?;
        ed.find_file(&path)?;
        ed.goto_line(line, 0);
        Ok(())
    })
}

/// Visits a file chosen from those visited recently.
pub fn recent_file(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    let candidates: Vec<Candidate> = ed
        .history
        .recent_files()
        .iter()
        .map(|file| Candidate::new(abbreviate_path(file)))
        .collect();
    if candidates.is_empty() {
        return Err(CmdError::msg("No files have been visited yet"));
    }
    let completion = Completion::Items {
        candidates,
        category: "file".to_owned(),
    };
    ed.read_string("Recent file: ", "", completion, |ed, path| {
        ed.find_file(&expand_path(&path, Path::new("/")))
    })
}

/// Shows a buffer chosen by name, or a recently visited file. A name that
/// matches nothing makes a new buffer.
pub fn switch_to_buffer(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    let mut candidates = ed.buffer_candidates(false);
    // Recently visited files that are no longer open can be reopened here.
    let open: Vec<&Path> = ed
        .buffers
        .values()
        .filter_map(|b| b.file.as_deref())
        .collect();
    let closed = ed
        .history
        .recent_files()
        .iter()
        .filter(|file| !open.contains(&file.as_path()));
    candidates.extend(closed.map(|file| Candidate {
        text: abbreviate_path(file),
        annotation: "recent file".to_owned(),
        value: Some(format!("\0{}", file.display())),
    }));

    let completion = Completion::Items {
        candidates,
        category: "buffer".to_owned(),
    };
    ed.read_string("Switch to buffer: ", "", completion, |ed, name| {
        // A recent file's value is marked so that it cannot be a buffer name.
        if let Some(path) = name.strip_prefix('\0') {
            return ed.find_file(Path::new(path));
        }
        if name.is_empty() {
            return Ok(());
        }
        let id = match ed.find_buffer(&name) {
            Some(id) => id,
            None => ed.add_buffer(Buffer::new(name)),
        };
        ed.switch_to_buffer(id);
        Ok(())
    })
}

/// After a yank, replaces it with an earlier kill. Otherwise inserts a kill
/// chosen from the kill ring.
pub fn yank_pop(ed: &mut Editor) -> CmdResult {
    if ed.kill_ring.last_yank().is_some() {
        return super::edit::yank_pop(ed);
    }
    ed.forbid_in_minibuffer()?;
    ed.import_clipboard();
    let candidates: Vec<Candidate> = ed
        .kill_ring
        .entries()
        .enumerate()
        .map(|(index, text)| Candidate {
            text: text.to_owned(),
            annotation: String::new(),
            value: Some(index.to_string()),
        })
        .collect();
    if candidates.is_empty() {
        return Err(CmdError::msg("Kill ring is empty"));
    }
    let completion = Completion::Items {
        candidates,
        category: "kill".to_owned(),
    };
    ed.read_candidate("Yank: ", "", completion, |ed, accepted| {
        let Accepted::Candidate(candidate) = accepted else {
            return Err(CmdError::msg("No matching kill"));
        };
        let start = ed.point();
        ed.insert(&candidate.text)?;
        ed.buffer_mut().set_mark(start);
        Ok(())
    })
}

/// Shows the selected window's buffer position in another window, making
/// one if the frame has only one.
fn in_other_window(ed: &mut Editor) -> CmdResult {
    if ed.frame().layout.leaves().len() == 1 {
        super::windows::split_right(ed)?;
    }
    super::windows::other_window(ed)
}

/// Acts on the selected candidate in some way other than accepting it.
/// Asks which with a single key.
pub fn act_on_candidate(ed: &mut Editor) -> CmdResult {
    ed.settle_minibuffer();
    let Some((category, candidate)) = ed.candidate_at_point() else {
        return Err(CmdError::msg("No candidate to act on"));
    };
    let mut actions = vec!["w copy", "i insert"];
    match category.as_str() {
        "buffer" => actions.extend(["k kill", "o other window"]),
        "file" => actions.push("o other window"),
        _ => {}
    }
    let prompt = format!("Act on {}: {} ", candidate.text.trim(), actions.join(", "));
    ed.ask_key(prompt, move |ed, key| {
        let action = key.self_insert_char();
        dismiss_minibuffer(ed);
        match (action, category.as_str()) {
            (Some('w'), _) => {
                ed.kill_new(candidate.value().to_owned());
                ed.message("Copied");
                Ok(())
            }
            (Some('i'), _) => ed.insert(candidate.value()),
            (Some('k'), "buffer") => {
                let id = ed
                    .find_buffer(&candidate.text)
                    .ok_or_else(|| CmdError::msg(format!("No such buffer: {}", candidate.text)))?;
                ed.kill_buffer(id);
                Ok(())
            }
            (Some('o'), "buffer") => {
                let id = ed
                    .find_buffer(&candidate.text)
                    .ok_or_else(|| CmdError::msg(format!("No such buffer: {}", candidate.text)))?;
                in_other_window(ed)?;
                ed.switch_to_buffer(id);
                Ok(())
            }
            (Some('o'), "file") => {
                let path = expand_path(candidate.value(), ed.working_directory());
                in_other_window(ed)?;
                ed.find_file(&path)
            }
            _ if key == Key::ctrl('g') => Err(CmdError::Quit),
            _ => Err(CmdError::msg("No such action")),
        }
    });
    Ok(())
}
