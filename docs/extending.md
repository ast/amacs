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
`find-file-hook` when a file is first visited, `before-save-hook` and
`after-save-hook` around the writing of a buffer to its file, and a hook
named after each mode when the mode is switched.

The functions of `before-save-hook` run in the buffer that is about to be
saved, and what they change is written. They run before the formatter of
`format-on-save!`. One that fails is reported and the buffer is saved all
the same:

```scheme
(define (ensure-final-newline)
  "End the text of the buffer with a line break."
  (let ((end (point-max)))
    (unless (or (= end 0) (equal? (buffer-substring (- end 1) end) "\n"))
      (save-excursion (goto-char end) (insert "\n")))))
(add-hook! 'before-save-hook ensure-final-newline)
```

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
`guard` does not see it.

Code evaluated as text, with `M-:` or `amacsclient --eval`, can wait too. The
evaluation then answers at once, without a value, and the forms after the
one that waits run when the wait is over.

## The pointer

What is done with the pointer becomes keys, bound like any others:

| Key | When |
|---|---|
| `<mouse-1>`, `<mouse-2>`, `<mouse-3>` | A button is pressed: the main one, the middle one, the one for menus |
| `<mouse-8>`, `<mouse-9>` | The buttons at the side of a mouse, for going back and forward |
| `<double-mouse-1>`, `<triple-mouse-1>` | It is pressed again in quick succession at the same place |
| `<motion-mouse-1>` | The pointer comes to another char cell with the button held |
| `<drag-mouse-1>` | The button is let go after the pointer has moved |

A press selects the window it is made in, and is looked up in the keymaps
of that window's buffer, with the modifiers that are held: `C-<mouse-1>`.
A press that is not bound with its modifiers or its number of clicks runs
what the plain press is bound to. Movement and letting go are looked up
without modifiers, in the buffer where the button went down.

The commands ask where things are:

```scheme
(define-command (report-press)
  "Say where the button was pressed."
  (message "Pressed at " (cadr (mouse-press-position))))
(define-key 'report-mode "<mouse-3>" 'report-press)
```

`(mouse-press-position)` is a list of the window and the position in its
buffer where the button went down, and `(mouse-position)` the same for
where the pointer is now, which can be in another window; either is `#f`
when there is none. `(mouse-modifiers)` lists what is held, of `control`,
`meta`, `shift` and `super`, also when the button is let go: that is how
the file manager tells copying from moving.

Everywhere, `<mouse-1>` is `mouse-set-point`, `S-<mouse-1>` is
`mouse-extend-region` and `<motion-mouse-1>` is `mouse-drag-region`. The
file manager binds them to its own commands; `runtime/lib/dired.scm`, from
"The pointer" on, is the model for dragging things.

## What is on screen

`(show-labels! '((window position "keys") ...) dim?)` draws key sequences over
places in windows, and `(hide-labels!)` removes them. `(avy-candidates match?)`
finds places on screen, `(avy-pick candidates)` lets the user choose one by
its label, and `(avy-jump candidates)` goes there.

## Buffers that code writes

A feature that shows something of its own, as the file manager does, writes
a buffer and decorates it:

```scheme
(switch-to-buffer "*report*")
(%replace-buffer-text! "Name   Size\nalpha    12\n")
(add-face! 0 4 'dired-header)
(set-buffer-read-only! #t)
(set-buffer-directory! "~/src")
```

`%replace-buffer-text!` sets the whole text, read-only or not, without undo
history. `add-face!` lays a face over a stretch of text, which stays on that
text as the text around it changes; `clear-faces!` takes them away, and
`set-face!` defines new faces. A major mode gives the buffer its keys:
`(define-major-mode report-mode #f "Report" "...")` and then
`(define-key 'report-mode "g" 'report-refresh)`. What the feature knows about
each of its buffers it keeps itself, in a hash table by buffer name.

## Files

