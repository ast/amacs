# The plan: terminal, agents, remote files

Written 2026-11-20, after M0 to M10 of the original plan. Four milestones,
in the order they depend on each other: processes and a terminal (M11),
Claude Code in that terminal with the editor as its IDE (M12), a native
agent buffer over the Agent Client Protocol (M13), and remote files (M14).
Each milestone leaves the editor better on its own, and each later one
builds on primitives the earlier ones add.

The style is that of the rest of the editor: the feature in Scheme under
`runtime/lib/`, primitives in Rust only for what Scheme cannot do or cannot
do fast enough, every procedure documented, tests that press keys and read
the screen. Nothing specific to Claude goes into the core: the agent layer
has a backend seam, and the Claude pieces are one Scheme file and one small
binary.

Decisions taken with Albin on 2026-11-20:

- Remote files are classic tramp done well, with the same remote layer
  usable by language models through Scheme and the MCP server. Targets are
  ssh to Linux hosts and sudo, with containers as another way to reach a
  shell. Passwords are supported, asked through the editor.
- The terminal is a full terminal in an ordinary buffer, as eat and vterm
  are; a line-oriented shell comes later on the same primitives.
- Claude Code runs first in the terminal, with the editor acting as its
  IDE; the native agent buffer comes after and is agent-neutral.
- Sessions are long-lived, one per project, several at once; permission
  prompts matter; prompts are composed in a buffer and sent with `C-c C-c`.
- On day one of remote files: visiting, saving, dired, shell commands and a
  terminal on the host. Git and language servers on the host come second.
- Keys follow the Emacs configuration: `C-c c` is the agent prefix (as
  claude-code.el has it) and `C-c v` opens a terminal (as vterm has it).

## What the core lacks today

Found by reading the code before planning; these are the primitives the
milestones add, roughly in order of need.

1. **Long-lived processes.** `run-process` runs a program to completion and
   holds typing meanwhile; `start-process` throws its output away. Nothing
   streams output, accepts input after the start, or reports an exit. The
   language server client has the right shape (a reader thread pushes into
   a queue and wakes the loop; `Editor::poll` drains it) but is not
   general. The event loop is threads on one mpsc channel, not a poll on
   file descriptors, so new sources follow that pattern.
2. **A catch-all key binding.** Keymaps have no default binding and Scheme
   cannot see the key that ran a command. A terminal needs both.
3. **Cheap text with colours.** Faces on text are a flat list searched per
   character, every face is named, and defining a face repaints every
   client. A terminal screen needs sorted spans with direct colours.
4. **JSON in Scheme.** Needed for the IDE protocol and ACP.
5. **An answer over the socket.** An `Eval` that waits for a prompt returns
   nothing; a helper program that asks a password through the editor needs
   the value.
6. **One place for file access.** `std::fs` is called from fifteen places
   with no trait between; remote files need one.

## M11: Processes and a terminal

### Processes

A process object in Rust (`crates/amacs-core/src/process.rs`), registered
beside the promise machinery in `extend.rs`, with these primitives in
`api.rs`:

| Primitive | Does |
|---|---|
| `(make-process name program args pty? cwd env)` | Start a program and keep it; returns a process id. With `pty?`, on a pseudo-terminal, otherwise on pipes. `cwd` and `env` default to the buffer's directory and the direnv environment, as `run-process` does |
| `(set-process-filter! p proc)` | `proc` gets `(p text)` for each chunk of output, on the editor's thread, in the buffer that was current when the filter was set; UTF-8 is decoded with incomplete sequences carried to the next chunk |
| `(set-process-sentinel! p proc)` | `proc` gets `(p status)` when the program ends |
| `(process-send-string p text)`, `(process-send-eof p)` | Input |
| `(signal-process p sig)`, `(delete-process p)` | Interrupt, terminate, kill; kill and forget |
| `(process-live? p)`, `(process-exit-status p)`, `(process-id p)`, `(process-name p)` | State |
| `(set-process-window-size! p rows cols)` | For terminals |
| `(process-list)` | Every live process, for `list-processes` and for killing them on exit |

