# amacs

An Emacs-like editor with a Rust core and Scheme as its extension language.
The goal is a daemon with thin terminal and Wayland frontends.

The Scheme is the editor's own, made so that the editor can be changed while
it runs: see `docs/scheme.md`.

## Status

Milestones M0 to M10 of the plan are done: the editor runs in a terminal or
a Wayland window, alone or as a server with any number of clients, edits
files with syntax highlighting, indentation and language servers, has a file
manager and a git interface, and is extended in Scheme while it runs, by
people and by language models.

Working now:

- Buffers on a rope, with markers and a branching undo tree
- Windows: split below and right, delete, cycle
- Emacs key sequences, prefix arguments, `ESC` as Meta, the kitty keyboard
  protocol where the terminal supports it
- Kill ring, region, yank and yank-pop
- Minibuffer with a candidate list that narrows as you type (below)
- Atomic saves, with a warning if the file changed on disk
- Files are looked at before they are read: one that is not text is
  refused, and one above 50 MB (`large-file-threshold`) is read after a
  question
- Line wrapping, tabs, wide characters, incremental redraw
- Scheme: commands, key bindings, options, hooks, major and minor modes, an
  init file, prompts that suspend the command, and `C-g` to stop runaway code

- Syntax highlighting and indentation for 22 languages, the Modus themes,
  line numbers, paired delimiters and paredit (below)
- Language servers: diagnostics, completion, definitions and references,
  rename, code actions, symbols; snippets; formatting on save; direnv (below)
- Projects, jumping by label, the undo history as a tree, rectangles,
  marks over the text, files re-read when they change on disk (below)
- A server (`amacs --daemon`) with `amacsclient` for frames, files and
  `--eval`; auto-save with recovery after a crash
- A Wayland window (`amacs-gui`, or `amacsclient -c`) with shaped text,
  ligatures, colour emoji, fractional scaling and the system clipboard
- A file manager (`C-x d`) and git views (`C-x g`): status, staging by file
  or hunk, commits, branches, stashes, logs, blame, marks in the gutter
  (below)
- A Model Context Protocol server (`amacs-mcp`) through which a language
  model uses, inspects and changes the running editor: see `docs/ai.md`

Not yet: a terminal emulator, Org, spelling, remote files, and the rest of
the later milestones. `TODO.md` lists what is left.

## Building and running

    nix develop          # Rust, libxkbcommon and pkg-config
    cargo build --release
    target/release/amacs FILE...

Keys follow Emacs defaults: `C-x C-f` opens a file, `C-x C-s` saves, `C-x C-c`
quits, `M-x` runs a command by name, `M-:` evaluates a Scheme expression.

To work on the editor, `just restart` builds a release and starts a server
on a socket of its own, `$XDG_RUNTIME_DIR/amacs/dev.sock`, apart from one
that may be running for real work, and `just open FILE` opens a window on
it. `just` alone lists the rest.

With direnv, `direnv allow` once is all it takes: the `.envrc` loads the
flake's shell, puts `target/release` on the path and points `amacsclient`
at the development server. Without direnv, `nix develop` and
`export AMACS_SOCKET="$XDG_RUNTIME_DIR/amacs/dev.sock"` do the same.

## Choosing things in the minibuffer

A prompt that has candidates lists them under the input. Typing narrows them:
every word must occur in a candidate, in any order, and an uppercase letter
makes the match case-sensitive.

| Key                                      | Does                                                               |
| ---------------------------------------- | ------------------------------------------------------------------ |
| `RET`                                    | Accept the selected candidate; enter a selected directory          |
| `M-RET`                                  | Accept the text as typed, for a new name that matches an old one   |
| `C-n`, `C-p`, `C-v`, `M-v`, `M-<`, `M->` | Move the selection                                                 |
| `TAB`                                    | Put the candidate in the input, to edit it                         |
| `DEL`                                    | After a `/` in a file name, go up a directory                      |
| `M-p`, `M-n`                             | Earlier and later inputs of the same kind                          |
| `C-.`                                    | Act on the candidate another way: copy, insert, kill, other window |

Commands built on it: `M-x`, `C-x C-f`, `C-x b` (buffers and recent files),
`C-s` (lines of the buffer, with a live preview), `C-c /` (the project's
files, with ripgrep, as you type), `C-x C-r` (recent files) and `M-y` (the
kill ring). Scheme code gets the same with `(completing-read prompt list)`.

After a prefix key such as `C-x`, a short pause shows the keys that can
follow. Input history and recent files are kept in `$XDG_STATE_HOME/amacs`.

## Languages

