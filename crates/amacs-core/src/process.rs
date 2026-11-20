//! Programs that run while the editor goes on: started by Scheme code, fed
//! input, read as they write, and reported on when they end.
//!
//! `run-process` (in `extend.rs`) runs a program to its end and holds what
//! is typed meanwhile, which suits a command that waits for `git`. A
//! process made here lives as long as it likes: a shell, the agent of a
//! language model, a helper on another machine. What it writes goes to its
//! filter, a Scheme procedure called on the editor's thread; when it ends
//! its sentinel is called. Nothing waits for it, so typing is never held.
//!
//! Each process has a thread that reads what it writes, one that writes
//! what it is sent and one that waits for it to end. They push onto one
//! channel and wake the editor, which takes it all in at [`Editor::poll`].
//! The channel is bounded, so a program that writes faster than the editor
//! reads is made to wait instead of filling memory.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use rustix::process::{Pid, Signal};

use crate::editor::Editor;
use crate::env::EnvState;
use crate::error::{CmdError, CmdResult};
use crate::script::{ScriptCall, ScriptFn, ScriptTarget, Value};

/// How much one read takes at most.
const CHUNK: usize = 64 * 1024;
/// How many chunks may wait for the editor before the readers are held.
const BACKLOG: usize = 256;

/// What the threads of a process report.
enum Incoming {
    /// Bytes from a program; the bool is true for its standard error.
    Output(u64, bool, Vec<u8>),
    /// A reader reached the end of what the program writes.
    Closed(u64),
    /// The program ended: its exit code, or `None` for a signal.
    Exited(u64, Option<i32>),
}

pub(crate) struct ProcessState {
    next_id: u64,
    processes: BTreeMap<u64, Process>,
    sender: SyncSender<Incoming>,
    incoming: Receiver<Incoming>,
}

impl Default for ProcessState {
    fn default() -> Self {
        let (sender, incoming) = mpsc::sync_channel(BACKLOG);
        Self {
            next_id: 0,
            processes: BTreeMap::new(),
            sender,
            incoming,
        }
    }
}

impl ProcessState {
    fn kill_all(&mut self) {
        for process in self.processes.values() {
            if process.live() {
                let _ = process.signal(Signal::KILL);
            }
        }
        self.processes.clear();
    }
}

/// However the editor ends, a panic included, its programs end with it.
impl Drop for ProcessState {
    fn drop(&mut self) {
        self.kill_all();
    }
}

struct Process {
    name: String,
    program: String,
    pid: u32,
    /// Where to send input; `None` once the input has been closed.
    input: Option<Sender<Vec<u8>>>,
    filter: Option<ScriptFn>,
    /// Where the program's standard error goes, apart from its output, for
    /// a protocol on standard output. `None` merges it into the output.
    stderr_filter: Option<ScriptFn>,
    sentinel: Option<ScriptFn>,
    /// Output not yet given to the filter: what arrived since the last
    /// poll, after the bytes of a character that the last chunk cut.
    pending: Vec<u8>,
    /// The same for standard error, when it has a filter of its own.
    pending_err: Vec<u8>,
    /// Readers that have not reached the end.
    readers: usize,
    /// The exit status once the program has ended.
    exited: Option<Option<i32>>,
    /// Whether the sentinel has been given the end.
    reported: bool,
    /// The terminal the program runs on, if it runs on one.
    pty: Option<Box<dyn MasterPty + Send>>,
}

/// A program just started, however it was started.
struct Spawned {
    pid: u32,
    /// Each reader with whether it is the program's standard error.
    readers: Vec<(bool, Box<dyn Read + Send>)>,
    writer: Option<Box<dyn Write + Send>>,
    /// Waits for the program to end and gives its exit code, or `None`
    /// for a signal.
    wait: Box<dyn FnOnce() -> Option<i32> + Send>,
    pty: Option<Box<dyn MasterPty + Send>>,
}

type Variables = [(String, Option<String>)];

