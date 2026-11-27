# The editor's own Scheme

`crates/amacs-scheme` is a Scheme written for this editor. It exists so that
the editor can be changed while it runs, by a person or by a program, without
a Rust rebuild: a definition takes effect at once, every procedure can be
asked what it is, and an error says where it happened and how the program got
there.

The editor used Steel until this was written. Steel is a good Scheme, but it
is a general one: values could not be traced back to their source, a call
could not be suspended and resumed from outside without `call/cc` tricks, and
starting it took a tenth of a second.

## What it is

Source text is read into syntax that knows its position. Macros are expanded
and every name is resolved to the variable it means; the result is compiled
to instructions for a stack machine. Objects live in a heap of numbered
slots that a mark-and-sweep collector cleans, so the crate has no unsafe
code. Nothing depends on the editor: the crate is a library with tests of
its own.

The dialect is R7RS-small with these differences:

- Numbers are 64-bit integers and floats. There are no fractions, complex
  numbers or integers of any size; integer arithmetic that would overflow
  gives a float.
- `call/cc` gives an escape procedure: it works while the call that made it
  is in progress. Coroutines (`make-coroutine`, `coroutine-resume`, `yield`)
  cover what full continuations are otherwise used for.
- Strings cannot be changed in place, and positions in them count chars.
- There are no ports, bytevectors, libraries or environments. `display` and
  its like write to a buffer that the host takes; in the editor it ends up
  in `*Messages*`. `with-output-to-string` collects it instead.
- Brackets can be nested 1000 deep in source text.

Beyond the standard: hash tables that keep insertion order, `string-split`,
`string-join`, `string-search`, `string-replace`, `sort`, `filter`, `fold`
and their like, `unwind-protect`, `while`, `define-syntax-rule`, keywords
written `#:name`, and a string at the start of a procedure's body as its
documentation.

Of the 980 tests and test forms in chibi-scheme's R7RS suite, 735 pass; the
rest exercise the parts listed above. `tests/r7rs.rs` holds the numbers, and
fails if they get worse.

## Looking at the running program

| Call | Gives |
|---|---|
| `(describe 'name)` | What a name means: how it is called, its documentation, where it is defined |
| `(apropos "part")` | The names that contain a string |
| `(procedure-source f)` | The text of a procedure's definition |
| `(procedure-location f)` | `"file:line:column"` |
| `(procedure-documentation f)`, `(procedure-arity f)`, `(procedure-parameters f)`, `(procedure-name f)` | The parts of that |
| `(global-names)`, `(bound? 'name)`, `(global-ref 'name)`, `(global-location 'name)` | The top level |
| `(global-set! 'name value)` | A definition made by a program |
| `(error-object-trace e)` | The calls that led to an error, as data |
| `(record->list r)` | The fields of any record |
| `(documentation 'name)`, `(set-documentation! 'name text)` | Documentation, also for macros and variables, which cannot carry their own |
| `(load "file.scm")`, `(eval-string text)` | Run a file or a text; definitions take effect at once |

All of it works through `amacsclient --eval`, which is how a program outside
the editor uses it. An error comes back like this:

```
error: car: expected a pair, got 5
  at init.scm:12:8
  in helper, at init.scm:12:8
  in outer, at init.scm:15:9
  in top level, at init.scm:20:1
```

With `--json`, `amacsclient --eval` prints one JSON object for each
expression instead: `ok`, and then either `value` and `output` (what the code
wrote with `display`), or `message`, `location`, `trace` (a list of
`procedure` and `location`), `output` and the `report` above.

A name refers to a numbered variable, not to the value it had when the code
that uses it was compiled. Defining a procedure again changes it for every
caller, including procedures that are built in.

## For the host

`Vm::eval` and `Vm::call` run code and return `Outcome::Done(value)`, or
`Outcome::Suspended` when the code called `%suspend` to wait for something;
`Vm::resume` continues it later with a value. That is what `await` in the
editor is built on: a command that asks a question is suspended, with its
handlers and its `dynamic-wind` cleanups, until the answer is there. If the
question is dismissed, the code is left the way an error leaves it, so the
cleanups run.

Procedures written in Rust get the interpreter's state and their arguments,
and cannot call back into Scheme. That keeps every call stack inside the
machine, where it can be shown in a trace, suspended, and interrupted:
`Vm::interrupt_handle` gives a flag that stops running code from another
thread, and `Vm::fuel` bounds how long code may run.

## Speed

Measured against Steel 0.8.3 with the same programs, before Steel was
removed, on the machine this was written on:

| | Steel | own |
|---|---|---|
| Start an interpreter | 124 ms | 0.9 ms |
| `fib 27` | 37 ms | 35 ms |
| 1M calls of a host procedure | 86 ms | 56 ms |
| `map` and `filter` over 300k items | 130 ms | 93 ms |
| 1M calls of a closure that assigns | 160 ms | 86 ms |
| 200k string appends and comparisons | 35 ms | 23 ms |
| 100k lookups in an association list | 252 ms | 37 ms |

`cargo run --release -p amacs-scheme --example bench` times a few programs
on the interpreter as it is now, and `--example micro` single features of
the machine.

A loop costs no calls. A named `let` or a `do` whose name is only called as
the last thing its body does is part of the procedure around it: its
variables are that procedure's, and going round is a jump. Any other
procedure that calls itself last starts over in the call it is in. The
common built-in procedures, such as `+`, `car` and `null?`, are instructions
of their own for as long as their names mean what they did at first; a name
that is given another meaning has it at once, also in code compiled before.

The collector's work goes by the number of objects there are, not by how
many there were at most.