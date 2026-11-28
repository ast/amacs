//! Types shared between the amacs server and its frontends.
//!
//! A frontend sends [`InputEvent`]s and applies [`DrawOp`]s. It knows nothing
//! about buffers or commands, so the same editor core can drive a terminal, a
//! Wayland window or a headless test grid.

mod draw;
mod key;
mod screen;
mod wire;

pub use draw::{
    Color, CursorShape, DisplayLine, DrawOp, FaceAttrs, FaceId, ImageId, ImageSlice, PaneId,
    PaneRect, Run,
};
pub use key::{Key, KeyCode, Mods, NamedKey, ParseKeyError, parse_keys};
pub use screen::{Band, Screen};
pub use wire::{
    Answer, Ask, ClientMsg, EvalFailure, FileSpec, FrameRequest, PROTOCOL_VERSION, ServerMsg,
    SubpixelOrder, pid_path, read_msg, socket_path, write_msg,
};

use serde::{Deserialize, Serialize};
use unicode_width::UnicodeWidthChar;

/// How many cells a char of a [`DisplayLine`] takes: two for a wide one,
/// none for one that joins the char before it, as a combining accent does,
/// and one for any other. The server lays lines out by this and every
/// frontend draws them by it, so that both agree on where a column is.
pub fn char_cells(c: char) -> usize {
    c.width().unwrap_or(1)
}

/// How many cells a text of a [`DisplayLine`] takes: the sum of its chars'.
pub fn text_cells(text: &str) -> usize {
    text.chars().map(char_cells).sum()
}

/// Something the user did in a frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputEvent {
    Key(Key),
    /// The frame now has this many character cells. A frontend that draws
    /// pictures says how wide and high a cell is in pixels, by which the
    /// server knows how many cells a picture takes; one that does not say
    /// is sent no pictures.
    Resize {
        cols: u16,
        rows: u16,
        cell: Option<(u16, u16)>,
    },
    /// Text pasted by the terminal or compositor, to be inserted verbatim.
    Paste(String),
    Mouse(MouseEvent),
}

/// Something done with the pointer, at a character cell of the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseEvent {
    /// The wheel was turned, or two fingers moved: so many lines, forward
    /// in the text if positive.
    Scroll { col: u16, row: u16, lines: i16 },
    /// A button was pressed: 1 is the main one, 2 the middle one, 3 the
    /// one for menus. `clicks` is 1, or 2 or 3 for a press in quick
    /// succession at the same cell.
    Press {
        col: u16,
        row: u16,
        button: u8,
        mods: Mods,
        clicks: u8,
    },
    /// The pointer came to another cell with the button held.
    Drag { col: u16, row: u16, mods: Mods },
    /// The button was let go.
    Release { col: u16, row: u16, mods: Mods },
}