fn spawn_on_pipes(
    program: &str,
    args: &[String],
    directory: Option<PathBuf>,
    variables: &Variables,
) -> std::io::Result<Spawned> {
    use std::os::unix::process::CommandExt;
    let mut command = Command::new(program);
    command.args(args).process_group(0);
    if let Some(directory) = directory {
        command.current_dir(directory);
    }
    for (name, value) in variables {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let mut readers: Vec<(bool, Box<dyn Read + Send>)> = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        readers.push((false, Box::new(stdout)));
    }
    if let Some(stderr) = child.stderr.take() {
        readers.push((true, Box::new(stderr)));
    }
    let writer = child
        .stdin
        .take()
        .map(|stdin| Box::new(stdin) as Box<dyn Write + Send>);
    Ok(Spawned {
        pid: child.id(),
        readers,
        writer,
        wait: Box::new(move || child.wait().ok().and_then(|status| status.code())),
        pty: None,
    })
}

fn spawn_on_pty(
    program: &str,
    args: &[String],
    directory: Option<PathBuf>,
    variables: &Variables,
    cols: u16,
    rows: u16,
) -> std::io::Result<Spawned> {
    let other = |error: anyhow::Error| std::io::Error::other(error.to_string());
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(other)?;
    let mut command = CommandBuilder::new(program);
    command.args(args);
    if let Some(directory) = directory {
        command.cwd(directory);
    }
    for (name, value) in variables {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        }
    }
    let mut child = pair.slave.spawn_command(command).map_err(other)?;
    // The child has the slave; this side is done with it.
    drop(pair.slave);
    let reader = pair.master.try_clone_reader().map_err(other)?;
    let writer = pair.master.take_writer().map_err(other)?;
    let pid = child.process_id().unwrap_or(0);
    Ok(Spawned {
        pid,
        readers: vec![(false, reader)],
        writer: Some(writer),
        wait: Box::new(move || {
            let status = child.wait().ok()?;
            match status.signal() {
                Some(_) => None,
                None => i32::try_from(status.exit_code()).ok(),
            }
        }),
        pty: Some(pair.master),
    })
}

/// The directory that a program is in now, where the system tells: Linux
/// does, through `/proc`. A directory that was deleted under the program
/// does not count.
pub(crate) fn process_directory(pid: u32) -> Option<PathBuf> {
    let directory = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
    directory.is_dir().then_some(directory)
}

/// Takes a pending byte buffer as text. A character cut by the end of a
/// chunk waits in the buffer for its rest, unless `all` says there is none
/// to come.
fn decode_pending(buffer: &mut Vec<u8>, all: bool) -> String {
    let bytes = std::mem::take(buffer);
    let mut text = String::new();
    let mut rest = &bytes[..];
    loop {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                text.push_str(valid);
                break;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                text.push_str(&String::from_utf8_lossy(&rest[..valid]));
                match error.error_len() {
                    Some(bad) => {
                        text.push(char::REPLACEMENT_CHARACTER);
                        rest = &rest[valid + bad..];
                    }
                    None if all => {
                        text.push(char::REPLACEMENT_CHARACTER);
                        break;
                    }
                    None => {
                        *buffer = rest[valid..].to_vec();
                        break;
                    }
                }
            }
        }
    }
    text
}

impl Process {
    fn live(&self) -> bool {
        self.exited.is_none()
    }

    fn signal(&self, signal: Signal) -> CmdResult {
        let pid = Pid::from_raw(self.pid as i32)
            .ok_or_else(|| CmdError::msg(format!("{} has no process id", self.name)))?;
        // The program was put in a group of its own, so that what it started
        // goes with it.
        rustix::process::kill_process_group(pid, signal)
            .map_err(|error| CmdError::msg(format!("cannot signal {}: {error}", self.name)))
    }
}

/// A signal by its name, as `signal-process` takes it.
fn signal_named(name: &str) -> CmdResult<Signal> {
    Ok(match name.to_ascii_lowercase().as_str() {
        "int" | "sigint" | "interrupt" => Signal::INT,
        "term" | "sigterm" | "terminate" => Signal::TERM,
        "kill" | "sigkill" => Signal::KILL,
        "hup" | "sighup" | "hangup" => Signal::HUP,
        "quit" | "sigquit" => Signal::QUIT,
        other => return Err(CmdError::msg(format!("unknown signal: {other}"))),
    })
}

