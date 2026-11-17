//! The amacs editor core: buffers, windows, keymaps, commands and redisplay.
//!
//! The core is single-threaded and does no terminal or socket IO. A frontend
//! feeds it [`amacs_proto::InputEvent`]s and applies the
//! [`amacs_proto::DrawOp`]s that [`Editor::redisplay`] returns.

mod buffer;
mod commands;
mod complete;
mod completion;
mod decorate;
mod diagnostics;
mod display;
mod editor;
mod env;
mod error;
mod extend;
mod faces;
mod files;
mod format;
mod frame;
mod history;
mod keymap;
mod kill_ring;
mod lsp;
mod minibuffer;
mod project;
mod redisplay;
mod script;
mod sexp;
mod snippet;
mod undo_view;
mod window;

pub use buffer::{Buffer, BufferId, LineNumbers};
pub use completion::{Candidate, Completion, Waker};
pub use editor::{
    Clipboard, Command, CommandBody, Editor, EditorEvent, GLOBAL_MAP, MINIBUFFER_MAP, Prefix,
};
pub use env::EnvState;
pub use error::{CmdError, CmdResult};
pub use extend::window_number;
pub use faces::{FaceSpec, Faces};
pub use files::expand_path;
pub use format::Formatter;
pub use frame::FrameId;
pub use keymap::{Keymap, Lookup};
pub use lsp::ServerConfig;
pub use minibuffer::Accepted;
pub use script::{FUNDAMENTAL_MODE, ScriptCall, ScriptFn, ScriptOutcome, ScriptTarget, Value};
pub use window::{Rect, SplitDir, Window, WindowId};
