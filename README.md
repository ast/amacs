# amacs

An Emacs-like editor with a Rust core and Scheme (Steel) as its extension
language. The goal is a daemon with thin terminal and Wayland frontends.

## Status

Milestones M0 and M1 of the plan are done: the editor runs as a single process
in a terminal and can edit files.

Working now:

- Buffers on a rope, with markers and a branching undo tree
- Windows: split below and right, delete, cycle
- Emacs key sequences, prefix arguments, `ESC` as Meta, the kitty keyboard
  protocol where the terminal supports it
- Kill ring, region, yank and yank-pop
- Minibuffer with prefix completion for files, buffers and commands
- Atomic saves, with a warning if the file changed on disk
- Line wrapping, tabs, wide characters, incremental redraw

Not yet: Scheme, the daemon, the Wayland frontend, syntax highlighting, LSP,
and everything else in the later milestones. See `docs/steel-spike.md` for the
Steel feasibility results.

## Building and running

    nix develop          # or any Rust 1.95 toolchain
    cargo run --release -p amacs-server -- FILE...

Keys follow Emacs defaults: `C-x C-f` opens a file, `C-x C-s` saves, `C-x C-c`
quits, `M-x` runs a command by name.

## Testing

    cargo test                                   # unit, end-to-end and property tests
    cargo test --release -- --ignored large_file # 100 MB file round trip
    cargo deny check                             # dependency policy
    nix flake check                              # build, tests, clippy, rustfmt

End-to-end tests drive the editor through a headless frontend
(`crates/amacs-testkit`): they press keys and assert on the resulting screen.

## Layout

| Crate | Role |
|---|---|
| `amacs-text` | Rope, markers, undo tree |
| `amacs-proto` | Keys, input events and draw operations shared with frontends |
| `amacs-core` | Editor state, commands, keymaps, redisplay |
| `amacs-tty` | Terminal frontend |
| `amacs-server` | The `amacs` binary |
| `amacs-script` | Scheme host (currently only the Steel spike) |
| `amacs-testkit` | Headless frontend for tests |