`(directory-entries dir)`, `(file-exists? name)`, `(file-directory? name)`,
`(make-directory name)`, `(delete-file name)`, `(rename-file from to)`,
`(read-file name)`, `(expand-file-name name)`, `(abbreviate-file-name name)`,
`(file-name-directory name)`, `(file-name-nondirectory name)`. Copying is
left to `cp`, through `run-process`, so that it does not hold the editor up.
`(read-file-name "Prompt: ")` and `(read-directory-name "Prompt: ")` ask for
names with completion.

## The file manager

`C-x d`, or visiting a directory, lists it; `runtime/lib/dired.scm` is the
code. The keys are dired's: `RET` visits, `^` or `DEL` goes up, `m` `u` `U`
`t` mark, `C` copies, `R` moves or renames, `D` deletes, `d` and `x` flag and
delete, `+` makes a directory, `g` reads again, `q` quits. `TAB` opens a
directory in place, `/` narrows to names that contain a text, `.` hides the
dot files, `s` changes the order, `b` goes to a directory of the
`dired-quick-access` option and `r` to one visited before. With two listings
side by side, copying and moving offer the other one as where to.

With the pointer: a press goes to a file and a second one opens it,
`C-<mouse-1>` marks or unmarks, `S-<mouse-1>` marks from the file point is on
to the one pressed, the middle button opens in another window and the
button for menus lists what can be done. Dragging carries the file, or the
marked files if it is one of them, to the directory it is let go over: a
directory in a listing, or a listing itself, in this window or another.
They are moved, or copied if Control is held when the button is let go,
after a question that the `dired-confirm-drop` option turns off. A file
let go over a window that shows something else is visited there.

The first line, which names the directory, leads upward: a press on a part
of the name shows that directory, and files let go over a part are moved
there. The button at the side of a mouse that goes back, `<mouse-8>`, shows
the directory above.

Its parts are there for other code: `(dired-open dir)`,
`(dired-file-at-point)`, `(dired-marked-files)`, `(dired-refresh)`.

## Git

`C-x g` shows the status of the repository that the current buffer is in;
`runtime/lib/git.scm` is the code, and everything is done by running git.

| Key | Does |
|---|---|
| `TAB` | Fold or open the section at point: a group, a file, a hunk |
| `RET` | Go to the file and line of a change; show a commit or a stash |
| `s` / `u` | Stage / unstage the file or hunk at point, or the whole group |
| `k` | Discard the change, delete the untracked file, or drop the stash, after asking |
| `c` | Commit: `c` a new commit, `a` amend, `e` extend; `-a` and `-n` are switches |
| `b` | Branch: `b` check out, `c` create, `k` delete |
| `P` / `F` / `f` | Push (`-f` for force-with-lease), pull, fetch |
| `z` | Stash: `z` stash, `p` pop, `a` apply |
| `r` | Rebase: `u` onto the upstream, `c` continue, `s` skip, `a` abort |
| `l` | Log: `l` this branch, `a` all |
| `g` / `q` / `$` | Refresh, quit, show what git was asked and answered |

The message of a commit is written in a buffer: `C-c C-c` commits and
`C-c C-k` cancels. `M-x git-blame` shows who last changed each line of a
file. With `(enable-mode! 'git-gutter-mode)` and line numbers on, the lines
of a file that differ from what git has are marked beside their numbers when
the file is visited or saved.

The parts are there for other code: `(git root "arg" ...)` runs git and
returns what it wrote, `(git-lines root ...)` the same as lines,
`(git-parse-diff text)` takes a diff apart, and `(git-root)` finds the
repository. The views are built from `runtime/lib/section.scm`, buffers of
sections that fold, and the menus from `runtime/lib/menu.scm`:

```scheme
(key-menu "Push" '(("-f" "Force" "--force")) '(("p" "Push" push)))
```

## Loading code

`(load "file.scm")` runs a file; loading it again after a change replaces
the definitions. Set `AMACS_RUNTIME` to the `runtime` directory of a checkout
and the editor reads its own Scheme from there at start instead of from the
binary.
