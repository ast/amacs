//! Projects: directories that hold a body of work, and the commands that
//! act on one as a whole.
//!
//! A project is the nearest directory at or above a file that holds one of
//! the entries named by the `project-root-markers` option, such as `.git`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::buffer::BufferId;
use crate::completion::{Candidate, Completion};
use crate::editor::Editor;
use crate::error::{CmdError, CmdResult};
use crate::file_system;
use crate::files::{abbreviate_path, expand_path};
use crate::script::Value;

/// Files listed for a project at most.
const FILE_LIMIT: usize = 200_000;

impl Editor {
    fn project_markers(&self) -> Vec<String> {
        match self.option("project-root-markers") {
            Ok(Value::List(markers)) => markers
                .iter()
                .filter_map(|marker| marker.as_name().map(str::to_owned))
                .collect(),
            _ => vec![".git".to_owned(), ".projectile".to_owned()],
        }
    }

    /// The root of the project that `dir` is in, if it is in one.
    pub fn project_of(&self, dir: &Path) -> Option<PathBuf> {
        let markers = self.project_markers();
        dir.ancestors()
            .find(|ancestor| {
                markers
                    .iter()
                    .any(|marker| file_system::exists(&ancestor.join(marker)))
            })
            .map(Path::to_owned)
    }

    /// The root of the project that `dir` is in, or `dir` itself.
    pub fn project_root(&self, dir: &Path) -> PathBuf {
        self.project_of(dir).unwrap_or_else(|| dir.to_owned())
    }

    /// The project of the current buffer, or why there is none.
    fn current_project(&self) -> CmdResult<PathBuf> {
        self.project_of(self.working_directory())
            .ok_or_else(|| CmdError::msg("This buffer is not in a project"))
    }

    /// Remembers a project, for switching to it later.
    pub(crate) fn note_project(&mut self, file: &Path) {
        if let Some(root) = file.parent().and_then(|dir| self.project_of(dir)) {
            self.history.add("project", root.display().to_string());
        }
    }

    fn project_buffers(&self, root: &Path) -> Vec<BufferId> {
        self.buffer_ids()
            .filter(|id| {
                self.buffers[*id]
                    .file
                    .as_deref()
                    .is_some_and(|file| file.starts_with(root))
            })
            .collect()
    }

    /// Lets the user pick a file of a project and visits it.
    fn find_file_in_project(&mut self, root: PathBuf) -> CmdResult {
        // ripgrep knows which files version control ignores.
        let output = Command::new("rg")
            .args(["--files", "--hidden", "--glob", "!.git"])
            .current_dir(&root)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map_err(|_| {
                CmdError::msg(
                    "Listing a project's files needs ripgrep (rg), which is not installed",
                )
            })?;
        let listing = String::from_utf8_lossy(&output.stdout);
        let mut files: Vec<&str> = listing.lines().take(FILE_LIMIT).collect();
        files.sort_unstable();
        if files.is_empty() {
            return Err(CmdError::msg(format!(
                "No files in {}",
                abbreviate_path(&root)
            )));
        }
        let completion = Completion::Items {
            candidates: files.into_iter().map(Candidate::new).collect(),
            category: "project-file".to_owned(),
        };
        let prompt = format!("Find file in {}: ", abbreviate_path(&root));
        self.read_string(prompt, "", completion, move |ed, file| {
            ed.find_file(&root.join(file))
        })
    }
}

pub fn project_find_file(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    let root = ed.current_project()?;
    ed.find_file_in_project(root)
}

/// Picks one of the projects visited before and then a file in it.
pub fn project_switch_project(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    let candidates: Vec<Candidate> = ed
        .history
        .entries("project")
        .iter()
        .rev()
        .filter(|root| file_system::is_dir(Path::new(root)))
        .map(|root| Candidate::new(abbreviate_path(Path::new(root))))
        .collect();
    if candidates.is_empty() {
        return Err(CmdError::msg("No projects have been visited yet"));
    }
    let completion = Completion::Items {
        candidates,
        category: "project".to_owned(),
    };
    ed.read_string("Switch to project: ", "", completion, |ed, root| {
        let root = expand_path(&root, Path::new("/"));
        if !file_system::is_dir(&root) {
            return Err(CmdError::msg(format!(
                "{} is not a directory",
                abbreviate_path(&root)
            )));
        }
        ed.find_file_in_project(root)
    })
}

/// Shows one of the project's buffers.
pub fn project_switch_to_buffer(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    let root = ed.current_project()?;
    let current = ed.window().buffer;
    let mut ids = ed.project_buffers(&root);
    // The current buffer last: switching to it does nothing.
    ids.sort_by_key(|id| *id == current);
    let candidates = ids
        .into_iter()
        .map(|id| {
            let buffer = &ed.buffers[id];
            let relative = buffer
                .file
                .as_deref()
                .and_then(|file| file.strip_prefix(&root).ok())
                .map(|path| path.display().to_string());
            Candidate::annotated(&buffer.name, relative.unwrap_or_default())
        })
        .collect();
    let completion = Completion::Items {
        candidates,
        category: "buffer".to_owned(),
    };
    ed.read_string("Project buffer: ", "", completion, |ed, name| {
        let id = ed
            .find_buffer(&name)
            .ok_or_else(|| CmdError::msg(format!("No such buffer: {name}")))?;
        ed.switch_to_buffer(id);
        Ok(())
    })
}

/// Kills the project's buffers, after asking.
pub fn project_kill_buffers(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    let root = ed.current_project()?;
    let ids = ed.project_buffers(&root);
    let modified = ids
        .iter()
        .filter(|id| ed.buffers[**id].is_modified())
        .count();
    let unsaved = match modified {
        0 => String::new(),
        count => format!(", {count} with unsaved changes"),
    };
    let question = format!(
        "Kill the {} buffers of {}{unsaved}?",
        ids.len(),
        abbreviate_path(&root)
    );
    ed.ask_yes_no(question, move |ed| {
        for id in ids {
            ed.kill_buffer(id);
        }
        Ok(())
    });
    Ok(())
}
