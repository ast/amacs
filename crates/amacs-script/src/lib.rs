//! Scheme scripting for amacs.
//!
//! [`api`] lists the editor primitives in an engine-independent form.
//! [`Session`] pairs an editor with the Steel engine and runs the Scheme
//! runtime from the `runtime` directory on top of those primitives.

pub mod api;
mod runtime;
mod session;
mod steel;

pub use session::Session;
pub use steel::{EvalError, EvalResult, InterruptHandle};
