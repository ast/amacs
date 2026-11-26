//! Spelling.
//!
//! Drawing a window never waits for a dictionary. It asks what is known of
//! each word it shows: a word known to be wrong is marked, and one that
//! nothing is known of yet is drawn as it is and handed to a thread of its
//! own, which has the dictionaries. What the thread finds comes back
//! through [`Editor::poll`], and the next drawing has it.
//!
//! The dictionaries are those of Hunspell, an `.aff` and a `.dic` file for
//! each language. A word is right if any of a buffer's languages knows it.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::mpsc;

use amacs_treesit::{Highlight, Highlights};
use spellbook::Dictionary;

use crate::buffer::BufferId;
use crate::completion::Waker;
use crate::editor::Editor;
use crate::script::Value;

pub(crate) const SPELL_MODE: &str = "spell-mode";
pub(crate) const LANGUAGES: &str = "spell-languages";
pub(crate) const DICTIONARY_PATH: &str = "spell-dictionary-path";
pub(crate) const ONLY_IN: &str = "spell-only-in";
pub(crate) const NOT_IN: &str = "spell-not-in";

/// No word of a language is as long as this, so what is cannot be one.
const LONGEST_WORD: usize = 60;
/// Text without a space in it for longer than this is not prose.
const LONGEST_CHUNK: usize = 200;

/// A word of a text, where it is and what to look up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Word {
    /// In chars.
    pub range: Range<usize>,
    /// The byte that the word starts at.
    pub byte: usize,
    pub text: String,
}

/// The words to check in `len` chars, which start at char `start` and byte
/// `byte` of their text. A word that the last of them is in is read to its
/// end, so `chars` may go on for longer than `len`.
///
/// A word is letters, with apostrophes between them. Left out are the
/// things that look like words and are not: what is glued to digits or to
/// the punctuation of code, as in `foo_bar`, `x.len()` and `--flag`, the
/// parts of addresses and file names, and words all in capitals.
pub(crate) fn words(
    chars: impl Iterator<Item = char>,
    start: usize,
    byte: usize,
    len: usize,
) -> Vec<Word> {
    let mut found = Vec::new();
    let mut chunk: Vec<char> = Vec::new();
    let (mut chunk_start, mut chunk_byte) = (start, byte);
    let mut byte = byte;
    for (offset, c) in chars.chain(Some('\n')).enumerate() {
        if c.is_whitespace() {
            if !chunk.is_empty() && chunk.len() <= LONGEST_CHUNK {
                chunk_words(&chunk, chunk_start, chunk_byte, &mut found);
            }
            chunk.clear();
            if offset >= len {
                break;
            }
        } else {
            if chunk.is_empty() {
                (chunk_start, chunk_byte) = (start + offset, byte);
            }
            // What is too long is counted and not kept.
            if chunk.len() <= LONGEST_CHUNK {
                chunk.push(c);
            }
        }
        byte += c.len_utf8();
    }
    found
}

fn is_apostrophe(c: char) -> bool {
    c == '\'' || c == '\u{2019}'
}

