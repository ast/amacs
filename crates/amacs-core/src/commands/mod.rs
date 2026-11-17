//! The built-in commands and their default key bindings.

mod edit;
mod files;
pub(crate) mod indent;
mod misc;
mod motion;
mod rect;
mod search;
mod sexp;
mod windows;

use crate::completion::Completion;
use crate::editor::{CommandBody, Editor, GLOBAL_MAP, MINIBUFFER_MAP, Prefix};
use crate::error::{CmdError, CmdResult};
use crate::minibuffer;
use crate::{complete, decorate, diagnostics, format, project, snippet, undo_view};

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

        "indent-for-tab-command" => indent::indent_for_tab_command, "Indent the current line, or the lines of the active region.";
        "indent-region" => indent::indent_region, "Indent every line of the region.";
        "forward-sexp" => sexp::forward_sexp, "Move forward over a balanced expression.";
        "backward-sexp" => sexp::backward_sexp, "Move backward over a balanced expression.";
        "kill-sexp" => sexp::kill_sexp, "Kill the balanced expression after point.";
        "backward-kill-sexp" => sexp::backward_kill_sexp, "Kill the balanced expression before point.";
        "mark-sexp" => sexp::mark_sexp, "Put the mark at the end of the balanced expression after point.";
        "backward-up-list" => sexp::backward_up_list, "Move out of the enclosing list, to its start.";
        "up-list" => sexp::up_list, "Move out of the enclosing list, past its end.";
        "down-list" => sexp::down_list, "Move into the next list.";
        "paredit-open-round" => sexp::paredit_open_round, "Insert a balanced pair of round brackets.";
        "paredit-open-square" => sexp::paredit_open_square, "Insert a balanced pair of square brackets.";
        "paredit-close" => sexp::paredit_close, "Move past the closing bracket of the enclosing list.";
        "paredit-doublequote" => sexp::paredit_doublequote, "Insert a pair of double quotes, or step over or escape one.";
        "paredit-wrap-round" => sexp::paredit_wrap_round, "Wrap the following expression in round brackets.";
        "paredit-splice-sexp" => sexp::paredit_splice, "Remove the brackets of the enclosing list.";
        "paredit-raise-sexp" => sexp::paredit_raise, "Replace the enclosing list with the following expression.";
        "paredit-forward-slurp-sexp" => sexp::paredit_forward_slurp, "Take the expression after the enclosing list into it.";
        "paredit-forward-barf-sexp" => sexp::paredit_forward_barf, "Put the last expression of the enclosing list after it.";
        "paredit-backward-slurp-sexp" => sexp::paredit_backward_slurp, "Take the expression before the enclosing list into it.";
        "paredit-backward-barf-sexp" => sexp::paredit_backward_barf, "Put the first expression of the enclosing list before it.";

        "completion-at-point" => complete::completion_at_point, "Show ways to complete the word before point.";
        "completion-next" => complete::completion_next, "Select the next completion.";
        "completion-previous" => complete::completion_previous, "Select the previous completion.";
        "completion-accept" => complete::completion_accept, "Insert the selected completion.";
        "completion-quit" => complete::completion_quit, "Close the list of completions.";
        "snippet-complete" => complete::snippet_complete, "Expand the snippet named before point, or show those it could be.";
        "snippet-insert" => snippet_insert, "Choose a snippet and insert it.";
        "snippet-next-field" => snippet::next_field, "Move to the next field of the snippet.";
        "snippet-previous-field" => snippet::previous_field, "Move to the previous field of the snippet.";
        "format-buffer" => format::format_buffer, "Format the buffer with its formatter.";
        "next-error" => diagnostics::next_error, "Move to the next diagnostic.";
        "previous-error" => diagnostics::previous_error, "Move to the previous diagnostic.";
        "list-diagnostics" => diagnostics::list_diagnostics, "Choose one of the buffer's diagnostics and go to it.";
        "xref-find-definitions" => Editor::xref_find_definitions, "Go to the definition of the symbol at point.";
        "xref-find-references" => Editor::xref_find_references, "List the places that refer to the symbol at point.";
        "xref-go-back" => Editor::xref_go_back, "Return to where the last jump started.";
        "lsp" => Editor::lsp_ensure_command, "Start the language server for this buffer.";
        "lsp-restart" => Editor::lsp_restart, "Stop this buffer's language server and start it again.";
        "lsp-shutdown" => Editor::lsp_shutdown, "Stop this buffer's language server.";
        "lsp-hover" => Editor::lsp_hover, "Show what the language server knows about the symbol at point.";
        "lsp-rename" => Editor::lsp_rename, "Rename the symbol at point everywhere it is used.";
        "lsp-code-actions" => Editor::lsp_code_actions, "Choose among the fixes and refactorings offered at point.";
        "lsp-document-symbols" => Editor::lsp_document_symbols, "Go to a symbol of this buffer.";
        "lsp-workspace-symbols" => Editor::lsp_workspace_symbols, "Find a symbol of the project by name.";
        "direnv-reload" => direnv_reload, "Read the project environments from direnv again.";

        "windmove-left" => windows::windmove_left, "Select the window to the left.";
        "windmove-right" => windows::windmove_right, "Select the window to the right.";
        "windmove-up" => windows::windmove_up, "Select the window above.";
        "windmove-down" => windows::windmove_down, "Select the window below.";
        "enlarge-window" => windows::enlarge_window, "Make the selected window a row taller.";
        "shrink-window" => windows::shrink_window, "Make the selected window a row shorter.";
        "enlarge-window-horizontally" => windows::enlarge_window_horizontally, "Make the selected window a column wider.";
        "shrink-window-horizontally" => windows::shrink_window_horizontally, "Make the selected window a column narrower.";
        "balance-windows" => windows::balance_windows, "Give all windows the same share of the frame.";
        "find-file-other-window" => misc::find_file_other_window, "Visit a file in another window.";
        "switch-to-buffer-other-window" => misc::switch_to_buffer_other_window, "Show a buffer in another window.";
        "project-find-file" => project::project_find_file, "Visit a file of the current project.";
        "project-switch-project" => project::project_switch_project, "Pick a project visited before, then a file in it.";
        "project-switch-to-buffer" => project::project_switch_to_buffer, "Show one of the current project's buffers.";
        "project-kill-buffers" => project::project_kill_buffers, "Kill the buffers of the current project.";
        "undo-tree-visualize" => undo_view::visualize, "Show the undo history as a tree to move about in.";
        "undo-tree-backward" => undo_view::backward, "Go to the state before this one.";
        "undo-tree-forward" => undo_view::forward, "Go to the next state on this branch.";
        "undo-tree-next-branch" => undo_view::next_branch, "Go to the branch below.";
        "undo-tree-previous-branch" => undo_view::previous_branch, "Go to the branch above.";
        "undo-tree-quit" => undo_view::quit, "Leave the undo tree, keeping the text as it is.";
        "undo-tree-abort" => undo_view::abort, "Leave the undo tree and go back to where it was opened.";
        "kill-rectangle" => rect::kill_rectangle, "Kill the rectangle whose corners are point and mark.";
        "copy-rectangle-as-kill" => rect::copy_rectangle, "Save the rectangle as if killed.";
        "delete-rectangle" => rect::delete_rectangle, "Delete the rectangle whose corners are point and mark.";
        "yank-rectangle" => rect::yank_rectangle, "Insert the last killed rectangle with its corner at point.";
        "open-rectangle" => rect::open_rectangle, "Fill the rectangle with spaces, pushing its text right.";
        "string-rectangle" => rect::string_rectangle, "Replace the rectangle's text on each line with a string.";
        "rectangle-mark-mode" => rect::rectangle_mark_mode, "Mark a rectangle: the region is the columns between point and mark.";
        "scroll-up-line" => misc::scroll_up_line, "Scroll the text up a line.";
        "scroll-down-line" => misc::scroll_down_line, "Scroll the text down a line.";
        "comment-or-uncomment-region" => misc::comment_or_uncomment, "Comment out the lines of the region or of point, or uncomment them.";
        "move-text-up" => misc::move_text_up, "Move the line, or the lines of the region, up a line.";
        "move-text-down" => misc::move_text_down, "Move the line, or the lines of the region, down a line.";
        "back-to-indentation" => misc::back_to_indentation, "Move to the first char of the line that is not blank.";
        "delete-horizontal-space" => misc::delete_horizontal_space, "Delete the spaces and tabs around point.";
        "just-one-space" => misc::just_one_space, "Leave one space where there are spaces and tabs around point.";
        "delete-indentation" => misc::delete_indentation, "Join this line to the one above.";
        "upcase-region" => misc::upcase_region, "Make the region upper case.";
        "downcase-region" => misc::downcase_region, "Make the region lower case.";
        "upcase-word" => misc::upcase_word, "Make the rest of the word upper case.";
        "downcase-word" => misc::downcase_word, "Make the rest of the word lower case.";
        "capitalize-word" => misc::capitalize_word, "Capitalise the rest of the word.";
        "read-only-mode" => misc::read_only_mode, "Make the buffer read-only, or editable again.";
        "query-replace" => misc::query_replace, "Replace some occurrences of a text after point, asking at each.";
        "symbol-next" => misc::symbol_next, "Go to the next occurrence of the symbol at point.";
        "symbol-previous" => misc::symbol_previous, "Go to the previous occurrence of the symbol at point.";
        "symbol-rename" => misc::symbol_rename, "Rename the symbol at point everywhere in the buffer.";
        "bookmark-set" => misc::bookmark_set, "Remember this place in this file under a name.";
        "bookmark-jump" => misc::bookmark_jump, "Go to a bookmark.";
        "bookmark-delete" => misc::bookmark_delete, "Forget a bookmark.";
        "shell-command" => misc::shell_command, "Run a shell command and show what it writes.";
        "shell-command-on-region" => misc::shell_command_on_region, "Run a shell command on the region; with a prefix argument, replace the region with what it writes.";
        "describe-key" => misc::describe_key, "Say which command a key sequence runs.";
        "describe-command" => misc::describe_command, "Show what a command does and its keys.";
        "revert-buffer" => files::revert_buffer, "Replace the buffer's text with what its file holds now.";

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
        "save-some-buffers" => files::save_some_buffers, "Save every modified buffer that visits a file.";
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
        ("TAB", "indent-for-tab-command"),
        ("C-i", "indent-for-tab-command"),
        ("C-M-\\", "indent-region"),
        ("C-M-f", "forward-sexp"),
        ("C-M-<right>", "forward-sexp"),
        ("C-M-b", "backward-sexp"),
        ("C-M-<left>", "backward-sexp"),
        ("C-M-k", "kill-sexp"),
        ("C-M-DEL", "backward-kill-sexp"),
        ("C-M-SPC", "mark-sexp"),
        ("C-M-@", "mark-sexp"),
        ("C-M-u", "backward-up-list"),
        ("C-M-<up>", "backward-up-list"),
        ("C-M-n", "up-list"),
        ("C-M-d", "down-list"),
        ("C-M-<down>", "down-list"),
        ("C-g", "keyboard-quit"),
        ("M-g c", "avy-goto-char"),
        ("M-g w", "avy-goto-word"),
        ("M-g l", "avy-goto-line"),
        ("M-z", "zap-to-char"),
        ("M-Z", "zap-up-to-char"),
        ("M-o", "ace-window"),
        ("S-<left>", "windmove-left"),
        ("S-<right>", "windmove-right"),
        ("S-<up>", "windmove-up"),
        ("S-<down>", "windmove-down"),
        ("C-x ^", "enlarge-window"),
        ("C-x }", "enlarge-window-horizontally"),
        ("C-x {", "shrink-window-horizontally"),
        ("C-x +", "balance-windows"),
        ("C-x 4 f", "find-file-other-window"),
        ("C-x 4 b", "switch-to-buffer-other-window"),
        ("C-x C-b", "switch-to-buffer"),
        ("C-c p f", "project-find-file"),
        ("C-c p p", "project-switch-project"),
        ("C-c p b", "project-switch-to-buffer"),
        ("C-c p k", "project-kill-buffers"),
        ("C-c p s", "search-files"),
        ("C-x r k", "kill-rectangle"),
        ("C-x r M-w", "copy-rectangle-as-kill"),
        ("C-x r d", "delete-rectangle"),
        ("C-x r y", "yank-rectangle"),
        ("C-x r o", "open-rectangle"),
        ("C-x r t", "string-rectangle"),
        ("C-x SPC", "rectangle-mark-mode"),
        ("C-x r m", "bookmark-set"),
        ("C-x r b", "bookmark-jump"),
        ("M-;", "comment-or-uncomment-region"),
        ("C-x C-;", "comment-or-uncomment-region"),
        ("M-m", "back-to-indentation"),
        ("M-\\", "delete-horizontal-space"),
        ("M-SPC", "just-one-space"),
        ("M-^", "delete-indentation"),
        ("C-x C-u", "upcase-region"),
        ("C-x C-l", "downcase-region"),
        ("M-u", "upcase-word"),
        ("M-l", "downcase-word"),
        ("M-c", "capitalize-word"),
        ("C-x C-q", "read-only-mode"),
        ("M-%", "query-replace"),
        ("M-!", "shell-command"),
        ("M-|", "shell-command-on-region"),
        ("C-h k", "describe-key"),
        ("C-h f", "describe-command"),
        ("C-h x", "describe-command"),
        ("C-M-i", "completion-at-point"),
        ("M-TAB", "completion-at-point"),
        ("M-+", "snippet-complete"),
        ("M-*", "snippet-insert"),
        ("M-.", "xref-find-definitions"),
        ("M-,", "xref-go-back"),
        ("M-?", "xref-find-references"),
        ("M-g n", "next-error"),
        ("M-g M-n", "next-error"),
        ("M-g p", "previous-error"),
        ("M-g M-p", "previous-error"),
        ("M-g i", "lsp-document-symbols"),
        ("C-c C-r", "lsp-rename"),
        ("C-c C-a", "lsp-code-actions"),
        ("C-c C-s", "lsp-workspace-symbols"),
        ("C-c C-d", "lsp-hover"),
        ("C-c C-f", "format-buffer"),
        ("C-c !", "list-diagnostics"),
        ("C-x s", "save-some-buffers"),
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
    // Structural editing for Lisp. The mode itself is switched from Scheme.
    const PAREDIT: &[(&str, &str)] = &[
        ("(", "paredit-open-round"),
        (")", "paredit-close"),
        ("[", "paredit-open-square"),
        ("]", "paredit-close"),
        ("\"", "paredit-doublequote"),
        ("M-(", "paredit-wrap-round"),
        ("M-s", "paredit-splice-sexp"),
        ("M-r", "paredit-raise-sexp"),
        ("C-)", "paredit-forward-slurp-sexp"),
        ("C-<right>", "paredit-forward-slurp-sexp"),
        ("C-}", "paredit-forward-barf-sexp"),
        ("C-<left>", "paredit-forward-barf-sexp"),
        ("C-(", "paredit-backward-slurp-sexp"),
        ("C-M-<left>", "paredit-backward-slurp-sexp"),
        ("C-{", "paredit-backward-barf-sexp"),
        ("C-M-<right>", "paredit-backward-barf-sexp"),
    ];
    ed.define_mode(sexp::PAREDIT_MODE, None, "Paredit");
    ed.define_mode(edit::AUTO_PAIR_MODE, None, "");
    let paredit = ed.keymap_mut(sexp::PAREDIT_MODE);
    for (keys, command) in PAREDIT {
        paredit.bind_str(keys, command);
    }

    const POPUP: &[(&str, &str)] = &[
        ("C-n", "completion-next"),
        ("<down>", "completion-next"),
        ("C-p", "completion-previous"),
        ("<up>", "completion-previous"),
        ("RET", "completion-accept"),
        ("C-m", "completion-accept"),
        ("TAB", "completion-accept"),
        ("C-i", "completion-accept"),
        ("C-g", "completion-quit"),
    ];
    let popup = ed.keymap_mut(complete::POPUP_MAP);
    for (keys, command) in POPUP {
        popup.bind_str(keys, command);
    }
    const UNDO_TREE: &[(&str, &str)] = &[
        ("b", "undo-tree-backward"),
        ("C-b", "undo-tree-backward"),
        ("<left>", "undo-tree-backward"),
        ("f", "undo-tree-forward"),
        ("C-f", "undo-tree-forward"),
        ("<right>", "undo-tree-forward"),
        ("n", "undo-tree-next-branch"),
        ("C-n", "undo-tree-next-branch"),
        ("<down>", "undo-tree-next-branch"),
        ("p", "undo-tree-previous-branch"),
        ("C-p", "undo-tree-previous-branch"),
        ("<up>", "undo-tree-previous-branch"),
        ("q", "undo-tree-quit"),
        ("RET", "undo-tree-quit"),
        ("C-g", "undo-tree-abort"),
    ];
    let tree = ed.keymap_mut(undo_view::UNDO_MAP);
    for (keys, command) in UNDO_TREE {
        tree.bind_str(keys, command);
    }
    for mode in [
        decorate::HL_LINE_MODE,
        decorate::SYMBOL_HIGHLIGHT_MODE,
        decorate::HL_TODO_MODE,
        decorate::WHITESPACE_MODE,
    ] {
        ed.define_mode(mode, None, "");
    }
    let fields = ed.keymap_mut(snippet::SNIPPET_MAP);
    for keys in ["TAB", "C-i"] {
        fields.bind_str(keys, "snippet-next-field");
    }
    fields.bind_str("S-TAB", "snippet-previous-field");

    let minibuffer = ed.keymap_mut(MINIBUFFER_MAP);
    for (keys, command) in MINIBUFFER {
        minibuffer.bind_str(keys, command);
    }
}

impl Editor {
    /// Scrolls the selected window by a number of lines, forward if positive.
    pub fn scroll_lines(&mut self, lines: i64) -> CmdResult {
        misc::scroll_rows(self, lines)
    }
}

fn direnv_reload(ed: &mut Editor) -> CmdResult {
    ed.reload_environments();
    ed.message("Environments will be read again when next needed");
    Ok(())
}

/// Inserts a snippet chosen by name.
fn snippet_insert(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    let candidates: Vec<crate::completion::Candidate> = ed
        .snippets_here()
        .into_iter()
        .map(|(name, _)| crate::completion::Candidate::new(name))
        .collect();
    if candidates.is_empty() {
        return Err(CmdError::msg("No snippets are defined for this mode"));
    }
    let completion = Completion::Items {
        candidates,
        category: "snippet".to_owned(),
    };
    ed.read_string("Snippet: ", "", completion, |ed, name| {
        ed.expand_snippet_named(&name)
    })
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
    // A prefix argument is for the command that is named.
    let prefix = ed.raw_prefix();
    ed.read_string("M-x ", "", Completion::Command, move |ed, name| {
        ed.prefix_arg = prefix;
        ed.execute(name.trim());
        Ok(())
    })
}
