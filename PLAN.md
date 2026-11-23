# The plan: terminal, agents, remote files

Written 2026-11-20, after M0 to M10 of the original plan. Three milestones,
in the order they depend on each other: processes and a terminal (M11), a
coding agent in the editor over the Agent Client Protocol (M12), and remote
files (M14). There is no M13: it was the native agent buffer, which became
M12 when the IDE bridge that M12 was at first was dropped.
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
the window following its size, and `C-c v`. Decided 2026-11-20: `C-c`
goes to the program, as in any terminal, and one escape key (`C-q`, the
`term-escape-key` option, as the tmux prefix) leads to the terminal's
commands and sends a reserved key. Still to do in a later pass:
OSC 7 and OSC 133 shell integration (prompt motion, and the directory of
a shell that `/proc` does not show; the local shell's directory is tracked
through `/proc` since 2026-11-20, with `term-dired` on top), the title in the mode line, and a line mode (editing the input as
text). Char mode and a one-off key sender were added 2026-11-20.

## M12: Claude Code in amacs over the Agent Client Protocol

Decided 2026-11-20 with Albin: use the standard protocol and the tools as
intended, not a reverse-engineered one. So Claude lives in amacs over the
Agent Client Protocol (agentclientprotocol.com), JSON-RPC over stdio, the
protocol Zed, Neovim and Emacs's agent-shell use. Claude Code speaks it
through the official adapter `claude-agent-acp` (nixpkgs has 0.36.1), and
Gemini, Codex, Goose and others speak it too, so the layer is agent-neutral
from the start. The terminal of M11 stays, for shells and for running
`claude`'s own interface when wanted; the integrated experience is ACP.

The earlier idea, a WebSocket IDE bridge reverse-engineered from the VS Code
extension (`~/.claude/ide/PORT.lock`, an MCP server over a WebSocket, the
`x-claude-code-ide-authorization` header), is dropped. The `crates/amacs-ide`
binary and `runtime/lib/ide.scm` are not built.

### Confirmed by a handshake (2026-11-20)

Against the installed adapter: `initialize` returns protocol version 1,
`loadSession`, session capabilities (close, delete, fork, list, resume) and
`authMethods: []`, so the user's own `claude` login is used with no auth
step. `session/new` returns the `sessionId`, the models (default, Opus 1M,
Fable, Sonnet, Haiku), the permission modes (default, acceptEdits, plan,
bypassPermissions and so on), `configOptions` for mode, model and effort,
and then a `session/update` with `available_commands_update` listing every
slash command. All structured, nothing to scrape.

### Parts

- **`runtime/lib/json.scm`**: JSON in Scheme. Objects are symbol-keyed
  association lists, arrays are vectors, so `{}` and `[]` never collide;
  `(json-ref obj 'key)` reads a field. Done 2026-11-20.
- **`runtime/lib/acp.scm`**: the client, on `make-process` (pipes) and the
  JSON. Newline-delimited JSON-RPC: send requests with ids and match their
  replies, dispatch the agent's `session/update` notifications, and answer
  the methods the client implements. `initialize`, `session/new`,
  `session/load` and `session/resume`, `session/prompt`, `session/cancel`,
  `session/set_mode` and the config options. The client side: `fs/read_text_file`
  and `fs/write_text_file`, answered from buffers so unsaved text is seen;
  `terminal/*`, answered with `make-process`; and
  `session/request_permission`, answered by a key menu.
- **`runtime/lib/agent.scm`**, agent-neutral: a backend is an ACP command
  and a few procedures (how to point at code, its pretty name). Sessions,
  the compose buffer, context references, attention, the quick keys.
  Nothing names Claude.
- **`runtime/lib/agents/claude.scm`**: the Claude backend, the command
  `claude-agent-acp`, the reference syntax `@path#L3-9`, and the binding
  of `C-c c`. `(define-agent 'gemini ...)` adds another agent; the
  `agent-default` option picks one.
- **The conversation buffer** `*agent: project*`, built on `section.scm`
  as the git buffer is: a section per turn, agent text with Markdown faces,
  thoughts folded, each tool call a section with its title, status and diff
  in diff faces, the plan as a checklist, the model, mode and token use in
  the mode line. `RET` on a file location goes there; `TAB` folds.

