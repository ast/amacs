# Matrix

`M-x matrix` shows the rooms of a Matrix account and their messages in
buffers. The editor does not speak Matrix itself: a program beside it does,
`amacs-matrix`, which has the Matrix SDK with its end-to-end encryption and
keeps what it knows on disk. This file says how to set it up, what the keys
do, and how the two programs talk, for whoever changes either.

Calls are not part of it.

## Setting up

**The program.** `amacs-matrix` is built with the editor, from
`crates/amacs-matrix`, and the Nix package installs it beside `amacs`. The
editor runs what the option `matrix-program` names, `amacs-matrix` on the
`PATH` unless changed. In a checkout, `just build` builds it and the
development shell has `target/debug` on its `PATH`. It needs the
certificates of the web's authorities that the system has, as a browser
does.

**Who you are.** In the init file:

```scheme
(set-option! 'matrix-user-id "@name:example.org")
```

The server is found from the address. If yours is somewhere else, name it
with `matrix-homeserver`, as `"https://matrix.example.org"`. It has to let
you log in through its own page, with OAuth, which matrix.org does.

**A passphrase.** The program keeps its keys of encryption and your session
on disk, locked with a passphrase that the editor gives it. The editor
takes it from a secret, asked for as the host `amacs-matrix` with your
address as the user. With the password store that is one of:

```
pass generate amacs-matrix/@name:example.org
pass generate amacs-matrix
```

The option `matrix-store-secret` is that host, if another name suits your
store. Without such a secret, `M-x matrix` asks for the passphrase each
time it starts the program. Keep to one passphrase: with another, the
program cannot open what it wrote.

**The first time.**

1. `M-x matrix` starts the program and shows the list of rooms, which says
   that you are not logged in.
2. `M-x matrix-login` opens the page of your server in the browser. Log in
   there and allow the device. The rooms come when you have.
3. The device is new to your account, so it cannot read the earlier
   messages of encrypted rooms yet, and the list of rooms says that it is
   not verified. Do one of:
   - `M-x matrix-verify`, which asks another of your devices, such as
     Element. Accept there, and say in the editor whether both show the
     same emojis.
   - `M-x matrix-recover`, which takes the recovery key of the account.

From then on `M-x matrix` is all it takes: the session is kept.

**The editor that you use every day**, if it is one that is pinned, has
Matrix once its pin is at a commit that has it. The option goes in its own
init file, not in `examples/init.scm`, which is the development server's.

## When something is wrong

- What the program has to say is in `*Messages*`, each line after
  `matrix:`. The first line of the list of rooms says what state it is in.
- `g` in the list of rooms starts the program again if it stopped, and
  `M-x matrix-quit` stops it.
- What it keeps is under `$XDG_STATE_HOME/amacs/matrix/`, in a directory
  named after your address: `store/`, the SDK's, and `session`, both
  encrypted. `~/.local/state` is where that is unless set. Files and
  pictures that it fetched are under `$XDG_CACHE_HOME/amacs/matrix/` and
  can be deleted at any time.
- To start over, stop the program, move the directory of your address
  away, and log in again. The device is then a new one and needs to be
  verified again; the old one can be removed from the account in Element.
- A server that ends the session, as when the device is removed from the
  account, puts the program back to "not logged in". It moves the old
  store aside and deletes nothing.

## Keys

In the list of rooms, `*matrix*`:

| Key | |
|---|---|
| `RET` | Show the room of the line |
| `s` | Show a room chosen by name |
| `g` | Draw the list again, and start the program if it stopped |

In a room, or a thread:

| Key | |
|---|---|
| `RET` | Write a message of one line |
| `c` | Write a message of more lines in a buffer: `C-c C-c` sends it, `C-c C-k` gives it up |
| `R`, `E` | Answer and change in such a buffer |
| `r` | Answer the message at point |
| `e` | Change the message at point, if it is yours |
| `a` | Put a reaction on it, or take yours off |
| `d` | Delete it for everybody |
| `t` | Show its thread in a buffer of its own |
| `o` | Open its file or picture with the desktop's program |
| `u` | Send a file |
| `p` | Show earlier messages |
| `l` | Go to the list of rooms |
| `k` | Close the room |

A picture is shown under its message in a frame that draws pictures, no
higher than the option `matrix-image-rows`. A message that the rules of
your account say to tell you of shows in the echo area and, unless
`matrix-notify` is `#f`, on the desktop through `matrix-notify-program`. A
room is read when its buffer is the one in use and point is at its end.

## How the two programs talk

