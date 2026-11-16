# amacs

An Emacs-like editor with a Rust core and Scheme (Steel) as its extension
language. The goal is a daemon with thin terminal and Wayland frontends.

## Status

Milestones M0 to M2 of the plan are done: the editor runs as a single process
in a terminal, edits files, and is extended in Scheme.

Working now:

- Buffers on a rope, with markers and a branching undo tree
- Windows: split below and right, delete, cycle
- Emacs key sequences, prefix arguments, `ESC` as Meta, the kitty keyboard
  protocol where the terminal supports it
- Kill ring, region, yank and yank-pop
- Minibuffer with prefix completion for files, buffers and commands
- Atomic saves, with a warning if the file changed on disk
- Line wrapping, tabs, wide characters, incremental redraw
- Scheme: commands, key bindings, options, hooks, major and minor modes, an
  init file, prompts that suspend the command, and `C-g` to stop runaway code

Not yet: the daemon, the Wayland frontend, syntax highlighting, LSP, and
everything else in the later milestones.

## Building and running

    nix develop          # or any Rust 1.95 toolchain
    cargo run --release -p amacs-server -- FILE...

Keys follow Emacs defaults: `C-x C-f` opens a file, `C-x C-s` saves, `C-x C-c`
quits, `M-x` runs a command by name, `M-:` evaluates a Scheme expression.

## Scheme

The init file is `$XDG_CONFIG_HOME/amacs/init.scm`. `examples/init.scm` is a
starting point:

```scheme
(set-option! 'tab-width 4)

(define-command (insert-date)
  "Insert today's date as YYYY-MM-DD."
  (insert (format-current-time "%Y-%m-%d")))

(global-set-key "C-c d" 'insert-date)

(define-command (greet)
  "Ask for a name and insert a greeting."
  (insert "Hello, " (read-string "Name: ")))

(add-hook! 'scheme-mode-hook (lambda () (set-local! 'tab-width 2)))
```

The primitives are listed with their documentation in
`crates/amacs-script/src/api.rs`. The rest of the Scheme API (`define-command`,
`define-major-mode`, `add-hook!`, `read-string`, `deftest` and so on) is
written in Scheme under `runtime/core/`. Things to know:

- Buffer positions are zero-based.
- A prompt such as `read-string` suspends the command until the user answers.
  It works inside commands and hooks, not at the top level of a file.
- There is no `with-current-buffer` yet. Primitives act on the buffer of the
  selected window; some take a buffer name.
- Hooks run after the event that triggered them, not in the middle of it. A
  hook that must run before a save does not exist yet.
- Options hold data (numbers, strings, symbols, lists), not procedures.

Useful flags: `--init FILE`, `-q` (no init file), `-l FILE`, `--eval EXPR`, and
`--batch` to run without a terminal.

## Testing

    cargo test                                   # unit, end-to-end and property tests
    amacs --batch -q -l runtime/tests/core.scm --test   # Scheme tests only
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
| `amacs-script` | Editor primitives for Scheme and the Steel backend |
| `amacs-testkit` | Headless frontend for tests |

`runtime/` holds the part of the editor written in Scheme, and its tests.

`docs/steel-spike.md` records the Steel feasibility results.
