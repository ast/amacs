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

A keymap can have a default: `(define-key 'term-mode #t 'term-send-key)`
runs the command for any single key that no active keymap binds, and
`(last-key)` tells the command which key it was. Keys that the global map
binds, such as `C-x C-f` and `M-x`, keep working; a mode that wants one of
them binds it outright. A mode that binds `"<paste>"` gets pasted text
through `(last-paste)` instead of having it inserted. `(window-width)` and
`(window-height)` are the size of the text area of a window.

## Hooks

`(add-hook! 'name procedure)` and `(remove-hook! 'name procedure)`. The
editor runs `pre-command-hook` and `post-command-hook` around every command,
`find-file-hook` when a file is first visited, `before-save-hook` and
`after-save-hook` around the writing of a buffer to its file,
`kill-buffer-hook` after a buffer is killed, with its name,
`window-size-change-hook` when the text area of a window changes size or
comes to show another buffer, `term-directory-hook` when the program in a
terminal has changed directory, with the buffer's name and the directory,
and a hook named after each mode when the mode is switched.

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
`(window-start w)`, `(window-end w)`, `(window-point w)`. `(window-list)`
is about the current frame, but a window's number is good in any:
`(window-live? w)` says whether it is still there, and
`with-selected-window` reaches a window of another frame, which code that
runs for a program rather than for a key may need. Such code does not know
which frame is current when it runs: `(buffer-windows name)` gives the
windows that show a buffer in every frame.

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

While a command waits for a program, what is typed waits too, and is
handled when the command has gone on: it was typed for the question that
the command then asks or the buffer that it then shows, as `c c` in the git
buffer is for the menu that the first `c` brings up once git has answered.
`C-g` stops the waiting; the command is left and what was typed for it is
dropped. This holds for commands only, not for hooks, timers or code that
is evaluated as text. A command whose program takes long and that asks
nothing afterwards, as a push over the network, calls `(release-input!)`
first, and typing goes on meanwhile.

Code evaluated as text, with `M-:` or `amacsclient --eval`, can wait too. The
evaluation then answers at once, without a value, and the forms after the
one that waits run when the wait is over.

While code waits inside `with-current-buffer`, that buffer is the one on
screen in the selected window. A hook of a buffer runs in it, so a hook
that waits for a program, as the git gutter waits for `git diff` after a
save, hands the waiting to `(run-later thunk)`: the procedure runs once
the running code has returned to the editor, outside any buffer made
current for it.

## Programs that go on running

`run-process` waits for a program to end. A process made with
`make-process` goes on as long as it likes, and the editor hears from it as
it writes:

```scheme
(define p (make-process "shell" '("sh")
                        'filter (lambda (p text) (message text))
                        'sentinel (lambda (p status) (message "ended: " status))))
(process-send-string p "echo hello\n")
(process-send-eof p)
```

A program on a pseudo-terminal is started with `'pty`, either `#t` or a
list of columns and rows; its screen then belongs in a terminal buffer,
which `runtime/lib/term.scm` builds. See the terminal in the README.

A process is a number. The properties after the command are `'filter`, a
procedure called with the process and the text its program writes, as it
writes it, on the editor's thread; `'sentinel`, called with the process and
its exit status when it ends, after the filter has had all the output, with
`#f` for a status if a signal ended it; `'buffer`, the name of a buffer the
process belongs to, into which the output goes when there is no filter, and
whose killing ends the process; `'directory`, where to run; and
`'environment`, an alist of variables to set over those of the directory's
project. `(process-live? p)`, `(process-exit-status p)`,
`(process-buffer p)`, `(get-buffer-process name)`, `(signal-process p 'int)`,
`(interrupt-process p)`, `(kill-process p)` and `(delete-process p)`, which
ends the program without word to the sentinel and forgets the process, do
what they say. `(process-list)` is every process not yet deleted, and
`M-x list-processes` shows them. Nothing waits for a process, so typing is
never held by one; every program still running is ended when the editor
exits.

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
`set-face!` defines new faces. Where the colours are not worth a face of
their own, as in the output of a program, `(add-span! 0 4 'fg "#ff0000"
'bg 'bg-dim 'bold #t)` lays them over the text directly, with the
attributes of `set-face!` and `inverse`; a span takes the place of the
spans under it, and `clear-spans!` takes them away. A major mode gives the buffer its keys:
`(define-major-mode report-mode #f "Report" "...")` and then
`(define-key 'report-mode "g" 'report-refresh)`. What the feature knows about
each of its buffers it keeps itself, in a hash table by buffer name. What it
has to say about a buffer in few words goes on the mode line after the
modes: `(set-mode-line-text! "[working]")`, and `#f` to take it away.

