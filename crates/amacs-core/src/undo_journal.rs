//! The undo history of a file, on disk.
//!
//! An [`UndoTree`] that keeps a log says what was done to it as
//! [`Record`]s. They are added to the end of a file in the state
//! directory, the journal of the visited file, a little after they are
//! made. The journal read back makes the same tree, so the history is
//! there again when the file is next visited, and what was typed and not
//! saved before a crash or a power failure is in it.
//!
//! The file starts with a line that names the format. Then come the
//! records, each as the length of its bytes, a check of them, and the
//! bytes. A record that is cut short or fails its check ends the journal:
//! that is what a power failure leaves, and what came before it is good.
//!
//! The journal is of a file as it is on disk: its records give the hash
//! of the text in the saved state. A file that something else has changed
//! has another hash, and its journal is then of no use and is started anew.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use amacs_text::{Edit, Record, Transaction, UndoTree};

use crate::file_system::{FileSystem, Local};

const MAGIC: &[u8] = b"amacs undo 1\n";

/// A journal is written anew, with the tree as it is and not how it came
/// to be, once it is twice the size it had when that was last done, but
/// not while it is smaller than this.
const COMPACT_ABOVE: u64 = 256 << 10;

/// A journal above this size is not read: the history starts anew.
pub(crate) const READ_LIMIT: u64 = 64 << 20;

/// The journal of one buffer's file.
#[derive(Debug)]
pub(crate) struct UndoJournal {
    path: PathBuf,
    /// The file does not hold the tree: the next write is of all of it.
    rewrite: bool,
    /// The size of the file after this journal last wrote to it. Another
    /// size means that something else wrote to it too.
    len: u64,
    compact_at: u64,
}

impl UndoJournal {
    /// A journal at `path` that is yet to be written.
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            rewrite: true,
            len: 0,
            compact_at: COMPACT_ABOVE,
        }
    }

    /// Reads the journal at `path`, which further records are then added
    /// to. Returns it with its records, or `None` if there is nothing to
    /// go on with: no file, another format, or a file above `limit`.
    pub(crate) fn open(path: PathBuf, limit: u64) -> Option<(Self, Vec<Record>)> {
        if fs::metadata(&path).ok()?.len() > limit {
            return None;
        }
        let bytes = fs::read(&path).ok()?;
        let (records, good) = decode_journal(&bytes)?;
        if good < bytes.len() {
            // What a power failure cut short must not be in front of what
            // is written next.
            OpenOptions::new()
                .write(true)
                .open(&path)
                .and_then(|file| file.set_len(good as u64))
                .ok()?;
        }
        let len = good as u64;
        let journal = Self {
            path,
            rewrite: false,
            len,
            compact_at: COMPACT_ABOVE.max(len.saturating_mul(2)),
        };
        Some((journal, records))
    }

    /// The journal goes to another file from now on, as when the buffer's
    /// file gets another name.
    pub(crate) fn move_to(&mut self, path: PathBuf) {
        if path != self.path {
            self.path = path;
            self.rewrite = true;
        }
    }

    /// Writes what the tree has logged since the last time, and waits
    /// until it is on the disk.
    pub(crate) fn write(&mut self, tree: &mut UndoTree) -> io::Result<()> {
        let log = tree.take_log();
        if log.is_empty() && !self.rewrite {
            return Ok(());
        }
        // A base in the log is a new tree: what was before it is gone.
        let base = log
            .iter()
            .rposition(|record| matches!(record, Record::Base { .. }));
        let outcome = if self.rewrite || self.len >= self.compact_at || self.changed_under_us() {
            self.write_anew(&tree.dump())
        } else if let Some(base) = base {
            self.write_anew(&log[base..])
        } else {
            self.append(&log)
        };
        // The log is taken and cannot be written again: after a failure
        // the next write is of the whole tree.
        self.rewrite = outcome.is_err();
        outcome
    }

    /// Whether the file is not as this journal left it: it is gone, or
    /// another editor has the same file and wrote its own history.
    fn changed_under_us(&self) -> bool {
        fs::metadata(&self.path).map_or(true, |meta| meta.len() != self.len)
    }

    fn write_anew(&mut self, records: &[Record]) -> io::Result<()> {
        let mut bytes = MAGIC.to_vec();
        for record in records {
            frame(record, &mut bytes);
        }
        if let Some(dir) = self.path.parent() {
            // The history holds text that was deleted: it is for the user
            // only, as auto-save data is.
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)?;
        }
        Local.write_atomically(&self.path, &mut |out| out.write_all(&bytes))?;
        self.len = bytes.len() as u64;
        self.compact_at = COMPACT_ABOVE.max(self.len.saturating_mul(2));
        Ok(())
    }

    fn append(&mut self, records: &[Record]) -> io::Result<()> {
        let mut bytes = Vec::new();
        for record in records {
            frame(record, &mut bytes);
        }
        let mut file: File = OpenOptions::new().append(true).open(&self.path)?;
        file.write_all(&bytes)?;
        file.sync_data()?;
        self.len += bytes.len() as u64;
        Ok(())
    }
}

