# Matrix

`M-x matrix` shows the rooms of a Matrix account and their messages in
buffers. The editor does not speak Matrix itself: a program beside it does,
`amacs-matrix`, which has the Matrix SDK with its end-to-end encryption and
keeps what it knows on disk. This file says how to set it up, what the keys
do, and how the two programs talk, for whoever changes either.

Calls are not part of it.

## Setting up

```scheme
(set-option! 'matrix-user-id "@name:example.org")
```

The program keeps its keys of encryption and your session under
`$XDG_STATE_HOME/amacs/matrix/`, locked with a passphrase. It takes the
passphrase from the secret that the option `matrix-store-secret` names,
`amacs/matrix` unless changed: with the password store, `pass generate
amacs/matrix` makes one. Without such a secret, `M-x matrix` asks each time.

The first time, `M-x matrix-login` opens the page of your server in the
browser. When you have logged in there, the rooms come. The device is new
to your account, so it cannot read the earlier messages of encrypted rooms
before it is verified, and the list of rooms says so:

- `M-x matrix-verify` asks another of your devices, such as Element, to
  verify this one. Accept there, and say whether both show the same emojis.
- `M-x matrix-recover` takes the recovery key of the account instead.

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
| `RET` | Write a message |
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

`crates/amacs-script/examples/fake-matrix.rs` speaks this to no server, for
the tests in `crates/amacs-script/tests/matrix.rs`.