The reader thread reads into a bounded channel so that a program that
floods output blocks instead of filling memory. `Editor::poll` takes what is
there and queues the filter as a Scheme call (`ScriptTarget::Func`, as
commands are queued). Filters run with input released: these processes
never hold typing. `kill-buffer` on a buffer with a process asks first, as
Emacs does; the daemon terminates every process on shutdown.

The pseudo-terminal is opened through `portable-pty` (MIT), which sets the
controlling terminal in the child without `unsafe` in this workspace and
gives a child handle that can be waited on. `alacritty_terminal`'s own
`tty` module is the fallback if `portable-pty` fails `cargo deny`.

With this alone, `M-x shell` becomes possible as a Scheme afternoon
(`runtime/lib/comint.scm`): it is not part of this milestone, but the
primitive is designed so that it can be.

### The terminal

A new crate `amacs-term` wraps `alacritty_terminal` 0.26 (Apache-2.0,
the VT state machine of Alacritty): a `Term` fed by the process's output,
read back as a grid of cells with colours and attributes. The crate also
encodes `Key` values from `amacs-proto` as terminal input, with the
terminal's modes in mind (application cursor keys, bracketed paste, and
CSI u for modified keys that have no classic sequence, which the TUIs of
agents need for Shift-Return). Parsing happens on the editor's thread in
`poll`, bounded per poll, with redraws coalesced to at most one every
16 ms per terminal.

The screen is shown in an ordinary buffer, as eat does it, so that
searching, copying, the kill ring and every other buffer command work.
The Rust side keeps the buffer in step with the grid:

- Lines that scroll off the top are appended to the scrollback part of the
  buffer once, as plain text with their colours, and never touched again,
  so point and marks in the scrollback stay put. `term-scrollback-lines`
  (10000) bounds it.
- The live screen is the last `rows` lines; only the lines the terminal
  reports as damaged are rewritten.
- Colours and attributes go into a new kind of decoration on buffers:
  sorted, non-overlapping spans with direct attributes (`FaceAttrs`
  already carries RGB on the wire), looked up by binary search in
  `decorate.rs`. Truecolor from the start, no named faces per colour, no
  repaint of every client. Backgrounds past the end of a line are drawn by
  padding the line with spaces. The same span store is what an ANSI
  colouring of compilation output or a rendered Markdown view would use.
- The buffer has no undo history and is never "modified".

`runtime/lib/term.scm` is the mode:

- `term-mode`, a major mode with three ways to treat keys, as eat has:
  *semi-char* (the default: every key goes to the program except the ones
  in the `term-emacs-keys` option, `C-x`, `C-c`, `M-x`, `M-:`, `M-o`, `C-u`
  and the window-selecting arrows), *char* (everything but `C-c C-e`), and
  *line* (the buffer is text; input is edited at the end and `RET` sends
  it). `C-c C-j`, `C-c C-k` and `C-c C-e` switch. `C-c C-c` sends `C-c`.
- To send "every other key" the core gets a default binding: a keymap
  entry for `#t`, as Emacs's `[t]`, and a `(last-key)` primitive so that
  the command knows which key it was. Paste is intercepted the same way
  and becomes a bracketed paste.
- Point follows the cursor while keys go to the program; when the user
  moves point away, the buffer stops following until they press a key for
  the program or `C-c C-l` (as `term-char-mode` restores it).
- The window's size sets the program's size: a `window-size-change-hook`
  and `(window-width w)`, `(window-height w)` primitives, which do not
  exist yet.
- Shell integration, which is what makes it good: OSC 7 sets the buffer's
  directory, so `C-x C-f` and `M-!` start where the shell is; OSC 133
  prompt marks give `C-c C-p` and `C-c C-n` between prompts and
  `term-copy-last-output`; OSC 52 copies to the kill ring; the title shows
  in the mode line; the bell shows in the mode line and the echo area when
  the buffer is not visible. The environment of the program carries
  `TERM=xterm-256color`, `COLORTERM=truecolor`, `AMACS=1`,
  `EDITOR=amacsclient` and `AMACS_SOCKET`, so `git commit` in the terminal
  opens its message in the editor.