/// The file name of the journal of a visited file, in the directory for
/// journals: the path with `/` written as `!`, and `!` doubled so that no
/// two files share one.
pub(crate) fn journal_name(file: &Path) -> String {
    file.to_string_lossy().replace('!', "!!").replace('/', "!")
}

// The format

/// FNV-1a, folded to four bytes: against chance, which is what a torn
/// write is.
fn check(bytes: &[u8]) -> u32 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    (hash ^ (hash >> 32)) as u32
}

/// Adds a record to `out` as its length, its check and its bytes.
fn frame(record: &Record, out: &mut Vec<u8>) {
    let mut payload = Vec::new();
    encode(record, &mut payload);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&check(&payload).to_le_bytes());
    out.extend_from_slice(&payload);
}

/// The records of a journal and how many of its bytes they take, or
/// `None` if it is not a journal of this format. The records end where
/// one is cut short, fails its check or makes no sense.
fn decode_journal(bytes: &[u8]) -> Option<(Vec<Record>, usize)> {
    let mut rest = bytes.strip_prefix(MAGIC)?;
    let mut records = Vec::new();
    let mut good = MAGIC.len();
    while let Some((header, after)) = rest.split_first_chunk::<8>() {
        let len = u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
        let sum = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
        let Some(payload) = after.get(..len) else {
            break;
        };
        if check(payload) != sum {
            break;
        }
        let Some(record) = decode(payload) else {
            break;
        };
        records.push(record);
        good += 8 + len;
        rest = &after[len..];
    }
    Some((records, good))
}

fn put_number(number: u64, out: &mut Vec<u8>) {
    let mut number = number;
    while number >= 0x80 {
        out.push((number as u8 & 0x7f) | 0x80);
        number >>= 7;
    }
    out.push(number as u8);
}

fn put_text(text: &str, out: &mut Vec<u8>) {
    put_number(text.len() as u64, out);
    out.extend_from_slice(text.as_bytes());
}

fn put_edits(edits: &[Edit], out: &mut Vec<u8>) {
    put_number(edits.len() as u64, out);
    for edit in edits {
        put_number(edit.pos as u64, out);
        put_text(&edit.deleted, out);
        put_text(&edit.inserted, out);
    }
}

fn encode(record: &Record, out: &mut Vec<u8>) {
    match record {
        Record::Base { hash } => {
            out.push(0);
            out.extend_from_slice(&hash.to_le_bytes());
        }
        Record::Push { parent, txn } => {
            out.push(1);
            put_number(*parent as u64, out);
            put_number(txn.point_before as u64, out);
            put_number(txn.point_after as u64, out);
            put_edits(&txn.edits, out);
        }
        Record::Amend { edits, point_after } => {
            out.push(2);
            put_number(*point_after as u64, out);
            put_edits(edits, out);
        }
        Record::Choose { child } => {
            out.push(3);
            put_number(*child as u64, out);
        }
        Record::Goto { state } => {
            out.push(4);
            put_number(*state as u64, out);
        }
        Record::Saved { hash } => {
            out.push(5);
            out.extend_from_slice(&hash.to_le_bytes());
        }
        Record::Unsaved => out.push(6),
    }
}

/// Reads the parts of a record from its bytes.
struct Reader<'a> {
    rest: &'a [u8],
}

