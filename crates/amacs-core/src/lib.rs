//! The amacs editor core: buffers, windows, keymaps, commands and redisplay.
//!
//! The core is single-threaded and does no terminal or socket IO. A frontend
//! feeds it [`amacs_proto::InputEvent`]s and applies the
//! [`amacs_proto::DrawOp`]s that [`Editor::redisplay`] returns.

mod buffer;
mod commands;
mod display;
mod editor;
mod error;
mod faces;
mod files;
mod keymap;
mod kill_ring;
mod minibuffer;
mod redisplay;
mod window;

pub use buffer::{Buffer, BufferId};
pub use editor::{Command, Editor, Prefix};
pub use error::{CmdError, CmdResult};
pub use keymap::{Keymap, Lookup};
pub use minibuffer::Completion;
pub use window::{Rect, SplitDir, Window, WindowId};
