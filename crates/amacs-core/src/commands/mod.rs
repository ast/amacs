//! The built-in commands and their default key bindings.

mod edit;
mod files;
mod motion;
mod search;
mod windows;

use crate::completion::Completion;
use crate::editor::{CommandBody, Editor, GLOBAL_MAP, MINIBUFFER_MAP, Prefix};
use crate::error::{CmdError, CmdResult};
use crate::minibuffer;

pub(crate) use edit::delete_backward_char;

macro_rules! commands {
    ($editor:expr; $( $name:literal => $run:path, $doc:literal; )*) => {
        $( $editor.define_command($name, $doc, CommandBody::Native($run)); )*
    };
}

pub fn install(ed: &mut Editor) {
    commands! { ed;
        "self-insert-command" => edit::self_insert, "Insert the character you type.";
        "newline" => edit::newline, "Insert a newline.";
        "open-line" => edit::open_line, "Insert a newline and leave point before it.";
        "delete-char" => edit::delete_char, "Delete the following character.";
        "delete-backward-char" => edit::delete_backward_char, "Delete the previous character.";
        "kill-line" => edit::kill_line, "Kill the rest of the line, or the newline at the end of a line.";
        "kill-word" => edit::kill_word, "Kill to the end of the next word.";
        "backward-kill-word" => edit::backward_kill_word, "Kill back to the start of the previous word.";
        "kill-region" => edit::kill_region, "Kill the text between point and mark.";
        "kill-ring-save" => edit::kill_ring_save, "Save the region as if killed, without killing it.";
        "yank" => edit::yank, "Reinsert the most recently killed text.";
        "yank-pop" => search::yank_pop, "Replace the just-yanked text with an earlier kill, or choose a kill to insert.";
        "search-lines" => search::search_lines, "Go to a line of the buffer chosen by its text.";
        "search-files" => search::search_files, "Search the project's files with ripgrep and visit a match.";
        "recent-file" => search::recent_file, "Visit a recently visited file.";
        "act-on-candidate" => search::act_on_candidate, "Do something else with the selected candidate.";
        "undo" => edit::undo, "Undo the last change.";
        "undo-redo" => edit::undo_redo, "Redo the last undone change.";
        "set-mark-command" => edit::set_mark, "Set the mark at point and activate the region.";
        "exchange-point-and-mark" => edit::exchange_point_and_mark, "Swap point and mark.";
        "mark-whole-buffer" => edit::mark_whole_buffer, "Put point at the beginning and mark at the end of the buffer.";
        "keyboard-quit" => keyboard_quit, "Cancel the current operation.";
        "universal-argument" => universal_argument, "Begin a numeric argument for the following command.";
        "digit-argument" => digit_argument, "Part of the numeric argument for the next command.";
        "execute-extended-command" => execute_extended_command, "Read a command name and run it.";
        "eval-expression" => eval_expression, "Read a Scheme expression, evaluate it and show the result.";
        "eval-buffer" => eval_buffer, "Evaluate the current buffer as Scheme code.";

        "forward-char" => motion::forward_char, "Move point right one character.";
        "backward-char" => motion::backward_char, "Move point left one character.";
        "next-line" => motion::next_line, "Move point down one line.";
        "previous-line" => motion::previous_line, "Move point up one line.";
        "move-beginning-of-line" => motion::beginning_of_line, "Move point to the beginning of the line.";
        "move-end-of-line" => motion::end_of_line, "Move point to the end of the line.";
        "forward-word" => motion::forward_word, "Move point forward one word.";
        "backward-word" => motion::backward_word, "Move point backward one word.";
        "forward-paragraph" => motion::forward_paragraph, "Move to the end of the paragraph.";
        "backward-paragraph" => motion::backward_paragraph, "Move to the start of the paragraph.";
        "beginning-of-buffer" => motion::beginning_of_buffer, "Move point to the beginning of the buffer.";
        "end-of-buffer" => motion::end_of_buffer, "Move point to the end of the buffer.";
        "scroll-up-command" => motion::scroll_up, "Scroll text upward by nearly a full window.";
        "scroll-down-command" => motion::scroll_down, "Scroll text downward by nearly a full window.";
        "recenter-top-bottom" => motion::recenter, "Scroll so that point is in the middle of the window.";

        "find-file" => files::find_file, "Visit a file in its own buffer.";
        "save-buffer" => files::save_buffer, "Save the current buffer in its file.";
        "write-file" => files::write_file, "Write the current buffer to a file and visit that file.";
        "switch-to-buffer" => search::switch_to_buffer, "Show another buffer, or a recent file, in the selected window.";
        "kill-buffer" => files::kill_buffer, "Kill a buffer.";
        "save-buffers-kill-terminal" => files::save_buffers_kill_terminal, "Leave the editor from this terminal: close a client's frame, or exit.";
        "kill-editor" => files::kill_editor, "Exit the editor and close every frame.";
        "delete-frame" => files::delete_frame, "Close the current frame.";
        "server-edit" => files::server_edit, "Finish editing the buffer for the client that is waiting for it.";
        "recover-this-file" => files::recover_this_file, "Replace the buffer's text with its auto-save data.";
        "goto-line" => files::goto_line, "Read a line number and move to that line.";

        "split-window-below" => windows::split_below, "Split the selected window into two, one above the other.";
        "split-window-right" => windows::split_right, "Split the selected window into two side-by-side windows.";
        "delete-window" => windows::delete_window, "Remove the selected window.";
        "delete-other-windows" => windows::delete_other_windows, "Make the selected window fill the frame.";
        "other-window" => windows::other_window, "Select the next window.";

        "exit-minibuffer" => minibuffer::exit_minibuffer, "Finish minibuffer input.";
        "abort-minibuffer" => minibuffer::abort_minibuffer, "Cancel minibuffer input.";
        "exit-minibuffer-with-input" => minibuffer::exit_with_input, "Accept the text as typed, ignoring the candidates.";
        "next-candidate" => minibuffer::next_candidate, "Select the next candidate.";
        "previous-candidate" => minibuffer::previous_candidate, "Select the previous candidate, or the input itself.";
        "next-candidate-page" => minibuffer::next_candidate_page, "Move the selection down by a page of candidates.";
        "previous-candidate-page" => minibuffer::previous_candidate_page, "Move the selection up by a page of candidates.";
        "first-candidate" => minibuffer::first_candidate, "Select the first candidate.";
        "last-candidate" => minibuffer::last_candidate, "Select the last candidate.";
        "insert-candidate" => minibuffer::insert_candidate, "Replace the input with the selected candidate.";
        "minibuffer-delete-backward" => minibuffer::delete_backward, "Delete backward; in a file name, delete a whole directory.";
        "previous-history-element" => minibuffer::previous_history, "Replace the input with the previous one from history.";
        "next-history-element" => minibuffer::next_history, "Replace the input with the next one from history.";
    }

    const GLOBAL: &[(&str, &str)] = &[
        ("RET", "newline"),
        ("C-m", "newline"),
        ("C-j", "newline"),
        ("C-o", "open-line"),
        ("C-d", "delete-char"),
        ("<delete>", "delete-char"),
        ("DEL", "delete-backward-char"),
        ("C-k", "kill-line"),
        ("M-d", "kill-word"),
        ("M-DEL", "backward-kill-word"),
        ("C-DEL", "backward-kill-word"),
        ("C-w", "kill-region"),
        ("M-w", "kill-ring-save"),
        ("C-y", "yank"),
        ("M-y", "yank-pop"),
        ("C-/", "undo"),
        ("C-_", "undo"),
        ("C-x u", "undo"),
        ("C-?", "undo-redo"),
        ("C-M-_", "undo-redo"),
        ("C-SPC", "set-mark-command"),
        ("C-@", "set-mark-command"),
        ("C-x C-x", "exchange-point-and-mark"),
        ("C-x h", "mark-whole-buffer"),
        ("C-g", "keyboard-quit"),
        ("C-u", "universal-argument"),
        ("M-x", "execute-extended-command"),
        ("M-:", "eval-expression"),
        ("C-f", "forward-char"),
        ("<right>", "forward-char"),
        ("C-b", "backward-char"),
        ("<left>", "backward-char"),
        ("C-n", "next-line"),
        ("<down>", "next-line"),
        ("C-p", "previous-line"),
        ("<up>", "previous-line"),
        ("C-a", "move-beginning-of-line"),
        ("<home>", "move-beginning-of-line"),
        ("C-e", "move-end-of-line"),
        ("<end>", "move-end-of-line"),
        ("M-f", "forward-word"),
        ("M-<right>", "forward-word"),
        ("C-<right>", "forward-word"),
        ("M-b", "backward-word"),
        ("M-<left>", "backward-word"),
        ("C-<left>", "backward-word"),
        ("M-}", "forward-paragraph"),
        ("C-<down>", "forward-paragraph"),
        ("M-{", "backward-paragraph"),
        ("C-<up>", "backward-paragraph"),
        ("M-<", "beginning-of-buffer"),
        ("C-<home>", "beginning-of-buffer"),
        ("M->", "end-of-buffer"),
        ("C-<end>", "end-of-buffer"),
        ("C-v", "scroll-up-command"),
        ("<next>", "scroll-up-command"),
        ("M-v", "scroll-down-command"),
        ("<prior>", "scroll-down-command"),
        ("C-l", "recenter-top-bottom"),
        ("C-x C-f", "find-file"),
        ("C-x C-s", "save-buffer"),
        ("C-x C-w", "write-file"),
        ("C-x b", "switch-to-buffer"),
        ("C-s", "search-lines"),
        ("C-c /", "search-files"),
        ("C-x C-r", "recent-file"),
        ("C-x k", "kill-buffer"),
        ("C-x C-c", "save-buffers-kill-terminal"),
        ("C-x 5 0", "delete-frame"),
        ("C-x #", "server-edit"),
        ("M-g g", "goto-line"),
        ("M-g M-g", "goto-line"),
        ("C-x 2", "split-window-below"),
        ("C-x 3", "split-window-right"),
        ("C-x 0", "delete-window"),
        ("C-x 1", "delete-other-windows"),
        ("C-x o", "other-window"),
    ];
    let global = ed.keymap_mut(GLOBAL_MAP);
    for (keys, command) in GLOBAL {
        global.bind_str(keys, command);
    }
    for digit in 0..=9 {
        global.bind_str(&format!("M-{digit}"), "digit-argument");
    }

    const MINIBUFFER: &[(&str, &str)] = &[
        ("RET", "exit-minibuffer"),
        ("C-m", "exit-minibuffer"),
        ("C-j", "exit-minibuffer"),
        ("C-g", "abort-minibuffer"),
        ("M-RET", "exit-minibuffer-with-input"),
        ("TAB", "insert-candidate"),
        ("C-i", "insert-candidate"),
        ("C-n", "next-candidate"),
        ("<down>", "next-candidate"),
        ("C-p", "previous-candidate"),
        ("<up>", "previous-candidate"),
        ("C-v", "next-candidate-page"),
        ("<next>", "next-candidate-page"),
        ("M-v", "previous-candidate-page"),
        ("<prior>", "previous-candidate-page"),
        ("M-<", "first-candidate"),
        ("M->", "last-candidate"),
        ("DEL", "minibuffer-delete-backward"),
        ("M-p", "previous-history-element"),
        ("M-n", "next-history-element"),
        ("C-.", "act-on-candidate"),
    ];
    let minibuffer = ed.keymap_mut(MINIBUFFER_MAP);
    for (keys, command) in MINIBUFFER {
        minibuffer.bind_str(keys, command);
    }
}

