//! An editor with its Scheme runtime, driven by keys and read as a screen.

#![allow(dead_code)]

pub mod crypt;

use std::time::{Duration, Instant};

use amacs_proto::{FaceAttrs, InputEvent, Key, KeyCode, Mods, MouseEvent, NamedKey, parse_keys};
use amacs_script::Session;
use amacs_testkit::Screen;

pub struct Harness {
    pub session: Session,
    pub screen: Screen,
}

impl Harness {
    pub fn new(cols: u16, rows: u16) -> Self {
        let mut session = Session::new(cols, rows).unwrap_or_else(|e| panic!("{}", e.report));
        session
            .editor_mut()
            .set_default_directory("/nonexistent/amacs-test");
        let mut harness = Self {
            session,
            screen: Screen::default(),
        };
        harness.redisplay();
        harness
    }

    pub fn redisplay(&mut self) {
        // Wait for the matcher's worker threads, so that what a test sees
        // does not depend on timing.
        let start = Instant::now();
        loop {
            self.session.poll();
            if !self.session.editor().is_matching() || start.elapsed() > Duration::from_secs(5) {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let ops = self.session.redisplay();
        self.screen.apply(&ops);
    }

    pub fn keys(&mut self, keys: &str) -> &mut Self {
        for key in parse_keys(keys).unwrap() {
            self.session.handle_event(InputEvent::Key(key));
            self.redisplay();
        }
        self
    }

    pub fn type_text(&mut self, text: &str) -> &mut Self {
        for c in text.chars() {
            self.session.handle_event(InputEvent::Key(Key::char(c)));
        }
        self.redisplay();
        self
    }

    /// Presses a button of the pointer at a cell of the frame: `keys` is
    /// the press as a key, such as `<mouse-1>` or `C-<double-mouse-1>`.
    pub fn press(&mut self, keys: &str, col: u16, row: u16) -> &mut Self {
        let key: Key = keys.parse().unwrap();
        let (button, clicks) = match key.code {
            KeyCode::Named(NamedKey::Mouse(button)) => (button, 1),
            KeyCode::Named(NamedKey::DoubleMouse(button)) => (button, 2),
            KeyCode::Named(NamedKey::TripleMouse(button)) => (button, 3),
            _ => panic!("{keys} is not a press of a button"),
        };
        self.mouse(MouseEvent::Press {
            col,
            row,
            button,
            mods: key.mods,
            clicks,
        })
    }

    /// Moves the pointer to a cell with the button held.
    pub fn drag_to(&mut self, col: u16, row: u16) -> &mut Self {
        self.mouse(MouseEvent::Drag {
            col,
            row,
            mods: Mods::empty(),
        })
    }

    /// Lets the button go at a cell, with some modifiers held.
    pub fn release(&mut self, col: u16, row: u16, mods: Mods) -> &mut Self {
        self.mouse(MouseEvent::Release { col, row, mods })
    }

    pub fn mouse(&mut self, event: MouseEvent) -> &mut Self {
        self.session.handle_event(InputEvent::Mouse(event));
        self.redisplay();
        self
    }

    pub fn paste(&mut self, text: &str) -> &mut Self {
        self.session
            .handle_event(InputEvent::Paste(text.to_owned()));
        self.redisplay();
        self
    }

    /// Evaluates Scheme that is expected to succeed.
    pub fn eval(&mut self, source: &str) -> String {
        let result = self
            .session
            .eval(source)
            .unwrap_or_else(|e| panic!("{}", e.report));
        self.redisplay();
        result
    }

    /// Lets time pass until `done` holds, for what happens on other
    /// threads or after a delay.
    pub fn wait_until(&mut self, done: impl Fn(&mut Self) -> bool) {
        let start = Instant::now();
        while !done(self) {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "nothing happened; the screen is:\n{}",
                self.screen.text()
            );
            std::thread::sleep(Duration::from_millis(2));
            self.redisplay();
        }
    }

    /// Inserts a tab and an `x`, to see where the next tab stop is.
    pub fn paste_tab(&mut self) {
        self.paste("\tx");
    }

    pub fn echo(&self) -> String {
        self.screen.row(self.screen.size().1 - 1)
    }

    /// The row with the minibuffer prompt. Candidates are listed below it.
    pub fn prompt(&self) -> String {
        let rows = usize::from(self.session.editor().echo_rows());
        self.screen.row(self.screen.size().1 - rows)
    }

    pub fn mode_line(&self) -> String {
        self.screen.row(self.screen.size().1 - 2)
    }

    pub fn text(&self) -> String {
        self.session.editor().text().to_string()
    }

    /// The attributes that the named face has in the current theme.
    pub fn face(&self, name: &str) -> FaceAttrs {
        let editor = self.session.editor();
        let faces = editor.faces();
        faces.table()[faces.face_id(name).unwrap().0 as usize]
    }
}
