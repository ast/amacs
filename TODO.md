# What is left

State on 2026-11-23. Milestones M0 to M10 of the original plan are in, and
since then: tooling for language models (the MCP server), mouse selection
and buttons as keys, the file manager driven by the pointer, Markdown,
files looked at before they are read, a renderer in parts with a second
backend that draws with the GPU, typing ahead while a command waits for
a program, a terminal emulator, a coding agent over the Agent Client
Protocol, and spelling. What is done is in the history; this file is what
is not.

Start by reading `CLAUDE.md`, then `docs/extending.md` and `docs/ai.md`.
Features go in Scheme under `runtime/`; Rust only for missing primitives.

The order within each section is a suggestion, most useful first.

## Next

What I would take first, across the sections below:

1. The GPU backend keeps the rows that did not change, and then becomes
   the default if it measures better (see "Drawing").
2. Files are read in the background (see "Files").
3. Dropping files from other programs into the editor (see "Mouse").

## Extension surface

- [ ] Markers as Scheme values.
- [ ] Records and hash tables cannot be passed to editor primitives.
- [ ] A long copy or move in the file manager holds typing until it is
      done or `C-g`, as any command that waits for a program does. It
      could call `(release-input!)`, at the price of the "replace it?"
      question no longer getting what was typed ahead.
- [ ] No command renames a buffer or parts it from its file.

## Editing commands (`runtime/lib/simple.scm`)

The small commands of Emacs were gone through on 2026-11-24; these are
the ones that are still not there, the ones that need the core first.

- [ ] Narrowing (`C-x n n`, `C-x n w`, `narrow-to-defun`): the core has to
      keep a buffer to a part of its text, for point, search and display.
- [ ] `repeat` (`C-x z`), which needs the last command with its argument
      and its keys, and `negative-argument` (`M--`).
- [ ] Help in the editor for more than commands and keys:
      `describe-function`, `describe-variable`, `describe-bindings`,
      `describe-mode`, `where-is`, `apropos`. The MCP server has all of it
      (`describe`, `apropos`); the commands only have to show it.
- [ ] Regular expressions in `C-s` (`search-lines`) and `C-c /`
      (`search-files`), and a way not to mind case in `M-%` and `C-M-%`
      without writing `(?i)`. The syntax is Rust's, not Emacs's, and no
      translation is offered.
- [ ] Keyboard macros: `apply-macro-to-region-lines` (`C-x C-k r`),
      counters, `insert-kbd-macro` to write one into `init.scm`, editing
      one. Pastes and the mouse are not recorded. `C-g` does not reach a
      macro that runs: one that never fails stops after 10 000 runs.
- [ ] Buffers in the order they were last shown, for `next-buffer`,
      `previous-buffer` (now by name) and `bury-buffer`;
      `kill-current-buffer`, `kill-buffer-and-window` (`C-x 4 0`).
- [ ] Sentences (`M-a`, `M-e`, `M-k`); definitions by the syntax tree
      (`C-M-a`, `C-M-e`, `C-M-h`, `eval-defun`); the ring of marks
      (`C-u C-SPC`, `C-x C-SPC`).
- [ ] `dabbrev-expand` (`M-/`), `overwrite-mode`, `capitalize-region`,
      `tabify` and `untabify`, `align-regexp`, `sort-fields`, the size of
      the text (`C-x C-+`).
- [ ] `visual-line-mode`: the rows after the first of a list item or an
      indented line start at the left edge; they could hang under the
      text.

## Coding agents (`runtime/lib/acp.scm`)

- [ ] Only the first page of the conversations that were is listed
      (`C-c c R`), and their times are shown in UTC.
- [ ] An image is read with the `base64` program; a primitive that reads
      a file as base64 would not need it. Pasting an image from the
      clipboard into a prompt.
- [ ] Sessions are never closed or deleted at the agent (`session/close`,
      `session/delete`), and each has an agent process of its own where
      one process could hold them all.
- [ ] Other agents than Claude by name: the commands are `claude-…` and
      there is one `acp-command`.
- [ ] Not tried against the real adapter, only the fake one: a prompt
      that waits its turn, one sent into a turn (`C-c C-s`), an image, and
      the servers and directories of the options.

## Scheme language (`crates/amacs-scheme`)

- [ ] Keyword arguments and a module system.
- [ ] R7RS suite: 735 pass, 231 fail, 14 forms do not read. The thresholds
      are in `tests/r7rs.rs`; raise them as failures are fixed.
- [ ] `call/cc` is escape-only. Full re-entrant continuations were left out
      on purpose; revisit only if something needs them.

## Files

- [ ] Files are read in one go on the editor's thread. A large file that
      was agreed to (`large-file-threshold`) blocks while it is read. Read
      in the background.
