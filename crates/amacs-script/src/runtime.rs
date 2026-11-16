//! The Scheme runtime: the part of the editor that is written in Scheme.
//!
//! The files are compiled into the binary. Setting `AMACS_RUNTIME` to the
//! `runtime` directory of a source checkout loads them from disk instead, so
//! that they can be changed without rebuilding.

use std::path::PathBuf;

/// Runtime files in load order, relative to the runtime directory.
const FILES: &[(&str, &str)] = &[
    (
        "core/compat.scm",
        include_str!("../../../runtime/core/compat.scm"),
    ),
    (
        "core/async.scm",
        include_str!("../../../runtime/core/async.scm"),
    ),
    (
        "core/commands.scm",
        include_str!("../../../runtime/core/commands.scm"),
    ),
    (
        "core/hooks.scm",
        include_str!("../../../runtime/core/hooks.scm"),
    ),
    (
        "core/modes.scm",
        include_str!("../../../runtime/core/modes.scm"),
    ),
    (
        "core/test.scm",
        include_str!("../../../runtime/core/test.scm"),
    ),
];

/// Name and source of each runtime file, in load order.
pub fn sources() -> Vec<(String, String)> {
    let root = std::env::var_os("AMACS_RUNTIME").map(PathBuf::from);
    FILES
        .iter()
        .map(|(name, embedded)| {
            let on_disk = root
                .as_ref()
                .and_then(|root| std::fs::read_to_string(root.join(name)).ok());
            (
                (*name).to_owned(),
                on_disk.unwrap_or_else(|| (*embedded).to_owned()),
            )
        })
        .collect()
}
