//! The Scheme runtime: the part of the editor that is written in Scheme.
//!
//! The files are compiled into the binary. Setting `AMACS_RUNTIME` to the
//! `runtime` directory of a source checkout loads them from disk instead, so
//! that they can be changed without rebuilding.

use std::path::PathBuf;

use crate::host::Engine;

macro_rules! file {
    ($name:literal) => {
        ($name, include_str!(concat!("../../../runtime/", $name)))
    };
}

/// Runtime files in load order, relative to the runtime directory. The
/// first two are written for one engine each.
const STEEL: &[(&str, &str)] = &[file!("core/compat.scm"), file!("core/async.scm")];
const OWN: &[(&str, &str)] = &[file!("core/compat-own.scm"), file!("core/async-own.scm")];
const SHARED: &[(&str, &str)] = &[
    file!("core/prompts.scm"),
    file!("core/commands.scm"),
    file!("core/hooks.scm"),
    file!("core/modes.scm"),
    file!("core/themes.scm"),
    file!("core/tools.scm"),
    file!("modes/languages.scm"),
    file!("core/test.scm"),
];

/// Name and source of each runtime file for an engine, in load order.
pub fn sources(engine: Engine) -> Vec<(String, String)> {
    let root = std::env::var_os("AMACS_RUNTIME").map(PathBuf::from);
    let own = match engine {
        Engine::Steel => STEEL,
        Engine::Own => OWN,
    };
    own.iter()
        .chain(SHARED)
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
