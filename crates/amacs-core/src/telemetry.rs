//! What the editor was used for and how long it took: counts and times of
//! commands, hooks, other calls into Scheme, and redisplay. Nothing of what
//! was typed or of any buffer is kept, only the names of what ran.
//!
//! The numbers of this session are in memory. With a state directory they
//! are also added to a file there, which holds the sums of all sessions.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

/// What took at least this many microseconds counts as slow: it is longer
/// than a screen takes to show one picture, so it can be noticed.
const SLOW_MICROS: u64 = 16_000;

/// The file in the state directory.
const FILE: &str = "telemetry";

/// The numbers of one thing that ran.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stat {
    /// How many times it ran.
    pub count: u64,
    /// Microseconds, of all the times together.
    pub total: u64,
    /// Microseconds of the time that took longest.
    pub max: u64,
    /// How many of the times were slow.
    pub slow: u64,
}

impl Stat {
    fn add(&mut self, other: Stat) {
        self.count += other.count;
        self.total += other.total;
        self.max = self.max.max(other.max);
        self.slow += other.slow;
    }
}

/// The numbers by kind, such as "command", and name.
type Stats = BTreeMap<String, BTreeMap<String, Stat>>;

#[derive(Debug, Default)]
pub struct Telemetry {
    /// What ran in this session.
    session: Stats,
    /// The part of it that is not in the file yet.
    unsaved: Stats,
}

impl Telemetry {
    /// Notes that something ran `count` times, which took `took` together.
    pub fn add(&mut self, kind: &str, name: &str, count: u64, took: Duration) {
        if count == 0 {
            return;
        }
        let micros = u64::try_from(took.as_micros()).unwrap_or(u64::MAX);
        let each = micros / count;
        let stat = Stat {
            count,
            total: micros,
            max: each,
            slow: if each >= SLOW_MICROS { count } else { 0 },
        };
        for stats in [&mut self.session, &mut self.unsaved] {
            match stats.get_mut(kind).and_then(|names| names.get_mut(name)) {
                Some(known) => known.add(stat),
                None => {
                    stats
                        .entry(kind.to_owned())
                        .or_default()
                        .insert(name.to_owned(), stat);
                }
            }
        }
    }

    /// The numbers of this session, as (kind, name, numbers).
    pub fn session(&self) -> Vec<(String, String, Stat)> {
        listed(&self.session)
    }

    /// The numbers of all sessions: what the file in `dir` holds and what
    /// is not in it yet.
    pub fn all(&self, dir: &Path) -> Vec<(String, String, Stat)> {
        let mut stats = read(dir);
        merge(&mut stats, &self.unsaved);
        listed(&stats)
    }

    /// Forgets the numbers of this session, and those in `dir` if one is
    /// given.
    pub fn clear(&mut self, dir: Option<&Path>) -> io::Result<()> {
        self.session.clear();
        self.unsaved.clear();
        match dir.map(|dir| fs::remove_file(dir.join(FILE))) {
            Some(Err(error)) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    /// Adds what is not in the file yet to the file in `dir`. The file is
    /// read first, so that several editors can add to the same one, and
    /// replaced atomically.
    pub fn save(&mut self, dir: &Path) -> io::Result<()> {
        if self.unsaved.is_empty() {
            return Ok(());
        }
        let mut stats = read(dir);
        merge(&mut stats, &self.unsaved);
        let mut content = String::new();
        for (kind, name, stat) in listed(&stats) {
            content.push_str(&format!(
                "{kind}\t{name}\t{}\t{}\t{}\t{}\n",
                stat.count, stat.total, stat.max, stat.slow
            ));
        }
        fs::create_dir_all(dir)?;
        let temp = dir.join(format!(".{FILE}.{}.tmp", std::process::id()));
        fs::write(&temp, content)?;
        fs::rename(&temp, dir.join(FILE))?;
        self.unsaved.clear();
        Ok(())
    }
}

fn listed(stats: &Stats) -> Vec<(String, String, Stat)> {
    stats
        .iter()
        .flat_map(|(kind, names)| {
            names
                .iter()
                .map(move |(name, stat)| (kind.clone(), name.clone(), *stat))
        })
        .collect()
}

fn merge(into: &mut Stats, from: &Stats) {
    for (kind, name, stat) in listed(from) {
        into.entry(kind)
            .or_default()
            .entry(name)
            .or_default()
            .add(stat);
    }
}

/// What the file in a directory holds. A missing file and lines that are
/// not as expected give nothing: the numbers are a convenience.
fn read(dir: &Path) -> Stats {
    let mut stats = Stats::new();
    let Ok(text) = fs::read_to_string(dir.join(FILE)) else {
        return stats;
    };
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        let [kind, name, numbers @ ..] = fields.as_slice() else {
            continue;
        };
        let numbers: Vec<u64> = numbers.iter().filter_map(|n| n.parse().ok()).collect();
        if let [count, total, max, slow] = numbers[..] {
            stats
                .entry((*kind).to_owned())
                .or_default()
                .entry((*name).to_owned())
                .or_default()
                .add(Stat {
                    count,
                    total,
                    max,
                    slow,
                });
        }
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_are_summed_by_kind_and_name() {
        let mut telemetry = Telemetry::default();
        telemetry.add("command", "next-line", 1, Duration::from_micros(40));
        telemetry.add("command", "next-line", 1, Duration::from_micros(20_000));
        telemetry.add("hook", "next-line", 1, Duration::from_micros(5));
        telemetry.add("scheme", "collect", 4, Duration::from_micros(400));
        telemetry.add("scheme", "collect", 0, Duration::from_micros(400));
        let stat = |count, total, max, slow| Stat {
            count,
            total,
            max,
            slow,
        };
        assert_eq!(
            telemetry.session(),
            [
                (
                    "command".to_owned(),
                    "next-line".to_owned(),
                    stat(2, 20_040, 20_000, 1)
                ),
                ("hook".to_owned(), "next-line".to_owned(), stat(1, 5, 5, 0)),
                (
                    "scheme".to_owned(),
                    "collect".to_owned(),
                    stat(4, 400, 100, 0)
                ),
            ]
        );
    }

    #[test]
    fn sessions_add_to_the_same_file() {
        let dir = std::env::temp_dir().join(format!("amacs-telemetry-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut first = Telemetry::default();
        let mut second = Telemetry::default();
        first.add("command", "undo", 1, Duration::from_micros(10));
        second.add("command", "undo", 1, Duration::from_micros(30));
        first.save(&dir).unwrap();
        second.save(&dir).unwrap();
        // What is saved is not added again.
        first.save(&dir).unwrap();
        first.add("command", "undo", 1, Duration::from_micros(2));

        let all = first.all(&dir);
        assert_eq!(all.len(), 1);
        assert_eq!(
            all[0].2,
            Stat {
                count: 3,
                total: 42,
                max: 30,
                slow: 0
            }
        );
        assert_eq!(first.session()[0].2.count, 2);

        first.clear(Some(&dir)).unwrap();
        assert!(first.all(&dir).is_empty());
        first.clear(Some(&dir)).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }
}
