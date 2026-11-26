//! Text storage for amacs: a rope, markers that track edits, and an undo tree.
//!
//! All positions are char offsets from the start of the text, as in Emacs
//! (but zero-based). Nothing here knows about buffers, windows or display.

mod marker;
mod text;
mod undo;

pub use marker::{MarkerId, MarkerSet};
pub use text::Text;
pub use undo::{Edit, Record, Transaction, UndoTree};
