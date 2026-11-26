//! Candidates for the minibuffer and the matcher that narrows them.
//!
//! Matching runs on worker threads, so a large or still-growing candidate
//! set never blocks typing. The words of the input must all occur in a
//! candidate, in any order, as with Emacs's orderless style.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nucleo::pattern::{CaseMatching, Normalization};
use nucleo::{Config, Injector, Nucleo};

use crate::file_system;

/// Called from a worker thread when new results are ready, to wake the
/// thread that owns the editor.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// Rows of candidates shown under the prompt.
pub const VISIBLE_CANDIDATES: usize = 10;
/// A search starts this long after the last change to its query.
const SEARCH_DELAY: Duration = Duration::from_millis(150);
/// A search stops after this many results. More than anyone reads.
const SEARCH_LIMIT: usize = 5000;
/// Shortest query worth searching for.
const SEARCH_MIN_QUERY: usize = 2;

/// One thing the user can pick.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Candidate {
    /// What is shown and matched against.
    pub text: String,
    /// Extra information shown beside the text, such as a command's
    /// documentation. Not matched.
    pub annotation: String,
    /// What accepting the candidate yields, when that is not the text.
    pub value: Option<String>,
    /// It is not an answer yet but the start of one, as the host in the
    /// name of a remote file is: taking it goes on from it.
    pub continues: bool,
}

impl Candidate {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }

    pub fn annotated(text: impl Into<String>, annotation: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            annotation: annotation.into(),
            ..Self::default()
        }
    }

    /// What accepting the candidate yields.
    pub fn value(&self) -> &str {
        self.value.as_deref().unwrap_or(&self.text)
    }
}

/// What the minibuffer completes.
#[derive(Debug, Clone, Default)]
pub enum Completion {
    /// Nothing: the input is free text.
    #[default]
    None,
    /// A fixed set. The category names the kind of thing, for history and
    /// for the actions that apply.
    Items {
        candidates: Vec<Candidate>,
        category: String,
    },
    /// File names. The input is a path; candidates are the entries of its
    /// directory part.
    File,
    /// Where to put something: like `File`, but only directories are
    /// offered, and a path that ends in a slash is taken as it stands.
    Directory,
    /// Buffer names, most recently selected first.
    Buffer,
    /// Command names, with their documentation.
    Command,
    /// Lines matching the input as a regular expression, in files under a
    /// directory, found with ripgrep as the input changes.
    Grep { directory: PathBuf },
}

/// Candidates that a prompt for file names was given from outside, for
/// the last part of a name: the hosts after `/ssh:`, say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offers {
    /// A name for what is offered. While it stays the same from one
    /// input to the next, so do the candidates.
    pub stage: String,
    /// The text that narrows them, in place of the last part of the
    /// input as typed.
    pub pattern: String,
    pub candidates: Vec<Candidate>,
    /// The entries of the directory are not listed with them.
    pub only: bool,
    /// The input that they were worked out for.
    pub input: String,
}

/// Where a session's candidates come from.
enum Source {
    Static,
    Files {
        /// Directory whose entries are the candidates, as typed.
        listed: Option<String>,
        /// What relative paths are resolved against.
        base: PathBuf,
        /// Whether files that are not directories are left out.
        only_directories: bool,
        /// What is offered besides, or in place of, the entries of the
        /// directory: see [`Session::set_offers`].
        offers: Option<Offers>,
    },
    Grep(Search),
}

struct Search {
    directory: PathBuf,
    /// The query to run and when to start, while one is waiting.
    pending: Option<(String, Instant)>,
    running: Option<RunningSearch>,
}

struct RunningSearch {
    cancel: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
}

impl Drop for RunningSearch {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Ok(mut child) = self.child.lock()
            && let Some(child) = child.as_mut()
        {
            // The reader thread reaps it.
            let _ = child.kill();
        }
    }
}