impl Editor {
    /// Starts a program that goes on running. It gets its arguments, runs
    /// in `directory` (the working directory if `None`) with the
    /// environment of that directory's project and `environment` on top,
    /// in a process group of its own. With `pty`, the size of a terminal,
    /// it runs on a pseudo-terminal of that size, and its standard error
    /// comes with its output; otherwise on pipes. What it writes goes to
    /// `filter` as text; `sentinel` is called when it ends. Returns the
    /// process's number. An error if the program cannot be started.
    #[allow(clippy::too_many_arguments)]
    pub fn make_process(
        &mut self,
        name: &str,
        program: &str,
        args: &[String],
        directory: Option<PathBuf>,
        environment: &[(String, String)],
        filter: Option<ScriptFn>,
        stderr_filter: Option<ScriptFn>,
        sentinel: Option<ScriptFn>,
        pty: Option<(u16, u16)>,
    ) -> CmdResult<u64> {
        let directory = directory.unwrap_or_else(|| self.working_directory().to_owned());
        let mut variables: Vec<(String, Option<String>)> = match self.environment(&directory) {
            Some(EnvState::Ready(environment)) => environment,
            _ => Vec::new(),
        };
        variables.extend(
            environment
                .iter()
                .map(|(name, value)| (name.clone(), Some(value.clone()))),
        );
        let directory = directory.is_dir().then_some(directory);
        let spawned = match pty {
            Some((cols, rows)) => spawn_on_pty(program, args, directory, &variables, cols, rows),
            None => spawn_on_pipes(program, args, directory, &variables),
        }
        .map_err(|error| CmdError::msg(format!("cannot run {program}: {error}")))?;
        self.processes.next_id += 1;
        let id = self.processes.next_id;

        let readers = spawned.readers.len();
        for (is_stderr, output) in spawned.readers {
            let (sender, waker) = (self.processes.sender.clone(), self.waker.clone());
            std::thread::spawn(move || read_output(id, is_stderr, output, &sender, &waker));
        }

        let input = spawned.writer.map(|mut writer| {
            let (sender, receiver) = mpsc::channel::<Vec<u8>>();
            std::thread::spawn(move || {
                // The sender being dropped closes the input: `writer` goes
                // out of scope when the loop ends.
                while let Ok(bytes) = receiver.recv() {
                    if writer
                        .write_all(&bytes)
                        .and_then(|_| writer.flush())
                        .is_err()
                    {
                        break;
                    }
                }
            });
            sender
        });

        let (sender, waker) = (self.processes.sender.clone(), self.waker.clone());
        let wait = spawned.wait;
        std::thread::spawn(move || {
            let status = wait();
            if sender.send(Incoming::Exited(id, status)).is_ok() {
                waker();
            }
        });

        self.processes.processes.insert(
            id,
            Process {
                name: name.to_owned(),
                program: program.to_owned(),
                pid: spawned.pid,
                input,
                filter,
                stderr_filter,
                sentinel,
                pending: Vec::new(),
                pending_err: Vec::new(),
                readers,
                exited: None,
                reported: false,
                pty: spawned.pty,
            },
        );
        Ok(id)
    }

    fn process(&self, id: u64) -> CmdResult<&Process> {
        self.processes
            .processes
            .get(&id)
            .ok_or_else(|| CmdError::msg(format!("There is no process {id}")))
    }

    fn process_mut(&mut self, id: u64) -> CmdResult<&mut Process> {
        self.processes
            .processes
            .get_mut(&id)
            .ok_or_else(|| CmdError::msg(format!("There is no process {id}")))
    }

    /// The numbers of the processes, started and not yet deleted, oldest
    /// first.
    pub fn process_list(&self) -> Vec<u64> {
        self.processes.processes.keys().copied().collect()
    }

    pub fn process_name(&self, id: u64) -> CmdResult<&str> {
        Ok(&self.process(id)?.name)
    }

    pub fn process_program(&self, id: u64) -> CmdResult<&str> {
        Ok(&self.process(id)?.program)
    }

    pub fn process_pid(&self, id: u64) -> CmdResult<u32> {
        Ok(self.process(id)?.pid)
    }

    /// Whether the program is still running.
    pub fn process_live(&self, id: u64) -> CmdResult<bool> {
        Ok(self.process(id)?.live())
    }