- Commands: `term` (`C-c v`), `term-other-window` (`C-c V`), both reusing
  a live terminal of the current project unless given a prefix argument,
  `project-term` (`C-c p t`), `term-send-string`, `term-clear`,
  `list-processes`.

### Tests

An example binary beside `fake-lsp`, `fake-tty`, prints chosen escape
sequences on demand; tests in `crates/amacs-script/tests/term.rs` run it
and plain `sh` on a pseudo-terminal and read the screen: colours, cursor
movement, scrollback, resize, the three key modes, the bell, OSC 7. Whether
`/dev/ptmx` works in the flake's sandbox is checked in the first step and,
if it does not, those tests are marked to run outside it.

### Steps

1. `make-process` on pipes with filter and sentinel, tested with `cat` and
   `sh`; `process-list`, shutdown. Done 2026-11-20.
2. The default key binding and `last-key`; the window size primitives.
   Done 2026-11-20, with `<paste>` as a bindable key and
   `window-size-change-hook`.
3. Decoration spans with direct colours, with the decorate lookup made
   logarithmic. Done 2026-11-20: `add-span!` and `clear-spans!`, and
   `Buffer::replace_spans` for the terminal.
4. `amacs-term`: the grid in a buffer, key encoding, resize; `term-mode`
   with semi-char mode. Try `htop`, `vim`, `claude` and a flood of output.
5. Line and char modes, shell integration, scrollback, tests, README.

Done 2026-11-20: the `amacs-term` crate (VT state machine, key encoding),
the terminal in a buffer with spans, `make-process` on a pseudo-terminal,
`term-mode` with semi-char keys, scrollback, point following the cursor,
the window following its size, and `C-c v`. Still to do in a later pass:
OSC 7 and OSC 133 shell integration (directory tracking and prompt
motion), the title in the mode line, and a line mode (editing the input as
text). Char mode and a one-off key sender were added 2026-11-20.

## M12: Claude Code in the terminal, the editor as its IDE

The idea is the one the VS Code and JetBrains extensions use: `claude`
keeps its own interface, and the editor supplies context and shows diffs,
through the protocol those extensions speak. Verified from the
reverse-engineered description in claudecode.nvim's `PROTOCOL.md` and the
flags of Claude Code 2.1.303: the editor writes `~/.claude/ide/PORT.lock`
(`pid`, `workspaceFolders`, `ideName`, `transport: "ws"`, a random
`authToken`), serves MCP over a WebSocket on `127.0.0.1:PORT`, checks the
`x-claude-code-ide-authorization` header, and launches `claude` with
`CLAUDE_CODE_SSE_PORT` and `ENABLE_IDE_INTEGRATION=true` or `--ide`.

### Parts

- **`crates/amacs-ide`**, a small binary like `amacs-mcp`: a WebSocket
  server (`tungstenite`, no async runtime) that speaks JSON-RPC, writes the
  lock file, checks the token, and forwards every tool call as an
  evaluation of `(ide-tool "name" params)` to the editor over the socket,
  returning the result. It knows the lock-file conventions and nothing
  about what the tools mean. The directory, file name and header are
  flags, so another agent's variant of the protocol is a flag away.
- **An `Eval` that waits** on the wire protocol (version 11): a flag on
  `ClientMsg::Eval` makes the server answer when the code's promise
  settles instead of at once. `amacsclient --eval --wait` and the MCP
  `eval` tool gain it too. This is what `openDiff` needs (it blocks until
  the user accepts or rejects) and what the askpass helper of M14 needs.
- **JSON primitives**: `(json-read text)` and `(json-write value)` over
  alists with symbol keys, vectors, strings, numbers, booleans and `'null`.
