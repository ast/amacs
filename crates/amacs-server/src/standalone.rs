//! The editor as a single process in the terminal it was started from.

use std::io::IsTerminal;
use std::process::ExitCode;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Instant;

use amacs_proto::{InputEvent, Key};
use amacs_tty::Tty;
use anyhow::{Context, Result, bail};

use crate::{
    AUTO_SAVE_IDLE, AUTO_SAVE_INTERVAL, Options, save_everything, start_session, wait_time,
};

enum Event {
    Input(std::io::Result<InputEvent>),
    /// A worker thread of the editor has results.
    Wake,
}

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
    {
        let sender = sender.clone();
        session.editor_mut().set_waker(std::sync::Arc::new(move || {
            let _ = sender.send(Event::Wake);
        }));
    }
    std::thread::spawn(move || {
        loop {
            let event = amacs_tty::read_event();
            if matches!(&event, Ok(InputEvent::Key(key)) if *key == Key::ctrl('g')) {
                interrupt.interrupt();
            }
            let failed = event.is_err();
            if sender.send(Event::Input(event)).is_err() || failed {
                break;
            }
        }
    });

    let mut last_auto_save = Instant::now();
    let mut last_input = Instant::now();
    while !session.should_exit() {
        tty.apply(&session.redisplay())?;
        let mut next = match events.recv_timeout(wait_time(&session)) {
            Ok(event) => Some(event),
            Err(RecvTimeoutError::Timeout) => {
                if last_input.elapsed() >= AUTO_SAVE_IDLE {
                    save_everything(&mut session, false);
                    last_auto_save = Instant::now();
                }
                Some(Event::Wake)
            }
            Err(RecvTimeoutError::Disconnected) => bail!("the input thread stopped"),
        };
        // Handle everything that has already arrived before drawing again.
        while let Some(event) = next {
            match event {
                Event::Wake => session.poll(),
                Event::Input(event) => {
                    last_input = Instant::now();
                    let event = match event.context("cannot read from the terminal")? {
                        // Terminals report a resize once per step of a drag.
                        // Trust the current size rather than replaying each.
                        InputEvent::Resize { .. } => {
                            let (cols, rows) = tty.size()?;
                            InputEvent::Resize { cols, rows }
                        }
                        other => other,
                    };
                    session.handle_event(event);
                }
            }
            next = if session.should_exit() {
                None
            } else {
                events.try_recv().ok()
            };
        }
        if last_auto_save.elapsed() >= AUTO_SAVE_INTERVAL {
            save_everything(&mut session, false);
            last_auto_save = Instant::now();
        }
    }
    session.editor_mut().save_state();
    Ok(ExitCode::SUCCESS)
}
