//! Times what a key press costs: the command and working out what the
//! frame shows after it.
//!
//! Run with `cargo run --release -p amacs-script --example redisplay FILE`,
//! from the root of a checkout. The file is shown as `examples/init.scm`
//! shows a program: with line numbers and the marks of the modes it turns
//! on.

use std::time::{Duration, Instant};

use amacs_proto::{InputEvent, parse_keys};
use amacs_script::Session;

const SETUP: &str = "
(add-hook! 'prog-mode-hook
  (lambda ()
    (set-local! 'line-numbers 'relative)
    (enable-mode! 'hl-line-mode)
    (enable-mode! 'symbol-highlight-mode)
    (enable-mode! 'hl-todo-mode)
    (enable-mode! 'whitespace-mode)))";

/// What is timed: its name, where point starts, as the text that it is
/// put after, and the keys that are pressed.
const WORK: &[(&str, &str, &str)] = &[
    ("move down a line", "", "C-n"),
    ("move forward a char", "", "C-f"),
    ("type a char in a comment", "/// ", "x"),
    ("type a char in code", "fn ", "x"),
    ("type a bracket in code", "fn ", "("),
    ("scroll three lines", "", "C-u 3 C-v"),
];

fn main() {
    let file = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "crates/amacs-core/src/editor.rs".to_owned());
    let file = std::path::absolute(&file).expect("the name of a file");
    let mut session = Session::new(200, 60).expect("the runtime loads");
    // As in the server: with something to wake, text with syntax errors
    // is parsed on another thread, and a key press does not wait for it.
    session.editor_mut().set_waker(std::sync::Arc::new(|| {}));
    session.eval(SETUP).expect("the setup runs");
    session
        .eval(&format!("(find-file {:?})", file.display().to_string()))
        .expect("the file opens");
    session
        .eval("(define original (buffer-string))")
        .expect("the text is kept");
    session.redisplay();

    println!(
        "{:>12} {:>12}  per key, in a frame of 200 by 60",
        "command", "redisplay"
    );
    let only = std::env::args().nth(2);
    for (name, after, keys) in WORK {
        if only
            .as_ref()
            .is_some_and(|only| !name.contains(only.as_str()))
        {
            continue;
        }
        let keys = parse_keys(keys).expect("keys");
        // The best of several rounds: other work on the machine only
        // ever makes a round slower.
        let mut best = (Duration::MAX, Duration::MAX);
        for _ in 0..5 {
            session
                .eval(&format!(
                    "(unless (equal? (buffer-string) original)
                       (%replace-buffer-text! original))
                     (goto-char (+ (string-search (buffer-string) {after:?}) (string-length {after:?})))"
                ))
                .expect("point moves");
            session.redisplay();
            let (mut command, mut redisplay) = (Duration::ZERO, Duration::ZERO);
            const TIMES: u32 = 200;
            for _ in 0..TIMES {
                let started = Instant::now();
                for key in &keys {
                    session.handle_event(InputEvent::Key(*key));
                }
                let handled = Instant::now();
                session.redisplay();
                redisplay += handled.elapsed();
                command += handled - started;
            }
            best = (best.0.min(command / TIMES), best.1.min(redisplay / TIMES));
        }
        println!("{:>12.1?} {:>12.1?}  {name}", best.0, best.1);
    }
}
