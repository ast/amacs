//! What the server said to show: faces, panes, their lines and the cursor.

use std::collections::HashMap;
use std::sync::Arc;

use amacs_proto::{Color, CursorShape, DisplayLine, DrawOp, FaceAttrs, ImageId, PaneId, PaneRect};

use super::{DEFAULT_BG, DEFAULT_FG};

/// A picture that rows show bands of: pixels of four bytes each, red,
/// green, blue and opacity, by rows from the top.
#[derive(Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The pictures that the server has sent.
pub type Images = HashMap<ImageId, Arc<Image>>;

/// A pane and the lines it shows.
pub struct Pane {
    pub rect: PaneRect,
    pub lines: Vec<DisplayLine>,
    /// The rows whose lines changed since the last paint.
    pub dirty: Vec<bool>,
}

pub struct Scene {
    faces: Vec<FaceAttrs>,
    /// In the order of the layout.
    panes: Vec<Pane>,
    cursor: Option<(PaneId, u16, u16)>,
    /// How the cursor looks, as the server last said.
    cursor_shape: CursorShape,
    cursor_color: Option<Color>,
    images: Images,
    /// Everything must be drawn afresh, as after a resize.
    stale: bool,
}

impl Default for Scene {
    fn default() -> Self {
        Self {
            faces: Vec::new(),
            panes: Vec::new(),
            cursor: None,
            cursor_shape: CursorShape::Block,
            cursor_color: None,
            images: Images::new(),
            stale: true,
        }
    }
}

impl Scene {
    /// Records draw operations. Returns whether they complete a frame,
    /// which is then ready to be painted, and whether lines shaped before
    /// may look different now.
    pub fn apply(&mut self, ops: &[DrawOp]) -> (bool, bool) {
        let (mut flushed, mut shapes_changed) = (false, false);
        for op in ops {
            match op {
                DrawOp::DefineFaces(faces) => {
                    // A table that only adds faces changes nothing drawn.
                    let grown = faces.starts_with(&self.faces);
                    self.faces.clone_from(faces);
                    if !grown {
                        shapes_changed = true;
                        self.stale = true;
                    }
                }
                DrawOp::Layout { panes, .. } => {
                    self.panes = panes
                        .iter()
                        .map(|rect| {
                            let rows = usize::from(rect.h);
                            Pane {
                                rect: *rect,
                                lines: vec![DisplayLine::default(); rows],
                                dirty: vec![true; rows],
                            }
                        })
                        .collect();
                    self.cursor = None;
                    self.stale = true;
                }
                DrawOp::Lines {
                    pane,
                    first_row,
                    lines,
                } => {
                    let Some(pane) = self.panes.iter_mut().find(|p| p.rect.id == *pane) else {
                        continue;
                    };
                    for (offset, line) in lines.iter().enumerate() {
                        let row = usize::from(*first_row) + offset;
                        if row < pane.lines.len() {
                            pane.lines[row].clone_from(line);
                            pane.dirty[row] = true;
                        }
                    }
                }
                DrawOp::Cursor {
                    pane,
                    row,
                    col,
                    shape,
                    color,
                } => {
                    self.cursor = Some((*pane, *row, *col));
                    self.cursor_shape = *shape;
                    self.cursor_color = *color;
                }
                DrawOp::DefineImage {
                    id,
                    width,
                    height,
                    rgba,
                } => {
                    // A picture that is not of its size is not drawn.
                    if rgba.len() == *width as usize * *height as usize * 4 {
                        let image = Image {
                            width: *width,
                            height: *height,
                            rgba: rgba.clone(),
                        };
                        self.images.insert(*id, Arc::new(image));
                    }
                }
                DrawOp::DropImages(ids) => {
                    for id in ids {
                        self.images.remove(id);
                    }
                }
                DrawOp::Flush => flushed = true,
            }
        }
        (flushed, shapes_changed)
    }

    pub fn images(&self) -> &Images {
        &self.images
    }

    pub fn panes(&self) -> &[Pane] {
        &self.panes
    }

    pub fn is_stale(&self) -> bool {
        self.stale
    }

    pub fn set_stale(&mut self) {
        self.stale = true;
    }

    /// Everything recorded so far has been drawn.
    pub fn painted(&mut self) {
        self.stale = false;
        for pane in &mut self.panes {
            pane.dirty.fill(false);
        }
    }

    pub fn faces(&self) -> &[FaceAttrs] {
        &self.faces
    }

    pub fn face(&self, index: usize) -> FaceAttrs {
        self.faces.get(index).copied().unwrap_or_default()
    }

    /// Foreground and background of a face, with defaults filled in and
    /// inverse video applied.
    pub fn colors(&self, index: usize) -> (Color, Color) {
        let face = self.face(index);
        let fg = face.fg.unwrap_or(DEFAULT_FG);
        let bg = face.bg.unwrap_or(DEFAULT_BG);
        if face.inverse { (bg, fg) } else { (fg, bg) }
    }

    /// The shape of the cursor and the colour that the theme gives it.
    pub fn cursor_look(&self) -> (CursorShape, Option<Color>) {
        (self.cursor_shape, self.cursor_color)
    }

    /// The cell the cursor is in, as (column, row) on the frame.
    pub fn cursor_cell(&self) -> Option<(u32, u32)> {
        let (pane, row, col) = self.cursor?;
        let rect = self.panes.iter().find(|p| p.rect.id == pane)?.rect;
        Some((
            u32::from(rect.x) + u32::from(col),
            u32::from(rect.y) + u32::from(row),
        ))
    }
}