- [ ] No way to look inside a file that is not text (a hex view, or
      `find-file-literally`). `C-x C-f` refuses such a file; only the file
      manager hands it to `xdg-open`.

## File manager (`runtime/lib/dired.scm`)

- [ ] Preview pane.
- [ ] Refresh when the directory changes on disk.
- [ ] Renames inside a repository through `git mv`.
- [ ] No history to go back and forward in: `<mouse-8>` goes up and
      `<mouse-9>` does nothing.
- [ ] A plain drop always moves; other file managers copy when the target
      is on another file system.
- [ ] The window scrolls when a menu of keys takes rows from it, and does
      not scroll back.

## Git (`runtime/lib/git.scm`)

- [ ] Interactive rebase.
- [ ] The gutter updates only on open and save, and is drawn only when line
      numbers are on.
- [ ] Forge features (pull requests, issues). Out of scope for v1 in the
      plan.

## Mouse

- [ ] Drag and drop with other programs: Wayland's data device, which
      would also give a native clipboard in place of `wl-copy` and
      `wl-paste`. Dropping in first (`<drop>` with a list of paths),
      dragging out after. The client is on libwayland already.
- [ ] A selection made with the mouse does not become the primary
      selection, and the middle button does not paste it.
- [ ] A menu at the pointer for `<mouse-3>`; the file manager uses the
      menu of keys under the echo area.
- [ ] Mouse in the terminal frontend (only the Wayland client sends it).
- [ ] The pointer has tests in the editor's core and in Scheme only. What
      the Wayland client sends (buttons, drags, click counts) is tried by
      hand.

## Markdown (`runtime/lib/markdown.scm`, grammar in `crates/amacs-treesit`)

- [ ] Tables are lined up by changing the text. Showing an unaligned table
      aligned without touching it needs virtual text in redisplay.
- [ ] Hiding markup and a reading mode, as `markdown-ts-view-mode` has.
- [ ] Moving subtrees, a table of contents, struck-through text drawn
      struck through (it is drawn as a comment).
- [ ] Text in another language is parsed anew each time it is drawn
      (`paint_injections`); cache the trees if large documents feel slow.

## Drawing (`crates/amacs-gui/src/render`)

The CPU backend is the default; `--renderer gpu` or `AMACS_RENDERER=gpu`
chooses the other. Measured at 3840x2160 on an Intel integrated GPU
(`timing_of_both_backends_at_4k`): a keystroke costs 3.6 ms on the CPU and
4.6 ms on the GPU, a page of scrolling 11 ms and 6 ms. The GPU wins where
everything changes and loses where little does, because it builds the
whole frame anew.

- [ ] The GPU backend keeps the instances of rows that did not change
      (`Frame::changed` names the others). Then measure again; if a
      keystroke is no dearer than on the CPU, make it the default.
- [ ] An option in Scheme to choose the backend, beside the flag and the
      environment.
- [ ] The flake check has no Vulkan, so the tests of the GPU backend pass
      there without running. Mesa's lavapipe in the sandbox would run
      them.
- [ ] The cursor in the window is its cell inverted; the theme's cursor
      colour, which the terminal now uses, is ignored there.
- [ ] Proportional fonts.

## MCP server (`crates/amacs-mcp`)

- [ ] A way to press buttons of the pointer, so that a model can try what
      a person does with the mouse.
- [ ] A tool to list frames and to look at a user's frame instead of the
      server's own headless one.
- [ ] Insight into the Rust side: a panic in the server shows only as a
      dropped connection. Surface the panic message and the daemon log
      (`target/amacs-daemon.log`).
- [ ] The echo area keeps an old message ("Quit") on later screens, which
      reads as if it belonged to the last key.
- [ ] The picture from `screen` uses the server's configured font and falls
      back silently to another monospace font when it is missing.

## Text fields of a web browser (`runtime/lib/ghost-text.scm`)

- [ ] A change made in the page replaces everything between what the two
      texts share at the start and at the end. A real diff would keep
      point and the undo history finer when the page changes two places.
- [ ] The cursor goes to the page with changed text only: moving about in
      the buffer does not move the cursor in the page.
- [ ] A change that no command made, as by a coding agent, goes to the
      page after the next command in the buffer. A hook for changes to a
      buffer would send it at once.
- [ ] Closing a field's own window with the compositor leaves the field
      open in its buffer until the page closes it.
- [ ] WebSocket servers carry text only, without TLS, and the editor
      cannot connect to one as a client.

## From the plan, to check before assuming done

Not verified since they were written. Check each against the code before
doing any work:

