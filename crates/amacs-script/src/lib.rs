//! Scheme scripting for amacs.
//!
//! [`api`] lists the editor primitives in an engine-independent form.
//! [`Session`] pairs an editor with a Scheme engine and runs the Scheme
//! runtime from the `runtime` directory on top of those primitives. There
//! are two engines while the editor moves from one to the other: Steel, and
//! amacs's own Scheme; see [`Engine`].

pub mod api;
mod host;
mod runtime;
mod scheme;
mod session;
mod steel;

pub use host::{Engine, EvalError, EvalResult, InterruptHandle};
pub use session::Session;
