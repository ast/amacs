# amacs

An Emacs-like editor with a Rust core and Scheme (Steel) as its extension
language. The goal is a daemon with thin terminal and Wayland frontends.

## Status

Milestones M0 to M3 of the plan are done: the editor runs in a terminal, alone
or as a server with any number of terminal clients, edits files, and is
extended in Scheme.

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

- A server (`amacs --daemon`) with `amacsclient` for frames, files and
  `--eval`; auto-save with recovery after a crash

Not yet: the Wayland frontend, syntax highlighting, LSP, and
everything else in the later milestones.

## Building and running

    nix develop          # or any Rust 1.95 toolchain
    cargo build --release
    target/release/amacs FILE...

Keys follow Emacs defaults: `C-x C-f` opens a file, `C-x C-s` saves, `C-x C-c`
quits, `M-x` runs a command by name, `M-:` evaluates a Scheme expression.

## Server and clients

`amacs --daemon` runs the editor without a terminal. `amacsclient` talks to it
and starts it if it is not running:

    amacsclient -t [FILE]...       # open a frame in this terminal
    amacsclient FILE               # visit FILE and wait until C-x # finishes it
    amacsclient -n FILE            # visit FILE and return at once
    amacsclient +12:4 FILE         # go to line 12, column 4
    amacsclient --eval '(buffer-list)'

`EDITOR=amacsclient` works for `git commit` and the like: without `-t`, the
file opens in a frame that some client already has, or else in this terminal.
In a client frame, `C-x C-c` closes that frame only; `M-x kill-editor` stops
the server.

The socket is `$XDG_RUNTIME_DIR/amacs/server.sock` (override with
`AMACS_SOCKET` or `-s`). The Nix package installs systemd user units,
`amacs.service` and a socket-activated `amacs.socket`.

Modified buffers that visit files are auto-saved to
`$XDG_STATE_HOME/amacs/auto-save` after five idle seconds, every thirty
seconds while busy, and when the server is asked to stop or crashes. Visiting
a file that has auto-save data says so; `M-x recover-this-file` restores it.
The visited file itself is only ever written by saving.

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
| `amacs-server` | The `amacs` binary: standalone editor, server, batch mode |
| `amacs-client` | The `amacsclient` binary |
| `amacs-script` | Editor primitives for Scheme and the Steel backend |
| `amacs-testkit` | Headless frontend for tests |

`runtime/` holds the part of the editor written in Scheme, and its tests.

`docs/steel-spike.md` records the Steel feasibility results.