- [ ] M3: systemd units (`amacs.socket`, `amacs.service`), the panic hook
      that dumps modified buffers, recovery after `kill -9`.
- [ ] M4: golden-image tests for the Wayland client, the latency trace
      (target: key press to commit under 5 ms), the Swedish key matrix.
- [ ] M5: the benchmark of 1M candidates filtered under 30 ms.
- [ ] M7: a rename with rust-analyzer in a real project; each of the seven
      language servers tried once.
- [ ] The sample `examples/init.scm` against the Emacs configuration that
      the editor is modelled on: which bindings and custom commands are
      still not ported.

## Later milestones in the plan

`PLAN.md` lays out the next three: processes and a terminal (M11), a coding
agent over the Agent Client Protocol (M12) and remote files (M14). M11 is
in but for a later pass, M12 is at its last step of five, and M14 has
three of its six steps and part of the fifth.

- [x] Terminal emulator (M11): a shell or program on a pseudo-terminal in
      a buffer (`C-c v`), with scrollback, colours, point following the
      cursor, semi-char and char keys, and a configurable terminal palette
      and background. Later: a line mode, OSC 7 and OSC 133 shell
      integration, the title in the mode line.
- [ ] A coding agent inside the editor (M12). In: the protocol client,
      the compose buffer, and the conversation as sections with tool
      calls, diffs and the plan; leave asked with the tool call and
      answered by keys; the mode, the model and the other options; slash
      commands; `terminal/*`. Left: several sessions, the seam for other
      agents (`agent.scm`, with nothing in it naming Claude), attention
      when a turn ends or leave is asked out of sight (`C-c c a`), and
      docs. A request for leave from the real adapter is still to be tried
      by hand. Slash commands are not completed in the compose buffer.
      A redraw of the conversation writes the whole buffer; `section.scm`
      may need to redraw one section.
- [ ] An empty Markdown buffer draws its first row in the colour of a
      removed line (seen in the compose buffer and in any empty buffer in
      `markdown-mode`, in the picture from `screen`).
- [ ] Remote files (M14). In: tramp's file names (`Location`), the SFTP
      client, one place for file access in the core (`file_system.rs`),
      and visiting, saving, reverting and completing files under
      `/ssh:host:`, in the background, with keys or an agent; `dired`
      there, with copying and moving between hosts; `sudo`, several
      hops and containers where nothing is asked, as methods that Scheme
      defines (`define-remote-method`), with a prompt that offers each
      next step of a name and says it in words, and `sudo-edit`. Left:
      programs and the terminal on the host, passwords, git and language
      servers. Still to be tried by hand, since no test can:
      `C-x C-f //sudo::/etc/hosts`, `/ssh:host|sudo::/etc/hosts` and
      `/ssh:a|ssh:b:/etc/hostname`.
- [ ] Org.
- [ ] age encryption.

## Things that bit before

- `nix flake check` takes ten minutes or more (twenty-five when wgpu is
  built anew), sees only files git knows, has no network, and its home
  directory is the build directory. Tests that build paths under a
  temporary directory must not assume it is outside home
  (`abbreviate-file-name` turns it into `~/...`).
- The flake's clippy and format checks reuse the package's recipe and
  install nothing: a step added to the package that touches `$out/bin`
  must be kept out of them (`dontFixup`).
- Count test results plainly: save the output to a file, then
  `grep -c '^test result: ok'` and grep for `FAILED`. A filter once hid a
  failure.
- The development server at `$XDG_RUNTIME_DIR/amacs/dev.sock` is the
  user's. Before `just restart`, ask it whether a buffer has unsaved
  changes; for experiments start a server on a private socket with a short
  path, set through `AMACS_SOCKET`.
- Stop processes by their ids, never by name: `pkill amacs-gui` or
  `pkill sway` would take the user's windows and desktop with them. A
  headless sway for trying a window needs `XDG_RUNTIME_DIR` at a short
  path and `WLR_BACKENDS=headless`.
- A session of Claude Code keeps the `amacs-mcp` it started. After a
  change to the wire protocol its tools fail until the session is started
  anew.
- `amacsclient --json --eval '...'`: the flag goes before `--eval`, and
  `--eval` takes one expression: a second argument is a file to visit,
  which the client then waits for. In zsh
  use single quotes and `(quote x)` instead of `'x`, or put the code in a
  file and `load` it.
- rustfmt rewraps lines, so re-read a file after formatting before making
  exact-string edits.
- A change to the wire protocol (now version 10) needs `just restart` and
  the version bumped in `amacs-proto`.
- Tests of highlighting must ask for parts of a text, not only the whole:
  a range that ended inside a table once crashed the server. Tests of
  redrawing must use lines that differ, or scrolling changes nothing.
