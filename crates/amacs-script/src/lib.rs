//! Scheme scripting for amacs.
//!
//! [`api`] lists the editor primitives as plain functions over the core's values.
//! [`Session`] pairs an editor with the Scheme interpreter, `amacs-scheme`,
//! and runs the Scheme runtime from the `runtime` directory on top of those
//! primitives.

pub mod api;
mod host;
mod runtime;
mod scheme;
mod session;

pub use host::{EvalError, EvalResult, InterruptHandle};
pub use session::Session;