    /// The exit status of a process that has ended: `Some(None)` if a
    /// signal ended it, `None` while it runs.
    pub fn process_exit_status(&self, id: u64) -> CmdResult<Option<Option<i32>>> {
        Ok(self.process(id)?.exited)
    }

    pub fn set_process_filter(&mut self, id: u64, filter: Option<ScriptFn>) -> CmdResult {
        self.process_mut(id)?.filter = filter;
        Ok(())
    }

    pub fn set_process_sentinel(&mut self, id: u64, sentinel: Option<ScriptFn>) -> CmdResult {
        self.process_mut(id)?.sentinel = sentinel;
        Ok(())
    }

    /// Sends text to the program's input. The text is handed to a thread,
    /// so a program that is slow to read does not hold the editor.
    pub fn process_send(&mut self, id: u64, text: &str) -> CmdResult {
        self.process_send_bytes(id, text.as_bytes().to_vec())
    }

    pub fn process_send_bytes(&mut self, id: u64, bytes: Vec<u8>) -> CmdResult {
        let process = self.process_mut(id)?;
        if !process.live() {
            return Err(CmdError::msg(format!("{} has ended", process.name)));
        }
        let Some(input) = &process.input else {
            return Err(CmdError::msg(format!(
                "the input of {} has been closed",
                process.name
            )));
        };
        input
            .send(bytes)
            .map_err(|_| CmdError::msg(format!("{} no longer reads its input", process.name)))
    }

    /// Whether the program runs on a pseudo-terminal.
    pub fn process_on_pty(&self, id: u64) -> CmdResult<bool> {
        Ok(self.process(id)?.pty.is_some())
    }

    /// Tells a program on a pseudo-terminal the size of its terminal.
    pub fn set_process_window_size(&mut self, id: u64, cols: u16, rows: u16) -> CmdResult {
        let process = self.process(id)?;
        let Some(pty) = &process.pty else {
            return Err(CmdError::msg(format!(
                "{} does not run on a terminal",
                process.name
            )));
        };
        pty.resize(PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| CmdError::msg(format!("cannot resize the terminal: {error}")))
    }

    /// Closes the program's input, which is the end of its file.
    pub fn process_send_eof(&mut self, id: u64) -> CmdResult {
        self.process_mut(id)?.input = None;
        Ok(())
    }

    /// Sends a signal by name (`int`, `term`, `kill`, `hup`, `quit`) to
    /// the program and what it started. The sentinel is called when the
    /// program ends.
    pub fn signal_process(&mut self, id: u64, signal: &str) -> CmdResult {
        let signal = signal_named(signal)?;
        let process = self.process(id)?;
        if !process.live() {
            return Ok(());
        }
        process.signal(signal)
    }

    /// Ends a program at once, without word to the sentinel, and forgets
    /// the process.
    pub fn delete_process(&mut self, id: u64) -> CmdResult {
        let process = self.process(id)?;
        if process.live() {
            // A program that has just ended is no error.
            let _ = process.signal(Signal::KILL);
        }
        self.processes.processes.remove(&id);
        Ok(())
    }

    /// Ends every program still running, as when the editor exits.
    pub fn kill_all_processes(&mut self) {
        self.processes.kill_all();
    }

