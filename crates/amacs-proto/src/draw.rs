use serde::{Deserialize, Serialize};

/// A rectangular area of the frame that shows lines of text: a window's text
/// area, a mode line, a divider between windows, or the echo area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PaneId(pub u32);

/// Index into the face table sent with [`DrawOp::DefineFaces`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct FaceId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// Fully resolved appearance of a run of text. `None` colours mean the
/// frontend's own default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct FaceAttrs {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    /// The underline is a wave, not a straight line. Frontends that cannot
    /// draw one draw the line straight.
    pub undercurl: bool,
    pub inverse: bool,
    /// Colour of the underline, if it differs from the foreground.
    pub underline_color: Option<Color>,
    /// Colour of a thin line along the top and bottom of the text's row,
    /// which sets a mode line apart from the text above it. Frontends that
    /// cannot draw lines ignore it.
    pub box_color: Option<Color>,
}

/// `len` bytes of a [`DisplayLine`]'s text drawn in one face.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub len: u32,
    pub face: FaceId,
}

/// A picture sent with [`DrawOp::DefineImage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ImageId(pub u32);

/// The part of a picture that one row shows. A picture takes whole cells:
/// `cols` of them across and `bands` rows down, from the cell at `col` of
/// the first of its rows, and the row with this slice shows the band
/// `band` of them, counted from 0. The picture is drawn as large as fits
/// in those cells with its proportions kept, from their top left corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageSlice {
    pub id: ImageId,
    pub col: u16,
    pub cols: u16,
    pub band: u16,
    pub bands: u16,
}

/// One screen row of a pane. The server guarantees the text fits the pane.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DisplayLine {
    pub text: String,
    /// Consecutive runs covering all of `text`.
    pub runs: Vec<Run>,
    /// Face for the cells between the end of the text and the pane's edge.
    pub fill: FaceId,
    /// The pictures that the row shows a band of, over its cells. Only a
    /// frontend that said it draws pictures is sent any.
    pub images: Vec<ImageSlice>,
}

impl DisplayLine {
    /// A line drawn entirely in one face.
    pub fn plain(text: impl Into<String>, face: FaceId) -> Self {
        let mut line = Self {
            fill: face,
            ..Self::default()
        };
        line.push(&text.into(), face);
        line
    }

    /// Appends text, extending the last run if the face is unchanged.
    pub fn push(&mut self, text: &str, face: FaceId) {
        if text.is_empty() {
            return;
        }
        self.text.push_str(text);
        let len = text.len() as u32;
        match self.runs.last_mut() {
            Some(run) if run.face == face => run.len += len,
            _ => self.runs.push(Run { len, face }),
        }
    }

    /// The text of each run with its face.
    pub fn segments(&self) -> impl Iterator<Item = (&str, FaceId)> {
        let mut offset = 0;
        self.runs.iter().map(move |run| {
            let end = offset + run.len as usize;
            let segment = &self.text[offset..end];
            offset = end;
            (segment, run.face)
        })
    }
}

/// Position and size of a pane in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneRect {
    pub id: PaneId,
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CursorShape {
    Block,
    Bar,
}

/// An instruction to a frontend. A batch always ends with [`DrawOp::Flush`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DrawOp {
    /// Replaces the face table. A table that only adds faces to the one
    /// before leaves what is drawn as it is.
    DefineFaces(Vec<FaceAttrs>),
    /// Replaces all panes. Everything previously drawn is discarded.
    Layout {
        cols: u16,
        rows: u16,
        panes: Vec<PaneRect>,
    },
    /// Replaces rows `first_row..` of a pane.
    Lines {
        pane: PaneId,
        first_row: u16,
        lines: Vec<DisplayLine>,
    },
    /// Places the cursor at a cell of a pane.
    Cursor {
        pane: PaneId,
        row: u16,
        col: u16,
        shape: CursorShape,
        /// The colour that the theme gives the cursor, for frontends that
        /// can choose it. Without it, a cursor is whatever stands out.
        color: Option<Color>,
    },
    /// A picture that rows from now on may show: `width` by `height`
    /// pixels, four bytes each of red, green, blue and opacity, by rows
    /// from the top.
    DefineImage {
        id: ImageId,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    /// Pictures that no row shows any more, to be forgotten.
    DropImages(Vec<ImageId>),
    /// Everything since the last flush forms one consistent frame.
    Flush,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_merges_runs_of_the_same_face() {
        let mut line = DisplayLine::default();
        line.push("ab", FaceId(0));
        line.push("ö", FaceId(0));
        line.push("c", FaceId(1));
        let segments: Vec<_> = line.segments().collect();
        assert_eq!(segments, [("abö", FaceId(0)), ("c", FaceId(1))]);
    }
}
