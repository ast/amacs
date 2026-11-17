//! Per-directory environments from direnv.
//!
//! A project's `.envrc` says what its tools need in their environment: a
//! compiler, a language server, a formatter. The editor asks `direnv` what
//! that comes to and gives it to the programs it starts for files of that
//! project. The editor's own environment is never changed, so buffers of
//! projects with different tools can be open side by side.
//!
//! Asking takes a while the first time, so it happens on a thread.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::completion::Waker;

/// Changes to the editor's environment: variables to set, or with `None`
/// to remove.
pub type Environment = Vec<(String, Option<String>)>;

/// What is known of a directory's environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvState {
    /// direnv is still working it out.
    Loading,
    Ready(Environment),
    /// direnv refused or failed; the text is its complaint.
    Failed(String),
}

struct Entry {
    state: EnvState,
    /// When `.envrc` was last changed, as of loading.
    stamp: Option<SystemTime>,
}

type Results = Arc<Mutex<Vec<(PathBuf, Result<Environment, String>)>>>;

/// The environments of the directories that have been asked about.
#[derive(Default)]
pub struct Environments {
    /// By the directory that holds the `.envrc`.
    entries: HashMap<PathBuf, Entry>,
    results: Results,
}

/// The nearest directory at or above `dir` with an `.envrc`.
pub fn envrc_directory(dir: &Path) -> Option<&Path> {
    dir.ancestors()
        .find(|ancestor| ancestor.join(".envrc").is_file())
}

fn stamp(root: &Path) -> Option<SystemTime> {
    crate::files::modified_time(&root.join(".envrc"))
}

/// Reads the output of `direnv export json`: an object of variables, with
/// `null` for those to remove. Nothing at all means no changes.
fn parse_export(output: &str) -> Result<Environment, String> {
    if output.trim().is_empty() {
        return Ok(Vec::new());
    }
    let value: serde_json::Value =
        serde_json::from_str(output).map_err(|error| format!("unexpected output: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "unexpected output".to_owned())?;
    Ok(object
        .iter()
        .map(|(name, value)| (name.clone(), value.as_str().map(str::to_owned)))
        .collect())
}

fn export(program: &str, root: &Path) -> Result<Environment, String> {
    let output = Command::new(program)
        .args(["export", "json"])
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("cannot run {program}: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // direnv's own words, without its colours.
        let line = stderr
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("it failed without saying why");
        let plain: String = strip_escapes(line);
        return Err(plain.trim_start_matches("direnv: ").to_owned());
    }
    parse_export(&String::from_utf8_lossy(&output.stdout))
}

/// Removes terminal escape sequences.
fn strip_escapes(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // `ESC [ ... letter`.
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

impl Environments {
    /// The environment for programs run for files in `dir`, or `None` if no
    /// `.envrc` applies there. The first call for a project starts loading
    /// and returns [`EnvState::Loading`]; `waker` is called when it is done.
    pub fn lookup(&mut self, dir: &Path, program: &str, waker: &Waker) -> Option<EnvState> {
        let root = envrc_directory(dir)?.to_owned();
        let current = stamp(&root);
        if let Some(entry) = self.entries.get(&root)
            && (entry.stamp == current || entry.state == EnvState::Loading)
        {
            return Some(entry.state.clone());
        }
        self.entries.insert(
            root.clone(),
            Entry {
                state: EnvState::Loading,
                stamp: current,
            },
        );
        let (results, waker, program) = (self.results.clone(), waker.clone(), program.to_owned());
        std::thread::spawn(move || {
            let result = export(&program, &root);
            if let Ok(mut results) = results.lock() {
                results.push((root, result));
            }
            waker();
        });
        Some(EnvState::Loading)
    }

    /// Takes in what the threads have found. Returns the directories that
    /// finished loading, each with a message for the user if it failed.
    pub fn poll(&mut self) -> Vec<(PathBuf, Option<String>)> {
        let finished = match self.results.lock() {
            Ok(mut results) => std::mem::take(&mut *results),
            Err(_) => return Vec::new(),
        };
        finished
            .into_iter()
            .map(|(root, result)| {
                let (state, problem) = match result {
                    Ok(environment) => (EnvState::Ready(environment), None),
                    Err(problem) => (EnvState::Failed(problem.clone()), Some(problem)),
                };
                if let Some(entry) = self.entries.get_mut(&root) {
                    entry.state = state;
                }
                (root, problem)
            })
            .collect()
    }

    /// Whether any directory is still loading.
    pub fn is_loading(&self) -> bool {
        self.entries
            .values()
            .any(|entry| entry.state == EnvState::Loading)
    }

    /// Forgets everything, so that environments are loaded afresh.
    pub fn clear(&mut self) {
        self.entries
            .retain(|_, entry| entry.state == EnvState::Loading);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_output_is_a_set_of_changes() {
        let parsed = parse_export(r#"{"PATH": "/a:/b", "OLD": null}"#).unwrap();
        assert!(parsed.contains(&("PATH".to_owned(), Some("/a:/b".to_owned()))));
        assert!(parsed.contains(&("OLD".to_owned(), None)));
        assert_eq!(parse_export("").unwrap(), Vec::new());
        assert!(parse_export("not json").is_err());
        assert!(parse_export("[1]").is_err());
    }

    #[test]
    fn the_nearest_envrc_decides() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a/b");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(envrc_directory(&nested), None);
        std::fs::write(dir.path().join(".envrc"), "").unwrap();
        assert_eq!(envrc_directory(&nested), Some(dir.path()));
        std::fs::write(dir.path().join("a/.envrc"), "").unwrap();
        assert_eq!(
            envrc_directory(&nested),
            Some(dir.path().join("a").as_path())
        );
    }

    #[test]
    fn escapes_are_removed_from_messages() {
        assert_eq!(
            strip_escapes("\u{1b}[31mdirenv: error .envrc is blocked.\u{1b}[0m"),
            "direnv: error .envrc is blocked."
        );
    }
}