### What the user gets

| Key | Does |
|---|---|
| `C-c c c` | Start Claude for this project, or switch to it; with `C-u`, resume a conversation |
| `C-c c p` | The compose buffer: write the prompt, `C-c C-c` sends it, `M-p` and `M-n` walk earlier prompts, `C-c C-r` inserts a reference to the region |
| `C-c c s` | Send a reference to the region as context |
| `C-c c k`, `C-c c y`, `C-c c n` | Cancel the turn; answer a permission request yes or no, from anywhere |
| `C-c c m` | Set the mode (plan, accept edits, ...) or the model, from the config options |
| `C-c c /` | A slash command, from `available_commands` |
| `C-c c b` | Switch between sessions |

Attention: a turn that ends or a permission request while the buffer is not
on screen shows in the echo area and the mode line, and `C-c c a` goes to
it.

### Tests

A fake ACP agent (an example binary that speaks the protocol) drives the
client in `crates/amacs-script/tests`: initialize, a session, a prompt, the
updates rendered, a permission request answered, a diff shown. The JSON has
its own tests. The real `claude-agent-acp` is tried by hand, since it spends
the user's quota.

### Steps

1. JSON (`json.scm`). Done 2026-11-20.
2. `acp.scm`: the transport and the handshake, `make-process` on pipes,
   requests and replies, notifications; a prompt's updates printed to a
   buffer, with a fake agent for tests. Done 2026-11-20 as a first slice:
   `claude`, `claude-send`, `claude-cancel`; standard error kept apart
   from the protocol (a `'stderr` filter on `make-process`); the client's
   `fs/read_text_file` and `fs/write_text_file` and a permission prompt.
   Confirmed live: Claude answered over ACP.
3. The conversation buffer on `section.scm`; the compose buffer and
   references; `session/prompt` and cancel. The compose buffer, the chat
   layout, prompt history and references done 2026-11-20. The conversation
   on `section.scm` done 2026-11-23: a model of the turns that the buffer
   is drawn from, thoughts folded, tool calls that change where they stand,
   changes as diffs made in Scheme from the text before and after, the plan
   as a list, `RET` to the file and line, and the model, the mode and the
   use of the context on the mode line (`set-mode-line-text!`). Tried
   against the real adapter, which sends the model and the mode among
   `configOptions` (not in `models`) and the use of the context as
   `usage_update` with `used` and `size`. A `fake-acp` example drives one
   test through the whole protocol. A redraw writes the whole buffer anew:
   7 ms for 3200 lines in the dev build, with the turns that are over kept
   as drawn; if long conversations feel slow, `section.scm` needs a way to
   redraw one section.
4. Permissions, modes and models, slash commands; the client's
   `fs/*` and `terminal/*` methods. Done 2026-11-23. Leave is not asked in
   the minibuffer, which took whatever was being typed for an answer: the
   request waits with its tool call, which lists the agent's options with a
   key each, and `C-c c y`, `C-c c n` and `C-c c o` (a menu of keys) answer
   it from anywhere; a cancelled turn answers what waits with `cancelled`,
   as the protocol asks. `C-c c m` sets any of the `configOptions` with
   `session/set_config_option`, and falls back on `session/set_mode` for an
   agent that only has `modes`. `C-c c /` sends one of
   `available_commands`. `fs/read_text_file` honours `line` and `limit`.
   `terminal/create`, `output`, `wait_for_exit`, `kill` and `release` run
   programs on `make-process`, keep the end of the output within the byte
   limit, and show it under the tool call that names the terminal. The
   adapter of Claude Code (0.107.0) never calls `terminal/*`: it runs its
   own commands and sends their output as text, or as `_meta.terminal_output`
   to a client that says it draws those, which amacs does not say. So the
   terminals are for other agents, and are tried with `fake-acp` only.
   Checked against the real adapter without a prompt: the options (mode,
   model, effort, fast mode, ultracode, agent) with names and descriptions,
   seventy commands with their hints, and the answer of
   `session/set_config_option`. A request for leave from the real adapter
   has not been tried; its options are read from the adapter's source.
5. The backend seam and a second agent; attention; docs.

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
