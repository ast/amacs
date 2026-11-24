//! The editor primitives that Scheme code can call.
//!
//! Each primitive is a plain function over the core's [`Value`] type, so this
//! table does not depend on the Scheme interpreter. Names starting with `%` are
//! building blocks for the Scheme runtime in `runtime/`, not for users.
//!
//! Buffer positions are zero-based char offsets: the first position is 0 and
//! the last is the buffer's length.

use amacs_core::file_system;
use amacs_core::{
    Buffer, BufferId, Candidate, CmdError, CmdResult, CommandBody, Completion, Editor, FaceSpec,
    Formatter, ServerConfig, Value, WindowId, expand_path, window_number,
};
use amacs_proto::{FaceAttrs, Mods, parse_keys};

/// One primitive.
pub struct ApiFn {
    pub name: &'static str,
    pub min_args: usize,
    /// `None` for any number of further arguments.
    pub max_args: Option<usize>,
    pub doc: &'static str,
    pub run: fn(&mut Editor, &[Value]) -> CmdResult<Value>,
}

macro_rules! api {
    ($( $name:literal ($min:literal, $max:expr) $doc:literal => $run:expr; )*) => {
        &[ $( ApiFn { name: $name, min_args: $min, max_args: $max, doc: $doc, run: $run } ),* ]
    };
}

fn wrong_type(index: usize, expected: &str, got: &Value) -> CmdError {
    CmdError::msg(format!(
        "argument {} must be {expected}, got {got:?}",
        index + 1
    ))
}

fn int(args: &[Value], index: usize) -> CmdResult<i64> {
    match &args[index] {
        Value::Int(n) => Ok(*n),
        other => Err(wrong_type(index, "an integer", other)),
    }
}

fn string(args: &[Value], index: usize) -> CmdResult<&str> {
    match &args[index] {
        Value::Str(s) => Ok(s),
        other => Err(wrong_type(index, "a string", other)),
    }
}

/// A name given as a symbol or a string.
fn name(args: &[Value], index: usize) -> CmdResult<&str> {
    args[index]
        .as_name()
        .ok_or_else(|| wrong_type(index, "a symbol or string", &args[index]))
}

fn boolean(args: &[Value], index: usize) -> bool {
    !matches!(args[index], Value::Bool(false))
}

/// A process by its number.
fn process_id(args: &[Value], index: usize) -> CmdResult<u64> {
    u64::try_from(int(args, index)?).map_err(|_| wrong_type(index, "a process", &args[index]))
}

/// A procedure, or `#f` for none.
fn procedure_or_false(args: &[Value], index: usize) -> CmdResult<Option<amacs_core::ScriptFn>> {
    match &args[index] {
        Value::Bool(false) => Ok(None),
        Value::Func(func) => Ok(Some(func.clone())),
        other => Err(wrong_type(index, "a procedure or #f", other)),
    }
}

/// A position in the current buffer.
fn pos(ed: &Editor, args: &[Value], index: usize) -> CmdResult<usize> {
    let value = int(args, index)?;
    let len = ed.buffer().len();
    usize::try_from(value)
        .ok()
        .filter(|pos| *pos <= len)
        .ok_or_else(|| {
            CmdError::msg(format!(
                "position {value} is outside the buffer (0 to {len})"
            ))
        })
}

/// An optional position argument, defaulting to point.
fn pos_or_point(ed: &Editor, args: &[Value], index: usize) -> CmdResult<usize> {
    if index < args.len() {
        pos(ed, args, index)
    } else {
        Ok(ed.point())
    }
}

/// A path without `.` and `..` parts, worked out from the name alone.
fn tidy_path(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut tidy = std::path::PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                tidy.pop();
            }
            other => tidy.push(other),
        }
    }
    tidy
}

/// An optional window argument, defaulting to the selected window.
fn window_arg(ed: &Editor, args: &[Value], index: usize) -> CmdResult<WindowId> {
    if index < args.len() {
        ed.window_by_number(int(args, index)?)
    } else {
        Ok(ed.selected_window())
    }
}

/// An optional buffer-name argument, defaulting to the current buffer.
fn buffer_arg(ed: &Editor, args: &[Value], index: usize) -> CmdResult<BufferId> {
    if index >= args.len() {
        return Ok(ed.window().buffer);
    }
    let name = name(args, index)?;
    ed.find_buffer(name)
        .ok_or_else(|| CmdError::msg(format!("No such buffer: {name}")))
}

fn buffer<'a>(ed: &'a Editor, args: &[Value], index: usize) -> CmdResult<&'a Buffer> {
    let id = buffer_arg(ed, args, index)?;
    Ok(ed.buffer_by_id(id).expect("the buffer was just found"))
}

fn keys(args: &[Value], index: usize) -> CmdResult<Vec<amacs_proto::Key>> {
    parse_keys(string(args, index)?).map_err(|error| CmdError::msg(error.to_string()))
}

fn completion(args: &[Value], index: usize) -> CmdResult<Completion> {
    match name(args, index)? {
        "none" => Ok(Completion::None),
        "file" => Ok(Completion::File),
        "directory" => Ok(Completion::Directory),
        "buffer" => Ok(Completion::Buffer),
        "command" => Ok(Completion::Command),
        other => Err(CmdError::msg(format!("unknown completion kind: {other}"))),
    }
}

/// A list of strings, or a single string standing for a list of one.
fn strings(args: &[Value], index: usize) -> CmdResult<Vec<String>> {
    let expected = "a list of strings";
    match &args[index] {
        Value::List(items) => items
            .iter()
            .map(|item| {
                item.as_name()
                    .map(str::to_owned)
                    .ok_or_else(|| wrong_type(index, expected, item))
            })
            .collect(),
        other => other
            .as_name()
            .map(|name| vec![name.to_owned()])
            .ok_or_else(|| wrong_type(index, expected, other)),
    }
}

/// A list of two-element lists of names, such as palette entries.
fn name_pairs(args: &[Value], index: usize) -> CmdResult<Vec<(String, String)>> {
    let expected = "a list of (name value) lists";
    let Value::List(entries) = &args[index] else {
        return Err(wrong_type(index, expected, &args[index]));
    };
    entries
        .iter()
        .map(|entry| match entry {
            Value::List(pair) if pair.len() == 2 => pair[0]
                .as_name()
                .zip(pair[1].as_name())
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .ok_or_else(|| wrong_type(index, expected, entry)),
            other => Err(wrong_type(index, expected, other)),
        })
        .collect()
}

/// Applies `attribute value ...` arguments to a face.
fn apply_face_attributes(spec: &mut FaceSpec, attributes: &[Value]) -> CmdResult {
    if !attributes.len().is_multiple_of(2) {
        return Err(CmdError::msg(
            "set-face! takes attributes and values in pairs",
        ));
    }
    for pair in attributes.chunks(2) {
        let on = !matches!(pair[1], Value::Bool(false));
        let color = || match &pair[1] {
            Value::Bool(false) => Ok(None),
            value => value
                .as_name()
                .map(|name| Some(name.to_owned()))
                .ok_or_else(|| CmdError::msg("a colour is a palette name, \"#rrggbb\" or #f")),
        };
        match pair[0].as_name() {
            Some("fg") => spec.fg = color()?,
            Some("bg") => spec.bg = color()?,
            Some("bold") => spec.bold = on,
            Some("italic") => spec.italic = on,
            Some("underline") => {
                spec.underline = on;
                spec.undercurl = pair[1].as_name() == Some("wavy");
            }
            Some("underline-color") => spec.underline_color = color()?,
            Some("box") => spec.box_color = color()?,
            _ => {
                return Err(CmdError::msg(format!(
                    "unknown face attribute {:?}; use fg, bg, bold, italic, underline, underline-color or box",
                    pair[0]
                )));
            }
        }
    }
    Ok(())
}

/// The attributes of a span from `attribute value ...` arguments, with the
/// colours resolved against the palette now.
fn span_attributes(ed: &Editor, attributes: &[Value]) -> CmdResult<FaceAttrs> {
    if !attributes.len().is_multiple_of(2) {
        return Err(CmdError::msg(
            "add-span! takes attributes and values in pairs",
        ));
    }
    let mut attrs = FaceAttrs::default();
    for pair in attributes.chunks(2) {
        let on = !matches!(pair[1], Value::Bool(false));
        let color = || match &pair[1] {
            Value::Bool(false) => Ok(None),
            value => {
                let name = value.as_name().ok_or_else(|| {
                    CmdError::msg("a colour is a palette name, \"#rrggbb\" or #f")
                })?;
                ed.faces()
                    .color(name)
                    .map(Some)
                    .ok_or_else(|| CmdError::msg(format!("unknown colour: {name}")))
            }
        };
        match pair[0].as_name() {
            Some("fg") => attrs.fg = color()?,
            Some("bg") => attrs.bg = color()?,
            Some("bold") => attrs.bold = on,
            Some("italic") => attrs.italic = on,
            Some("underline") => {
                attrs.underline = on;
                attrs.undercurl = pair[1].as_name() == Some("wavy");
            }
            Some("inverse") => attrs.inverse = on,
            Some("underline-color") => attrs.underline_color = color()?,
            _ => {
                return Err(CmdError::msg(format!(
                    "unknown span attribute {:?}; use fg, bg, bold, italic, underline, inverse or underline-color",
                    pair[0]
                )));
            }
        }
    }
    Ok(attrs)
}