A file's name selects its major mode, which sets the buffer's language. The
language gives it highlighting from a tree-sitter grammar, kept up to date as
you type, and indentation.

Languages: Bash, C, C++, CMake, CSS, Devicetree, Emacs Lisp, Go, HCL and
Terraform, HTML, JavaScript, JSON, Make, Markdown, Nix, Python, Rust, Scheme,
TOML, TSX, TypeScript and YAML. The grammars are compiled into the binary.
Dockerfiles have a mode but no grammar.


| Key                           | Does                                                                |
| ----------------------------- | ------------------------------------------------------------------- |
| `TAB`                         | Indent the line, or the lines of the active region                  |
| `RET`                         | Start a new line, indented; trailing blanks on the old line go      |
| `C-M-\`                       | Indent the region                                                   |
| `C-M-f`, `C-M-b`              | Move over a bracketed expression, string or word                    |
| `C-M-u`, `C-M-d`, `C-M-n`     | Out of the enclosing list backward, into the next list, out forward |
| `C-M-k`, `C-M-DEL`, `C-M-SPC` | Kill the expression after or before point; mark it                  |

How lines are indented depends on the language:

- Languages with brackets are indented from the syntax tree: one level for
  each bracket pair or similar construct that is open at the start of the
  line. Arguments are not lined up under the opening bracket. Where a syntax
  error breaks the tree, the line above decides.
- Lisp is indented as in Emacs: bodies of special forms by two columns,
  arguments under the first argument.
- In Python and YAML the line above decides, and pressing `TAB` again steps
  out one level at a time. `indent-region` refuses to guess there.
- Elsewhere a new line starts where the last one did and `TAB` moves to the
  next tab stop.

The options `indent-offset` and `indent-tabs-mode` are set by each mode and
can be changed in a mode hook. `line-numbers` is `#f`, `'absolute` or
`'relative`.

Two minor modes deal with delimiters:

- `auto-pair-mode` (global) inserts brackets and quotes in pairs, steps over
  a closing delimiter that is typed, deletes an empty pair as one, wraps the
  active region, and opens a block when `RET` is pressed between brackets.
- `paredit-mode` keeps Lisp code balanced. `(`, `[` and `"` insert pairs;
  `)` leaves the list; `DEL`, `C-d` and `C-k` never delete half a pair;
  `C-)` and `C-}` (also `C-<right>`, `C-<left>`) slurp and barf forward,
  `C-(` and `C-{` backward; `M-s` splices, `M-r` raises, `M-(` wraps.

```scheme
(add-hook! 'prog-mode-hook (lambda () (set-local! 'line-numbers 'relative)))
(enable-mode! 'auto-pair-mode)
(add-hook! 'scheme-mode-hook (lambda () (enable-mode! 'paredit-mode)))
```

## Markdown

Markdown is highlighted in two layers: the blocks, and what is within
paragraphs, headings and table cells, such as emphasis, code and links.
The code in a fenced block is highlighted as its language when the fence
names one that the editor knows (` ```rust `, ` ```sh `), front matter as
YAML or TOML, and blocks of HTML as HTML.

| Key                             | Does                                                                                                                       |
| ------------------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| `TAB`, `S-TAB`                  | In a table: line up its columns and go to the next or previous cell; after the last cell, start a new row                  |
| `C-c C-c`                       | Line up the table at point, or tick or clear the box of a list item                                                        |
| `C-c \|`                        | Insert a table                                                                                                             |
| `RET`                           | In a list: start the next item, with the next number and an empty box if the list has them; on an empty item, end the list |
| `C-c C-n`, `C-c C-p`, `C-c C-j` | Next heading, previous heading, a heading chosen by its text                                                               |
| `M-<left>`, `M-<right>`         | Promote or demote the heading at point                                                                                     |

Lining up a table changes its text: every cell of a column is made as wide
as the widest, counted in screen columns, and placed as the row of dashes
says (`:--` left, `--:` right, `:-:` in the middle). It is written in
Scheme, in `runtime/lib/markdown.scm`.

## Language servers

A buffer is attached to a language server when its mode's hook calls
`lsp-ensure`. Servers are started once per project, which is the nearest
directory above the file with a `.git` or `.projectile`, and shared by its
buffers. Programs are set with `lsp-server!`; clangd, rust-analyzer, nixd,
gopls, pyright, terraform-ls and typescript-language-server are known by
default.

```scheme
(lsp-server! "rust" '("rust-analyzer") "{\"check\": {\"command\": \"clippy\"}}")
(add-hook! 'rust-mode-hook lsp-ensure)
```

