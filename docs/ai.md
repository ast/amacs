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

## Working on the editor itself

`CLAUDE.md` says how: write features in Scheme, try them in the running
editor through the MCP server, load changed files instead of restarting.
