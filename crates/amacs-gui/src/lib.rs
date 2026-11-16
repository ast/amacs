//! The Wayland frontend for amacs.
//!
//! [`render`] turns draw operations into pixels and knows nothing about
//! Wayland, so it can be tested without a compositor. [`keys`] translates
//! keyboard symbols. The window itself is in the binary.

pub mod keys;
pub mod render;