fn keyboard_quit(ed: &mut Editor) -> CmdResult {
    ed.buffer_mut().mark_active = false;
    Err(CmdError::Quit)
}

fn universal_argument(ed: &mut Editor) -> CmdResult {
    let next = match ed.raw_prefix() {
        Some(Prefix::Universal(n)) => Prefix::Universal(n.saturating_mul(4)),
        Some(other) => other,
        None => Prefix::Universal(4),
    };
    ed.prefix_arg = Some(next);
    Ok(())
}

fn digit_argument(ed: &mut Editor) -> CmdResult {
    let digit = ed
        .last_key
        .and_then(|key| match key.code {
            amacs_proto::KeyCode::Char(c) => c.to_digit(10),
            amacs_proto::KeyCode::Named(_) => None,
        })
        .ok_or_else(|| CmdError::msg("digit-argument must be bound to a digit key"))?;
    let digit = i64::from(digit);
    ed.prefix_arg = Some(match ed.raw_prefix() {
        Some(prefix) => prefix.push_digit(digit),
        None => Prefix::Number(digit),
    });
    Ok(())
}

fn eval_expression(ed: &mut Editor) -> CmdResult {
    if !ed.has_script_host() {
        return Err(CmdError::msg("Scheme is not available in this session"));
    }
    ed.read_string("Eval: ", "", Completion::None, |ed, source| {
        ed.queue_eval(source, true)
    })
}

fn eval_buffer(ed: &mut Editor) -> CmdResult {
    let source = ed.text().to_string();
    ed.queue_eval(source, false)
}

fn execute_extended_command(ed: &mut Editor) -> CmdResult {
    ed.read_string("M-x ", "", Completion::Command, |ed, name| {
        ed.execute(name.trim());
        Ok(())
    })
}