/// The matcher's pattern for an input: each word as a literal substring.
/// The matcher's own syntax is fuzzy by default and gives `^`, `$` and `!`
/// special meaning, which a leading quote and escapes turn off.
fn literal_pattern(input: &str) -> String {
    input
        .split_whitespace()
        .map(|word| {
            // Only a `$` at the end is special once the word is quoted.
            match word.strip_suffix('$') {
                Some(rest) => format!("'{rest}\\$"),
                None => format!("'{word}"),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn inject(injector: &Injector<Candidate>, candidate: Candidate) {
    injector.push(candidate, |candidate, columns| {
        columns[0] = candidate.text.as_str().into();
    });
}

/// The entries of a directory as candidates, directories first and with a
/// trailing slash.
fn list_directory(dir: &Path) -> Vec<Candidate> {
    let Ok(entries) = file_system::read_dir(dir) else {
        return Vec::new();
    };
    // A link to a directory counts, and only a link needs another look
    // to tell. All of them are looked at together.
    let links: Vec<std::ffi::OsString> = entries
        .iter()
        .filter(|entry| entry.metadata.is_none_or(|meta| meta.is_symlink()))
        .map(|entry| entry.name.clone())
        .collect();
    let mut looked = file_system::links(dir, &links)
        .unwrap_or_default()
        .into_iter();
    let mut names: Vec<(bool, String)> = entries
        .into_iter()
        .filter_map(|entry| {
            let is_dir = match entry.metadata {
                Some(meta) if !meta.is_symlink() => meta.is_dir(),
                _ => looked
                    .next()
                    .and_then(|(_, followed)| followed)
                    .is_some_and(|meta| meta.is_dir()),
            };
            let name = entry.name.into_string().ok()?;
            Some((!is_dir, if is_dir { format!("{name}/") } else { name }))
        })
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|(_, name)| Candidate::new(name))
        .collect()
}

/// Splits a path being typed into its directory part, ending in a slash or
/// empty, and the last component.
pub fn split_path(input: &str) -> (&str, &str) {
    input.split_at(input.rfind('/').map_or(0, |slash| slash + 1))
}

/// A set of candidates being narrowed by the minibuffer input.
pub struct Session {
    matcher: Nucleo<Candidate>,
    source: Source,
    pub category: String,
    /// The pattern last given to the matcher, or `None` if it must be given
    /// one again whatever it is.
    pattern: Option<String>,
    /// Index of the selected match.
    index: usize,
    /// The user moved the selection up onto the input itself.
    on_input: bool,
    /// Index of the first match shown.
    scroll: usize,
    /// The workers had not finished at the last tick.
    running: bool,
    waker: Waker,
}

impl Session {
    /// Creates a session, or `None` if there is nothing to complete.
    ///
    /// `base` is the directory that relative file names are resolved against.
    pub fn new(completion: Completion, base: &Path, waker: Waker) -> Option<Self> {
        let (source, candidates, category) = match completion {
            Completion::None => return None,
            Completion::Items {
                candidates,
                category,
            } => (Source::Static, candidates, category),
            Completion::File | Completion::Directory => {
                let only_directories = matches!(completion, Completion::Directory);
                let source = Source::Files {
                    listed: None,
                    base: base.to_owned(),
                    only_directories,
                    offers: None,
                };
                let category = if only_directories {
                    "directory"
                } else {
                    "file"
                };
                (source, Vec::new(), category.to_owned())
            }
            Completion::Grep { directory } => {
                let search = Search {
                    directory,
                    pending: None,
                    running: None,
                };
                (Source::Grep(search), Vec::new(), "grep".to_owned())
            }
            // The editor turns these into `Items` before they get here.
            Completion::Buffer | Completion::Command => return None,
        };
        let matcher = Nucleo::new(Config::DEFAULT, waker.clone(), None, 1);
        let injector = matcher.injector();
        for candidate in candidates {
            inject(&injector, candidate);
        }
        Some(Self {
            matcher,
            source,
            category,
            pattern: None,
            index: 0,
            on_input: false,
            scroll: 0,
            running: true,
            waker,
        })
    }

    /// Gives a session for file names candidates for the last part of
    /// its input, besides or in place of the entries of the directory,
    /// or takes them away with `None`. They hold while the directory part
    /// of the input stays what it was for `offers.input`. Returns whether
    /// anything changed, in which case [`Session::update`] is due.
    pub fn set_offers(&mut self, given: Option<Offers>) -> bool {
        let Source::Files { offers, .. } = &mut self.source else {
            return false;
        };
        if *offers == given {
            return false;
        }
        *offers = given;
        // The pattern may be another for the same input.
        self.pattern = None;
        true
    }

    /// Narrows to a new input. Called whenever the minibuffer text changes.
    pub fn update(&mut self, input: &str, now: Instant) {
        let pattern = match &mut self.source {
            Source::Static => literal_pattern(input),
            Source::Files {
                listed,
                base,
                only_directories,
                offers,
            } => {
                let (dir, name) = split_path(input);
                let directory = crate::files::expand_typed_path(dir, base);
                // What was offered for another directory is not for
                // this one.
                if offers
                    .as_ref()
                    .is_some_and(|offers| split_path(&offers.input).0 != dir)
                {
                    *offers = None;
                }
                let key = match offers {
                    Some(offers) => format!("{dir}\0{}", offers.stage),
                    None => dir.to_owned(),
                };
                let pattern = match offers {
                    Some(offers) if offers.input == input => offers.pattern.clone(),
                    _ => name.to_owned(),
                };
                if listed.as_deref() != Some(key.as_str()) {
                    *listed = Some(key);
                    self.matcher.restart(true);
                    let injector = self.matcher.injector();
                    let only_offers = offers.as_ref().is_some_and(|offers| offers.only);
                    for candidate in offers.iter().flat_map(|offers| &offers.candidates) {
                        inject(&injector, candidate.clone());
                    }
                    let only_directories = *only_directories;
                    let far = file_system::is_remote(&directory);
                    let list = move || {
                        for candidate in list_directory(&directory) {
                            if !only_directories || candidate.text.ends_with('/') {
                                inject(&injector, candidate);
                            }
                        }
                    };
                    if only_offers {
                    } else if far {
                        // A directory on another host is listed when the
                        // host has answered, which the prompt does not
                        // wait for. Names that come after another
                        // directory was asked for go nowhere.
                        let waker = self.waker.clone();
                        std::thread::spawn(move || {
                            file_system::patiently(list);
                            waker();
                        });
                    } else {
                        list();
                    }
                    // The matcher still holds the pattern for the old directory.
                    self.pattern = None;
                }
                literal_pattern(&pattern)
            }
            Source::Grep(search) => {
                // The input is the query, not a filter on its results.
                search.pending = Some((input.to_owned(), now + SEARCH_DELAY));
                String::new()
            }
        };
        if self.pattern.as_deref() != Some(pattern.as_str()) {
            // Extending a pattern can only narrow the matches, which lets
            // the matcher reuse them.
            let append = self
                .pattern
                .as_deref()
                .is_some_and(|old| !old.is_empty() && pattern.starts_with(old));
            self.matcher.pattern.reparse(
                0,
                &pattern,
                CaseMatching::Smart,
                Normalization::Smart,
                append,
            );
            self.pattern = Some(pattern);
        }
        self.index = 0;
        self.on_input = false;
        self.scroll = 0;
        self.tick(10);
    }

    /// Takes in results from the workers, waiting up to `timeout_ms` for
    /// them. Returns whether the matches changed.
    pub fn tick(&mut self, timeout_ms: u64) -> bool {
        let status = self.matcher.tick(timeout_ms);
        self.running = status.running;
        self.clamp_selection();
        status.changed
    }

    /// Waits, up to `limit`, until the workers have narrowed to the current
    /// input, so that what is selected is what the user asked for.
    pub fn settle(&mut self, limit: Duration) {
        let start = Instant::now();
        self.tick(0);
        while self.running && start.elapsed() < limit {
            self.tick(10);
        }
    }

    /// Whether the workers are still narrowing, so that the matches shown
    /// are not final.
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Starts a search whose delay has passed. Returns whether it did.
    pub fn poll(&mut self, now: Instant) -> bool {
        let Source::Grep(search) = &mut self.source else {
            return false;
        };
        let Some((query, _)) = search.pending.take_if(|(_, due)| *due <= now) else {
            return false;
        };
        search.running = None;
        self.matcher.restart(true);
        if query.chars().count() >= SEARCH_MIN_QUERY {
            search.running = start_search(
                &query,
                &search.directory,
                self.matcher.injector(),
                self.waker.clone(),
            );
        }
        self.tick(0);
        true
    }

    /// When [`Session::poll`] next has something to do.
    pub fn deadline(&self) -> Option<Instant> {
        match &self.source {
            Source::Grep(search) => search.pending.as_ref().map(|(_, due)| *due),
            _ => None,
        }
    }

    /// Number of candidates that match.
    pub fn count(&self) -> usize {
        self.matcher.snapshot().matched_item_count() as usize
    }

    fn clamp_selection(&mut self) {
        self.index = self.index.min(self.count().saturating_sub(1));
        if self.index < self.scroll {
            self.scroll = self.index;
        } else if self.index >= self.scroll + VISIBLE_CANDIDATES {
            self.scroll = self.index + 1 - VISIBLE_CANDIDATES;
        }
    }

    /// Index of the selected match, or `None` when the input itself is
    /// selected: because nothing matches, or because the user moved up
    /// past the first match.
    pub fn selected(&self) -> Option<usize> {
        (!self.on_input && self.count() > 0).then_some(self.index)
    }

    /// Moves the selection by `delta` matches. Moving up from the first
    /// selects the input itself.
    pub fn move_selection(&mut self, delta: i64) {
        let current = self.selected().map_or(-1, |index| index as i64);
        let target = (current + delta).clamp(-1, self.count() as i64 - 1);
        self.on_input = target < 0;
        self.index = target.max(0) as usize;
        self.clamp_selection();
    }

    pub fn select_first(&mut self) {
        self.on_input = false;
        self.index = 0;
        self.clamp_selection();
    }

    pub fn select_last(&mut self) {
        self.on_input = false;
        self.index = self.count().saturating_sub(1);
        self.clamp_selection();
    }

    pub fn selected_candidate(&self) -> Option<Candidate> {
        let item = self
            .matcher
            .snapshot()
            .get_matched_item(self.selected()? as u32)?;
        Some(item.data.clone())
    }

    /// The matches on screen, with their indices.
    pub fn visible(&self) -> Vec<(usize, Candidate)> {
        let snapshot = self.matcher.snapshot();
        let end = (self.scroll + VISIBLE_CANDIDATES).min(self.count());
        let start = self.scroll.min(end);
        snapshot
            .matched_items(start as u32..end as u32)
            .enumerate()
            .map(|(offset, item)| (start + offset, item.data.clone()))
            .collect()
    }

    /// For file names: the directory part of the input and what relative
    /// paths are resolved against.
    pub fn file_base(&self) -> Option<&Path> {
        match &self.source {
            Source::Files { base, .. } => Some(base),
            _ => None,
        }
    }
}

/// The value of a search result: where it is.
pub fn search_value(path: &Path, line: usize) -> String {
    format!("{}\0{line}", path.display())
}

/// Reads back [`search_value`].
pub fn parse_search_value(value: &str) -> Option<(PathBuf, usize)> {
    let (path, line) = value.split_once('\0')?;
    Some((PathBuf::from(path), line.parse().ok()?))
}

/// Runs ripgrep and feeds its matches to the matcher from a thread.
fn start_search(
    query: &str,
    directory: &Path,
    injector: Injector<Candidate>,
    waker: Waker,
) -> Option<RunningSearch> {
    let mut child = Command::new("rg")
        .args([
            "--null",
            "--line-number",
            "--no-heading",
            "--color=never",
            "--smart-case",
        ])
        .args(["--max-columns=300", "--max-columns-preview", "--regexp"])
        .arg(query)
        .arg("./")
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let cancel = Arc::new(AtomicBool::new(false));
    let child = Arc::new(Mutex::new(Some(child)));
    let running = RunningSearch {
        cancel: cancel.clone(),
        child: child.clone(),
    };
    let directory = directory.to_owned();

    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut record = Vec::new();
        let mut found = 0;
        // Each match is `path NUL line:text NEWLINE`.
        while found < SEARCH_LIMIT && !cancel.load(Ordering::Relaxed) {
            record.clear();
            if reader.read_until(b'\n', &mut record).unwrap_or(0) == 0 {
                break;
            }
            let text = String::from_utf8_lossy(&record);
            let Some((path, rest)) = text.trim_end_matches('\n').split_once('\0') else {
                continue;
            };
            let Some((line, matched)) = rest.split_once(':') else {
                continue;
            };
            let Ok(line) = line.parse::<usize>() else {
                continue;
            };
            let path = path.strip_prefix("./").unwrap_or(path);
            let candidate = Candidate {
                text: format!("{path}:{line}: {}", matched.trim()),
                annotation: String::new(),
                value: Some(search_value(&directory.join(path), line)),
                continues: false,
            };
            inject(&injector, candidate);
            found += 1;
        }
        if let Ok(mut child) = child.lock()
            && let Some(mut child) = child.take()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
        waker();
    });
    Some(running)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(names: &[&str]) -> Session {
        let completion = Completion::Items {
            candidates: names.iter().map(|name| Candidate::new(*name)).collect(),
            category: "test".to_owned(),
        };
        let mut session = Session::new(completion, Path::new("/"), Arc::new(|| {})).unwrap();
        session.update("", Instant::now());
        settle(&mut session);
        session
    }

    /// Waits for the workers to finish.
    fn settle(session: &mut Session) {
        while session.matcher.tick(50).running {}
        session.clamp_selection();
    }

    fn matches(session: &mut Session, input: &str) -> Vec<String> {
        session.update(input, Instant::now());
        settle(session);
        session.visible().into_iter().map(|(_, c)| c.text).collect()
    }

    #[test]
    fn words_match_as_substrings_in_any_order() {
        let mut s = session(&[
            "find-file",
            "find-file-other-window",
            "kill-buffer",
            "FILE-up",
        ]);
        assert_eq!(matches(&mut s, "").len(), 4);
        assert_eq!(
            matches(&mut s, "file find"),
            ["find-file", "find-file-other-window"]
        );
        assert_eq!(matches(&mut s, "other"), ["find-file-other-window"]);
        assert_eq!(
            matches(&mut s, "fdfl"),
            Vec::<String>::new(),
            "matching is not fuzzy"
        );
        assert_eq!(
            matches(&mut s, "FILE"),
            ["FILE-up"],
            "an uppercase letter makes it case-sensitive"
        );
        assert_eq!(matches(&mut s, "file").len(), 3);
    }

    #[test]
    fn pattern_characters_are_literal() {
        let mut s = session(&["a^b", "cost$", "!important", "it's", "back\\slash"]);
        assert_eq!(matches(&mut s, "^b"), ["a^b"]);
        assert_eq!(matches(&mut s, "t$"), ["cost$"]);
        assert_eq!(matches(&mut s, "!imp"), ["!important"]);
        assert_eq!(matches(&mut s, "t's"), ["it's"]);
        assert_eq!(matches(&mut s, "k\\s"), ["back\\slash"]);
    }

    #[test]
    fn selection_moves_scrolls_and_reaches_the_input() {
        let names: Vec<String> = (0..25).map(|n| format!("item-{n:02}")).collect();
        let mut s = session(&names.iter().map(String::as_str).collect::<Vec<_>>());
        assert_eq!(s.selected(), Some(0));
        s.move_selection(12);
        assert_eq!(s.selected_candidate().unwrap().text, "item-12");
        assert_eq!(
            s.visible().first().unwrap().0,
            3,
            "the list scrolls to keep the selection in view"
        );
        s.move_selection(100);
        assert_eq!(s.selected(), Some(24));
        s.select_first();
        s.move_selection(-1);
        assert_eq!(
            s.selected(),
            None,
            "above the first candidate is the input itself"
        );
        assert!(s.selected_candidate().is_none());
        s.move_selection(-5);
        assert_eq!(s.selected(), None);
        s.select_last();
        assert_eq!(s.selected(), Some(24));
    }

    #[test]
    fn narrowing_to_nothing_selects_the_input() {
        let mut s = session(&["alpha", "beta"]);
        assert!(matches(&mut s, "zzz").is_empty());
        assert_eq!(s.selected(), None);
        assert_eq!(matches(&mut s, "bet"), ["beta"]);
        assert_eq!(s.selected(), Some(0));
    }

    #[test]
    fn paths_split_at_the_last_slash() {
        assert_eq!(split_path("/usr/lib/foo"), ("/usr/lib/", "foo"));
        assert_eq!(split_path("/usr/lib/"), ("/usr/lib/", ""));
        assert_eq!(split_path("foo"), ("", "foo"));
        assert_eq!(split_path(""), ("", ""));
    }

    #[test]
    fn search_values_round_trip() {
        let value = search_value(Path::new("/a b/c:d.rs"), 42);
        assert_eq!(
            parse_search_value(&value),
            Some((PathBuf::from("/a b/c:d.rs"), 42))
        );
        assert_eq!(parse_search_value("no separator"), None);
    }

    /// A million candidates must narrow fast enough to feel instant. Run with
    /// `cargo test --release -p amacs-core -- --ignored --nocapture million`.
    #[test]
    #[ignore = "a measurement, not a check"]
    fn a_million_candidates_narrow_quickly() {
        let names: Vec<String> = (0..1_000_000)
            .map(|n| format!("crates/module-{}/src/file_{n}.rs", n % 977))
            .collect();
        let start = Instant::now();
        let mut s = session(&names.iter().map(String::as_str).collect::<Vec<_>>());
        eprintln!("loaded 1M candidates in {:?}", start.elapsed());
        for input in ["f", "fi", "file_12", "file_12 module-3", "file_123456"] {
            let start = Instant::now();
            s.update(input, Instant::now());
            settle(&mut s);
            eprintln!(
                "{input:20} -> {:7} matches in {:?}",
                s.count(),
                start.elapsed()
            );
        }
    }
}
