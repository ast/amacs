//! The `amacs` binary.
//!
//! For now this is a single process that edits in the terminal it was started
//! from. It becomes the daemon when the client/server split lands.

use std::io::IsTerminal;
use std::path::Path;
use std::process::ExitCode;

use amacs_core::Editor;
use amacs_proto::InputEvent;
use amacs_tty::Tty;
use anyhow::{Context, Result, bail};

const USAGE: &str = "Usage: amacs [FILE]...

Edit FILEs in the terminal. The last one given is shown first.

Options:
  -h, --help     Show this help
  -V, --version  Show the version";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("amacs: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let mut files = Vec::new();
    let mut options_done = false;
    for arg in std::env::args_os().skip(1) {
        match arg.to_str() {
            Some("-h" | "--help") if !options_done => {
                println!("{USAGE}");
                return Ok(());
            }
            Some("-V" | "--version") if !options_done => {
                println!("amacs {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            Some("--") if !options_done => options_done = true,
            Some(option) if !options_done && option.starts_with('-') && option != "-" => {
                bail!("unknown option `{option}`\n{USAGE}");
            }
            _ => files.push(arg),
        }
    }
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        bail!("standard input and output must be a terminal");
    }

    let mut tty = Tty::new().context("cannot set up the terminal")?;
    let (cols, rows) = tty.size().context("cannot read the terminal size")?;
    let mut editor = Editor::new(cols, rows);
    for file in &files {
        let path = std::path::absolute(Path::new(file)).context("cannot resolve file name")?;
        if let Err(error) = editor.find_file(&path) {
            editor.message(error.to_string());
        }
    }

    while !editor.should_exit() {
        tty.apply(&editor.redisplay())?;
        let event = tty.read_event()?;
        // Terminals report a resize once per step of a drag. Trust the
        // current size rather than replaying each one.
        let event = match event {
            InputEvent::Resize { .. } => {
                let (cols, rows) = tty.size()?;
                InputEvent::Resize { cols, rows }
            }
            other => other,
        };
        editor.handle_event(event);
    }
    Ok(())
}
