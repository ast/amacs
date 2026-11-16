//! The amacs editor core: buffers, windows, keymaps, commands and redisplay.
//!
//! The core is single-threaded and does no terminal or socket IO. A frontend
//! feeds it [`amacs_proto::InputEvent`]s and applies the
//! [`amacs_proto::DrawOp`]s that [`Editor::redisplay`] returns.

mod buffer;
mod commands;
mod completion;
mod display;
mod editor;
mod error;
mod faces;
mod files;
mod frame;
mod history;
mod keymap;
mod kill_ring;
mod minibuffer;
mod redisplay;
mod script;
mod window;

pub use buffer::{Buffer, BufferId};
pub use completion::{Candidate, Completion, Waker};
pub use editor::{
    Clipboard, Command, CommandBody, Editor, EditorEvent, GLOBAL_MAP, MINIBUFFER_MAP, Prefix,
};
pub use error::{CmdError, CmdResult};
pub use files::expand_path;
pub use frame::FrameId;
pub use keymap::{Keymap, Lookup};
pub use minibuffer::Accepted;
pub use script::{FUNDAMENTAL_MODE, ScriptCall, ScriptFn, ScriptOutcome, ScriptTarget, Value};
pub use window::{Rect, SplitDir, Window, WindowId};