pub const API: &[ApiFn] = api! {
    // Text

    "point" (0, Some(0)) "Position of point in the current buffer." =>
        |ed, _| Ok(ed.point().into());
    "point-min" (0, Some(0)) "The first position in the buffer, which is 0." =>
        |_, _| Ok(Value::Int(0));
    "point-max" (0, Some(0)) "The last position in the buffer, which is its length." =>
        |ed, _| Ok(ed.buffer().len().into());
    "goto-char" (1, Some(1)) "Move point to a position." =>
        |ed, args| {
            let pos = pos(ed, args, 0)?;
            ed.set_point(pos);
            Ok(Value::Void)
        };
    "insert" (0, None) "Insert strings at point, leaving point after them." =>
        |ed, args| {
            for index in 0..args.len() {
                ed.insert(string(args, index)?)?;
            }
            Ok(Value::Void)
        };
    "delete-region" (2, Some(2)) "Delete the text between two positions." =>
        |ed, args| {
            let (a, b) = (pos(ed, args, 0)?, pos(ed, args, 1)?);
            ed.delete(a.min(b)..a.max(b))?;
            Ok(Value::Void)
        };
    "buffer-substring" (2, Some(2)) "The text between two positions." =>
        |ed, args| {
            let (a, b) = (pos(ed, args, 0)?, pos(ed, args, 1)?);
            Ok(ed.text().slice(a.min(b)..a.max(b)).into())
        };
    "string-width" (1, Some(1)) "The number of columns a string takes on screen: two for a wide char such as a CJK ideograph, none for a combining mark." =>
        |_, args| Ok(amacs_core::string_cells(string(args, 0)?).into());
    "buffer-string" (0, Some(1)) "The whole text of the current buffer, or of the named one." =>
        |ed, args| Ok(buffer(ed, args, 0)?.text().to_string().into());
    "erase-buffer" (0, Some(0)) "Delete all text in the current buffer." =>
        |ed, _| {
            let len = ed.buffer().len();
            ed.delete(0..len)?;
            Ok(Value::Void)
        };
    "char-after" (0, Some(1)) "The character at a position (default point) as a string, or #f at the end." =>
        |ed, args| {
            let pos = pos_or_point(ed, args, 0)?;
            Ok(ed.text().char_at(pos).map(String::from).into())
        };
    "line-beginning-position" (0, Some(1)) "Start of the line containing a position (default point)." =>
        |ed, args| {
            let pos = pos_or_point(ed, args, 0)?;
            let text = ed.text();
            Ok(text.line_start(text.line_of(pos)).into())
        };
    "line-end-position" (0, Some(1)) "End of the line containing a position (default point)." =>
        |ed, args| {
            let pos = pos_or_point(ed, args, 0)?;
            let text = ed.text();
            Ok(text.line_end(text.line_of(pos)).into())
        };
    "goto-line" (1, Some(1)) "Move point to the start of a line, counting from 1. A number beyond the last line goes to the last." =>
        |ed, args| {
            let line = usize::try_from(int(args, 0)?).unwrap_or(1).max(1);
            ed.goto_line(line, 0);
            Ok(Value::Void)
        };
    "line-number-at-pos" (0, Some(1)) "Line number of a position (default point), counting from 1." =>
        |ed, args| {
            let pos = pos_or_point(ed, args, 0)?;
            Ok((ed.text().line_of(pos) + 1).into())
        };
    "current-column" (0, Some(0)) "Characters between the start of the line and point." =>
        |ed, _| {
            let (text, point) = (ed.text(), ed.point());
            Ok((point - text.line_start(text.line_of(point))).into())
        };

    // Buffers

    "buffer-name" (0, Some(0)) "Name of the current buffer." =>
        |ed, _| Ok(ed.buffer().name.as_str().into());
    "buffer-file-name" (0, Some(1)) "File that a buffer visits, or #f." =>
        |ed, args| {
            let file = buffer(ed, args, 0)?.file.as_ref();
            Ok(file.map(|path| path.display().to_string()).into())
        };
    "buffer-modified?" (0, Some(1)) "Whether a buffer has unsaved changes." =>
        |ed, args| Ok(buffer(ed, args, 0)?.is_modified().into());
    "buffer-read-only?" (0, Some(1)) "Whether a buffer refuses edits." =>
        |ed, args| Ok(buffer(ed, args, 0)?.read_only.into());
    "set-buffer-read-only!" (1, Some(1)) "Make the current buffer read-only, or writable with #f." =>
        |ed, args| {
            ed.buffer_mut().read_only = boolean(args, 0);
            Ok(Value::Void)
        };
    "buffer-list" (0, Some(0)) "Names of all buffers, most recently selected first." =>
        |ed, _| {
            let names = ed.buffer_ids().filter_map(|id| ed.buffer_by_id(id));
            Ok(Value::List(names.map(|buffer| buffer.name.as_str().into()).collect()))
        };
    "switch-to-buffer" (1, Some(1)) "Show the named buffer in the selected window, creating it if needed." =>
        |ed, args| {
            let name = name(args, 0)?;
            let id = match ed.find_buffer(name) {
                Some(id) => id,
                None => ed.add_buffer(Buffer::new(name)),
            };
            ed.switch_to_buffer(id);
            Ok(Value::Void)
        };
    "kill-buffer" (0, Some(1)) "Remove a buffer without asking, even if it is modified." =>
        |ed, args| {
            let id = buffer_arg(ed, args, 0)?;
            ed.kill_buffer(id);
            Ok(Value::Void)
        };
    "file-remote?" (1, Some(1)) "Whether a file name is that of a file on another host, in tramp's syntax: /ssh:user@host:/path. A name that starts as one and is not whole counts as one." =>
        |ed, args| Ok(file_system::is_remote(&expand_path(string(args, 0)?, ed.working_directory())).into());
    "%fetch-file" (3, Some(3)) "Start reading a file into a buffer in the background, as files on other hosts are read, also one above large-file-threshold if the second argument is not #f. If the third is not #f, what is typed meanwhile waits for the file, as it does when a command asks for it. Returns a promise for a list: (buffer name note) with something to tell the user or #f, (directory name), (large description), or (error message). See %visit-remote." =>
        |ed, args| {
            let path = expand_path(string(args, 0)?, ed.working_directory());
            Ok(Value::Int(ed.fetch_file_promise(path, boolean(args, 1), boolean(args, 2)) as i64))
        };
    "%send-buffer" (1, Some(1)) "Start writing the current buffer to a file in the background, as files on other hosts are written, and make that its file when it is written, without running before-save-hook. Returns a promise for #t, or for a string that says what went wrong." =>
        |ed, args| {
            let path = expand_path(string(args, 0)?, ed.working_directory());
            Ok(Value::Int(ed.send_buffer_promise(path) as i64))
        };
    "remote-hosts" (0, Some(0)) "The hosts that files have been asked of since the editor started: for each a list of the start of the names of its files, such as \"/ssh:evo:\", and how its connection is: \"connected\", \"connecting\", or why there is none." =>
        |_, _| {
            let hosts = amacs_core::file_system::hosts();
            Ok(Value::List(hosts.into_iter().map(|(host, how)| Value::List(vec![host.into(), how.into()])).collect()))
        };
    "remote-disconnect" (1, Some(1)) "End the connection to a host, named as remote-hosts names it. What is under way there fails, and the next file asked for connects again. Returns whether the host was known." =>
        |_, args| Ok(amacs_core::file_system::disconnect(string(args, 0)?).into());
    "%find-file" (1, Some(2)) "Visit a file in the selected window, as find-file does. A file on another host is shown when it has been read, after this returns." =>
        |ed, args| {
            let path = expand_path(string(args, 0)?, ed.working_directory());
            if args.get(1).is_some_and(|anyway| *anyway != Value::Bool(false)) {
                ed.find_file_anyway(&path)?;
            } else {
                ed.find_file(&path)?;
            }
            Ok(Value::Void)
        };
    "file-size" (1, Some(1)) "The size of a file in bytes." =>
        |ed, args| {
            let path = expand_path(string(args, 0)?, ed.working_directory());
            let meta = file_system::metadata(&path)
                .map_err(|error| CmdError::msg(format!("cannot look at {}: {error}", path.display())))?;
            Ok(Value::Int(meta.len as i64))
        };
    "file-text?" (1, Some(1)) "Whether a file looks like text: its start holds no NUL byte. Only the start is read. A file that cannot be read is an error." =>
        |ed, args| {
            let path = expand_path(string(args, 0)?, ed.working_directory());
            amacs_core::is_binary(&path)
                .map(|binary| (!binary).into())
                .map_err(|error| CmdError::msg(format!("cannot read {}: {error}", path.display())))
        };
    "file-large?" (1, Some(1)) "Whether a file is text above the size of the large-file-threshold option, which is read only when asked to in so many words." =>
        |ed, args| {
            let path = expand_path(string(args, 0)?, ed.working_directory());
            Ok(matches!(ed.file_kind(&path), amacs_core::FileKind::Large(_)).into())
        };
    "release-input!" (0, Some(0)) "From here on, the running command does not keep what is typed waiting while it runs programs. Without this, keys typed while a command waits for a program are handled when the command has gone on, since they may be for what it then asks or shows. For work that takes long and asks nothing after it." =>
        |ed, _| {
            ed.release_input();
            Ok(Value::Void)
        };
    "start-process" (2, Some(2)) "Start a program, with its arguments as a list of strings, and leave it to itself: nothing waits for it and what it writes is thrown away. For programs that show something of their own. An error if it cannot be started." =>
        |ed, args| {
            let Value::List(given) = &args[1] else {
                return Err(wrong_type(1, "a list of strings", &args[1]));
            };
            let arguments = given
                .iter()
                .map(|arg| match arg {
                    Value::Str(text) => Ok(text.clone()),
                    other => Err(wrong_type(1, "a list of strings", other)),
                })
                .collect::<CmdResult<Vec<String>>>()?;
            ed.start_process(string(args, 0)?, &arguments)?;
            Ok(Value::Void)
        };
    "%write-buffer" (1, Some(1)) "Write the current buffer to a file and make that its file, without running before-save-hook." =>
        |ed, args| {
            let path = expand_path(string(args, 0)?, ed.working_directory());
            ed.write_buffer_now(&path)?;
            Ok(Value::Void)
        };
    "read-file" (1, Some(1)) "The contents of a file, as a string. A relative name is taken from the working directory, and a leading ~ is the home directory." =>
        |ed, args| {
            let path = expand_path(string(args, 0)?, ed.working_directory());
            file_system::read_to_string(&path)
                .map(Value::from)
                .map_err(|error| CmdError::msg(format!("cannot read {}: {error}", path.display())))
        };
    "working-directory" (0, Some(0)) "Directory that relative file names are resolved against." =>
        |ed, _| Ok(ed.working_directory().display().to_string().into());

    // Mark and kill ring

    "mark" (0, Some(0)) "Position of the mark, or #f if it is not set." =>
        |ed, _| Ok(ed.buffer().mark().into());
    "set-mark!" (1, Some(1)) "Set the mark without activating the region." =>
        |ed, args| {
            let pos = pos(ed, args, 0)?;
            ed.buffer_mut().set_mark(pos);
            Ok(Value::Void)
        };
    "region-active?" (0, Some(0)) "Whether the region is active." =>
        |ed, _| Ok((ed.buffer().mark_active && ed.buffer().mark().is_some()).into());
    "activate-mark!" (0, Some(0)) "Activate the region." =>
        |ed, _| {
            ed.buffer_mut().mark_active = true;
            Ok(Value::Void)
        };
    "deactivate-mark!" (0, Some(0)) "Deactivate the region." =>
        |ed, _| {
            ed.buffer_mut().mark_active = false;
            Ok(Value::Void)
        };
    "kill-region" (2, Some(2)) "Delete the text between two positions and put it on the kill ring." =>
        |ed, args| {
            let (a, b) = (pos(ed, args, 0)?, pos(ed, args, 1)?);
            let text = ed.delete(a.min(b)..a.max(b))?;
            ed.kill_new(text);
            Ok(Value::Void)
        };
    "kill-new" (1, Some(1)) "Put a string on the kill ring." =>
        |ed, args| {
            ed.kill_new(string(args, 0)?.to_owned());
            Ok(Value::Void)
        };
    "current-kill" (0, Some(0)) "The newest kill ring entry, or #f." =>
        |ed, _| Ok(ed.current_kill().into());

    // Commands and keys

    "%define-command" (3, Some(3)) "Define a command: name, documentation, procedure." =>
        |ed, args| {
            let Value::Func(func) = &args[2] else {
                return Err(wrong_type(2, "a procedure", &args[2]));
            };
            ed.define_command(name(args, 0)?, string(args, 1)?, CommandBody::Script(func.clone()));
            Ok(Value::Void)
        };
    "%script-command" (1, Some(1)) "The procedure of a command defined in Scheme, or #f." =>
        |ed, args| Ok(match ed.command(name(args, 0)?).map(|command| &command.body) {
            Some(CommandBody::Script(func)) => Value::Func(func.clone()),
            _ => Value::Bool(false),
        });
    "%call-native-command" (1, Some(1)) "Run a built-in command by name." =>
        |ed, args| {
            let name = name(args, 0)?;
            match ed.native_command(name) {
                Some(run) => run(ed).map(|()| Value::Void),
                None if ed.command(name).is_some() => {
                    Err(CmdError::msg(format!("{name} is not a built-in command")))
                }
                None => Err(CmdError::msg(format!("Unknown command: {name}"))),
            }
        };
    "%restore-native-command" (1, Some(1)) "Make a name stand for its built-in command again." =>
        |ed, args| {
            let name = name(args, 0)?;
            let Some(run) = ed.native_command(name) else {
                return Err(CmdError::msg(format!("{name} is not a built-in command")));
            };
            let doc = ed.command(name).map(|command| command.doc.clone()).unwrap_or_default();
            ed.define_command(name, &doc, CommandBody::Native(run));
            Ok(Value::Void)
        };
    "%native-command?" (1, Some(1)) "Whether there is a built-in command of this name, even if a command defined in Scheme has taken the name." =>
        |ed, args| Ok(ed.native_command(name(args, 0)?).is_some().into());
    "command-names" (0, Some(0)) "The names of all commands, as strings, in order." =>
        |ed, _| {
            let mut names: Vec<&str> = ed.command_names().collect();
            names.sort_unstable();
            Ok(Value::List(names.into_iter().map(Value::from).collect()))
        };
    "command?" (1, Some(1)) "Whether a command with this name exists." =>
        |ed, args| Ok(ed.command(name(args, 0)?).is_some().into());
    "command-documentation" (1, Some(1)) "Documentation of a command, or #f." =>
        |ed, args| Ok(ed.command(name(args, 0)?).map(|command| command.doc.as_str()).into());
    "prefix-arg" (0, Some(0)) "Numeric prefix argument of the running command; 1 if none was given." =>
        |ed, _| Ok(Value::Int(ed.arg()));
    "prefix-arg-given?" (0, Some(0)) "Whether the running command was given a prefix argument." =>
        |ed, _| Ok(ed.raw_prefix().is_some().into());
    "%bind-key" (3, Some(3)) "Bind a key sequence in a keymap: keymap name, keys, command name. With #t for the keys, the command is the keymap's default: it runs for any single key that no active keymap binds." =>
        |ed, args| {
            let command = name(args, 2)?.to_owned();
            if args[1] == Value::Bool(true) {
                ed.keymap_mut(name(args, 0)?).set_default(Some(&command));
                return Ok(Value::Void);
            }
            let keys = keys(args, 1)?;
            ed.keymap_mut(name(args, 0)?).bind(&keys, &command);
            Ok(Value::Void)
        };
    "%unbind-key" (2, Some(2)) "Remove a key binding from a keymap: keymap name, keys; #t for the keys removes the default." =>
        |ed, args| {
            if args[1] == Value::Bool(true) {
                ed.keymap_mut(name(args, 0)?).set_default(None);
                return Ok(Value::Void);
            }
            let keys = keys(args, 1)?;
            ed.keymap_mut(name(args, 0)?).unbind(&keys);
            Ok(Value::Void)
        };
    "this-command" (0, Some(0)) "The name of the command that is running, as a symbol, or #f outside a command." =>
        |ed, _| Ok(ed.this_command().map(Value::sym).into());
    "last-key" (0, Some(0)) "The key that ran the command that is running, as a string such as \"a\", \"C-x\" or \"<paste>\"; for a command bound as a keymap's default, which key it was. #f before any key." =>
        |ed, _| Ok(ed.last_key().map(|key| key.to_string()).into());
    "last-paste" (0, Some(0)) "The text last pasted into a buffer whose keymap binds <paste>, which the command bound to it takes from here. #f before any." =>
        |ed, _| Ok(ed.last_paste().map(str::to_owned).into());
    "key-binding" (1, Some(1)) "Name of the command a key sequence runs in the current buffer, or #f." =>
        |ed, args| {
            let keys = keys(args, 0)?;
            Ok(ed.key_binding(&keys).map(Value::Sym).unwrap_or(Value::Bool(false)))
        };

    // Messages and prompts

    "%message" (1, Some(1)) "Show a string in the echo area." =>
        |ed, args| {
            ed.message(string(args, 0)?);
            Ok(Value::Void)
        };
    "log-message" (1, Some(1)) "Append a string to the *Messages* buffer." =>
        |ed, args| {
            ed.log(string(args, 0)?);
            Ok(Value::Void)
        };
    "%queue-eval" (1, Some(1)) "Evaluate source text at top level once the running code has returned to the editor." =>
        |ed, args| {
            ed.queue_eval(string(args, 0)?, false)?;
            Ok(Value::Void)
        };
    "%read-string" (3, Some(3)) "Start reading a string: prompt, initial input, completion kind. Returns a promise." =>
        |ed, args| {
            let completion = completion(args, 2)?;
            let id = ed.read_string_promise(string(args, 0)?, string(args, 1)?, completion)?;
            Ok(Value::Int(id as i64))
        };
    "%completing-read" (3, Some(3)) "Start choosing among candidates: prompt, candidates, category. A candidate is a string or a list of a string and its annotation. Returns a promise." =>
        |ed, args| {
            let Value::List(items) = &args[1] else {
                return Err(wrong_type(1, "a list", &args[1]));
            };
            let candidates = items
                .iter()
                .map(|item| match item {
                    Value::Str(text) => Ok(Candidate::new(text)),
                    Value::List(parts) => match parts.as_slice() {
                        [Value::Str(text), Value::Str(annotation)] => {
                            Ok(Candidate::annotated(text, annotation))
                        }
                        _ => Err(CmdError::msg("a candidate list must be (text annotation)")),
                    },
                    other => Err(wrong_type(1, "a list of strings or (text annotation) lists", other)),
                })
                .collect::<CmdResult<Vec<_>>>()?;
            let completion = Completion::Items {
                candidates,
                category: name(args, 2)?.to_owned(),
            };
            let id = ed.read_string_promise(string(args, 0)?, "", completion)?;
            Ok(Value::Int(id as i64))
        };
    "%y-or-n" (1, Some(1)) "Start asking a yes-or-no question. Returns a promise." =>
        |ed, args| Ok(Value::Int(ed.ask_yes_no_promise(string(args, 0)?) as i64));

    // Options, hooks and modes

    "define-option" (3, Some(3)) "Define a user option: name, default value, documentation." =>
        |ed, args| {
            ed.define_option(name(args, 0)?, args[1].clone(), string(args, 2)?);
            Ok(Value::Void)
        };
    "option" (1, Some(1)) "Value of an option in the current buffer." =>
        |ed, args| ed.option(name(args, 0)?);
    "set-option!" (2, Some(2)) "Set the global value of an option." =>
        |ed, args| ed.set_option(name(args, 0)?, args[1].clone()).map(|()| Value::Void);
    "set-local!" (2, Some(2)) "Set the value of an option in the current buffer only." =>
        |ed, args| ed.set_local_option(name(args, 0)?, args[1].clone()).map(|()| Value::Void);
    "%hook-active!" (2, Some(2)) "Tell the editor whether a hook has any functions." =>
        |ed, args| {
            ed.set_hook_active(name(args, 0)?, boolean(args, 1));
            Ok(Value::Void)
        };
    "%define-mode" (3, Some(3)) "Register a mode: name, parent mode or #f, name for the mode line." =>
        |ed, args| {
            let parent = args[1].as_name().map(str::to_owned);
            ed.define_mode(name(args, 0)?, parent.as_deref(), string(args, 2)?);
            Ok(Value::Void)
        };
    "%mode-parent" (1, Some(1)) "Parent of a mode, or #f." =>
        |ed, args| Ok(ed.mode_parent(name(args, 0)?).map(Value::sym).unwrap_or(Value::Bool(false)));
    "%set-major-mode!" (1, Some(1)) "Set the current buffer's major mode." =>
        |ed, args| ed.set_major_mode(name(args, 0)?).map(|()| Value::Void);
    "major-mode" (0, Some(0)) "The current buffer's major mode, as a symbol." =>
        |ed, _| Ok(Value::sym(&ed.buffer().major_mode));
    "%set-minor-mode!" (3, Some(3)) "Turn a minor mode on or off: name, global?, enabled?." =>
        |ed, args| {
            ed.set_minor_mode(name(args, 0)?, boolean(args, 1), boolean(args, 2))
                .map(|()| Value::Void)
        };
    "minor-mode-enabled?" (1, Some(1)) "Whether a minor mode is on in the current buffer or globally." =>
        |ed, args| Ok(ed.minor_mode_enabled(name(args, 0)?).into());

    // Languages, faces and themes

    "set-buffer-language!" (1, Some(1)) "Set the language of the current buffer, which decides highlighting and indentation; #f for none." =>
        |ed, args| {
            ed.set_language(args[0].as_name())?;
            Ok(Value::Void)
        };
    "buffer-language" (0, Some(0)) "Name of the current buffer's language, or #f." =>
        |ed, _| Ok(ed.buffer().language().map(|language| language.name).into());
    "indent-line" (0, Some(0)) "Indent the current line as its language says." =>
        |ed, _| {
            ed.indent_current_line()?;
            Ok(Value::Void)
        };
    "load-theme" (1, Some(1)) "Switch to the named theme." =>
        |ed, args| {
            ed.faces_mut().load_theme(name(args, 0)?)?;
            Ok(Value::Void)
        };
    "current-theme" (0, Some(0)) "Name of the theme in use, as a symbol." =>
        |ed, _| Ok(Value::sym(ed.faces().theme()));
    "theme-names" (0, Some(0)) "Names of all themes, as strings." =>
        |ed, _| Ok(Value::List(ed.faces().theme_names().map(Value::from).collect()));
    "define-theme" (3, Some(3)) "Define a theme: its name, the theme it starts from, and a list of (name value) palette entries that differ. A value is \"#rrggbb\" or the name of another entry." =>
        |ed, args| {
            let entries = name_pairs(args, 2)?;
            let (theme, base) = (name(args, 0)?.to_owned(), name(args, 1)?.to_owned());
            ed.faces_mut().define_theme(&theme, &base, entries)?;
            Ok(Value::Void)
        };
    "set-palette-overrides!" (1, Some(1)) "Replace palette entries in every theme: a list of (name value) lists, such as '((comment yellow-faint))." =>
        |ed, args| {
            let entries = name_pairs(args, 0)?;
            ed.faces_mut().set_overrides(entries);
            Ok(Value::Void)
        };
    "set-face!" (1, None) "Change a face, or define a new one: its name, then attributes and values. The attributes are 'fg and 'bg (a palette name, \"#rrggbb\" or #f) 'bold and 'italic (booleans), 'underline (#t, #f, or 'wavy for a wave), 'underline-color for a line in another colour than the text, and 'box (the colour of thin lines above and below)." =>
        |ed, args| {
            let face = name(args, 0)?.to_owned();
            let mut spec = ed.faces().spec(&face).cloned().unwrap_or_default();
            apply_face_attributes(&mut spec, &args[1..])?;
            ed.faces_mut().set_face(&face, spec);
            Ok(Value::Void)
        };

    // Language servers, formatters and snippets

    "lsp-server!" (2, Some(3)) "Say which program is the language server for some languages: a language name or a list of them, the command as a list of strings, and optionally the server's initialization options as a string of JSON." =>
        |ed, args| {
            let command = strings(args, 1)?;
            let (program, rest) = command
                .split_first()
                .ok_or_else(|| CmdError::msg("the command needs a program"))?;
            let options = match args.get(2) {
                Some(Value::Str(json)) => serde_json::from_str(json)
                    .map_err(|error| CmdError::msg(format!("the options are not JSON: {error}")))?,
                Some(Value::Bool(false)) | None => serde_json::Value::Null,
                Some(other) => return Err(wrong_type(2, "a string of JSON", other)),
            };
            ed.lsp_define_server(ServerConfig {
                languages: strings(args, 0)?,
                program: program.clone(),
                args: rest.to_vec(),
                options,
            });
            Ok(Value::Void)
        };
    "lsp-ensure" (0, Some(0)) "Attach the current buffer to the language server for its language, starting the server if need be. Does nothing if there is none." =>
        |ed, _| {
            ed.lsp_ensure()?;
            Ok(Value::Void)
        };
    "lsp-busy?" (0, Some(0)) "Whether a language server is starting or has yet to answer something." =>
        |ed, _| Ok(ed.lsp_busy().into());
    "buffer-diagnostics" (0, Some(0)) "The diagnostics of the current buffer: lists of start, end, severity ('error, 'warning or 'note) and message." =>
        |ed, _| Ok(ed.diagnostics_value());
    "define-formatter" (3, Some(3)) "Define a formatter: its name, the program, and its arguments as a list of strings, in which \"{file}\" stands for the buffer's file name. The program reads the text on standard input and writes it formatted to standard output." =>
        |ed, args| {
            let formatter = Formatter {
                program: string(args, 1)?.to_owned(),
                args: strings(args, 2)?,
            };
            ed.define_formatter(name(args, 0)?, formatter);
            Ok(Value::Void)
        };
    "format-buffer" (0, Some(0)) "Format the current buffer with the formatter its `formatter` option names. Returns whether the text changed." =>
        |ed, _| Ok(ed.format_buffer()?.into());
    "%define-snippet" (3, Some(3)) "Define a snippet: mode, name, and the template as a list." =>
        |ed, args| {
            let Value::List(elements) = &args[2] else {
                return Err(wrong_type(2, "a list", &args[2]));
            };
            let (mode, snippet) = (name(args, 0)?.to_owned(), name(args, 1)?.to_owned());
            ed.define_snippet(&mode, &snippet, elements.clone());
            Ok(Value::Void)
        };
    "%expand-snippet-data" (1, Some(1)) "Expand a template, given as a list without procedures, at point." =>
        |ed, args| {
            let Value::List(elements) = &args[0] else {
                return Err(wrong_type(0, "a list", &args[0]));
            };
            ed.expand_snippet_elements(elements.clone())?;
            Ok(Value::Void)
        };
    "snippet-names" (0, Some(0)) "Names of the snippets that apply in the current buffer." =>
        |ed, _| {
            let names = ed.snippets_here();
            Ok(Value::List(names.into_iter().map(|(name, _)| Value::from(name)).collect()))
        };

    "scroll-lines" (1, Some(1)) "Scroll the selected window by a number of lines, forward if positive. Point moves only to stay in view." =>
        |ed, args| {
            ed.scroll_lines(int(args, 0)?)?;
            Ok(Value::Void)
        };
    "project-root" (0, Some(0)) "Root directory of the current buffer's project, or #f if it is in none." =>
        |ed, _| {
            let root = ed.project_of(ed.working_directory());
            Ok(root.map(|root| root.display().to_string()).into())
        };

    // Text that code writes and decorates

    "%replace-buffer-text!" (1, Some(1)) "Make a string the whole text of the current buffer, whether or not it is read-only. The undo history and the faces laid over the text are dropped, point goes to the start, and the buffer counts as unchanged." =>
        |ed, args| {
            let text = string(args, 0)?;
            let buffer = ed.buffer_mut();
            buffer.reset(text.into());
            buffer.mark_saved();
            ed.set_point(0);
            Ok(Value::Void)
        };
    "add-face!" (3, Some(3)) "Lay a face over the text between two positions of the current buffer. The face is a name, as given to set-face!. It stays on that text as the text around it changes." =>
        |ed, args| {
            let (a, b) = (pos(ed, args, 0)?, pos(ed, args, 1)?);
            let face = name(args, 2)?.to_owned();
            ed.buffer_mut().add_text_face(a.min(b)..a.max(b), &face);
            Ok(Value::Void)
        };
    "buffer-default-face" (0, Some(1)) "The name of the default face of a buffer (default the current one), or #f for the theme's default." =>
        |ed, args| Ok(buffer(ed, args, 0)?.default_face().map(Value::sym).into());
    "set-buffer-default-face!" (1, Some(2)) "Give a buffer (default the current one) a default face of its own, a name, in place of the theme's: the background and foreground of empty cells and of text with no face take its colours. #f restores the theme's default." =>
        |ed, args| {
            let face = match &args[0] {
                Value::Bool(false) => None,
                other => Some(other.as_name().ok_or_else(|| wrong_type(0, "a face name or #f", other))?.to_owned()),
            };
            let buffer = buffer_arg(ed, args, 1)?;
            ed.set_buffer_default_face(buffer, face);
            Ok(Value::Void)
        };
    "add-span!" (2, None) "Lay colours and attributes over the text between two positions of the current buffer, without a face of a name: after the positions come attributes and values in pairs, as set-face! takes them, among fg, bg, bold, italic, underline, underline-color and inverse. Colours are palette names or \"#rrggbb\", looked up now. The span stays on its text as the text around it changes, and takes the place of spans under it." =>
        |ed, args| {
            let (a, b) = (pos(ed, args, 0)?, pos(ed, args, 1)?);
            let attrs = span_attributes(ed, &args[2..])?;
            ed.buffer_mut().add_span(a.min(b)..a.max(b), attrs);
            Ok(Value::Void)
        };
    "clear-spans!" (0, Some(2)) "Take away the spans that add-span! laid over the current buffer: those that touch the text between two positions, which are cut there, or all of them." =>
        |ed, args| {
            let range = match args.len() {
                0 => None,
                2 => {
                    let (a, b) = (pos(ed, args, 0)?, pos(ed, args, 1)?);
                    Some(a.min(b)..a.max(b))
                }
                _ => return Err(CmdError::msg("give both ends of the range, or neither")),
            };
            ed.buffer_mut().clear_spans(range);
            Ok(Value::Void)
        };
    "clear-faces!" (0, Some(2)) "Take away the faces that add-face! laid over the current buffer: those that touch the text between two positions, or all of them." =>
        |ed, args| {
            let range = match args.len() {
                0 => None,
                2 => {
                    let (a, b) = (pos(ed, args, 0)?, pos(ed, args, 1)?);
                    Some(a.min(b)..a.max(b))
                }
                _ => return Err(CmdError::msg("give both ends of the range, or neither")),
            };
            ed.buffer_mut().clear_text_faces(range);
            Ok(Value::Void)
        };
    "set-line-signs!" (1, Some(1)) "Replace the marks beside the lines of the current buffer: a list of lists of a line number, counting from 1, and the name of the face to draw its mark in. The marks are shown where the line numbers are." =>
        |ed, args| {
            let Value::List(entries) = &args[0] else {
                return Err(wrong_type(0, "a list", &args[0]));
            };
            let mut signs = Vec::new();
            for entry in entries {
                match entry {
                    Value::List(parts) if parts.len() == 2 => match (&parts[0], parts[1].as_name()) {
                        (Value::Int(line), Some(face)) if *line >= 1 => {
                            signs.push((*line as usize - 1, face.to_owned()));
                        }
                        _ => return Err(wrong_type(0, "a list of (line face)", entry)),
                    },
                    other => return Err(wrong_type(0, "a list of (line face)", other)),
                }
            }
            ed.buffer_mut().set_line_signs(signs);
            Ok(Value::Void)
        };
    "set-buffer-directory!" (1, Some(1)) "Set the directory that relative file names are taken from in the current buffer, for a buffer that visits no file; #f for the default." =>
        |ed, args| {
            let directory = match &args[0] {
                Value::Bool(false) => None,
                _ => Some(expand_path(string(args, 0)?, ed.working_directory())),
            };
            ed.buffer_mut().directory = directory;
            Ok(Value::Void)
        };
    "set-mode-line-text!" (1, Some(1)) "Set text that the mode line of the current buffer shows after its modes, for what code has to say about the buffer; #f for none." =>
        |ed, args| {
            let text = match &args[0] {
                Value::Bool(false) => None,
                _ => Some(string(args, 0)?.to_owned()),
            };
            ed.buffer_mut().mode_line = text;
            Ok(Value::Void)
        };
    "mode-line-text" (0, Some(0)) "The text that set-mode-line-text! gave the current buffer, or #f." =>
        |ed, _| Ok(ed.buffer().mode_line.clone().into());

    // Files

    "expand-file-name" (1, Some(2)) "A file name made absolute: relative to the second argument, or to the working directory, with a leading ~ as the home directory and without . and .. parts." =>
        |ed, args| {
            let base = match args.get(1) {
                Some(_) => expand_path(string(args, 1)?, ed.working_directory()),
                None => ed.working_directory().to_owned(),
            };
            Ok(tidy_path(&expand_path(string(args, 0)?, &base)).display().to_string().into())
        };
    "abbreviate-file-name" (1, Some(1)) "A file name with the home directory written as ~." =>
        |_, args| Ok(amacs_core::abbreviate_path(std::path::Path::new(string(args, 0)?)).into());
    "file-name-directory" (1, Some(1)) "The directory part of a file name, without a slash at the end, or #f if it has none. Of a directory, that is its parent." =>
        |_, args| {
            let path = std::path::Path::new(string(args, 0)?);
            Ok(path.parent().filter(|parent| !parent.as_os_str().is_empty()).map(|parent| parent.display().to_string()).into())
        };
    "file-name-nondirectory" (1, Some(1)) "The last part of a file name." =>
        |_, args| {
            let path = std::path::Path::new(string(args, 0)?);
            Ok(path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned()).into())
        };
    "file-exists?" (1, Some(1)) "Whether there is a file, directory or link of this name." =>
        |ed, args| Ok(file_system::symlink_metadata(&expand_path(string(args, 0)?, ed.working_directory())).is_ok().into());
    "file-directory?" (1, Some(1)) "Whether the name is that of a directory, or of a link to one." =>
        |ed, args| Ok(file_system::is_dir(&expand_path(string(args, 0)?, ed.working_directory())).into());
    "directory-entries" (1, Some(1)) "What a directory holds, in order of name: for each entry a list of its name, 'directory or 'file (a link counts as what it leads to), its size in bytes, when it was last changed in seconds since 1970, what it links to if it is a link (else #f), and whether it can be run as a program." =>
        |ed, args| {
            let directory = expand_path(string(args, 0)?, ed.working_directory());
            let failed = |error: std::io::Error| {
                CmdError::msg(format!("cannot read {}: {error}", directory.display()))
            };
            let mut entries = Vec::new();
            for entry in file_system::read_dir(&directory).map_err(failed)? {
                let path = directory.join(&entry.name);
                let own = entry.metadata;
                let is_link = own.is_some_and(|own| own.is_symlink());
                let link = is_link.then(|| file_system::read_link(&path).ok()).flatten();
                // What a link leads to says what it is, if it leads
                // anywhere; nothing else needs a second look.
                let followed = match own {
                    Some(own) if !is_link => Some(own),
                    _ => file_system::metadata(&path).ok().or(own),
                };
                let (is_dir, size, changed, runs) = followed.map_or((false, 0, 0, false), |meta| {
                    let changed = meta
                        .modified
                        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                        .map_or(0, |since| since.as_secs());
                    (meta.is_dir(), meta.len, changed, meta.is_executable())
                });
                entries.push((
                    entry.name.to_string_lossy().into_owned(),
                    Value::List(vec![
                        Value::Void,
                        Value::sym(if is_dir { "directory" } else { "file" }),
                        Value::Int(size as i64),
                        Value::Int(changed as i64),
                        link.map(|target| target.display().to_string()).into(),
                        runs.into(),
                    ]),
                ));
            }
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Ok(Value::List(
                entries
                    .into_iter()
                    .map(|(name, mut entry)| {
                        if let Value::List(parts) = &mut entry {
                            parts[0] = name.into();
                        }
                        entry
                    })
                    .collect(),
            ))
        };
    "write-file" (2, Some(2)) "Write a string to a file, replacing what the file held." =>
        |ed, args| {
            let path = expand_path(string(args, 0)?, ed.working_directory());
            file_system::write(&path, string(args, 1)?.as_bytes())
                .map(|()| Value::Void)
                .map_err(|error| CmdError::msg(format!("cannot write {}: {error}", path.display())))
        };
    "make-directory" (1, Some(1)) "Create a directory, and the directories above it that are missing." =>
        |ed, args| {
            let path = expand_path(string(args, 0)?, ed.working_directory());
            file_system::mkdir(&path)
                .map(|()| Value::Void)
                .map_err(|error| CmdError::msg(format!("cannot create {}: {error}", path.display())))
        };
    "delete-file" (1, Some(1)) "Delete a file or a link. A directory is deleted with everything in it." =>
        |ed, args| {
            let path = expand_path(string(args, 0)?, ed.working_directory());
            file_system::delete(&path)
                .map(|()| Value::Void)
                .map_err(|error| CmdError::msg(format!("cannot delete {}: {error}", path.display())))
        };
    "rename-file" (2, Some(2)) "Give a file or directory another name, which can be in another directory on the same file system. It is an error if something has the new name already." =>
        |ed, args| {
            let from = expand_path(string(args, 0)?, ed.working_directory());
            let to = expand_path(string(args, 1)?, ed.working_directory());
            if file_system::symlink_metadata(&to).is_ok() {
                return Err(CmdError::msg(format!("{} exists already", to.display())));
            }
            file_system::rename(&from, &to)
                .map(|()| Value::Void)
                .map_err(|error| CmdError::msg(format!("cannot rename {}: {error}", from.display())))
        };

    // Spelling

    "spell-dictionaries" (0, Some(0)) "The names of the dictionaries there are, each a language such as \"en_US\": those with an .aff and a .dic file in the directories of the option spell-dictionary-path." =>
        |ed, _| Ok(Value::List(ed.spell_dictionaries().into_iter().map(Value::Str).collect()));
    "%spell-suggest" (2, Some(2)) "A promise of what a word may have been meant to be, in a list of languages: a list of words, the likeliest first." =>
        |ed, args| {
            let languages = strings(args, 1)?;
            Ok(Value::Int(ed.spell_suggest(string(args, 0)?, languages) as i64))
        };
    "%spell-misspellings" (2, Some(2)) "A promise of the misspelled words between two positions of the current buffer: a list of (start end) lists." =>
        |ed, args| {
            let (a, b) = (pos(ed, args, 0)?, pos(ed, args, 1)?);
            Ok(Value::Int(ed.spell_scan(a.min(b)..a.max(b)) as i64))
        };
    "spell-accept!" (1, Some(2)) "Take a word to be spelled right from now on, whatever the dictionaries say. With a second argument that is not #f, in the current buffer only. A word accepted in small letters is accepted with a capital too. Nothing is saved: spell-save-word does that." =>
        |ed, args| {
            let local = args.get(1).is_some_and(|value| !matches!(value, Value::Bool(false)));
            ed.spell_accept(string(args, 0)?, local);
            Ok(Value::Void)
        };
    "spell-forget!" (1, Some(1)) "No longer take a word to be spelled right because of spell-accept!, here or everywhere." =>
        |ed, args| {
            ed.spell_forget(string(args, 0)?);
            Ok(Value::Void)
        };
    "spell-accepted?" (1, Some(1)) "Whether a word was accepted with spell-accept!, everywhere or in the current buffer." =>
        |ed, args| Ok(Value::Bool(ed.spell_accepted(string(args, 0)?)));
    "spell-reload!" (0, Some(0)) "Forget what is known of words and read the dictionaries again when next needed, as after one was installed or spell-dictionary-path was changed." =>
        |ed, _| {
            ed.spell_reload();
            Ok(Value::Void)
        };
    "syntax-at" (0, Some(1)) "The kind of syntax of the char at a position (default point), as the name of the face it is drawn in, such as \"comment\" or \"string\"; #f for text of no kind and in buffers without a grammar." =>
        |ed, args| {
            let at = pos_or_point(ed, args, 0)?;
            Ok(ed.syntax_at(at).map(Value::str).into())
        };

    // Windows

    "window-list" (0, Some(0)) "The windows of the frame, as numbers, the selected one first." =>
        |ed, _| Ok(Value::List(ed.window_list().into_iter().map(|id| Value::Int(window_number(id))).collect()));
    "buffer-windows" (0, Some(1)) "The windows that show a buffer (default the current one), in every frame, as numbers, those of the current frame first." =>
        |ed, args| {
            let buffer = buffer_arg(ed, args, 0)?;
            Ok(Value::List(ed.buffer_windows(buffer).into_iter().map(|id| Value::Int(window_number(id))).collect()))
        };
    "selected-window" (0, Some(0)) "The selected window, as a number." =>
        |ed, _| Ok(Value::Int(window_number(ed.selected_window())));
    "window-live?" (1, Some(1)) "Whether a window is still there, in any frame." =>
        |ed, args| Ok(ed.window_by_number(int(args, 0)?).is_ok_and(|window| ed.window_live(window)).into());
    "frame-selected-window" (1, Some(1)) "The selected window of the frame that a window is in, which need not be the current frame." =>
        |ed, args| {
            let window = ed.window_by_number(int(args, 0)?)?;
            Ok(Value::Int(window_number(ed.frame_selected_window(window)?)))
        };
    "select-window" (1, Some(1)) "Select a window. A window of another frame makes that frame the current one until the code is done, or until a window of the first frame is selected again, as with-selected-window does." =>
        |ed, args| {
            let window = ed.window_by_number(int(args, 0)?)?;
            ed.select_window(window)?;
            Ok(Value::Void)
        };
    "select-other-window" (0, Some(0)) "Select another window, to show something in, making one beside this if it is the only one." =>
        |ed, _| {
            ed.select_other_window()?;
            Ok(Value::Void)
        };
    "window-buffer" (0, Some(1)) "Name of the buffer that a window (default the selected one) shows." =>
        |ed, args| Ok(ed.window_state(window_arg(ed, args, 0)?).0.into());
    "window-point" (0, Some(1)) "Where point is in a window (default the selected one)." =>
        |ed, args| Ok(ed.window_state(window_arg(ed, args, 0)?).1.into());
    "window-width" (0, Some(1)) "Columns of a window (default the selected one) that show text: the line numbers are not counted." =>
        |ed, args| Ok(ed.window_width(window_arg(ed, args, 0)?).into());
    "window-height" (0, Some(1)) "Rows of a window (default the selected one) that show text: the mode line is not counted." =>
        |ed, args| Ok(ed.window_height(window_arg(ed, args, 0)?).into());
    "window-start" (0, Some(1)) "Position of the first char that a window (default the selected one) shows." =>
        |ed, args| Ok(ed.visible_range(window_arg(ed, args, 0)?).start.into());
    "window-end" (0, Some(1)) "Position after the last char that a window (default the selected one) shows." =>
        |ed, args| Ok(ed.visible_range(window_arg(ed, args, 0)?).end.into());
    "mouse-position" (0, Some(0)) "Where the pointer is: a list of the window it is in and the position in that window's buffer of the char it is at, or #f if it is in no window." =>
        |ed, _| Ok(mouse_place(ed.mouse_position()));
    "mouse-press-position" (0, Some(0)) "Where a button of the pointer was last pressed: a list of the window and the position in its buffer, or #f." =>
        |ed, _| Ok(mouse_place(ed.mouse_press_position()));
    "mouse-modifiers" (0, Some(0)) "The modifier keys that were held when the pointer last did something: a list of the symbols control, meta, shift and super." =>
        |ed, _| {
            let mods = ed.mouse_modifiers();
            let names = [
                (Mods::CTRL, "control"),
                (Mods::META, "meta"),
                (Mods::SHIFT, "shift"),
                (Mods::SUPER, "super"),
            ];
            Ok(Value::List(
                names
                    .into_iter()
                    .filter(|(flag, _)| mods.contains(*flag))
                    .map(|(_, name)| Value::sym(name))
                    .collect(),
            ))
        };
    "in-minibuffer?" (0, Some(0)) "Whether the minibuffer is selected." =>
        |ed, _| Ok(ed.in_minibuffer().into());
    "%set-buffer" (1, Some(1)) "Make the selected window show the named buffer for a moment, without what goes with switching to it." =>
        |ed, args| {
            let id = buffer_arg(ed, args, 0)?;
            ed.set_current_buffer(id);
            Ok(Value::Void)
        };
    "show-labels!" (1, Some(2)) "Show key sequences over places on screen, as for jumping to them: a list of lists of a window, a position in its buffer or #f for the window itself, and the keys as a string. With a second argument that is not #f, the text under them is dimmed." =>
        |ed, args| {
            let Value::List(entries) = &args[0] else {
                return Err(wrong_type(0, "a list", &args[0]));
            };
            let mut labels = Vec::new();
            for entry in entries {
                let (window, pos, keys) = match entry {
                    Value::List(parts) => match parts.as_slice() {
                        [Value::Int(window), Value::Int(pos), Value::Str(keys)] => {
                            (*window, usize::try_from(*pos).ok(), keys)
                        }
                        [Value::Int(window), Value::Bool(false), Value::Str(keys)] => (*window, None, keys),
                        _ => return Err(wrong_type(0, "a list of (window position keys)", entry)),
                    },
                    other => return Err(wrong_type(0, "a list of (window position keys)", other)),
                };
                labels.push((ed.window_by_number(window)?, pos, keys.clone()));
            }
            ed.show_labels(labels, args.len() > 1 && boolean(args, 1));
            Ok(Value::Void)
        };
    "show-hint!" (1, Some(1)) "Show lines of text, a list of strings, under the echo area until the next key: what a menu of keys offers, for instance." =>
        |ed, args| {
            ed.show_hint(strings(args, 0)?);
            Ok(Value::Void)
        };
    "hide-labels!" (0, Some(0)) "Take away the labels that `show-labels!` put up." =>
        |ed, _| {
            ed.hide_labels();
            Ok(Value::Void)
        };

    // Waiting

    "%read-key" (1, Some(1)) "Start waiting for a key, with a prompt. Returns a promise for the key as a string such as \"a\" or \"C-x\"." =>
        |ed, args| Ok(Value::Int(ed.read_key_promise(string(args, 0)?) as i64));
    "%timer" (1, Some(1)) "A promise that resolves after a number of milliseconds." =>
        |ed, args| {
            let milliseconds = u64::try_from(int(args, 0)?)
                .map_err(|_| CmdError::msg("the time to wait cannot be negative"))?;
            Ok(Value::Int(ed.timer_promise(milliseconds) as i64))
        };
    "%run-process" (3, Some(3)) "Start a program: its name, its arguments as a list of strings, and the text for its standard input. Returns a promise for a list of the exit status, what it wrote to standard output and what it wrote to standard error." =>
        |ed, args| {
            let Value::List(given) = &args[1] else {
                return Err(wrong_type(1, "a list of strings", &args[1]));
            };
            let arguments = given
                .iter()
                .map(|arg| match arg {
                    Value::Str(text) => Ok(text.clone()),
                    other => Err(wrong_type(1, "a list of strings", other)),
                })
                .collect::<CmdResult<Vec<String>>>()?;
            let id = ed.process_promise(string(args, 0)?, &arguments, string(args, 2)?.to_owned());
            Ok(Value::Int(id as i64))
        };

    // Processes that go on running

    "%make-process" (8, Some(9)) "Start a program that goes on running: a name for it, the program, its arguments as a list of strings, the directory to run in or #f for the working directory, environment variables to set as a list of lists of a name and a value, a filter or #f, a sentinel or #f, #f for pipes or a list of columns and rows for a pseudo-terminal of that size, and optionally a filter for standard error or #f to merge it into the output. Returns the process's number. See `make-process`." =>
        |ed, args| {
            let pty = match &args[7] {
                Value::Bool(false) => None,
                Value::List(size) if size.len() == 2 => match (&size[0], &size[1]) {
                    (Value::Int(cols), Value::Int(rows)) => {
                        Some((u16::try_from(*cols).unwrap_or(80), u16::try_from(*rows).unwrap_or(24)))
                    }
                    _ => return Err(wrong_type(7, "#f or a list of columns and rows", &args[7])),
                },
                other => return Err(wrong_type(7, "#f or a list of columns and rows", other)),
            };
            let arguments = strings(args, 2)?;
            let directory = match &args[3] {
                Value::Bool(false) => None,
                Value::Str(dir) => Some(expand_path(dir, ed.working_directory())),
                other => return Err(wrong_type(3, "a directory or #f", other)),
            };
            let environment = match &args[4] {
                Value::List(pairs) => pairs
                    .iter()
                    .map(|pair| match pair {
                        Value::List(parts) if parts.len() == 2 => match (&parts[0], &parts[1]) {
                            (Value::Str(name), Value::Str(value)) => Ok((name.clone(), value.clone())),
                            _ => Err(wrong_type(4, "an alist of strings", pair)),
                        },
                        other => Err(wrong_type(4, "an alist of strings", other)),
                    })
                    .collect::<CmdResult<Vec<_>>>()?,
                other => return Err(wrong_type(4, "an alist of strings", other)),
            };
            let id = ed.make_process(
                string(args, 0)?,
                string(args, 1)?,
                &arguments,
                directory,
                &environment,
                procedure_or_false(args, 5)?,
                if args.len() > 8 { procedure_or_false(args, 8)? } else { None },
                procedure_or_false(args, 6)?,
                pty,
            )?;
            Ok(Value::Int(id as i64))
        };
    "process-list" (0, Some(0)) "The processes started with `make-process` and not yet deleted, oldest first, as numbers." =>
        |ed, _| Ok(Value::List(ed.process_list().into_iter().map(|id| Value::Int(id as i64)).collect()));
    "process-name" (1, Some(1)) "The name a process was given." =>
        |ed, args| Ok(ed.process_name(process_id(args, 0)?)?.into());
    "process-program" (1, Some(1)) "The program a process runs." =>
        |ed, args| Ok(ed.process_program(process_id(args, 0)?)?.into());
    "process-id" (1, Some(1)) "The system's id of the program a process runs." =>
        |ed, args| Ok(Value::Int(i64::from(ed.process_pid(process_id(args, 0)?)?)));
    "process-live?" (1, Some(1)) "Whether the program of a process is still running." =>
        |ed, args| Ok(ed.process_live(process_id(args, 0)?)?.into());
    "process-exit-status" (1, Some(1)) "The exit status of a process whose program has ended, #f if a signal ended it, and 'running while it runs." =>
        |ed, args| Ok(match ed.process_exit_status(process_id(args, 0)?)? {
            None => Value::sym("running"),
            Some(status) => status.map(i64::from).into(),
        });
    "set-process-filter!" (2, Some(2)) "Give a process the procedure that gets what its program writes: it is called with the process and the text, on the editor's thread. #f throws the output away." =>
        |ed, args| {
            ed.set_process_filter(process_id(args, 0)?, procedure_or_false(args, 1)?)?;
            Ok(Value::Void)
        };
    "set-process-sentinel!" (2, Some(2)) "Give a process the procedure that is called when its program ends, with the process and the exit status, which is #f if a signal ended it. It is called after the filter has had all the output." =>
        |ed, args| {
            ed.set_process_sentinel(process_id(args, 0)?, procedure_or_false(args, 1)?)?;
            Ok(Value::Void)
        };
    "process-send-string" (2, Some(2)) "Send text to the input of a process's program. The text is handed to a thread, so a program slow to read does not hold the editor. An error if the program has ended or its input is closed." =>
        |ed, args| {
            ed.process_send(process_id(args, 0)?, string(args, 1)?)?;
            Ok(Value::Void)
        };
    "process-send-eof" (1, Some(1)) "Close the input of a process's program, which reads the end of its file." =>
        |ed, args| {
            ed.process_send_eof(process_id(args, 0)?)?;
            Ok(Value::Void)
        };
    "signal-process" (2, Some(2)) "Send a signal, by name, to a process's program and what it started: int, term, kill, hup or quit. The sentinel is called when the program ends. Nothing happens if it has ended already." =>
        |ed, args| {
            ed.signal_process(process_id(args, 0)?, name(args, 1)?)?;
            Ok(Value::Void)
        };
    "delete-process" (1, Some(1)) "End a process's program at once, without word to the sentinel, and forget the process." =>
        |ed, args| {
            ed.delete_process(process_id(args, 0)?)?;
            Ok(Value::Void)
        };
    "set-process-window-size!" (3, Some(3)) "Tell a program on a pseudo-terminal the size of its terminal: columns and rows." =>
        |ed, args| {
            let (cols, rows) = (int(args, 1)?, int(args, 2)?);
            ed.set_process_window_size(process_id(args, 0)?, cols.clamp(1, 65535) as u16, rows.clamp(1, 65535) as u16)?;
            Ok(Value::Void)
        };

    // Terminals

    "%make-terminal" (5, Some(6)) "Make a buffer the screen of a process on a pseudo-terminal: the buffer's name, the process, columns, rows, the lines of scrollback to keep, and a procedure called with the buffer's name, an event and its argument when the program asks something of its terminal: 'title with the text or #f, 'bell, 'clipboard with the text, 'exit with the status. The buffer's text is taken over." =>
        |ed, args| {
            let buffer = buffer_arg(ed, args, 0)?;
            let (cols, rows) = (int(args, 2)?.max(1) as usize, int(args, 3)?.max(1) as usize);
            let scrollback = int(args, 4)?.max(0) as usize;
            let handler = if args.len() > 5 { procedure_or_false(args, 5)? } else { None };
            ed.make_terminal(buffer, process_id(args, 1)?, cols, rows, scrollback, handler)?;
            Ok(Value::Void)
        };
    "terminal-buffer?" (0, Some(1)) "Whether a buffer (default the current one) is the screen of a running program." =>
        |ed, args| Ok(ed.is_terminal(buffer_arg(ed, args, 0)?).into());
    "terminal-process" (0, Some(1)) "The process whose screen a buffer (default the current one) is, or #f." =>
        |ed, args| Ok(ed.terminal_process(buffer_arg(ed, args, 0)?).map(|id| id as i64).into());
    "terminal-send-key" (1, Some(2)) "Send a key, as a string such as \"C-c\" or \"<up>\", to the program of a terminal buffer (default the current one), encoded as a terminal would send it. #f if the key is one that sends nothing." =>
        |ed, args| {
            let key: amacs_proto::Key = string(args, 0)?.parse().map_err(|error| CmdError::msg(format!("{error}")))?;
            let buffer = buffer_arg(ed, args, 1)?;
            Ok(ed.terminal_send_key(buffer, &key)?.into())
        };
    "terminal-paste" (1, Some(2)) "Send text to the program of a terminal buffer (default the current one) as a paste: bracketed if the program asked for that." =>
        |ed, args| {
            let buffer = buffer_arg(ed, args, 1)?;
            ed.terminal_paste(buffer, string(args, 0)?)?;
            Ok(Value::Void)
        };
    "terminal-resize" (2, Some(3)) "Give the terminal of a buffer (default the current one), and its program, a size in columns and rows." =>
        |ed, args| {
            let (cols, rows) = (int(args, 0)?.max(1) as usize, int(args, 1)?.max(1) as usize);
            let buffer = buffer_arg(ed, args, 2)?;
            ed.terminal_resize(buffer, cols, rows)?;
            Ok(Value::Void)
        };
    "terminal-size" (0, Some(1)) "The columns and rows of the terminal of a buffer (default the current one), as a list, or #f." =>
        |ed, args| Ok(ed.terminal_size(buffer_arg(ed, args, 0)?).map(|(cols, rows)| Value::List(vec![cols.into(), rows.into()])).into());
    "terminal-follows?" (0, Some(1)) "Whether point follows the program's cursor in a terminal buffer (default the current one). Any key sent to the program makes it follow again." =>
        |ed, args| Ok(ed.terminal_follows(buffer_arg(ed, args, 0)?).into());
    "set-terminal-follow!" (1, Some(2)) "Make point follow the program's cursor in a terminal buffer (default the current one), or stop it." =>
        |ed, args| {
            let buffer = buffer_arg(ed, args, 1)?;
            ed.set_terminal_follow(buffer, boolean(args, 0))?;
            Ok(Value::Void)
        };
    "terminal-cursor" (0, Some(1)) "Where the program's cursor is in a terminal buffer (default the current one), as a position, or #f while it is hidden." =>
        |ed, args| Ok(ed.terminal_cursor(buffer_arg(ed, args, 0)?).into());
    "terminal-alt-screen?" (0, Some(1)) "Whether the program of a terminal buffer (default the current one) has switched to the alternate screen, as full screen programs do." =>
        |ed, args| Ok(ed.terminal_alt_screen(buffer_arg(ed, args, 0)?).into());

    // The editor itself

    "regexp-search" (2, Some(3)) "Where a regular expression first matches in a string, at or after a position (default 0): a list of (start end) lists, the whole match and then each group, with #f for a group that took no part; or #f if there is no match. The syntax is that of the regex crate of Rust, with ^ and $ at the starts and ends of lines." =>
        |_, args| {
            let regex = amacs_core::regexp::compile(string(args, 0)?)?;
            let from = if args.len() > 2 {
                usize::try_from(int(args, 2)?).unwrap_or(0)
            } else {
                0
            };
            let Some(found) = amacs_core::regexp::search(&regex, string(args, 1)?, from) else {
                return Ok(Value::Bool(false));
            };
            Ok(Value::List(
                (0..found.groups())
                    .map(|group| match found.range(group) {
                        Some(range) => Value::List(vec![
                            Value::Int(range.start as i64),
                            Value::Int(range.end as i64),
                        ]),
                        None => Value::Bool(false),
                    })
                    .collect(),
            ))
        };
    "regexp-replace" (3, Some(3)) "A string with every match of a regular expression replaced: the expression, the string, the replacement, in which \\& is what matched, \\1 to \\9 what a group matched and \\\\ a backslash. Returns a list of the new string and how many matches there were." =>
        |_, args| {
            let regex = amacs_core::regexp::compile(string(args, 0)?)?;
            let (text, count) =
                amacs_core::regexp::replace_all(&regex, string(args, 1)?, string(args, 2)?);
            Ok(Value::List(vec![Value::Str(text), Value::Int(count as i64)]))
        };
    "last-kbd-macro" (0, Some(0)) "The keys of the keyboard macro last defined, as a string such as \"C-a M-d RET\", or #f if none is." =>
        |ed, _| {
            let keys: Vec<String> = ed.last_macro().iter().map(ToString::to_string).collect();
            Ok(if keys.is_empty() { Value::Bool(false) } else { Value::Str(keys.join(" ")) })
        };
    "execute-kbd-macro" (1, Some(2)) "Type the keys of a string such as \"C-a M-d RET\" as a keyboard macro, once the running code has returned: each key when the commands of the one before are done. The optional argument is how many times; 0 is until a command fails." =>
        |ed, args| {
            let times = if args.len() > 1 {
                usize::try_from(int(args, 1)?).unwrap_or(1)
            } else {
                1
            };
            ed.macro_run(keys(args, 0)?, times)?;
            Ok(Value::Void)
        };
    "defining-kbd-macro?" (0, Some(0)) "Whether a keyboard macro is being defined." =>
        |ed, _| Ok(Value::Bool(ed.defining_macro()));
    "press-keys" (1, Some(1)) "Feed a key sequence to the editor as if typed. Built-in commands run at once; commands defined in Scheme run after the caller returns." =>
        |ed, args| {
            for key in keys(args, 0)? {
                ed.handle_key(key);
            }
            Ok(Value::Void)
        };
    "system-name" (0, Some(0)) "Host name of the machine the editor runs on." =>
        |_, _| {
            let name = std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_default();
            Ok(name.trim().into())
        };
    "getenv" (1, Some(1)) "Value of an environment variable, or #f if it is not set." =>
        |_, args| Ok(std::env::var(string(args, 0)?).ok().into());
    "exit-editor" (0, Some(0)) "Exit without asking about unsaved buffers." =>
        |ed, _| {
            ed.request_exit();
            Ok(Value::Void)
        };
};

/// A window and a position in its buffer as Scheme sees them.
fn mouse_place(place: Option<(WindowId, usize)>) -> Value {
    match place {
        Some((window, pos)) => Value::List(vec![Value::Int(window_number(window)), pos.into()]),
        None => Value::Bool(false),
    }
}
