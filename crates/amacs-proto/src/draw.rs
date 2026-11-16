use serde::{Deserialize, Serialize};

/// A rectangular area of the frame that shows lines of text: a window's text
/// area, a mode line, a divider between windows, or the echo area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PaneId(pub u32);

/// Index into the face table sent with [`DrawOp::DefineFaces`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct FaceId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FaceAttrs {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

/// `len` bytes of a [`DisplayLine`]'s text drawn in one face.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub len: u32,
    pub face: FaceId,
}

/// One screen row of a pane. The server guarantees the text fits the pane.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DisplayLine {
    pub text: String,
    /// Consecutive runs covering all of `text`.
    pub runs: Vec<Run>,
    /// Face for the cells between the end of the text and the pane's edge.
    pub fill: FaceId,
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
    /// Replaces the face table.
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
    },
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
