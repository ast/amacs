//! Formatting a buffer with an external program, such as rustfmt.
//!
//! A formatter reads the text on its standard input and writes the
//! formatted text to its standard output. The buffer is then changed to
//! match, touching only the lines that differ, so that point, the mark and
//! the undo history stay meaningful.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::editor::Editor;
use crate::env::{EnvState, Environment};
use crate::error::{CmdError, CmdResult};
use crate::script::Value;

/// A formatter that takes longer than this is given up on.
const TIMEOUT: Duration = Duration::from_secs(10);
/// How long saving waits for a project's environment to be worked out.
const ENVIRONMENT_WAIT: Duration = Duration::from_secs(5);

/// How to run a formatter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Formatter {
    pub program: String,
    /// Arguments. `{file}` stands for the buffer's file name.
    pub args: Vec<String>,
}

/// Runs a formatter over `input`. Fails with the formatter's complaint.
fn run(
    formatter: &Formatter,
    input: String,
    file: &Path,
    directory: &Path,
    environment: &Environment,
) -> Result<String, String> {
    let program = &formatter.program;
    let mut command = Command::new(program);
    for arg in &formatter.args {
        command.arg(arg.replace("{file}", &file.display().to_string()));
    }
    for (name, value) in environment {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
    // A buffer that visits no file may have a directory that is not there.
    if directory.is_dir() {
        command.current_dir(directory);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot run {program}: {error}"))?;
    let pipe = || format!("cannot talk to {program}");
    let mut stdin = child.stdin.take().ok_or_else(pipe)?;
    let stdout = child.stdout.take().ok_or_else(pipe)?;
    let stderr = child.stderr.take().ok_or_else(pipe)?;
    // Writing and reading happen at once, or a formatter that writes while
    // it reads would block on a full pipe.
    std::thread::spawn(move || {
        let _ = stdin.write_all(input.as_bytes());
    });
    let read = |mut pipe: Box<dyn Read + Send>| {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            let _ = sender.send(bytes);
        });
        receiver
    };
    let (out, err) = (read(Box::new(stdout)), read(Box::new(stderr)));
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < TIMEOUT => {
                std::thread::sleep(Duration::from_millis(2));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{program} took too long"));
            }
            Err(error) => return Err(format!("{program} failed: {error}")),
        }
    };
    let (out, err) = (
        out.recv().unwrap_or_default(),
        err.recv().unwrap_or_default(),
    );
    if !status.success() {
        let stderr = String::from_utf8_lossy(&err);
        let reason = stderr
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("it failed without saying why");
        return Err(format!("{program}: {}", reason.trim()));
    }
    String::from_utf8(out).map_err(|_| format!("{program} wrote text that is not UTF-8"))
}

impl Editor {
    pub fn define_formatter(&mut self, name: &str, formatter: Formatter) {
        self.formatters.insert(name.to_owned(), formatter);
    }

    /// The formatter that the `formatter` option names for this buffer.
    fn buffer_formatter(&self) -> CmdResult<Option<Formatter>> {
        let name = match self.option("formatter")? {
            Value::Bool(false) => return Ok(None),
            value => value
                .as_name()
                .map(str::to_owned)
                .ok_or_else(|| CmdError::msg("The formatter option must name a formatter"))?,
        };
        self.formatters
            .get(&name)
            .cloned()
            .map(Some)
            .ok_or_else(|| CmdError::msg(format!("No formatter named {name} is defined")))
    }

    /// Formats the current buffer with its formatter. Returns whether the
    /// text changed.
    pub fn format_buffer(&mut self) -> CmdResult<bool> {
        let formatter = self
            .buffer_formatter()?
            .ok_or_else(|| CmdError::msg("No formatter is set for this buffer"))?;
        let directory = self.local_directory()?;
        let file = self
            .buffer()
            .file
            .clone()
            .unwrap_or_else(|| directory.join(&self.buffer().name));

        // The formatter may come from the project's environment. Saving is
        // worth a short wait for that.
        let started = Instant::now();
        let environment = loop {
            match self.environment(&directory) {
                Some(EnvState::Ready(environment)) => break environment,
                Some(EnvState::Loading) if started.elapsed() < ENVIRONMENT_WAIT => {
                    std::thread::sleep(Duration::from_millis(20));
                    self.environment_poll();
                }
                _ => break Vec::new(),
            }
        };

        let before = self.text().to_string();
        let after = run(&formatter, before.clone(), &file, &directory, &environment)
            .map_err(CmdError::msg)?;
        if after == before {
            return Ok(false);
        }
        self.replace_text(&before, &after)?;
        Ok(true)
    }

    /// Changes the buffer's text from `before` to `after` by replacing the
    /// lines that differ, and puts point back on its line and column.
    fn replace_text(&mut self, before: &str, after: &str) -> CmdResult {
        let (old, new): (Vec<&str>, Vec<&str>) = (
            before.split_inclusive('\n').collect(),
            after.split_inclusive('\n').collect(),
        );
        let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        let suffix = old[prefix..]
            .iter()
            .rev()
            .zip(new[prefix..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let chars = |lines: &[&str]| lines.iter().map(|line| line.chars().count()).sum::<usize>();
        let start = chars(&old[..prefix]);
        let end = start + chars(&old[prefix..old.len() - suffix]);
        let replacement: String = new[prefix..new.len() - suffix].concat();

        let point = self.point();
        let text = self.text();
        let line = text.line_of(point);
        let column = point - text.line_start(line);
        // Lines above point that were added or removed move it with them.
        let moved = point >= end;
        self.delete(start..end)?;
        self.buffer_mut().insert(start, &replacement)?;
        if !moved && point > start {
            self.goto_line(line + 1, column);
        }
        Ok(())
    }

    /// Before saving: formats the buffer if it asks for that. A formatter
    /// that fails is reported and the buffer is saved as it is.
    pub(crate) fn format_before_save(&mut self) -> Option<String> {
        if !matches!(self.option("format-on-save"), Ok(value) if value != Value::Bool(false)) {
            return None;
        }
        // Formatters are programs, which are not run on other hosts yet.
        if crate::file_system::is_remote(self.working_directory()) {
            return None;
        }
        match self.buffer_formatter() {
            Ok(None) => None,
            Ok(Some(_)) => self.format_buffer().err().map(|error| error.to_string()),
            Err(error) => Some(error.to_string()),
        }
    }
}

pub fn format_buffer(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    let changed = ed.format_buffer()?;
    ed.message(if changed {
        "Formatted"
    } else {
        "Already formatted"
    });
    Ok(())
}