The third argument is the server's initialization options, as JSON.

| Key | Does |
|---|---|
| `M-.`, `M-,`, `M-?` | Go to the definition; go back; list references |
| `C-M-i` | Complete at point (completions also pop up as you type) |
| `C-c C-r` | Rename the symbol at point in every file |
| `C-c C-a` | Code actions at point or for the region |
| `C-c C-d` | Describe the symbol at point |
| `M-g i`, `C-c C-s` | Go to a symbol of the buffer; find one in the project |
| `M-g n`, `M-g p`, `C-c !` | Next and previous diagnostic; list them |
| `C-x s` | Save every modified file, as after a rename |

Diagnostics are underlined in the text and counted in the mode line, which
also names the server and what it is busy with. When point rests on a
diagnostic its message is shown in the echo area, and otherwise the
signature of the symbol there. `M-x lsp-restart` and `M-x lsp-shutdown` do
what they say; what a server writes to its log and standard error is in the
`*lsp-log*` buffer.

A rename or code action that touches files that are not open opens them.
Nothing is saved until you save.

### Completion

The list of completions appears shortly after two chars of a word are typed,
or at once after a char the server asks for, such as `.`. Typing narrows it;
`C-n` and `C-p` move in it; `RET` or `TAB` inserts the selection; `C-g`
closes it. Without a language server the candidates are the words of the
buffer. The names of the snippets for the mode are always among them. The
options `completion-delay` (milliseconds; negative for never) and
`completion-min-prefix` tune it.

### Snippets

```scheme
(define-snippets 'rust-mode
  `((fn "fn " p "(" p ") {" n> r> n "}")
    (today ,(lambda () (format-current-time "%Y-%m-%d")))))
```

A template is strings and symbols: `p` is a field, `(p "text")` a field
with a placeholder, `(s name)` a field repeated wherever the same name
appears, `q` where point ends up, `n` a line break, `n>` a line break with
indentation and `>` indentation. A procedure is called for its text.
`M-+` expands the snippet named before point or lists those it could be,
and `M-*` inserts one by name. `TAB` and `S-TAB` move between the fields.
Snippets that language servers send with their completions work the same.

### Formatting

```scheme
(format-on-save! 'rust-mode 'rustfmt)
(define-formatter 'clang-format "clang-format" '("--style=llvm" "--assume-filename" "{file}"))
```

A formatter reads the text on standard input and writes it formatted to
standard output. `C-c C-f` formats the buffer; `format-on-save!` also does
so on every save of a mode's buffers. Only the lines that change are
replaced, in one undo step. If the formatter fails the buffer is saved as it
is and the echo area says why. black, clang-format, cmake-format, gofmt,
goimports, nixfmt, prettier, rustfmt, shfmt and tofu-fmt are defined.

### direnv

Language servers and formatters are started with the environment that
`direnv` gives for the file's directory, so a project's `.envrc` (for
example `use flake`) decides which rust-analyzer or clang-format runs. The
editor's own environment does not change. The first file of a project waits
for direnv before its server starts, and the mode line says so. A blocked
`.envrc` is reported; run `direnv allow` and then `M-x direnv-reload` and
`M-x lsp-restart`. Set the option `use-direnv` to `#f` to do without.

## Getting about

**Jumping by label**, as avy does: `M-g c` and a char puts a label on every
place on screen with that char; pressing the label goes there. The nearest
places get one key, the rest two. `M-g w` takes the first char of a word,
`M-g l` labels the lines, and `avy-goto-char-2` takes two chars. `M-z` and
`M-Z` kill from point through, or up to, a char picked the same way.

**Windows**: `M-o` selects another window, by label when there are more than
two. `S-<arrow>` selects the window in that direction. `C-x ^`, `C-x }` and
`C-x {` resize, `C-x +` evens out, and `C-x 4 f` and `C-x 4 b` open a file
or buffer in another window.

**Projects**, with the keys of projectile: `C-c p f` picks a file of the
project, `C-c p p` another project visited before, `C-c p b` one of the
project's buffers, `C-c p k` kills them, and `C-c p s` searches. A project
starts at the nearest directory above a file that holds one of the names in
the `project-root-markers` option (`.git` and `.projectile` unless changed);
language servers start there too. Listing files uses ripgrep, so ignored
files are left out.

**The undo tree**: `M-x undo-tree-visualize` shows the history under the echo
area. Undoing and then editing starts a new branch and keeps the old one.
`b` and `f` step back and forward, `p` and `n` cross to another branch, `q`
keeps the text as it is, and `C-g` returns to where you started.

