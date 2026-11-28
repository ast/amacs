//! A frame as a grid of cells: what the draw operations that a frontend
//! receives come to, for programs that read the screen rather than show
//! it, such as tests and tools for language models.

use std::collections::HashMap;

use crate::{DisplayLine, DrawOp, FaceAttrs, FaceId, ImageId, PaneId, PaneRect};

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

/// A band of a picture on a row of the frame: the picture, the column
/// that it starts at, and which band of how many.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Band {
    pub id: ImageId,
    pub col: usize,
    pub cols: usize,
    pub band: usize,
    pub bands: usize,
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
    /// The pictures defined, with their sizes in pixels.
    images: HashMap<ImageId, (u32, u32)>,
    /// The band of a picture that each row shows, by row of the frame.
    bands: HashMap<usize, Vec<Band>>,
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
                    self.bands.clear();
                }
                DrawOp::DefineImage {
                    id,
                    width,
                    height,
                    rgba,
                } => {
                    assert_eq!(
                        rgba.len(),
                        *width as usize * *height as usize * 4,
                        "picture {id:?} is not of its size"
                    );
                    self.images.insert(*id, (*width, *height));
                }
                DrawOp::DropImages(ids) => {
                    for id in ids {
                        self.images.remove(id);
                    }
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

    fn draw_line(&mut self, rect: PaneRect, row: usize, line: &DisplayLine) {
        let y = usize::from(rect.y) + row;
        let (x0, width) = (usize::from(rect.x), usize::from(rect.w));
        let bands: Vec<Band> = line
            .images
            .iter()
            .map(|slice| {
                assert!(
                    self.images.contains_key(&slice.id),
                    "picture {:?} is shown and not defined",
                    slice.id
                );
                assert!(
                    usize::from(slice.col + slice.cols) <= width,
                    "picture overflows pane {:?}",
                    rect.id
                );
                Band {
                    id: slice.id,
                    col: x0 + usize::from(slice.col),
                    cols: usize::from(slice.cols),
                    band: usize::from(slice.band),
                    bands: usize::from(slice.bands),
                }
            })
            .collect();
        if bands.is_empty() {
            self.bands.remove(&y);
        } else {
            self.bands.insert(y, bands);
        }
        let mut col = 0;
        for (segment, face) in line.segments() {
            for c in segment.chars() {
                let cells = crate::char_cells(c);
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

    /// The bands of pictures that a row of the frame shows.
    pub fn bands(&self, row: usize) -> &[Band] {
        self.bands.get(&row).map_or(&[], Vec::as_slice)
    }

    /// The size in pixels of a picture that is defined.
    pub fn image_size(&self, id: ImageId) -> Option<(u32, u32)> {
        self.images.get(&id).copied()
    }

    pub fn size(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }
}