- **`runtime/lib/agent.scm`**, agent-neutral: sessions (name, project,
  buffer, backend), the compose buffer, sending, context references,
  attention. Nothing in it names Claude.
- **`runtime/lib/agents/claude.scm`**: the Claude Code backend: how to
  start it (`claude`, `--continue` or `--resume` with a prefix argument,
  in the project root, with the IDE port in its environment), the
  reference syntax `@path#L3-9`, its slash commands as editor commands,
  and the IDE tools in Scheme.
- **`runtime/lib/ide.scm`**: the tools, as procedures that may wait:
  `getCurrentSelection` and `getLatestSelection` from the region of the
  user's selected window, `getOpenEditors` from the file buffers,
  `getWorkspaceFolders` from the projects, `openFile`, `getDiagnostics`
  from the language server's (a primitive to list them is needed),
  `checkDocumentDirty`, `saveDocument`, `close_tab`, `closeAllDiffTabs`,
  and `openDiff`: a diff buffer of the proposed change, `C-c C-c` writes
  the new text into the buffer and saves it and answers `FILE_SAVED`,
  `C-c C-k` answers `DIFF_REJECTED`. `selection_changed` is sent,
  debounced, when point or the region moves in a file buffer, and
  `at_mentioned` when the user sends a region as context.

### What the user gets

| Key | Does |
|---|---|
| `C-c c c` | Start Claude for this project, or switch to it; with `C-u`, continue the last conversation; with `C-u C-u`, pick one to resume |
| `C-c c t` | Show or hide the session's window |
| `C-c c p` | The compose buffer: a Markdown buffer for the prompt. `C-c C-c` sends it (as a bracketed paste and `RET`), `C-c C-k` clears it, `M-p` and `M-n` walk earlier prompts, `C-c C-r` inserts a reference to the region or the file, `C-c C-y` inserts the region as a fenced block |
| `C-c c s` | Send the region, or a reference to it, as context, without leaving the buffer |
| `C-c c e` | Send the diagnostic at point with a request to fix it |
| `C-c c k`, `C-c c y`, `C-c c n`, `C-c c RET` | Interrupt; answer yes, no, or press return in the session from anywhere |
| `C-c c /` | A menu of slash commands (`/clear`, `/compact`, `/model`, `/cost`, and so on) |
| `C-c c d` | The diff of the project since the last prompt was sent (`git diff` against a stash made at sending) |
| `C-c c b` | The sessions, to switch between projects |
| `C-c c o` | Copy the last fenced code block of the session's output |

Attention: a bell from a session that is not on screen shows in the echo
area and the mode line (`[claude!]`), and `C-c c a` goes to it. Claude's
`terminal_bell` notification channel is what rings on a permission prompt;
a `Notification` hook that runs `amacsclient --eval` is documented as the
sharper alternative for people who want the message text.

### Tests

The IDE bridge is tested as the MCP server is, in
`crates/amacs-server/tests`: a test connects to the WebSocket with the
token, lists tools and calls `getCurrentSelection` and `openDiff`, and
accepts the diff by pressing keys in the harness. The agent commands are
tested with `fake-tty` standing in for `claude`.

### Steps

1. JSON primitives; the waiting `Eval`; a diagnostics primitive.
2. `agent.scm`: sessions, compose buffer, references, attention, with a
   `sh` backend for tests.
3. `agents/claude.scm` without the IDE: start, continue, send, answer.
4. `amacs-ide` and `ide.scm`; `openDiff`.
5. The rest of the keys, docs in README and `docs/extending.md`.

## M13: A native agent buffer over ACP

The Agent Client Protocol (agentclientprotocol.com, JSON-RPC over stdio)
is what Zed, Neovim and Emacs's agent-shell use to talk to coding agents.
Claude Code speaks it through the adapter `claude-agent-acp` (nixpkgs has
0.36.1), Gemini CLI, Codex, Goose, OpenCode and others speak it directly
or through adapters. Protocol version 1 is stable; the Rust SDK exists but
is future-based and the protocol is small, so the client is written in
Scheme on `make-process` and the JSON primitives.

