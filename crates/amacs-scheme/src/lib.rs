//! The Scheme that amacs is extended in.
//!
//! Source text is read into syntax with positions, macros are expanded and
//! names resolved, and the result is compiled to instructions for a stack
//! machine. The dialect is close to R7RS-small, with these differences:
//! numbers are 64-bit integers and floats; there are coroutines and escape
//! procedures rather than full continuations; strings cannot be changed in
//! place.
//!
//! Made to be changed while it runs: a new definition of a name is seen by
//! every caller at once, every procedure knows its name, documentation,
//! parameters and source, and errors say where they happened and how the
//! program got there.

mod builtins;
mod code;
mod codegen;
mod compile;
mod error;
mod expand;
mod fiber;
mod heap;
mod printer;
mod reader;
mod source;
mod symbol;
mod syntax;
mod value;
mod vm;

pub use error::{Error, Report};
pub use source::Location;
pub use value::{Handle, Object, Symbol, Value};
pub use vm::{Core, Outcome, Rooted, Vm};

/// Definitions written in Scheme itself, loaded into every interpreter.
pub(crate) const PRELUDE: &str = include_str!("prelude.scm");

/// Runs something that may recurse deeply, on more stack if what is left is
/// little. Source code and data can be nested without limit.
pub(crate) fn deep<R>(run: impl FnOnce() -> R) -> R {
    stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, run)
}
