//! The amacs editor core: buffers, windows, keymaps, commands and redisplay.
//!
//! The core is single-threaded and does no terminal or socket IO. A frontend
//! feeds it [`amacs_proto::InputEvent`]s and applies the
//! [`amacs_proto::DrawOp`]s that [`Editor::redisplay`] returns.

pub mod askpass;
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
pub mod file_system;
mod files;
mod fold;
mod format;
mod frame;
mod history;
mod keymap;
mod kill_ring;
mod kmacro;
mod lsp;
mod minibuffer;
mod mouse;
mod process;
mod project;
mod redisplay;
pub mod regexp;
mod script;
mod sexp;
mod snippet;
mod spell;
mod telemetry;
mod terminal;
mod undo_journal;
mod undo_view;
mod websocket;
mod window;

pub use buffer::{Buffer, BufferId, LineNumbers};
pub use completion::{Candidate, Completion, Offers, Waker};
pub use display::string_cells;
pub use editor::{
    Clipboard, Command, CommandBody, Editor, EditorEvent, GLOBAL_MAP, MINIBUFFER_MAP, Prefix,
    doc_summary,
};
pub use env::EnvState;
pub use error::{CmdError, CmdResult};
pub use extend::window_number;
pub use faces::{FaceSpec, Faces};
pub use files::{
    FileJob, FileKind, Listed, abbreviate_path, expand_path, expand_typed_path, format_size,
    is_binary, list_directory, typed_name,
};
pub use format::Formatter;
pub use frame::FrameId;
pub use keymap::{Keymap, Lookup};
pub use lsp::ServerConfig;
pub use minibuffer::Accepted;
pub use script::{FUNDAMENTAL_MODE, ScriptCall, ScriptFn, ScriptOutcome, ScriptTarget, Value};
pub use telemetry::Stat;

/// Compiles the highlight queries of every language in the background,
/// so that the first file in a language does not wait for its query.
pub use amacs_treesit::prepare_languages;
pub use window::{Rect, SplitDir, Window, WindowId};