### Parts

- **`runtime/lib/acp.scm`**: the client. `initialize` with the client's
  capabilities (`fs.readTextFile` and `writeTextFile`, answered from
  buffers so unsaved text is seen; `terminal`, answered with `make-process`
  on pipes and captured output), `session/new` and `session/load`,
  `session/prompt`, `session/cancel`, `session/set_mode`, and the
  `session/update` notifications: message chunks, thoughts, tool calls
  with their status and diffs, plans, available commands, modes, usage.
  `session/request_permission` becomes a key menu (`menu.scm`): allow
  once, always, reject, with `C-c c y` and `C-c c n` answering from any
  buffer, as in M12.
- **The conversation buffer** `*agent: project*`, built on `section.scm`
  as the git buffer is: a section per turn, agent text with Markdown
  faces, thoughts folded, each tool call a section with its title, status
  and diff in diff faces, the plan as a checklist, usage and mode in the
  mode line. `RET` on a location goes there; `TAB` folds.
- **The backend seam** in `agent.scm`: a backend is a record of
  procedures (`start`, `send`, `cancel`, `set-mode`, `reference-format`,
  `alive?`); M12 adds the terminal backend, M13 the ACP one. The compose
  buffer and all `C-c c` keys work the same over both. `(define-agent
  'gemini '("gemini" "--experimental-acp") ...)` registers another agent;
  the `agent-default` option picks one.

### Risks to check in the first step

Whether `claude-agent-acp` uses the subscription login of the `claude`
command (the adapter wraps the Agent SDK, which runs the CLI, so it
should); and how much of Claude Code's product (plan mode, subagents,
hooks, slash commands) survives the adapter. If too little does, M13 is
still worth it for the other agents, and Claude stays in the terminal.

### Steps

1. A spike: `claude-agent-acp` over `make-process`, one prompt, the
   updates printed into a buffer.
2. `acp.scm` complete, with a fake agent (an example binary that speaks
   ACP) for tests.
3. The conversation buffer, permissions, modes, the backend seam.
4. A second agent, docs.

## M14: Remote files

### Design

A remote file is a path with tramp's syntax, exactly, since both the
user's hands and language models know it: `/ssh:user@host:/path`,
`/sudo::/etc/nixos/configuration.nix`, `/ssh:host|sudo::/path`,
`/podman:name:/path`. Everything that takes a path takes these.

The file operations travel over **SFTP, through the user's own `ssh`**.
`ssh -s host sftp` opens the SFTP subsystem that every OpenSSH server has,
over a connection that `ssh` itself authenticates, with the user's config,
agent, jump hosts and `ControlMaster` (one connection per host, kept for
ten minutes after its last use, under `$XDG_RUNTIME_DIR/amacs/`). SFTP
version 3 is a small binary protocol (stat, read, write, readdir, rename,
remove, mkdir, realpath, permissions); a client for it is a few hundred
lines of Rust with no cryptography and no network code in this process.
This is the reimagining: nothing is installed on the remote host and no
shell is parsed. For `sudo`, the same protocol is spoken by the
`sftp-server` program that OpenSSH ships, run as root: `ssh host sudo -n
sftp-server`, and locally `sudo -A sftp-server`. A container is `podman
exec -i name sftp-server`. A host without `sftp-server` is reported as
such; a shell fallback is left for when it hurts.

Commands run on the host over the same `ssh` connection: `M-!`, `M-|`,
`run-process` and `make-process` take the location of their working
directory, so `dired`'s `cp -a` and later `git.scm` run there unchanged.
A terminal in a remote buffer is `ssh -t host` in the right directory.

### In the core

- `crates/amacs-remote`: the SFTP client, the connection manager (hops,
  `ControlMaster`, reconnection), a `Location` type that parses and
  prints tramp syntax, and the recipe for reaching a shell or an
  `sftp-server` for each method.