/// The words of text that has no space in it.
fn chunk_words(chunk: &[char], start: usize, byte: usize, found: &mut Vec<Word>) {
    let text_start = chunk.iter().take(4).collect::<String>();
    if chunk.iter().any(|c| matches!(c, '/' | '@' | '\\'))
        || text_start == "www."
        || (chunk[0] == '-' && chunk.get(1).is_some_and(|c| *c == '-' || c.is_alphabetic()))
    {
        return;
    }
    let at = |index: usize| chunk.get(index).copied();
    let (mut index, mut byte) = (0, byte);
    while index < chunk.len() {
        if !chunk[index].is_alphabetic() {
            byte += chunk[index].len_utf8();
            index += 1;
            continue;
        }
        let mut end = index + 1;
        while let Some(c) = at(end) {
            let inside = c.is_alphabetic()
                || (is_apostrophe(c) && at(end + 1).is_some_and(char::is_alphabetic));
            if !inside {
                break;
            }
            end += 1;
        }
        let word = &chunk[index..end];
        let before = index.checked_sub(1).and_then(at);
        let before_that = index.checked_sub(2).and_then(at);
        let (after, after_that) = (at(end), at(end + 1));
        let code_before = before.is_some_and(|c| c.is_numeric() || "_.$#&<={".contains(c))
            || (before == Some(':') && before_that == Some(':'));
        let code_after = after.is_some_and(|c| c.is_numeric() || "_(<=[}".contains(c))
            || (after == Some('.') && after_that.is_some_and(char::is_alphanumeric))
            || (after == Some(':') && after_that == Some(':'));
        let shouted = !word.iter().any(|c| c.is_lowercase());
        if (2..=LONGEST_WORD).contains(&word.len()) && !code_before && !code_after && !shouted {
            found.push(Word {
                range: start + index..start + end,
                byte,
                // Dictionaries have the apostrophe of a typewriter.
                text: word
                    .iter()
                    .map(|c| if is_apostrophe(*c) { '\'' } else { *c })
                    .collect(),
            });
        }
        byte += word.iter().map(|c| c.len_utf8()).sum::<usize>();
        index = end;
    }
}

/// Where spelling is checked in text with highlighted syntax.
pub(crate) struct Places {
    /// The kinds of syntax to check in, if not in all.
    only_in: Option<Vec<Highlight>>,
    not_in: Vec<Highlight>,
}

impl Places {
    fn kinds(value: &Value) -> Vec<Highlight> {
        let Value::List(names) = value else {
            return Vec::new();
        };
        names
            .iter()
            .filter_map(Value::as_name)
            .flat_map(|name| {
                // A name stands for the kinds named after it as well:
                // `heading` for `heading-2` and the other levels.
                Highlight::ALL.into_iter().filter(move |kind| {
                    kind.name()
                        .strip_prefix(name)
                        .is_some_and(|rest| rest.is_empty() || rest.starts_with('-'))
                })
            })
            .collect()
    }

    /// Whether the word that starts at a byte is to be checked. Where the
    /// words to check are of some kinds of syntax only and nothing says
    /// what the syntax is, none is checked: that is code with no grammar.
    pub(crate) fn has(&self, highlights: Option<&Highlights>, byte: usize) -> bool {
        let kind = highlights.and_then(|found| found.get(byte));
        match &self.only_in {
            Some(kinds) => kind.is_some_and(|kind| kinds.contains(&kind)),
            None => kind.is_none_or(|kind| !self.not_in.contains(&kind)),
        }
    }
}

/// The parts of a word written as `oneTwoThree` or `HTMLParser`.
fn camel_parts(word: &str) -> Vec<&str> {
    let chars: Vec<(usize, char)> = word.char_indices().collect();
    let mut parts = Vec::new();
    let mut from = 0;
    for index in 1..chars.len() {
        let (previous, (at, c)) = (chars[index - 1].1, chars[index]);
        let next_is_lower = chars.get(index + 1).is_some_and(|(_, c)| c.is_lowercase());
        if c.is_uppercase()
            && (previous.is_lowercase() || (previous.is_uppercase() && next_is_lower))
        {
            parts.push(&word[from..at]);
            from = at;
        }
    }
    parts.push(&word[from..]);
    parts
}

/// Whether one of the dictionaries knows a word, or each of its parts if
/// it is several words written as one with capitals between them.
fn known(dictionaries: &[&Dictionary], word: &str) -> bool {
    let whole = |word: &str| dictionaries.iter().any(|dictionary| dictionary.check(word));
    if whole(word) {
        return true;
    }
    let parts = camel_parts(word);
    parts.len() > 1
        && parts.iter().all(|part| {
            part.chars().count() < 2 || !part.chars().any(char::is_lowercase) || whole(part)
        })
}

