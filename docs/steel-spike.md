# Steel feasibility spike

The design assumes three things about Steel that its documentation did not
settle. `crates/amacs-script/examples/spike.rs` tests them against
`steel-core` 0.8.3 with default features:

    cargo run --release -p amacs-script --example spike

Results on 2026-11-16 (x86_64 Linux, rustc 1.95):

| Question | Result |
|---|---|
| Can a tight tail-recursive loop be interrupted from another thread? | Yes. `ThreadStateController::interrupt()` stopped `(define (spin) (spin))` within a millisecond, with the error `Interrupted by user`. |
| Is the engine usable afterwards? | Yes, after calling `ThreadStateController::resume()`. |
| Can a continuation captured in one `Engine::run` be resumed from a later one? | Yes, including several times. |
| Does an `await`-style command (capture the continuation, escape to the top, resume later with a value) work? | Yes, across two suspensions in one command. |
| Cost of calling a registered Rust function | About 57 ns per loop iteration (one million calls in 57 ms). |
| Error reports | The error carries a source span, and `Engine::raise_error_to_string` renders it with the offending line. No call stack is shown. |

Consequences for the design:

- `C-g` can interrupt Scheme code, so a runaway script does not need the
  editor to be killed.
- `await`, the minibuffer and recursive minibuffers can be built on
  `call/cc`. The CPS-macro fallback is not needed.
- Hot paths still belong in Rust. At 57 ns per call, a Scheme loop over every
  char of a large buffer would be too slow.

Things the spike found that the Scheme runtime has to respect:

- Names of built-in functions cannot be redefined with `set!`: a global
  called `log` failed with "cannot mutate module-required identifier".
- Error reports lack a backtrace. Good error messages for user code will need
  work in the adapter.
- `steel-core` pulls in four crates flagged as unmaintained (`im-rc`,
  `sized-chunks`, `bitmaps`, `bincode` 1). They are listed in `deny.toml`.
