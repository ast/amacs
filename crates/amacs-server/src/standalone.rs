//! The editor as a single process in the terminal it was started from.

use std::io::IsTerminal;
use std::process::ExitCode;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Instant;

use amacs_proto::{InputEvent, Key};
use amacs_tty::Tty;
use anyhow::{Context, Result, bail};

use crate::{AUTO_SAVE_IDLE, AUTO_SAVE_INTERVAL, Options, start_session};

pub fn run(options: &Options) -> Result<ExitCode> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        bail!("standard input and output must be a terminal (or use --batch or --daemon)");
    }

    let mut tty = Tty::new().context("cannot set up the terminal")?;
    let (cols, rows) = tty.size().context("cannot read the terminal size")?;
    let mut session = start_session(options, cols, rows)?;

    // Input is read on its own thread so that C-g can interrupt Scheme code
    // that the main thread is busy running.
    let (sender, events) = mpsc::channel();
    let interrupt = session.interrupt_handle();
    std::thread::spawn(move || {
        loop {
            let event = amacs_tty::read_event();
            if matches!(&event, Ok(InputEvent::Key(key)) if *key == Key::ctrl('g')) {
                interrupt.interrupt();
            }
            let failed = event.is_err();
            if sender.send(event).is_err() || failed {
                break;
            }
        }
    });

    let mut last_auto_save = Instant::now();
    while !session.should_exit() {
        tty.apply(&session.redisplay())?;
        let mut next = match events.recv_timeout(AUTO_SAVE_IDLE) {
            Ok(event) => Some(event),
            Err(RecvTimeoutError::Timeout) => {
                session.editor_mut().auto_save(false);
                last_auto_save = Instant::now();
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => bail!("the input thread stopped"),
        };
        // Handle everything that has already arrived before drawing again.
        while let Some(event) = next {
            let event = match event.context("cannot read from the terminal")? {
                // Terminals report a resize once per step of a drag. Trust
                // the current size rather than replaying each one.
                InputEvent::Resize { .. } => {
                    let (cols, rows) = tty.size()?;
                    InputEvent::Resize { cols, rows }
                }
                other => other,
            };
            session.handle_event(event);
            next = if session.should_exit() {
                None
            } else {
                events.try_recv().ok()
            };
        }
        if last_auto_save.elapsed() >= AUTO_SAVE_INTERVAL {
            session.editor_mut().auto_save(false);
            last_auto_save = Instant::now();
        }
    }
    Ok(ExitCode::SUCCESS)
}