impl Reader<'_> {
    fn byte(&mut self) -> Option<u8> {
        let (first, rest) = self.rest.split_first()?;
        self.rest = rest;
        Some(*first)
    }

    fn number(&mut self) -> Option<usize> {
        let mut number: u64 = 0;
        for shift in (0..64).step_by(7) {
            let byte = self.byte()?;
            number |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return usize::try_from(number).ok();
            }
        }
        None
    }

    fn hash(&mut self) -> Option<u64> {
        let (bytes, rest) = self.rest.split_first_chunk::<8>()?;
        self.rest = rest;
        Some(u64::from_le_bytes(*bytes))
    }

    fn text(&mut self) -> Option<String> {
        let len = self.number()?;
        let bytes = self.rest.get(..len)?;
        self.rest = &self.rest[len..];
        String::from_utf8(bytes.to_vec()).ok()
    }

    fn edits(&mut self) -> Option<Vec<Edit>> {
        let count = self.number()?;
        let mut edits = Vec::new();
        for _ in 0..count {
            edits.push(Edit {
                pos: self.number()?,
                deleted: self.text()?,
                inserted: self.text()?,
            });
        }
        Some(edits)
    }
}

fn decode(payload: &[u8]) -> Option<Record> {
    let mut reader = Reader { rest: payload };
    let record = match reader.byte()? {
        0 => Record::Base {
            hash: reader.hash()?,
        },
        1 => {
            let parent = reader.number()?;
            let point_before = reader.number()?;
            let point_after = reader.number()?;
            Record::Push {
                parent,
                txn: Transaction {
                    edits: reader.edits()?,
                    point_before,
                    point_after,
                },
            }
        }
        2 => {
            let point_after = reader.number()?;
            Record::Amend {
                edits: reader.edits()?,
                point_after,
            }
        }
        3 => Record::Choose {
            child: reader.number()?,
        },
        4 => Record::Goto {
            state: reader.number()?,
        },
        5 => Record::Saved {
            hash: reader.hash()?,
        },
        6 => Record::Unsaved,
        _ => return None,
    };
    reader.rest.is_empty().then_some(record)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(pos: usize, text: &str) -> Transaction {
        Transaction {
            edits: vec![Edit {
                pos,
                deleted: String::new(),
                inserted: text.to_owned(),
            }],
            point_before: pos,
            point_after: pos + text.chars().count(),
        }
    }

    /// A tree with a branch and a saved state, as a buffer would make it.
    fn tree() -> UndoTree {
        let mut tree = UndoTree::logged(11);
        tree.push(typed(0, "smörgås"));
        let mut more = vec![Edit {
            pos: 7,
            deleted: "x".repeat(300),
            inserted: "😀\n".to_owned(),
        }];
        assert!(tree.amend(&mut more, 9));
        tree.mark_saved(|| 12);
        tree.push(typed(9, "b"));
        tree.undo();
        tree.push(typed(9, "c"));
        tree
    }

    fn journal_in(dir: &tempfile::TempDir) -> PathBuf {
        dir.path()
            .join("undo")
            .join(journal_name(Path::new("/a/b!c")))
    }

    #[test]
    fn every_kind_of_record_is_read_as_it_was_written() {
        let mut tree = tree();
        tree.clear_saved();
        let records = tree.take_log();
        for kind in 0..7 {
            assert!(
                records.iter().any(|record| {
                    let mut bytes = Vec::new();
                    encode(record, &mut bytes);
                    bytes[0] == kind
                }),
                "no record of kind {kind}"
            );
        }
        let mut bytes = MAGIC.to_vec();
        for record in &records {
            frame(record, &mut bytes);
        }
        assert_eq!(decode_journal(&bytes), Some((records, bytes.len())));
        assert_eq!(decode_journal(b"something else"), None);
    }

    #[test]
    fn a_journal_is_added_to_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = journal_in(&dir);
        assert!(path.ends_with("undo/!a!b!!c"));
        let mut tree = tree();
        let mut journal = UndoJournal::new(path.clone());
        journal.write(&mut tree).unwrap();
        let first = fs::metadata(&path).unwrap().len();
        // Nothing new, nothing written.
        journal.write(&mut tree).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().len(), first);
        tree.push(typed(10, "d"));
        tree.undo();
        journal.write(&mut tree).unwrap();
        assert!(
            fs::metadata(&path).unwrap().len() > first,
            "it was added to"
        );

        let (mut journal, records) = UndoJournal::open(path.clone(), READ_LIMIT).unwrap();
        let mut again = UndoTree::from_records(records).unwrap();
        assert_eq!(again.dump(), tree.dump());
        assert_eq!(again.saved_hash(), Some(12));
        // And it goes on from there.
        again.push(typed(10, "e"));
        tree.push(typed(10, "e"));
        journal.write(&mut again).unwrap();
        let (_, records) = UndoJournal::open(path.clone(), READ_LIMIT).unwrap();
        assert_eq!(UndoTree::from_records(records).unwrap().dump(), tree.dump());

        assert!(UndoJournal::open(path, 10).is_none(), "too large to read");
    }

    #[test]
    fn what_a_power_failure_cut_short_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let path = journal_in(&dir);
        let mut tree = tree();
        let mut journal = UndoJournal::new(path.clone());
        journal.write(&mut tree).unwrap();
        let whole = tree.dump();
        let good = fs::metadata(&path).unwrap().len();

        // Half of the next record made it to the disk.
        tree.push(typed(10, "lost"));
        let mut bytes = Vec::new();
        frame(&tree.take_log()[0], &mut bytes);
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(&bytes[..bytes.len() / 2]).unwrap();
        drop(file);

        let (mut journal, records) = UndoJournal::open(path.clone(), READ_LIMIT).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().len(),
            good,
            "the rest is cut off"
        );
        let mut again = UndoTree::from_records(records).unwrap();
        assert_eq!(again.dump(), whole);
        // What is written next is read after it.
        again.push(typed(10, "kept"));
        journal.write(&mut again).unwrap();
        let (_, records) = UndoJournal::open(path.clone(), READ_LIMIT).unwrap();
        assert_eq!(UndoTree::from_records(records).unwrap().len(), 4);

        // A changed byte fails the check of its record.
        let mut bytes = fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        fs::write(&path, bytes).unwrap();
        let (_, records) = UndoJournal::open(path, READ_LIMIT).unwrap();
        assert_eq!(UndoTree::from_records(records).unwrap().dump(), whole);
    }

    #[test]
    fn a_journal_that_another_wrote_to_is_written_anew() {
        let dir = tempfile::tempdir().unwrap();
        let path = journal_in(&dir);
        let mut tree = tree();
        let mut journal = UndoJournal::new(path.clone());
        journal.write(&mut tree).unwrap();

        // Another editor with the same file puts its own history there.
        let mut other = UndoTree::logged(1);
        other.push(typed(0, "other"));
        UndoJournal::new(path.clone()).write(&mut other).unwrap();

        tree.push(typed(10, "d"));
        journal.write(&mut tree).unwrap();
        let (_, records) = UndoJournal::open(path.clone(), READ_LIMIT).unwrap();
        assert_eq!(UndoTree::from_records(records).unwrap().dump(), tree.dump());

        // So is one that is gone.
        fs::remove_file(&path).unwrap();
        tree.push(typed(11, "e"));
        journal.write(&mut tree).unwrap();
        let (_, records) = UndoJournal::open(path, READ_LIMIT).unwrap();
        assert_eq!(UndoTree::from_records(records).unwrap().dump(), tree.dump());
    }

    #[test]
    fn a_new_tree_and_a_long_journal_start_the_file_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = journal_in(&dir);
        let mut tree = tree();
        let mut journal = UndoJournal::new(path.clone());
        journal.write(&mut tree).unwrap();

        // The buffer was read again from its file: a new tree, whose log
        // starts with its base.
        let mut tree = UndoTree::logged(99);
        tree.push(typed(0, "new"));
        journal.write(&mut tree).unwrap();
        let (_, records) = UndoJournal::open(path.clone(), READ_LIMIT).unwrap();
        assert!(matches!(records[0], Record::Base { hash: 99 }));
        assert_eq!(records.len(), 2);

        // Going back and forth makes the journal long and the tree no
        // larger: past a size, the tree is written and not the going.
        let small = fs::metadata(&path).unwrap().len();
        for _ in 0..40_000 {
            tree.undo();
            tree.redo();
        }
        journal.write(&mut tree).unwrap();
        assert!(fs::metadata(&path).unwrap().len() > COMPACT_ABOVE);
        tree.undo();
        journal.write(&mut tree).unwrap();
        assert!(fs::metadata(&path).unwrap().len() < small + 64);
        let (_, records) = UndoJournal::open(path, READ_LIMIT).unwrap();
        assert_eq!(UndoTree::from_records(records).unwrap().dump(), tree.dump());
    }
}