Lines of JSON, one object each, on the standard input and output of
`amacs-matrix`. What it has to say to a person goes to standard error,
which the editor puts in `*Messages*`. The end of its input ends it.

```
{"id": 1, "method": "send", "params": {"room": "!a:example.org", "body": "Hello"}}
{"id": 1, "result": null}
{"id": 2, "error": "The room is not known."}
{"event": "room", "room": {...}}
```

Requests about a thread carry `thread`, the id of the message that the
thread starts with, beside `room`; so do the `timeline` events of a thread.

| Method | Parameters | |
|---|---|---|
| `start` | `user`, `homeserver` or null, `passphrase` | Open the store. With a session in it, go on from there; without, say `login` |
| `login` | | Log in through the browser: `login-url` follows |
| `timeline-open` | `room`, `thread`? | Send the messages now, as `reset`, and what changes from then on |
| `timeline-close` | `room`, `thread`? | Stop that |
| `paginate-back` | `room`, `thread`?, `count` | Earlier messages, as `prepend`. The result is false at the start of the room |
| `send` | `room`, `thread`?, `body` | A message of text |
| `reply` | `room`, `id`, `body` | An answer to a message |
| `edit` | `room`, `id`, `body` | Another text for a message of one's own |
| `react` | `room`, `id`, `key` | A reaction, or the end of one's own of that key |
| `redact` | `room`, `id` | Delete a message |
| `mark-read` | `room` | The room is read to its end |
| `upload` | `room`, `thread`?, `path` | Send a file |
| `download` | `room`, `id` | Fetch the file of a message. The result is where it is |
| `verify-start` | | Ask the other devices of the account to verify this one |
| `verify-accept`, `verify-cancel` | | Answer such a request of another device |
| `verify-confirm` | `match` | Whether the emojis are the same on both |
| `recover` | `key` | Verify with the recovery key |

| Event | Fields | |
|---|---|---|
| `state` | `state`, `message`?, `verified`? | `starting`, `login`, `syncing`, `offline` or `error`; whether the device is verified |
| `login-url` | `url` | The page to log in at |
| `room` | `room` | A room or a space, new or changed |
| `room-removed` | `id` | |
| `timeline` | `room`, `thread`?, `op`, `items` or `ids` | `reset`, `append`, `prepend`, `update` (items take the place of those with their ids) or `remove` |
| `notify` | `room`, `sender_name`, `body` | A message to tell the user of |
| `verify-request` | | Another device asks to verify this one |
| `verify-emojis` | `emojis`: `symbol`, `description` | What both devices should show |
| `verify-done` | `ok`, `message`? | |

A room: `id`, `name`, `unread`, `mentions`, `activity` (seconds since 1970),
`direct`, `space`, and `parents`, the ids of the spaces that it is in.

A message: `id`, `sender`, `sender_name`, `time`, `kind` (`text`, `notice`,
`emote`, `image`, `file`, `redacted`, `undecryptable`, `membership`,
`other`), `body`, `edited`, `own`, and where there is one: `reply_to`
(`sender_name`, `body`), `reactions` (`key`, `count`, `mine`), `thread`
(`count`, on the message that a thread starts with), `thread_root`, `file`
(`name`, and `thumbnail`, a picture on disk to show).

Some finer points of what `amacs-matrix` does:

- The id of a message names it in the list that it came in, a room's or a
  thread's, while the program runs: it is not the id that the server has for
  it, which is in `event_id`. A `thread` is the id of a message of the room,
  as the editor passes it, or an id of the server's that starts with `$`;
  the program gives the same back in the `timeline` events of the thread and
  in `thread_root`.
- The methods about one message, `reply`, `edit`, `react`, `redact` and
  `download`, take `thread` too, for a message in a thread, and need the
  room or the thread to be open. So do `mark-read` and `upload`. `send` to a
  room works without.
- Earlier messages can come as a `reset` in place of a `prepend`.
- `verified` is left out of `state` while it is not known, and `state` is
  sent again when it changes. A `start` that fails says so in a `state` of
  `error` as well as in its answer.
- A file is fetched to `$XDG_CACHE_HOME/amacs/matrix/<user>/files/`, and the
  pictures to show to `thumbnails/` beside it. A message with a picture
  comes without `thumbnail` first, and an `update` brings it.
- `notify` follows the rules of the account, without the user's own
  messages, invitations, and what was written before the program started.
- A device is verified with emojis, and only by another device of the same
  account.

`crates/amacs-script/examples/fake-matrix.rs` speaks this to no server, for
the tests in `crates/amacs-script/tests/matrix.rs`.