/// Whether a word is among words that were accepted. A word accepted in
/// small letters is accepted with a capital too, as at the start of a
/// sentence.
fn accepted(words: &HashSet<String>, word: &str) -> bool {
    words.contains(word)
        || (word.chars().next().is_some_and(char::is_uppercase)
            && words.contains(&word.to_lowercase()))
}

enum Request {
    /// Which of the words are right.
    Check {
        set: usize,
        epoch: u64,
        languages: Vec<String>,
        path: Vec<PathBuf>,
        words: Vec<String>,
        scan: Option<u64>,
    },
    /// What a word may have been meant to be.
    Suggest {
        languages: Vec<String>,
        path: Vec<PathBuf>,
        word: String,
        promise: u64,
    },
    /// The dictionaries are to be read again when next needed.
    Forget,
}

enum Answer {
    Verdicts {
        set: usize,
        epoch: u64,
        words: Vec<(String, bool)>,
        scan: Option<u64>,
    },
    Suggestions {
        promise: u64,
        words: Vec<String>,
    },
    /// Something for the message log.
    Note(String),
}

/// The text of a dictionary file. Those not in UTF-8 are taken to be in
/// Latin-1, which is what the others mostly are.
fn decode(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap_or_else(|error| {
        error
            .as_bytes()
            .iter()
            .map(|byte| char::from(*byte))
            .collect()
    })
}