**Rectangles**: `C-x SPC` marks one, shown as the columns between point and
mark; `C-w` and `M-w` then kill and copy it. With an ordinary region,
`C-x r k`, `C-x r M-w`, `C-x r d`, `C-x r o` and `C-x r t` kill, copy,
delete, open and fill the rectangle between point and mark, and `C-x r y`
yanks the last one.

**Bookmarks**: `C-x r m` names a place in a file and `C-x r b` returns to
one. They are kept across sessions.

## Files and git

**The file manager**: `C-x d`, or visiting a directory, lists it, with the
keys of dired, and `C-x C-j` lists the directory that the current file is
in, with point on the file. `RET` visits, `^` goes up, `TAB` opens a directory in place,
`/` narrows to the names that contain a text, `.` hides the dot files and
`s` changes the order. `m`, `u`, `U` and `t` mark; `C` copies, `R` moves or
renames and `D` deletes the marked files or the one at point, and with two
listings side by side the other one is offered as where to. `+` makes a
directory, `b` goes to a directory of the `dired-quick-access` option and
`r` to one shown before.

Opening a file that is not text, such as a picture or a disc image, hands
it to `xdg-open` (the `dired-open-program` option), and `E` does that for
any file.

It works with the pointer too: a second press on a file opens it,
`C-<mouse-1>` and `S-<mouse-1>` mark, the button for menus lists what can be
done, and files are dragged into a directory of a listing or over to
another listing, which moves them, or copies them with Control held. A
press on a part of the directory's name in the first line goes up to that
directory, and so does the back button of a mouse, one level at a time.

**Git**: `C-x g` shows the status of the repository of the current buffer,
in sections that `TAB` folds. `s` and `u` stage and unstage the file or hunk
at point, `k` discards it, and `RET` goes to the change. `c` commits, `b`
is for branches, `P`, `F` and `f` push, pull and fetch, `z` stashes, `r`
rebases and `l` shows the log; each opens a menu of what it can do.
`M-x git-blame` shows who last changed each line, and `git-gutter-mode`
marks the changed lines of a file beside their numbers.

Both are written in Scheme, in `runtime/lib/dired.scm` and
`runtime/lib/git.scm`; `docs/extending.md` has all their keys and the parts
that other code can use.

## Smaller things

Four minor modes lay marks over the text. Turn them on in a mode hook with
`(enable-mode! 'hl-line-mode)` and so on:

| Mode | Marks |
|---|---|
| `hl-line-mode` | The line point is on |
| `symbol-highlight-mode` | The other occurrences of the symbol at point |
| `hl-todo-mode` | TODO, FIXME, NOTE and their kin, in comments |
| `whitespace-mode` | Trailing whitespace and text past `whitespace-line-column` |

`symbol-next` and `symbol-previous` step through the occurrences of the
symbol at point, and `symbol-rename` renames it throughout the buffer.

A buffer without changes is read again when another program changes its
file, within five seconds; the echo area says so. Set the `auto-revert`
option to `#f` to stop that. `M-x revert-buffer` does it on demand.

