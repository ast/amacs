//! The Scheme runtime: the part of the editor that is written in Scheme.
//!
//! The files are compiled into the binary. Setting `AMACS_RUNTIME` to the
//! `runtime` directory of a source checkout loads them from disk instead, so
//! that they can be changed without rebuilding.

use std::path::PathBuf;

macro_rules! file {
    ($name:literal) => {
        ($name, include_str!(concat!("../../../runtime/", $name)))
    };
}

/// Runtime files in load order, relative to the runtime directory.
const FILES: &[(&str, &str)] = &[
    file!("core/compat.scm"),
    file!("core/async.scm"),
    file!("core/prompts.scm"),
    file!("core/commands.scm"),
    file!("core/hooks.scm"),
    file!("core/buffers.scm"),
    file!("core/modes.scm"),
    file!("core/themes.scm"),
    file!("core/tools.scm"),
    file!("modes/languages.scm"),
    file!("lib/avy.scm"),
    file!("lib/dired.scm"),
    file!("lib/section.scm"),
    file!("lib/menu.scm"),
    file!("lib/git.scm"),
    file!("core/test.scm"),
    file!("core/debug.scm"),
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