/// The names of the dictionaries in some directories, sorted.
pub(crate) fn dictionaries_in(path: &[PathBuf]) -> Vec<String> {
    let mut names: Vec<String> = path
        .iter()
        .filter_map(|directory| std::fs::read_dir(directory).ok())
        .flatten()
        .filter_map(|entry| {
            let file = entry.ok()?.path();
            let name = file.file_stem()?.to_str()?.to_owned();
            (file.extension()? == "aff" && file.with_extension("dic").exists()).then_some(name)
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Where dictionaries are when nothing says: where `DICPATH` says, and
/// where the system keeps data.
pub(crate) fn default_path() -> Vec<PathBuf> {
    let var = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
    let mut path: Vec<PathBuf> = var("DICPATH")
        .map(|value| std::env::split_paths(&value).collect())
        .unwrap_or_default();
    let mut data: Vec<PathBuf> = Vec::new();
    match var("XDG_DATA_HOME") {
        Some(home) => data.push(home.into()),
        None => data.extend(var("HOME").map(|home| PathBuf::from(home).join(".local/share"))),
    }
    match var("XDG_DATA_DIRS") {
        Some(dirs) => data.extend(std::env::split_paths(&dirs)),
        None => data.extend(["/usr/local/share".into(), "/usr/share".into()]),
    }
    for directory in data {
        path.push(directory.join("hunspell"));
        path.push(directory.join("myspell/dicts"));
    }
    path
}

/// The thread that has the dictionaries.
struct Worker {
    /// By language. `None` for one that was looked for and not found.
    dictionaries: HashMap<String, Option<Dictionary>>,
    answers: mpsc::Sender<Answer>,
    waker: Waker,
}

impl Worker {
    fn answer(&self, answer: Answer) {
        if self.answers.send(answer).is_ok() {
            (self.waker)();
        }
    }

    fn load(&mut self, languages: &[String], path: &[PathBuf]) {
        for language in languages {
            if self.dictionaries.contains_key(language) {
                continue;
            }
            let files = path.iter().find_map(|directory| {
                let aff = std::fs::read(directory.join(format!("{language}.aff"))).ok()?;
                let dic = std::fs::read(directory.join(format!("{language}.dic"))).ok()?;
                Some((decode(aff), decode(dic)))
            });
            let dictionary = match files {
                Some((aff, dic)) => match Dictionary::new(&aff, &dic) {
                    Ok(dictionary) => Some(dictionary),
                    Err(error) => {
                        self.answer(Answer::Note(format!(
                            "Spelling: the dictionary {language} cannot be read: {error}"
                        )));
                        None
                    }
                },
                None => {
                    // Where there are no dictionaries at all, spelling is
                    // not set up, and nothing is said about it.
                    if !dictionaries_in(path).is_empty() {
                        self.answer(Answer::Note(format!(
                            "Spelling: no dictionary {language}.aff and {language}.dic was found"
                        )));
                    }
                    None
                }
            };
            self.dictionaries.insert(language.clone(), dictionary);
        }
    }

    fn loaded(&self, languages: &[String]) -> Vec<&Dictionary> {
        languages
            .iter()
            .filter_map(|language| self.dictionaries.get(language)?.as_ref())
            .collect()
    }

    fn run(mut self, requests: mpsc::Receiver<Request>) {
        for request in requests {
            match request {
                Request::Check {
                    set,
                    epoch,
                    languages,
                    path,
                    words,
                    scan,
                } => {
                    self.load(&languages, &path);
                    let dictionaries = self.loaded(&languages);
                    // With no dictionary there is nothing to say a word is
                    // wrong.
                    let words = words
                        .into_iter()
                        .map(|word| {
                            let right = dictionaries.is_empty() || known(&dictionaries, &word);
                            (word, right)
                        })
                        .collect();
                    self.answer(Answer::Verdicts {
                        set,
                        epoch,
                        words,
                        scan,
                    });
                }
                Request::Suggest {
                    languages,
                    path,
                    word,
                    promise,
                } => {
                    self.load(&languages, &path);
                    let lists: Vec<Vec<String>> = self
                        .loaded(&languages)
                        .iter()
                        .map(|dictionary| {
                            let mut list = Vec::new();
                            dictionary.suggest(&word, &mut list);
                            list
                        })
                        .collect();
                    // The best of each language first, then the next best.
                    let longest = lists.iter().map(Vec::len).max().unwrap_or(0);
                    let mut words: Vec<String> = Vec::new();
                    for rank in 0..longest {
                        for list in &lists {
                            if let Some(word) = list.get(rank).filter(|word| !words.contains(word))
                            {
                                words.push(word.clone());
                            }
                        }
                    }
                    self.answer(Answer::Suggestions { promise, words });
                }
                Request::Forget => self.dictionaries.clear(),
            }
        }
    }
}

/// What is known of the words of the buffers that have one list of
/// languages.
struct LanguageSet {
    languages: Vec<String>,
    /// Whether a word is right. `None` for a word that was asked about and
    /// not answered yet.
    verdicts: HashMap<String, Option<bool>>,
}

/// Words that were looked for in a buffer, until it is known which of them
/// are wrong.
struct Scan {
    set: usize,
    buffer: BufferId,
    words: Vec<Word>,
}

pub(crate) struct SpellState {
    sets: Vec<LanguageSet>,
    /// Words that are right whatever the dictionaries say.
    accepted: HashSet<String>,
    /// And those that are right in one buffer.
    local: HashMap<BufferId, HashSet<String>>,
    /// Words that a drawing found nothing known of, with their languages.
    unknown: RefCell<Vec<(Vec<String>, String)>>,
    scans: HashMap<u64, Scan>,
    /// Goes up when what was known is thrown away, so that answers to
    /// earlier questions are too.
    epoch: u64,
    requests: Option<mpsc::Sender<Request>>,
    sender: mpsc::Sender<Answer>,
    answers: mpsc::Receiver<Answer>,
}

impl Default for SpellState {
    fn default() -> Self {
        let (sender, answers) = mpsc::channel();
        Self {
            sets: Vec::new(),
            accepted: HashSet::new(),
            local: HashMap::new(),
            unknown: RefCell::new(Vec::new()),
            scans: HashMap::new(),
            epoch: 0,
            requests: None,
            sender,
            answers,
        }
    }
}

impl SpellState {
    fn is_accepted(&self, buffer: BufferId, word: &str) -> bool {
        accepted(&self.accepted, word)
            || self
                .local
                .get(&buffer)
                .is_some_and(|words| accepted(words, word))
    }

    /// Whether a word of a buffer is wrong, as far as is known now. A word
    /// that nothing is known of is not, and is noted to be asked about.
    pub(crate) fn wrong(&self, languages: &[String], buffer: BufferId, word: &str) -> bool {
        if self.is_accepted(buffer, word) {
            return false;
        }
        let verdict = self
            .sets
            .iter()
            .find(|set| set.languages == languages)
            .and_then(|set| set.verdicts.get(word));
        match verdict {
            Some(verdict) => *verdict == Some(false),
            None => {
                self.unknown
                    .borrow_mut()
                    .push((languages.to_vec(), word.to_owned()));
                false
            }
        }
    }

    fn set(&mut self, languages: &[String]) -> usize {
        if let Some(index) = self.sets.iter().position(|set| set.languages == languages) {
            return index;
        }
        self.sets.push(LanguageSet {
            languages: languages.to_vec(),
            verdicts: HashMap::new(),
        });
        self.sets.len() - 1
    }
}

fn strings(value: Option<Value>) -> Vec<String> {
    match value {
        Some(Value::List(items)) => items
            .iter()
            .filter_map(|item| item.as_name().map(str::to_owned))
            .collect(),
        Some(Value::Str(one)) => vec![one],
        _ => Vec::new(),
    }
}

impl Editor {
    /// The languages that a buffer is written in.
    pub(crate) fn spell_languages(&self, buffer: BufferId) -> Vec<String> {
        strings(self.option_for(buffer, LANGUAGES))
    }

    /// Where in a buffer's syntax its spelling is checked.
    pub(crate) fn spell_places(&self, buffer: BufferId) -> Places {
        let only_in = match self.option_for(buffer, ONLY_IN) {
            Some(value @ Value::List(_)) => Some(Places::kinds(&value)),
            _ => None,
        };
        let not_in = self
            .option_for(buffer, NOT_IN)
            .map(|value| Places::kinds(&value))
            .unwrap_or_default();
        Places { only_in, not_in }
    }

    /// The directories that dictionaries are looked for in.
    pub fn spell_path(&self) -> Vec<PathBuf> {
        let path = strings(self.option(DICTIONARY_PATH).ok());
        if path.is_empty() {
            default_path()
        } else {
            path.iter()
                .map(|directory| crate::files::expand_path(directory, &self.default_directory))
                .collect()
        }
    }

    fn spell_request(&mut self, request: Request) {
        let requests = match &self.spell.requests {
            Some(requests) => requests.clone(),
            None => {
                let (requests, received) = mpsc::channel();
                let worker = Worker {
                    dictionaries: HashMap::new(),
                    answers: self.spell.sender.clone(),
                    waker: self.waker.clone(),
                };
                let started = std::thread::Builder::new()
                    .name("spelling".to_owned())
                    .spawn(move || worker.run(received));
                if started.is_err() {
                    return;
                }
                self.spell.requests = Some(requests.clone());
                requests
            }
        };
        let _ = requests.send(request);
    }

    /// Asks about the words that the last drawing found nothing known of.
    pub(crate) fn spell_flush(&mut self) {
        let unknown = std::mem::take(&mut *self.spell.unknown.borrow_mut());
        if unknown.is_empty() {
            return;
        }
        let mut asked: Vec<(usize, Vec<String>)> = Vec::new();
        for (languages, word) in unknown {
            let set = self.spell.set(&languages);
            if self.spell.sets[set].verdicts.contains_key(&word) {
                continue;
            }
            self.spell.sets[set].verdicts.insert(word.clone(), None);
            match asked.iter_mut().find(|(index, _)| *index == set) {
                Some((_, words)) => words.push(word),
                None => asked.push((set, vec![word])),
            }
        }
        let path = self.spell_path();
        for (set, words) in asked {
            self.spell_request(Request::Check {
                set,
                epoch: self.spell.epoch,
                languages: self.spell.sets[set].languages.clone(),
                path: path.clone(),
                words,
                scan: None,
            });
        }
    }

    /// Takes in what the thread with the dictionaries has found.
    pub(crate) fn spell_poll(&mut self) {
        while let Ok(answer) = self.spell.answers.try_recv() {
            match answer {
                Answer::Verdicts {
                    set,
                    epoch,
                    words,
                    scan,
                } => {
                    if epoch == self.spell.epoch
                        && let Some(set) = self.spell.sets.get_mut(set)
                    {
                        for (word, right) in words {
                            set.verdicts.insert(word, Some(right));
                        }
                    }
                    if let Some(promise) = scan {
                        self.spell_scanned(promise);
                    }
                }
                Answer::Suggestions { promise, words } => {
                    let words = words.into_iter().map(Value::Str).collect();
                    self.resolve_promise(promise, Value::List(words));
                }
                Answer::Note(text) => self.log(&text),
            }
        }
    }

    fn spell_scanned(&mut self, promise: u64) {
        let Some(scan) = self.spell.scans.remove(&promise) else {
            return;
        };
        let verdicts = self.spell.sets.get(scan.set).map(|set| &set.verdicts);
        let wrong = scan
            .words
            .iter()
            .filter(|word| {
                !self.spell.is_accepted(scan.buffer, &word.text)
                    && verdicts.and_then(|known| known.get(&word.text)) == Some(&Some(false))
            })
            .map(|word| {
                Value::List(vec![
                    Value::Int(word.range.start as i64),
                    Value::Int(word.range.end as i64),
                ])
            })
            .collect();
        self.resolve_promise(promise, Value::List(wrong));
    }

    /// The words of the current buffer between two positions that are to
    /// be checked: those of the kinds of syntax that its options name. A
    /// word that a position is in counts whole.
    pub(crate) fn spell_words(&mut self, range: Range<usize>) -> Vec<Word> {
        self.buffer_mut().update_syntax();
        let id = self.window().buffer;
        let places = self.spell_places(id);
        let buffer = self.buffer();
        let text = buffer.text();
        // Back to the start of the text without a space that the range
        // starts in, since what a word is depends on what it is glued to.
        let back = text
            .chars_before(range.start)
            .take(LONGEST_CHUNK)
            .take_while(|c| !c.is_whitespace())
            .count();
        let start = range.start - back;
        let end = range.end.clamp(start, text.len());
        let byte = text.char_to_byte(start);
        let found = words(text.chars_from(start), start, byte, end - start);
        let last = found
            .last()
            .map_or(byte, |word| word.byte + word.text.len());
        let highlights = buffer.highlights(byte..last.min(text.len_bytes()));
        found
            .into_iter()
            .filter(|word| word.range.end > range.start && word.range.start < range.end)
            .filter(|word| places.has(highlights.as_ref(), word.byte))
            .collect()
    }

    /// A promise of the misspelled words of the current buffer between
    /// two positions: a list of lists of where one starts and ends.
    pub fn spell_scan(&mut self, range: Range<usize>) -> u64 {
        let promise = self.new_promise();
        self.await_program(promise);
        let buffer = self.window().buffer;
        let languages = self.spell_languages(buffer);
        let found = self.spell_words(range);
        let set = self.spell.set(&languages);
        let mut ask: Vec<String> = Vec::new();
        for word in &found {
            let verdicts = &mut self.spell.sets[set].verdicts;
            if !verdicts.contains_key(&word.text) {
                verdicts.insert(word.text.clone(), None);
                ask.push(word.text.clone());
            }
        }
        self.spell.scans.insert(
            promise,
            Scan {
                set,
                buffer,
                words: found,
            },
        );
        // Asked even when there is nothing to ask, so that the answer
        // comes after those to what was asked before.
        let path = self.spell_path();
        self.spell_request(Request::Check {
            set,
            epoch: self.spell.epoch,
            languages,
            path,
            words: ask,
            scan: Some(promise),
        });
        promise
    }

    /// A promise of what a word may have been meant to be, in the
    /// languages given: a list of words, the likeliest first.
    pub fn spell_suggest(&mut self, word: &str, languages: Vec<String>) -> u64 {
        let promise = self.new_promise();
        self.await_program(promise);
        let path = self.spell_path();
        self.spell_request(Request::Suggest {
            languages,
            path,
            word: word.to_owned(),
            promise,
        });
        promise
    }

    /// Takes a word to be right from now on, whatever the dictionaries
    /// say: everywhere, or in the current buffer only.
    pub fn spell_accept(&mut self, word: &str, local: bool) {
        if local {
            let buffer = self.window().buffer;
            self.spell
                .local
                .entry(buffer)
                .or_default()
                .insert(word.to_owned());
        } else {
            self.spell.accepted.insert(word.to_owned());
        }
    }

    /// Undoes [`Editor::spell_accept`] for a word, wherever it was
    /// accepted.
    pub fn spell_forget(&mut self, word: &str) {
        self.spell.accepted.remove(word);
        let buffer = self.window().buffer;
        if let Some(words) = self.spell.local.get_mut(&buffer) {
            words.remove(word);
        }
    }

    /// Whether a word was accepted, everywhere or in the current buffer.
    pub fn spell_accepted(&self, word: &str) -> bool {
        self.spell.is_accepted(self.window().buffer, word)
    }

    /// Forgets what is known of words and has the dictionaries read again
    /// when next needed, as after one was installed.
    pub fn spell_reload(&mut self) {
        self.spell.epoch += 1;
        self.spell.sets.clear();
        self.spell_request(Request::Forget);
    }

    /// The names of the dictionaries there are.
    pub fn spell_dictionaries(&self) -> Vec<String> {
        dictionaries_in(&self.spell_path())
    }

    /// The kind of syntax of the char at a position of the current buffer,
    /// as the name of the face it is drawn in.
    pub fn syntax_at(&mut self, pos: usize) -> Option<&'static str> {
        self.buffer_mut().update_syntax();
        let buffer = self.buffer();
        let byte = buffer.text().char_to_byte(pos.min(buffer.len()));
        let highlights = buffer.highlights(byte..byte + 1)?;
        Some(highlights.get(byte)?.name())
    }

    pub(crate) fn spell_buffer_killed(&mut self, buffer: BufferId) {
        self.spell.local.remove(&buffer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(text: &str) -> Vec<String> {
        words(text.chars(), 0, 0, text.chars().count())
            .into_iter()
            .map(|word| word.text)
            .collect()
    }

    #[test]
    fn a_kind_of_syntax_stands_for_those_named_after_it() {
        let names = |names: &[&str]| {
            Value::List(
                names
                    .iter()
                    .map(|name| Value::Str((*name).into()))
                    .collect(),
            )
        };
        let kinds = Places::kinds(&names(&["code", "heading"]));
        assert_eq!(kinds.len(), 9, "{kinds:?}");
        assert!(kinds.contains(&Highlight::Code));
        assert!(kinds.contains(&Highlight::Heading));
        assert!(kinds.contains(&Highlight::Heading8));
        assert_eq!(
            Places::kinds(&names(&["heading-2", "head"])),
            vec![Highlight::Heading2]
        );
    }

    #[test]
    fn words_are_letters_with_apostrophes_between_them() {
        assert_eq!(
            texts("It isn't 'quoted', no."),
            ["It", "isn't", "quoted", "no"]
        );
        assert_eq!(texts("don\u{2019}t"), ["don't"], "as dictionaries write it");
        assert_eq!(texts("smörgåsbord är gott"), ["smörgåsbord", "är", "gott"]);
        assert_eq!(texts("a I x"), [] as [&str; 0], "a letter is not a word");
        assert_eq!(
            texts("well-known (see below)"),
            ["well", "known", "see", "below"]
        );
    }

    #[test]
    fn what_only_looks_like_words_is_left_out() {
        let none: [&str; 0] = [];
        assert_eq!(texts("foo_bar x86 utf8 2nd"), none);
        assert_eq!(
            texts("see https://example.org/page and me@example.org"),
            ["see", "and"]
        );
        assert_eq!(texts("in src/main.rs or ~/notes"), ["in", "or"]);
        assert_eq!(
            texts("call buffer.text() or Vec<String> or std::fmt"),
            ["call", "or", "or"]
        );
        assert_eq!(
            texts("pass --max-jobs or -v, set $HOME, #include"),
            ["pass", "or", "set"]
        );
        assert_eq!(texts("the TTY and GPU of NASA"), ["the", "and", "of"]);
        assert_eq!(texts("README.md, key=value, {name}"), none);
        assert_eq!(
            texts("The end. Then more..."),
            ["The", "end", "Then", "more"]
        );
    }

    #[test]
    fn a_word_is_where_it_is() {
        let found = words("é isn't så".chars(), 10, 100, 10);
        let places: Vec<(Range<usize>, usize)> = found
            .iter()
            .map(|word| (word.range.clone(), word.byte))
            .collect();
        assert_eq!(places, [(12..17, 103), (18..20, 109)]);
    }

    #[test]
    fn the_last_word_is_read_to_its_end() {
        let found = words("one two three".chars(), 0, 0, 5);
        let texts: Vec<&str> = found.iter().map(|word| word.text.as_str()).collect();
        assert_eq!(texts, ["one", "two"]);
    }

    #[test]
    fn words_joined_by_capitals_come_apart() {
        assert_eq!(camel_parts("oneTwoThree"), ["one", "Two", "Three"]);
        assert_eq!(camel_parts("HTMLParser"), ["HTML", "Parser"]);
        assert_eq!(camel_parts("Plain"), ["Plain"]);
        assert_eq!(camel_parts("Överåt"), ["Överåt"]);
    }

    fn dictionary() -> Dictionary {
        let aff = "SET UTF-8\nTRY esianrtolcdugmphbyfvkwz\nSFX S Y 1\nSFX S 0 s .\n";
        let dic = "4\nbuffer/S\nwindow/S\nParser\nmark\n";
        Dictionary::new(aff, dic).expect("a dictionary")
    }

    #[test]
    fn a_word_is_right_if_a_dictionary_knows_it_or_its_parts() {
        let dictionary = dictionary();
        let dictionaries = [&dictionary];
        assert!(known(&dictionaries, "buffers"));
        assert!(known(&dictionaries, "Window"), "as a sentence starts");
        assert!(!known(&dictionaries, "bufer"));
        assert!(known(&dictionaries, "windowBuffer"));
        assert!(
            known(&dictionaries, "HTMLParser"),
            "capitals are not looked up"
        );
        assert!(!known(&dictionaries, "windowBufer"));
    }

    #[test]
    fn an_accepted_word_is_right_with_a_capital_too() {
        let words: HashSet<String> = ["amacs".to_owned(), "Albin".to_owned()].into();
        assert!(accepted(&words, "amacs") && accepted(&words, "Amacs"));
        assert!(accepted(&words, "Albin") && !accepted(&words, "albin"));
    }

    #[test]
    fn a_dictionary_not_in_utf8_is_read_as_latin_1() {
        assert_eq!(decode(b"ma\xe7\xe3".to_vec()), "maçã");
        assert_eq!(decode("maçã".as_bytes().to_vec()), "maçã");
    }
}
