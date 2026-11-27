# Working on amacs

amacs is an Emacs-like editor: a Rust core, extended in its own Scheme, with
terminal and Wayland frontends that are thin clients of a server. Read
`README.md` for the lie of the land, `docs/scheme.md` for the language and
`docs/extending.md` for what Scheme code can do.

## Where things go

New features are written in Scheme, under `runtime/`, unless they are hot
paths: text storage, redisplay, tree-sitter, matching. When a feature needs
something the core does not offer, add the primitive in Rust
(`crates/amacs-script/src/api.rs`, with the logic in `crates/amacs-core`) and
write the feature in Scheme on top of it. `runtime/lib/avy.scm`,
`runtime/lib/dired.scm` and `runtime/lib/git.scm` are the models to follow.

Every procedure meant to be used starts with a docstring; macros and commands
get theirs through `set-documentation!` and `define-command`. A test checks
that nothing is left without.

## The running editor is the workbench

The `amacs` MCP server (`.mcp.json`, built by `just build`) connects to the
development server at `$XDG_RUNTIME_DIR/amacs/dev.sock` and starts one if
none is running. That server is set up by `examples/init.scm` (the
`AMACS_INIT` variable of `.envrc`, `justfile` and `.mcp.json`), not by the
init file in `~/.config`. Use it instead of guessing:

- `describe` and `apropos` before writing code that uses a name.
- `eval` to try an expression; errors come with their place and trace.
- `load` after editing a file under `runtime/`: definitions take effect at
  once, without a rebuild or a restart.
- `keys`, `type` and `screen` to use a feature as a person would and see
  what they would see; `screen` with `image` for colours and faces.
- `messages` for the full report of an error in a command, and for the
  output of `(trace! 'name)`.
- `eval` of `(display (telemetry-report))` for what ran and how long it
  took: commands, hooks, events, redisplay. With `#t` as its argument the
  report is of every session, the user's own among them.

Only changes to Rust need `just restart`. After a restart the MCP server
reconnects by itself on the next call.

## Building

`just restart` builds the dev profile: incremental, with optimised
dependencies, linked with mold. `cargo test` shares that build. The linker
flag is an environment variable of the devShell in `flake.nix`, not a
`.cargo/config.toml`, because the Nix build reads that file and has no
mold. `AMACS_PROFILE=release just restart` builds a release instead.

Builds and tests use six cores, not all of them: `CARGO_BUILD_JOBS=6` for
cargo and `just`. The flake check goes to the remote builder `evo`, which
`/etc/nix/machines` lists, so that this machine stays free:
`nix flake check --max-jobs 0 --cores 0`. When `evo` is not reachable,
`nix flake check --cores 6 --max-jobs 1` builds here. `just check-remote`
runs the pre-commit checks on `evo` from a copy of the working tree.

## Before a commit

`just check` runs what is needed short of the flake: format, clippy with
warnings as errors, the tests, `cargo deny`. `nix flake check` is the full
check and takes ten minutes or more; it only sees files that git knows, and
its build sandbox has no network and its home directory is the build
directory, which has caught tests that assumed otherwise.

Tests of features written in Scheme are in `crates/amacs-script/tests`, with
a harness that feeds keys and reads the screen. Tests of the server and of
the MCP server are in `crates/amacs-server/tests`.

## Constraints

- `unsafe` is forbidden in the workspace, with one exception that is not to
  grow: `crates/amacs-gpu-surface` holds the single call that hands a GPU's
  driver the pointers of a Wayland window. Anything else that seems to need
  `unsafe` needs another way.
- `~/src/nixos`, the Emacs configuration that the editor is modelled on, is
  for reading only.
