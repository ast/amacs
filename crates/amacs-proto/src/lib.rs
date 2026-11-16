//! Types shared between the amacs server and its frontends.
//!
//! A frontend sends [`InputEvent`]s and applies [`DrawOp`]s. It knows nothing
//! about buffers or commands, so the same editor core can drive a terminal, a
//! Wayland window or a headless test grid.

mod draw;
mod key;
mod wire;

pub use draw::{Color, CursorShape, DisplayLine, DrawOp, FaceAttrs, FaceId, PaneId, PaneRect, Run};
pub use key::{Key, KeyCode, Mods, NamedKey, ParseKeyError, parse_keys};
pub use wire::{
    ClientMsg, FileSpec, FrameRequest, PROTOCOL_VERSION, ServerMsg, read_msg, socket_path,
    write_msg,
};

use serde::{Deserialize, Serialize};

/// Something the user did in a frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputEvent {
    Key(Key),
    /// The frame now has this many character cells.
    Resize {
        cols: u16,
        rows: u16,
    },
    /// Text pasted by the terminal or compositor, to be inserted verbatim.
    Paste(String),
}
