use std::path::PathBuf;

use crate::buffer::BufferId;
use crate::completion::Completion;
use crate::editor::Editor;
use crate::error::{CmdError, CmdResult};
use crate::files::{abbreviate_path, expand_path, modified_time};
use crate::minibuffer::default_directory;

pub fn find_file(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    let dir = default_directory(ed);
    let base = ed.working_directory().to_owned();
    ed.read_string("Find file: ", &dir, Completion::File, move |ed, input| {
        ed.find_file(&expand_path(&input, &base))
    })
}

pub fn save_buffer(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    let Some(path) = ed.buffer().file.clone() else {
        return write_file(ed);
    };
    if !ed.buffer().is_modified() {
        ed.message("(No changes need to be saved)");
        return Ok(());
    }
    save_checking_disk(ed, path)
}

/// Saves to the buffer's own file, asking first if another program has
/// changed the file since it was read.
fn save_checking_disk(ed: &mut Editor, path: PathBuf) -> CmdResult {
    let on_disk = modified_time(&path);
    if on_disk.is_some() && on_disk != ed.buffer().disk_mtime {
        let name = abbreviate_path(&path);
        ed.ask_yes_no(
            format!("{name} changed on disk; overwrite it?"),
            move |ed| ed.write_buffer(&path),
        );
        return Ok(());
    }
    ed.write_buffer(&path)
}

pub fn write_file(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    let dir = default_directory(ed);
    let base = ed.working_directory().to_owned();
    ed.read_string("Write file: ", &dir, Completion::File, move |ed, input| {
        let path = expand_path(&input, &base);
        if input.is_empty() || input.ends_with('/') || path.is_dir() {
            return Err(CmdError::msg("Please give a file name"));
        }
        let is_own_file = ed.buffer().file.as_deref() == Some(&path);
        if path.exists() && !is_own_file {
            let name = abbreviate_path(&path);
            ed.ask_yes_no(format!("File {name} exists; overwrite?"), move |ed| {
                ed.write_buffer(&path)
            });
            return Ok(());
        }
        ed.write_buffer(&path)
    })
}

pub fn kill_buffer(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    let completion = Completion::Items {
        candidates: ed.buffer_candidates(true),
        category: "buffer".to_owned(),
    };
    ed.read_string("Kill buffer: ", "", completion, move |ed, name| {
        let name = &name;
        let id = ed
            .find_buffer(name)
            .ok_or_else(|| CmdError::msg(format!("No such buffer: {name}")))?;
        if needs_saving(ed, id) {
            ed.ask_yes_no(format!("Buffer {name} modified; kill anyway?"), move |ed| {
                ed.kill_buffer(id);
                Ok(())
            });
            return Ok(());
        }
        ed.kill_buffer(id);
        Ok(())
    })
}

/// Whether killing the buffer would lose changes to a file.
fn needs_saving(ed: &Editor, id: BufferId) -> bool {
    let buffer = &ed.buffers[id];
    buffer.file.is_some() && buffer.is_modified()
}

/// Leaves the editor from this terminal. For a client of a server that
/// closes the client's frame; otherwise it exits the editor.
pub fn save_buffers_kill_terminal(ed: &mut Editor) -> CmdResult {
    if ed.daemon && ed.current_frame() != ed.base_frame() {
        return ed.close_frame(ed.current_frame());
    }
    kill_editor(ed)
}

pub fn delete_frame(ed: &mut Editor) -> CmdResult {
    ed.close_frame(ed.current_frame())
}

/// Exits the editor, and with it every client's frame, after asking about
/// unsaved buffers.
pub fn kill_editor(ed: &mut Editor) -> CmdResult {
    let unsaved = ed.buffer_ids().filter(|id| needs_saving(ed, *id)).count();
    if unsaved == 0 {
        ed.request_exit();
        return Ok(());
    }
    let what = if unsaved == 1 {
        "1 modified buffer has".to_owned()
    } else {
        format!("{unsaved} modified buffers have")
    };
    ed.ask_yes_no(format!("{what} not been saved; exit anyway?"), |ed| {
        ed.request_exit();
        Ok(())
    });
    Ok(())
}

/// Declares the current buffer finished, for a client that is waiting for
/// it: `amacsclient FILE` returns, and the buffer is killed.
pub fn server_edit(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    if ed.buffer().waiters.is_empty() {
        return Err(CmdError::msg("No client is waiting for this buffer"));
    }
    let id = ed.window().buffer;
    if !needs_saving(ed, id) {
        ed.kill_buffer(id);
        return Ok(());
    }
    let name = ed.buffer().name.clone();
    ask_save_then_finish(ed, name, id);
    Ok(())
}

fn ask_save_then_finish(ed: &mut Editor, name: String, id: BufferId) {
    ed.ask_key(
        format!("Save {name} before finishing? (y or n) "),
        move |ed, key| {
            match key.self_insert_char() {
                Some('y' | 'Y') => {
                    if let Some(path) = ed.buffer().file.clone() {
                        ed.write_buffer(&path)?;
                    }
                    ed.kill_buffer(id);
                }
                Some('n' | 'N') => ed.kill_buffer(id),
                _ if key == amacs_proto::Key::ctrl('g') => return Err(CmdError::Quit),
                _ => ask_save_then_finish(ed, name, id),
            }
            Ok(())
        },
    );
}

pub fn recover_this_file(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    ed.recover_this_file()
}

pub fn goto_line(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    ed.read_string("Goto line: ", "", Completion::None, |ed, input| {
        let line: usize = input
            .trim()
            .parse()
            .map_err(|_| CmdError::msg("Please enter a line number"))?;
        let point = ed.point();
        ed.buffer_mut().set_mark(point);
        ed.goto_line(line, 0);
        Ok(())
    })
}
