# Extending the editor in Scheme

Everything here can be done in a running editor, from the init file, with
`M-:`, or from outside with `amacsclient --eval`. Nothing needs a rebuild.
`(describe 'name)` and `(apropos "part")` say what there is; `docs/scheme.md`
describes the language.

`runtime/lib/avy.scm` is a whole feature written this way, and a model for
others: jumping to places on screen by label, in 200 lines.

## Commands

```scheme
(define-command (insert-date)
  "Insert today's date."
  (insert (format-current-time "%Y-%m-%d")))
(global-set-key "C-c d" 'insert-date)
```

A command defined with the name of a built-in one replaces it, for keys and
`M-x` alike; `(call-built-in-command 'name)` still runs the original.
`(call-command 'name)` runs any command.

Advice changes a command without replacing it, whoever defined it:

```scheme
(add-advice! 'save-buffer 'before 'tidy (lambda () (message "saving")))
(add-advice! 'kill-buffer 'around 'ask
  (lambda (next) (when (y-or-n? "Really kill it? ") (next))))
(remove-advice! 'kill-buffer 'ask)
```

## Hooks

`(add-hook! 'name procedure)` and `(remove-hook! 'name procedure)`. The
editor runs `pre-command-hook` and `post-command-hook` around every command,
`find-file-hook` when a file is first visited, `after-save-hook` after a
buffer is written, and a hook named after each mode when the mode is
switched. There is no hook before saving yet.

## Buffers and windows

The primitives act on the current buffer: `point`, `goto-char`, `insert`,
`delete-region`, `buffer-substring` and so on. Positions count chars from 0.

```scheme
(with-current-buffer "notes.txt" (goto-char (point-max)) (insert "more\n"))
(save-excursion (goto-char 0) (insert ";; header\n"))
(with-selected-window (cadr (window-list)) (buffer-name))
```

Buffers are named by strings and windows by numbers: `(window-list)`,
`(selected-window)`, `(select-window w)`, `(window-buffer w)`,
`(window-start w)`, `(window-end w)`, `(window-point w)`.

## Waiting without blocking

A command that needs something that takes time waits for it, and the editor
goes on working meanwhile. Each of these returns when the thing is there:

| | |
|---|---|
| `(read-string "Prompt: ")`, `(completing-read "Prompt: " candidates)`, `(y-or-n? "Question? ")` | Ask in the minibuffer |
| `(read-key "Prompt: ")`, `(read-char "Prompt: ")` | The next key, as `"a"`, `"C-x"`, `"RET"` |
| `(sleep-for 200)` | Milliseconds |
| `(run-process "git" '("status" "--short"))` | A list of exit status, standard output, standard error |
| `(process-lines "git" "branch")` | The lines a program wrote; an error if it failed |

If the user quits with `C-g` while a command waits, the command is left the
way an error leaves it: `unwind-protect` and `dynamic-wind` clean up, and
`guard` does not see it. Waiting works in commands and hooks, not in code
evaluated as text.

## What is on screen

`(show-labels! '((window position "keys") ...) dim?)` draws key sequences over
places in windows, and `(hide-labels!)` removes them. `(avy-candidates match?)`
finds places on screen, `(avy-pick candidates)` lets the user choose one by
its label, and `(avy-jump candidates)` goes there.

## Loading code

`(load "file.scm")` runs a file; loading it again after a change replaces
the definitions. Set `AMACS_RUNTIME` to the `runtime` directory of a checkout
and the editor reads its own Scheme from there at start instead of from the
binary.