- One place for file access in `amacs-core`: a `FileSystem` with
  `metadata`, `read`, `write_atomically`, `read_dir`, `rename`, `delete`,
  `mkdir`, `canonicalize`, which the fifteen `std::fs` sites move to.
  Local paths behave exactly as before.
- Remote reads and writes are promises, so visiting a remote file goes
  through Scheme: `find-file` awaits the text and a new primitive makes
  the buffer from it. This is also the shape that the "files are read in
  the background" item of `TODO.md` needs, so local files get it too.
- Auto-revert polls remote buffers every thirty seconds, not five.
  Saving checks the remote modification time as it does locally.
- File-name completion in `C-x C-f` lists remote directories through the
  same client (with a short timeout, since completion is synchronous
  today) and completes host names from `~/.ssh/config` after `/ssh:`.

### Passwords

ssh and sudo ask through the editor: `SSH_ASKPASS` and `SUDO_ASKPASS`
point at `amacs-askpass`, a mode of `amacsclient` that evaluates
`(read-passwd prompt)` with the waiting `Eval` of M12 and prints the
answer; `SSH_ASKPASS_REQUIRE=force` and `sudo -A` make them use it. The
prompt appears, with its input hidden, in the frame the user last used;
two-factor codes come the same way. Before asking, the
`password-functions` hook is tried, and `runtime/lib/pass.scm` supplies
one that asks `pass`, so hosts in the password store never prompt.
`ControlMaster` and sudo's ticket mean one answer per host per idle
period.

### For language models

Since every tool of the MCP server takes paths, `open_file`, `read_buffer`
and `eval` work on remote paths without change, and `(run-process ...)`
in a remote buffer runs on the host. `docs/ai.md` gets a section on it,
and `(remote-hosts)`, `(remote-run location command)` and
`(remote-disconnect host)` are the explicit entry points.

### Second step: git and language servers

`git.scm` works once `run-process` honours locations; what remains is the
paths in its output, which are host-relative. Language servers on the host
need the client to spawn over `ssh` and translate URIs both ways, which is
a contained change in `crates/amacs-lsp`. Both are after the first
release of remote files.

### Tests

The SFTP client is tested against the local `sftp-server` binary over
pipes, which needs no network and no daemon, so it runs in the flake's
sandbox (`openssh` is added to the check's inputs). The `Location`
parser has unit tests; `dired` and `find-file` on `/sftp-test::` paths,
a method that runs `sftp-server` directly, are tested through the Scheme
harness. ssh and sudo themselves are tried by hand on `evo`.

### Steps

1. `Location`, the `FileSystem` seam with local behaviour unchanged, and
   the SFTP client against `sftp-server`.
2. Visiting and saving remote files, auto-revert, completion.
3. `dired` on remote paths; copying between local and remote.
4. Locations for `run-process`, `make-process`, `M-!`, the terminal.
5. Askpass, `read-passwd`, `pass.scm`; sudo and multi-hop.
6. README, `docs/extending.md`, `docs/ai.md`; then git and LSP.

## Dependencies added

| Crate | For | Licence |
|---|---|---|
| `alacritty_terminal` 0.26 | The VT state machine | Apache-2.0 |
| `portable-pty` | Pseudo-terminals without `unsafe` here | MIT |
| `tungstenite` | The WebSocket of the IDE bridge | MIT or Apache-2.0 |

All are allowed by `deny.toml`. `tokio` stays out.

## Open questions

Decided by whoever starts the step, or by Albin when asked:

- Whether the terminal's default is semi-char or char mode, once both
  exist and have been lived with for a week.
- Whether the compose buffer should also be the input of the native agent
  buffer, or that buffer gets a prompt area of its own at its end.
- Whether `C-c c` stays once agents are several, or becomes `C-c a` with
  the Claude session on `C-c a c`.
- What to do with Claude Code's own `--remote-control` and channels, which
  may make a better native path than ACP by the time M13 starts.
