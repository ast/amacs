# amacs for language models

The editor is built so that a language model can use it, inspect it and
change it while it runs, as efficiently as a person at the keyboard and a
programmer at a REPL can. There are two ways in.

## The MCP server

`amacs-mcp` speaks the Model Context Protocol on standard input and output
and connects to the amacs server at `AMACS_SOCKET` (or the default socket),
starting one if none is running. `.mcp.json` in this repository registers it
for the development server.

The model gets a frame of its own, which nobody sees and which does not get
in the user's way: it has the same buffers, but its own windows, point,
minibuffer and key sequences. A prompt that is open in the model's frame is
not open in the user's, and the other way round.

| Tool | Does |
|---|---|
| `eval` | Evaluate Scheme; value, printed output, or the error with its place and the calls that led to it. With `in_user_frame`, in the frame the user last used |
| `keys`, `type` | Press keys or type text in the model's frame; the screen comes back. `wait_for` waits for text to show, for what happens after a program has run |
| `screen` | The frame as numbered rows of text with the cursor's place, or as a PNG picture with colours and faces |
| `resize` | The size of the model's frame |
| `describe`, `apropos` | What a name means, with documentation, location and source; what there is by part of a name, commands included |
| `messages` | The end of the message log: full error reports with traces, printed output, the output of `trace!` |
| `buffers`, `read_buffer`, `open_file` | The buffers, the text of one with line numbers, visiting a file |
| `load` | Run a file of Scheme: definitions take effect at once |
| `run_tests` | Run tests defined with `deftest`, optionally after loading a file of them |

## The command line

`amacsclient --eval CODE` does what `eval` does. With `--json` before it,
the answer is one JSON object: `ok`, and then `value` and `output`, or
`message`, `location`, `trace` and `report`.

## What makes it work

- **Nothing written in Scheme needs a rebuild.** A definition replaces the
  one in force for every caller, whether it comes from `eval`, `load` or the
  init file. Commands can be redefined and advised; hooks can be added.
- **The editor describes itself.** Every procedure knows its name,
  documentation, parameters, location and source text.
- **Errors say where and how.** A message, `file:line:column`, and the chain
  of calls; in the log for commands run by keys.
- **Calls can be watched.** `(trace! 'name)` logs arguments and results,
  `(time expression)` how long something took.
- **Code can wait without blocking.** A command that asks a question or runs
  a program is suspended, and resumes in the frame it started in.
- **The screen can be read.** What is drawn for a frame goes to its client as
  operations on a grid; the MCP server keeps the grid as text and can render
  it as a window would.
- **Features are tested the way they are used.** Keys in, screen out, against
  the real runtime; see `crates/amacs-script/tests`.

## Files on other hosts

A file on another host has a name, and everything that takes the name of
a file takes it: `open_file`, `(find-file ...)`, `(read-file ...)`,
`(directory-entries ...)`, `(copy-file from to)`. The names are tramp's:
`/ssh:evo:/etc/hosts`, `/ssh:me@evo#2222:notes.txt` (from the home
directory), `/sudo::/etc/shadow` (this machine, as root),
`/ssh:evo|sudo::/etc/nixos` (root on evo), `/ssh:gate|ssh:evo:/srv`
(through another host). To be sure of one before using it,
`(file-remote-description name)` says it back in words, "Host evo. Then
user root. Path: /etc/nixos", or says what a name that is not whole still needs, and
is `#f` for a local name. `(file-as-root name)` builds the name for root,
`(remote-method-names)` lists the methods that a step can start with, and
`(remote-command name)` shows the command that would be run for a name
without running it.
`(remote-places)` lists the hosts that the user has worked on, the latest
first, and `(remote-hosts)` those connected now, with why not for those
that failed. What `ssh` asks on the way to a host, a password for one, is
asked of the user in a frame of theirs, never in yours: a host that wants
an answer waits for the user, and says `waiting for your answer` in
`(remote-hosts)` meanwhile. Programs are not
run on other hosts yet: `run-process` in a remote buffer is refused.

## Secrets

A buffer whose `buffer-secret` option is set holds a secret, such as an
entry of a password store. What a program asks of the editor is kept from
it: Scheme that came over the socket to be evaluated (the MCP server's
`eval`, `read_buffer` and `load`, and `amacsclient --eval`), the keys of a
frame that a program has for itself, and what such code leaves to run later
or waits for. Such code cannot read the text of a secret buffer, by name or
as the current buffer, and can do nothing in one but leave it; the buffer
is drawn as one line that says it is secret; a command is not run in it;
and `read-passwd` is refused. Such code is given no ticket to make the
editor ask the user for a password (`askpass-environment`), and cannot
answer what a program asked. A coding agent that asks for a file that a
secret buffer visits gets an error.

This keeps a model from reading a secret because it was there. It is not a
wall against a model that sets out to get one, and cannot be made one:

- Scheme has one world. Code that a program has evaluated can define a
  hook, an advice or a command, which then runs when the user does
  something, as the user. The `program-eval` option, set to `#f`, refuses
  evaluation for programs altogether; the tools that press keys and read
  the screen go on working.
- A program that can run other programs can run `gpg` or `age` itself, or
  connect to the editor as a person does.

What does stop that is outside the editor: a key that asks for a touch or a
PIN each time it is used, so that nothing is decrypted without the user.
The editor does not keep passphrases or PINs.

## Working on the editor itself

`CLAUDE.md` says how: write features in Scheme, try them in the running
editor through the MCP server, load changed files instead of restarting.
