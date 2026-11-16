//! What the user typed and visited before: minibuffer input by kind, and
//! recently visited files. Both are kept across sessions.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Entries kept per kind of input.
const HISTORY_LIMIT: usize = 200;
/// Recently visited files kept.
const RECENT_FILES_LIMIT: usize = 500;

#[derive(Debug, Default)]
pub struct History {
    /// Oldest first, by kind of input: "command", "file", "buffer" and so on.
    inputs: BTreeMap<String, Vec<String>>,
    /// Most recent first.
    recent_files: Vec<PathBuf>,
    /// Changed since it was last saved.
    dirty: bool,
}

/// Writes control characters and backslashes so that an entry is one line.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => out.push(c),
        }
    }
    out
}

impl History {
    /// The inputs of one kind, oldest first.
    pub fn entries(&self, kind: &str) -> &[String] {
        self.inputs.get(kind).map_or(&[], Vec::as_slice)
    }

    /// Records an input as the newest of its kind. An earlier copy moves up.
    pub fn add(&mut self, kind: &str, entry: String) {
        if entry.is_empty() {
            return;
        }
        let entries = self.inputs.entry(kind.to_owned()).or_default();
        entries.retain(|old| *old != entry);
        entries.push(entry);
        if entries.len() > HISTORY_LIMIT {
            entries.remove(0);
        }
        self.dirty = true;
    }

    /// Recently visited files, most recent first.
    pub fn recent_files(&self) -> &[PathBuf] {
        &self.recent_files
    }

    pub fn visit(&mut self, file: &Path) {
        if self.recent_files.first().map(PathBuf::as_path) == Some(file) {
            return;
        }
        self.recent_files.retain(|old| old != file);
        self.recent_files.insert(0, file.to_owned());
        self.recent_files.truncate(RECENT_FILES_LIMIT);
        self.dirty = true;
    }

    /// Reads what [`History::save`] wrote to a directory. Missing or
    /// malformed files leave the history empty: it is a convenience.
    pub fn load(dir: &Path) -> Self {
        let mut history = Self::default();
        if let Ok(text) = fs::read_to_string(dir.join("history")) {
            for line in text.lines() {
                if let Some((kind, entry)) = line.split_once('\t') {
                    history
                        .inputs
                        .entry(kind.to_owned())
                        .or_default()
                        .push(unescape(entry));
                }
            }
        }
        if let Ok(text) = fs::read_to_string(dir.join("recent-files")) {
            history.recent_files = text
                .lines()
                .map(|line| PathBuf::from(unescape(line)))
                .collect();
        }
        history
    }

    /// Writes the history to a directory if it has changed since the last
    /// save. Each file is replaced atomically.
    pub fn save(&mut self, dir: &Path) -> io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        fs::create_dir_all(dir)?;
        let mut inputs = String::new();
        for (kind, entries) in &self.inputs {
            for entry in entries {
                inputs.push_str(&format!("{kind}\t{}\n", escape(entry)));
            }
        }
        let recent: String = self
            .recent_files
            .iter()
            .map(|file| format!("{}\n", escape(&file.to_string_lossy())))
            .collect();
        for (name, content) in [("history", inputs), ("recent-files", recent)] {
            let temp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
            fs::write(&temp, content)?;
            fs::rename(&temp, dir.join(name))?;
        }
        self.dirty = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_inputs_move_to_the_end() {
        let mut history = History::default();
        history.add("command", "a".into());
        history.add("command", "b".into());
        history.add("command", "a".into());
        history.add("file", "f".into());
        history.add("file", String::new());
        assert_eq!(history.entries("command"), ["b", "a"]);
        assert_eq!(history.entries("file"), ["f"]);
        assert!(history.entries("other").is_empty());
    }

    #[test]
    fn recent_files_are_most_recent_first_without_duplicates() {
        let mut history = History::default();
        for name in ["/a", "/b", "/a", "/c"] {
            history.visit(Path::new(name));
        }
        let names: Vec<_> = history
            .recent_files()
            .iter()
            .map(|p| p.to_str().unwrap())
            .collect();
        assert_eq!(names, ["/c", "/a", "/b"]);
    }

    #[test]
    fn escaping_round_trips() {
        for text in [
            "plain",
            "tab\there",
            "line\nbreak",
            "back\\slash",
            "\\n literal",
            "",
            "ends\\",
        ] {
            assert_eq!(unescape(&escape(text)), text);
        }
    }
}
