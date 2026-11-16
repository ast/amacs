//! A headless frontend for tests: it applies draw operations to a grid of
//! cells, so tests can feed keys to an editor and assert on what a user
//! would see.

use std::collections::HashMap;

use amacs_core::Editor;
use amacs_proto::{DrawOp, FaceAttrs, FaceId, InputEvent, Key, PaneId, PaneRect, parse_keys};
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Cell {
    /// Empty for the second cell of a wide character.
    text: String,
    face: FaceId,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            text: " ".to_owned(),
            face: FaceId::default(),
        }
    }
}

/// The frame as a frontend would draw it.
#[derive(Debug, Default)]
pub struct Screen {
    cols: usize,
    rows: usize,
    cells: Vec<Cell>,
    panes: HashMap<PaneId, PaneRect>,
    faces: Vec<FaceAttrs>,
    cursor: Option<(usize, usize)>,
    /// Number of rows redrawn by the last batch of operations.
    pub rows_drawn: usize,
}

impl Screen {
    pub fn apply(&mut self, ops: &[DrawOp]) {
        self.rows_drawn = 0;
        for op in ops {
            match op {
                DrawOp::DefineFaces(faces) => self.faces.clone_from(faces),
                DrawOp::Layout { cols, rows, panes } => {
                    self.cols = usize::from(*cols);
                    self.rows = usize::from(*rows);
                    self.cells = vec![Cell::default(); self.cols * self.rows];
                    self.panes = panes.iter().map(|pane| (pane.id, *pane)).collect();
                }
                DrawOp::Lines {
                    pane,
                    first_row,
                    lines,
                } => {
                    let rect = self.panes[pane];
                    for (offset, line) in lines.iter().enumerate() {
                        let row = usize::from(*first_row) + offset;
                        assert!(
                            row < usize::from(rect.h),
                            "row {row} is outside pane {pane:?}"
                        );
                        self.draw_line(rect, row, line);
                        self.rows_drawn += 1;
                    }
                }
                DrawOp::Cursor { pane, row, col, .. } => {
                    let rect = self.panes[pane];
                    assert!(
                        *row < rect.h && *col < rect.w,
                        "cursor outside pane {pane:?}"
                    );
                    self.cursor = Some((usize::from(rect.y + row), usize::from(rect.x + col)));
                }
                DrawOp::Flush => {}
            }
        }
    }

    fn draw_line(&mut self, rect: PaneRect, row: usize, line: &amacs_proto::DisplayLine) {
        let y = usize::from(rect.y) + row;
        let (x0, width) = (usize::from(rect.x), usize::from(rect.w));
        let mut col = 0;
        for (segment, face) in line.segments() {
            for c in segment.chars() {
                let cells = c.width().unwrap_or(1);
                if cells == 0 {
                    // A combining char joins the cell before it.
                    if col > 0 {
                        self.cells[y * self.cols + x0 + col - 1].text.push(c);
                    }
                    continue;
                }
                assert!(
                    col + cells <= width,
                    "line overflows pane {:?}: {:?}",
                    rect.id,
                    line.text
                );
                let index = y * self.cols + x0 + col;
                self.cells[index] = Cell {
                    text: c.to_string(),
                    face,
                };
                for extra in 1..cells {
                    self.cells[index + extra] = Cell {
                        text: String::new(),
                        face,
                    };
                }
                col += cells;
            }
        }
        for col in col..width {
            self.cells[y * self.cols + x0 + col] = Cell {
                text: " ".to_owned(),
                face: line.fill,
            };
        }
    }

    /// The text of one row, without trailing spaces.
    pub fn row(&self, row: usize) -> String {
        let cells = &self.cells[row * self.cols..(row + 1) * self.cols];
        let text: String = cells.iter().map(|cell| cell.text.as_str()).collect();
        text.trim_end().to_owned()
    }

    /// The whole frame, one line per row, without trailing spaces.
    pub fn text(&self) -> String {
        (0..self.rows)
            .map(|row| self.row(row))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The cursor as (row, column) on the frame.
    pub fn cursor(&self) -> Option<(usize, usize)> {
        self.cursor
    }

    /// The face of a cell.
    pub fn face_at(&self, row: usize, col: usize) -> FaceAttrs {
        self.faces[self.cells[row * self.cols + col].face.0 as usize]
    }

    pub fn size(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }
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

    fn redisplay(&mut self) {
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

    pub fn paste(&mut self, text: &str) -> &mut Self {
        self.editor.handle_event(InputEvent::Paste(text.to_owned()));
        self.redisplay();
        self
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> &mut Self {
        self.editor.handle_event(InputEvent::Resize { cols, rows });
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
}