    /// Takes in what the programs wrote and hands it to their filters, and
    /// tells the sentinels of those that ended, once all their output has
    /// been handed on.
    pub(crate) fn process_poll(&mut self) {
        let mut touched: Vec<u64> = Vec::new();
        while let Ok(incoming) = self.processes.incoming.try_recv() {
            let id = match incoming {
                Incoming::Output(id, ..) | Incoming::Closed(id) | Incoming::Exited(id, _) => id,
            };
            let Some(process) = self.processes.processes.get_mut(&id) else {
                continue;
            };
            match incoming {
                // Standard error goes to its own filter when there is one,
                // so a protocol on standard output stays whole; otherwise it
                // is merged into the output.
                Incoming::Output(_, true, bytes) if process.stderr_filter.is_some() => {
                    process.pending_err.extend_from_slice(&bytes)
                }
                Incoming::Output(_, _, bytes) => process.pending.extend_from_slice(&bytes),
                Incoming::Closed(_) => process.readers = process.readers.saturating_sub(1),
                Incoming::Exited(_, status) => process.exited = Some(status),
            }
            if !touched.contains(&id) {
                touched.push(id);
            }
        }
        let mut calls = Vec::new();
        let mut feeds: Vec<(crate::buffer::BufferId, Vec<u8>)> = Vec::new();
        let mut ended_terminals = Vec::new();
        for id in touched {
            let terminal = self.terminal_of_process(id);
            let Some(process) = self.processes.processes.get_mut(&id) else {
                continue;
            };
            let ended = process.exited.is_some() && process.readers == 0;
            if let Some(buffer) = terminal {
                // Output for a terminal goes to its screen, as bytes.
                let bytes = std::mem::take(&mut process.pending);
                if !bytes.is_empty() {
                    feeds.push((buffer, bytes));
                }
            } else {
                let text = decode_pending(&mut process.pending, ended);
                if !text.is_empty()
                    && let Some(filter) = &process.filter
                {
                    calls.push((filter.clone(), vec![Value::Int(id as i64), text.into()]));
                }
            }
            let err = decode_pending(&mut process.pending_err, ended);
            if !err.is_empty()
                && let Some(filter) = &process.stderr_filter
            {
                calls.push((filter.clone(), vec![Value::Int(id as i64), err.into()]));
            }
            if ended && !process.reported {
                process.reported = true;
                process.input = None;
                if let Some(sentinel) = &process.sentinel {
                    let status: Option<i64> = process.exited.flatten().map(i64::from);
                    calls.push((sentinel.clone(), vec![Value::Int(id as i64), status.into()]));
                }
                if let Some(buffer) = terminal {
                    ended_terminals.push((buffer, process.exited.flatten()));
                }
            }
        }
        for (buffer, bytes) in feeds {
            self.terminal_feed(buffer, &bytes);
        }
        for (buffer, status) in ended_terminals {
            self.terminal_ended(buffer, status);
        }
        for (func, args) in calls {
            let _ = self.queue_script(ScriptCall {
                target: ScriptTarget::Func(func),
                args,
                command: None,
                echo_result: false,
                frame: None,
            });
        }
    }
}

/// Reads what a program writes until it closes the stream, in chunks.
fn read_output(
    id: u64,
    is_stderr: bool,
    mut output: Box<dyn Read + Send>,
    sender: &SyncSender<Incoming>,
    waker: &crate::completion::Waker,
) {
    let mut buffer = vec![0; CHUNK];
    loop {
        match output.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if sender
                    .send(Incoming::Output(id, is_stderr, buffer[..n].to_vec()))
                    .is_err()
                {
                    return;
                }
                waker();
            }
        }
    }
    if sender.send(Incoming::Closed(id)).is_ok() {
        waker();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process_with(pending: &[u8]) -> Process {
        Process {
            name: "p".into(),
            program: "p".into(),
            pid: 1,
            input: None,
            filter: None,
            stderr_filter: None,
            sentinel: None,
            pending: pending.to_vec(),
            pending_err: Vec::new(),
            readers: 0,
            exited: None,
            reported: false,
            pty: None,
        }
    }

    #[test]
    fn a_character_cut_by_a_chunk_waits_for_its_rest() {
        let mut process = process_with("a\u{e9}".as_bytes());
        process.pending.truncate(2);
        assert_eq!(decode_pending(&mut process.pending, false), "a");
        process.pending.push("\u{e9}".as_bytes()[1]);
        process.pending.extend_from_slice(b"b");
        assert_eq!(decode_pending(&mut process.pending, false), "\u{e9}b");
    }

    #[test]
    fn bad_bytes_become_the_replacement_character() {
        let mut process = process_with(b"a\xffb\xc3");
        assert_eq!(decode_pending(&mut process.pending, false), "a\u{fffd}b");
        assert_eq!(process.pending, b"\xc3");
        assert_eq!(decode_pending(&mut process.pending, true), "\u{fffd}");
    }

    #[test]
    fn signals_are_known_by_name() {
        assert_eq!(signal_named("INT").unwrap(), Signal::INT);
        assert_eq!(signal_named("sigterm").unwrap(), Signal::TERM);
        assert!(signal_named("frob").is_err());
    }
}
