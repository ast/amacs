//! The plumbing of the Language Server Protocol: starting a server,
//! exchanging JSON-RPC messages with it, and converting between its
//! positions and file names and the editor's.
//!
//! What the messages mean is the editor's business; this crate treats them
//! as JSON.

mod client;
mod position;
mod transport;
mod uri;

pub use client::{Client, Incoming};
pub use position::{Encoding, from_units, to_units};
pub use transport::{read_message, write_message};
pub use uri::{path_to_uri, uri_to_path};
