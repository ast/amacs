//! The editor primitives that Scheme code can call.
//!
//! Each primitive is a plain function over the core's [`Value`] type, so this
//! table does not depend on the Scheme engine. Names starting with `%` are
//! building blocks for the Scheme runtime in `runtime/`, not for users.
//!
//! Buffer positions are zero-based char offsets: the first position is 0 and
//! the last is the buffer's length.

use amacs_core::{
    Buffer, BufferId, Candidate, CmdError, CmdResult, CommandBody, Completion, Editor, FaceSpec,
    Formatter, ServerConfig, Value, expand_path,
};
use amacs_proto::parse_keys;

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
            Some("underline") => spec.underline = on,
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
    "find-file" (1, Some(1)) "Visit a file in the selected window." =>
        |ed, args| {
            let path = expand_path(string(args, 0)?, ed.working_directory());
            ed.find_file(&path)?;
            Ok(Value::Void)
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
            match ed.command(name).map(|command| command.body.clone()) {
                Some(CommandBody::Native(run)) => run(ed).map(|()| Value::Void),
                Some(CommandBody::Script(_)) => {
                    Err(CmdError::msg(format!("{name} is not a built-in command")))
                }
                None => Err(CmdError::msg(format!("Unknown command: {name}"))),
            }
        };
    "command?" (1, Some(1)) "Whether a command with this name exists." =>
        |ed, args| Ok(ed.command(name(args, 0)?).is_some().into());
    "command-documentation" (1, Some(1)) "Documentation of a command, or #f." =>
        |ed, args| Ok(ed.command(name(args, 0)?).map(|command| command.doc.as_str()).into());
    "prefix-arg" (0, Some(0)) "Numeric prefix argument of the running command; 1 if none was given." =>
        |ed, _| Ok(Value::Int(ed.arg()));
    "prefix-arg-given?" (0, Some(0)) "Whether the running command was given a prefix argument." =>
        |ed, _| Ok(ed.raw_prefix().is_some().into());
    "%bind-key" (3, Some(3)) "Bind a key sequence in a keymap: keymap name, keys, command name." =>
        |ed, args| {
            let keys = keys(args, 1)?;
            let command = name(args, 2)?.to_owned();
            ed.keymap_mut(name(args, 0)?).bind(&keys, &command);
            Ok(Value::Void)
        };
    "%unbind-key" (2, Some(2)) "Remove a key binding from a keymap: keymap name, keys." =>
        |ed, args| {
            let keys = keys(args, 1)?;
            ed.keymap_mut(name(args, 0)?).unbind(&keys);
            Ok(Value::Void)
        };
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
    "set-face!" (1, None) "Change a face, or define a new one: its name, then attributes and values. The attributes are 'fg and 'bg (a palette name, \"#rrggbb\" or #f) and 'bold, 'italic and 'underline (booleans)." =>
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

    // The editor itself

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
