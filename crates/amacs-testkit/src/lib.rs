//! A headless frontend for tests: it applies draw operations to a grid of
//! cells, so tests can feed keys to an editor and assert on what a user
//! would see.

use amacs_core::Editor;
pub use amacs_proto::Screen;
use amacs_proto::{InputEvent, Key, parse_keys};

/// The language server for tests, built from this crate's `fake-lsp`
/// example. It is found beside the test program that is running.
///
/// `cargo test` builds it, but not when only some test targets are asked
/// for with `--test`; then `cargo build -p amacs-testkit --examples` does.
///
/// # Panics
/// If it has not been built.
pub fn fake_lsp_path() -> std::path::PathBuf {
    example_path("fake-lsp")
}

/// A program for tests that is an example of a crate of the workspace, as
/// `fake-lsp` is of this one: it is found beside the test program that is
/// running. `cargo test` builds the examples of the crate under test.
///
/// # Panics
/// If it has not been built.
#[must_use]
pub fn example_path(name: &str) -> std::path::PathBuf {
    let test_program = std::env::current_exe().expect("the test program has a path");
    // target/debug/deps/test-xyz -> target/debug/examples/fake-lsp
    let path = test_program
        .parent()
        .and_then(std::path::Path::parent)
        .expect("test programs are two levels down")
        .join("examples")
        .join(name);
    assert!(path.exists(), "{} is not built", path.display());
    path
}

/// An editor attached to a [`Screen`].
pub struct Harness {
    pub editor: Editor,
    pub screen: Screen,
}

impl Harness {
    pub const DEFAULT_DIRECTORY: &str = "/nonexistent/amacs-test";

    /// Creates an editor whose file prompts start in [`Harness::DEFAULT_DIRECTORY`],
    /// which does not exist, so that a test cannot write into the source tree
    /// by accident.
    pub fn new(cols: u16, rows: u16) -> Self {
        let mut harness = Self {
            editor: Editor::new(cols, rows),
            screen: Screen::default(),
        };
        harness
            .editor
            .set_default_directory(Self::DEFAULT_DIRECTORY);
        harness.redisplay();
        harness
    }

    /// Waits until nothing is in flight: the matcher's worker threads are
    /// done and every language server has answered. What a test sees then
    /// does not depend on timing.
    fn settle(&mut self) {
        let start = std::time::Instant::now();
        let mut asked = false;
        loop {
            self.editor.poll(std::time::Instant::now());
            let busy = self.editor.is_matching() || self.editor.lsp_busy();
            if start.elapsed().as_secs() >= 10 {
                break;
            }
            if busy {
                std::thread::sleep(std::time::Duration::from_millis(1));
            } else if asked {
                break;
            } else {
                // The servers may still be about to say something of their
                // own accord, such as diagnostics. An answer to a question
                // asked now comes after all of that.
                self.editor.lsp_barrier();
                asked = true;
            }
        }
    }

    fn redisplay(&mut self) {
        self.settle();
        let ops = self.editor.redisplay();
        self.screen.apply(&ops);
    }

    /// Presses keys written in Emacs notation, such as `"C-x C-f"`.
    ///
    /// # Panics
    /// If the description does not parse.
    pub fn keys(&mut self, keys: &str) -> &mut Self {
        for key in parse_keys(keys).unwrap_or_else(|e| panic!("{e}")) {
            self.key(key);
        }
        self
    }

    pub fn key(&mut self, key: Key) -> &mut Self {
        self.editor.handle_event(InputEvent::Key(key));
        self.redisplay();
        self
    }

    /// Types text one character at a time. A newline presses RET.
    pub fn type_text(&mut self, text: &str) -> &mut Self {
        for c in text.chars() {
            if c == '\n' {
                self.keys("RET");
            } else {
                self.key(Key::char(c));
            }
        }
        self
    }

    /// Makes the frame one that draws pictures, with cells of so many
    /// pixels across and down.
    pub fn draw_pictures(&mut self, cell: (u16, u16)) -> &mut Self {
        let (cols, rows) = self.screen.size();
        self.editor.handle_event(InputEvent::Resize {
            cols: cols as u16,
            rows: rows as u16,
            cell: Some(cell),
        });
        self.redisplay();
        self
    }

    pub fn paste(&mut self, text: &str) -> &mut Self {
        self.editor.handle_event(InputEvent::Paste(text.to_owned()));
        self.redisplay();
        self
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> &mut Self {
        self.editor.handle_event(InputEvent::Resize {
            cols,
            rows,
            cell: None,
        });
        self.redisplay();
        self
    }

    /// The text of the current buffer.
    pub fn buffer_text(&self) -> String {
        self.editor.text().to_string()
    }

    /// The echo area, the last row of the frame.
    pub fn echo(&self) -> String {
        self.screen.row(self.screen.size().1 - 1)
    }

    /// The rows of the echo area: the message or prompt and what is listed
    /// under it, without trailing empty rows.
    pub fn echo_lines(&self) -> Vec<String> {
        let mut rows: Vec<String> = (self.prompt_row()..self.screen.size().1)
            .map(|row| self.screen.row(row))
            .collect();
        while rows.len() > 1 && rows.last().is_some_and(String::is_empty) {
            rows.pop();
        }
        rows
    }

    /// The row with the minibuffer prompt. Candidates are listed below it.
    pub fn prompt(&self) -> String {
        self.screen.row(self.prompt_row())
    }

    fn prompt_row(&self) -> usize {
        self.screen.size().1 - usize::from(self.editor.echo_rows())
    }

    /// The candidates listed under the prompt, without trailing empty rows.
    pub fn candidates(&self) -> Vec<String> {
        let mut rows: Vec<String> = (self.prompt_row() + 1..self.screen.size().1)
            .map(|row| self.screen.row(row))
            .collect();
        while rows.last().is_some_and(String::is_empty) {
            rows.pop();
        }
        rows
    }

    /// Advances the editor's clock, for things that happen after a delay.
    pub fn wait(&mut self, duration: std::time::Duration) -> &mut Self {
        self.editor.poll(std::time::Instant::now() + duration);
        self.redisplay();
        self
    }

    /// Lets everything in flight finish and draws the result, for after
    /// something was done to the editor directly rather than with keys.
    pub fn sync(&mut self) -> &mut Self {
        self.redisplay();
        self
    }
}