## Files

`(directory-entries dir)`, `(file-exists? name)`, `(file-directory? name)`,
`(make-directory name)`, `(delete-file name)`, `(rename-file from to)`,
`(read-file name)`, `(expand-file-name name)`, `(abbreviate-file-name name)`,
`(file-name-directory name)`, `(file-name-nondirectory name)`. Copying is
left to `cp`, through `run-process`, so that it does not hold the editor up.
`(read-file-name "Prompt: ")` and `(read-directory-name "Prompt: ")` ask for
names with completion.

A file is read into a buffer in one go, so the editor looks first:
`(find-file name)` is an error for a file that is not text, and for one
above the `large-file-threshold` option (megabytes; 50 unless changed, `#f`
for no limit) unless called as `(find-file name #t)`. `(file-text? name)`
and `(file-large? name)` say beforehand, from the start of the file and
its size, and `(file-size name)` gives the bytes. What the editor should
not read, another program can show: `(start-process "xdg-open" (list
name))` starts a program and leaves it to itself, where `run-process`
waits for it to end.

## The file manager

`C-x d`, or visiting a directory, lists it, and `C-x C-j` lists the
directory of the current buffer's file with point on the file; `runtime/lib/dired.scm` is the
code. The keys are dired's: `RET` visits, `^` or `DEL` goes up, `m` `u` `U`
`t` mark, `C` copies, `R` moves or renames, `D` deletes, `d` and `x` flag and
delete, `+` makes a directory, `g` reads again, `q` quits. `TAB` opens a
directory in place, `/` narrows to names that contain a text, `.` hides the
dot files, `s` changes the order, `b` goes to a directory of the
`dired-quick-access` option and `r` to one visited before. With two listings
side by side, copying and moving offer the other one as where to.

A file that is not text, such as a picture or a disc image, is not read:
`RET` hands it to the program of the `dired-open-program` option,
`xdg-open` unless changed, which opens it with what the desktop uses for
its kind. `E` does that for any file, text or not. A text file above
`large-file-threshold` is opened after a question.

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

## The buffer list

`C-x C-b` lists the buffers in groups, one for each project, with marks
and commands that act on the marked ones; `runtime/lib/ibuffer.scm` is the
code, and it is built from the sections of `runtime/lib/section.scm`, so a
group folds with `TAB`. The keys are ibuffer's: `RET` and `o` switch to the
buffer, `m` `u` `U` `t` mark, `*` marks by kind, `D` kills, `d` and `x`
flag and kill, `S` saves, `s` sorts, `/` narrows, `g` reads again and `q`
leaves. Marking on the heading of a group marks every buffer in it. With
the pointer, a press goes to a buffer or folds a group, a second press
opens it, `C-<mouse-1>` marks and the button for menus lists what can be
done.

A buffer that visits no file is listed under "Other buffers", unless its
major mode is in the `ibuffer-project-modes` option, which starts as the
file manager and git; those go under the project of their directory. The
`ibuffer-show-hidden` option says whether the buffers whose names start
with a space are listed; `.` toggles that in the list.

The parts are there for other code: `(ibuffer)`, `(ibuffer-rows)` with
`ibuffer-row-name`, `ibuffer-row-file`, `ibuffer-row-mode`,
`ibuffer-row-size` and `ibuffer-row-modified?`, `(ibuffer-buffer-at-point)`,
`(ibuffer-marked-buffers)` and `(ibuffer-refresh)`.

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