Editing: `M-%` query-replace, `M-;` comment or uncomment the region or line,
`move-text-up` and `move-text-down`, `M-u`, `M-l` and `M-c` for letter case,
`C-x C-u` and `C-x C-l` for the region, `M-m`, `M-^`, `M-\` and `M-SPC`,
`C-x C-q` read-only, `M-!` and `M-|` for shell commands (with `C-u`, the
output replaces the region), and `scroll-up-line` and `scroll-down-line`.
`C-h k` says what a key runs and `C-h f` what a command does.

## Themes

Faces take their colours from the palette of the current theme. The four
Modus themes are built in: `modus-operandi`, `modus-vivendi` and their
`-tinted` variants; `modus-operandi-tinted` is the default. The palettes are
those of the modus-themes package for Emacs by Protesilaos Stavrou, which is
licensed under the GPL, version 3 or later.

```scheme
(load-theme 'modus-vivendi-tinted)
;; Replace palette entries in every theme.
(set-palette-overrides! '((comment yellow-faint) (string green-warmer)))
;; Change one face. Colours are palette names or "#rrggbb".
(set-face! 'keyword 'fg 'magenta 'bold #f)
;; A theme of your own, starting from another.
(define-theme 'mine 'modus-vivendi '((bg-main "#101418")))
```

`M-x toggle-theme` switches between the themes in the `themes-to-toggle`
option, and `M-x choose-theme` picks one by name. The faces are listed in
`crates/amacs-core/src/faces.rs`; the syntax faces are `comment`, `doc`,
`string`, `escape`, `regexp`, `constant`, `number`, `keyword`, `operator`,
`bracket`, `delimiter`, `function-name`, `function-call`, `builtin`,
`preprocessor`, `type`, `variable-name`, `property`, `heading`, `emphasis`,
`strong`, `code` and `link`.

## Server and clients

`amacs --daemon` runs the editor without a terminal. `amacsclient` talks to it
and starts it if it is not running:

    amacsclient -t [FILE]...       # open a frame in this terminal
    amacsclient -c [FILE]...       # open a frame in a new window
    amacsclient FILE               # visit FILE and wait until C-x # finishes it
    amacsclient -n FILE            # visit FILE and return at once
    amacsclient +12:4 FILE         # go to line 12, column 4
    amacsclient --eval '(buffer-list)'
    amacsclient --kill-server      # stop the server, as after rebuilding

A client and a server from different builds may not understand each other;
the client then says so, and `amacsclient --kill-server` stops the old
server so that the next client starts a new one. It asks the server to
terminate, which makes it auto-save unsaved buffers first.

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

Text is shaped per run, so programming ligatures form, and then placed on
the cell grid.

Drawing is in parts (`crates/amacs-gui/src/render/`): what the server said
to show, the fonts, a list of the rectangles and glyphs that each row is
made of, and a backend that puts the list on screen. There are two
backends, which give the same picture.

The one that is used unless another is asked for draws on the CPU into
memory shared with the compositor, repainting only what changed. The other
draws with the GPU, through Vulkan, the whole frame each time from an atlas
of glyphs:

    amacs-gui --renderer gpu FILE
    AMACS_RENDERER=gpu amacsclient -c FILE

It says on standard error which GPU it draws with, or that it draws on the
CPU after all and why, as when there is no Vulkan.

The server reaches the system clipboard through `wl-copy` and `wl-paste`, for
terminal frames too: a kill is copied, and `C-y` yanks what another program
copied last. This needs `WAYLAND_DISPLAY` in the server's environment.

A click puts point there and selects the window, and the wheel scrolls the
window under the pointer. Dragging with the button held selects text, as
the region; a second click takes the word and a third the line, and
dragging on from there adds to it. Past the top or bottom of the window the
text follows the pointer.

Buttons are keys, `<mouse-1>` to `<mouse-3>` and what is done with them
held, so modes and the init file bind them as they bind other keys:
`docs/extending.md` says how.

Not there yet: the mouse in the terminal, dragging to and from other
programs, input methods and compose
sequences, and a native clipboard without the helper programs.

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
written in Scheme under `runtime/core/`, and the language modes under
`runtime/modes/`. Things to know:

- Buffer positions are zero-based.
- A prompt such as `read-string` suspends the command until the user answers.
  It works inside commands and hooks, not at the top level of a file.
- Primitives act on the current buffer, which is that of the selected window
  unless `(with-current-buffer name ...)` says otherwise.
- Hooks run after the event that triggered them, not in the middle of it,
  except `pre-command-hook` and `before-save-hook`, which the command or the
  save waits for.
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
The language server tests run against a small server of their own
(`crates/amacs-testkit/examples/fake-lsp.rs`) that speaks the real protocol.

## Layout

| Crate           | Role                                                                        |
| --------------- | --------------------------------------------------------------------------- |
| `amacs-text`    | Rope, markers, undo tree                                                    |
| `amacs-proto`   | Keys, input events and draw operations shared with frontends                |
| `amacs-treesit` | Grammars, highlighting queries and indentation rules                        |
| `amacs-lsp`     | Language Server Protocol transport, positions and URIs                      |
| `amacs-core`    | Editor state, commands, keymaps, redisplay                                  |
| `amacs-tty`     | Terminal frontend                                                           |
| `amacs-server`  | The `amacs` binary: standalone editor, server, batch mode                   |
| `amacs-client`  | The `amacsclient` binary and the connection code                            |
| `amacs-gui`     | The `amacs-gui` binary: the Wayland window and its renderer                 |
| `amacs-scheme`  | The editor's own Scheme: reader, compiler, virtual machine                  |
| `amacs-script`  | Editor primitives for Scheme, and the session that runs the two together    |
| `amacs-mcp`     | The `amacs-mcp` binary: a Model Context Protocol server for language models |
| `amacs-testkit` | Headless frontend for tests                                                 |

`runtime/` holds the part of the editor written in Scheme, and its tests.

`docs/scheme.md` describes the editor's own Scheme, `docs/extending.md` how
to extend the editor with it, and `docs/ai.md` how language models use,
inspect and change the running editor.
