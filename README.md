# amacs

An Emacs-like editor with a Rust core and Scheme (Steel) as its extension
language. The goal is a daemon with thin terminal and Wayland frontends.

## Status

Milestones M0 to M5 of the plan are done: the editor runs in a terminal or a
Wayland window, alone or as a server with any number of clients, edits files,
and is extended in Scheme.

Working now:

- Buffers on a rope, with markers and a branching undo tree
- Windows: split below and right, delete, cycle
- Emacs key sequences, prefix arguments, `ESC` as Meta, the kitty keyboard
  protocol where the terminal supports it
- Kill ring, region, yank and yank-pop
- Minibuffer with a candidate list that narrows as you type (below)
- Atomic saves, with a warning if the file changed on disk
- Line wrapping, tabs, wide characters, incremental redraw
- Scheme: commands, key bindings, options, hooks, major and minor modes, an
  init file, prompts that suspend the command, and `C-g` to stop runaway code

- A server (`amacs --daemon`) with `amacsclient` for frames, files and
  `--eval`; auto-save with recovery after a crash
- A Wayland window (`amacs-gui`, or `amacsclient -c`) with shaped text,
  ligatures, colour emoji, fractional scaling and the system clipboard

Not yet: syntax highlighting, LSP, and
everything else in the later milestones.

## Building and running

    nix develop          # Rust, libxkbcommon and pkg-config
    cargo build --release
    target/release/amacs FILE...

Keys follow Emacs defaults: `C-x C-f` opens a file, `C-x C-s` saves, `C-x C-c`
quits, `M-x` runs a command by name, `M-:` evaluates a Scheme expression.

## Choosing things in the minibuffer

A prompt that has candidates lists them under the input. Typing narrows them:
every word must occur in a candidate, in any order, and an uppercase letter
makes the match case-sensitive.

| Key | Does |
|---|---|
| `RET` | Accept the selected candidate; enter a selected directory |
| `M-RET` | Accept the text as typed, for a new name that matches an old one |
| `C-n`, `C-p`, `C-v`, `M-v`, `M-<`, `M->` | Move the selection |
| `TAB` | Put the candidate in the input, to edit it |
| `DEL` | After a `/` in a file name, go up a directory |
| `M-p`, `M-n` | Earlier and later inputs of the same kind |
| `C-.` | Act on the candidate another way: copy, insert, kill, other window |

Commands built on it: `M-x`, `C-x C-f`, `C-x b` (buffers and recent files),
`C-s` (lines of the buffer, with a live preview), `C-c /` (the project's
files, with ripgrep, as you type), `C-x C-r` (recent files) and `M-y` (the
kill ring). Scheme code gets the same with `(completing-read prompt list)`.

After a prefix key such as `C-x`, a short pause shows the keys that can
follow. Input history and recent files are kept in `$XDG_STATE_HOME/amacs`.

## Server and clients

`amacs --daemon` runs the editor without a terminal. `amacsclient` talks to it
and starts it if it is not running:

    amacsclient -t [FILE]...       # open a frame in this terminal
    amacsclient -c [FILE]...       # open a frame in a new window
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

## The Wayland window

`amacsclient -c` (or `amacs-gui` directly) opens a window on the server. With
`-n` it returns at once and the window stays; without, it lasts until the
files are finished or the window is closed.

The font comes from two options, for example in `init.scm`:

```scheme
(set-option! 'font-family "JetBrainsMono Nerd Font Mono")
(set-option! 'font-size 16)   ; pixels, before the output's scale
```

`--font` and `--font-size` override them for one window. The default family,
`monospace`, is whatever fontconfig says it is. The options are read when a
window opens; changing them does not affect windows that are already open.

The window draws on the CPU into shared memory, repainting only the rows that
changed. Text is shaped per run, so programming ligatures form, and then
placed on the cell grid. Colours are those of modus-operandi-tinted until
themes arrive.

The server reaches the system clipboard through `wl-copy` and `wl-paste`, for
terminal frames too: a kill is copied, and `C-y` yanks what another program
copied last. This needs `WAYLAND_DISPLAY` in the server's environment.

Not there yet: the mouse, input methods and compose sequences, and a native
clipboard without the helper programs.

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
| `amacs-client` | The `amacsclient` binary and the connection code |
| `amacs-gui` | The `amacs-gui` binary: the Wayland window and its renderer |
| `amacs-script` | Editor primitives for Scheme and the Steel backend |
| `amacs-testkit` | Headless frontend for tests |

`runtime/` holds the part of the editor written in Scheme, and its tests.

`docs/steel-spike.md` records the Steel feasibility results.
