//! Assorted editing commands: scrolling by lines, comments, moving lines,
//! letter case, replacing, bookmarks, shell commands and help.

use std::io::Write;
use std::ops::Range;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use amacs_proto::Key;
use amacs_text::Text;

use crate::buffer::Buffer;
use crate::completion::{Candidate, Completion};
use crate::display::{rows_back, rows_forward};
use crate::editor::Editor;
use crate::editor::KeyMeaning;
use crate::env::EnvState;
use crate::error::{CmdError, CmdResult};
use crate::files::{abbreviate_path, expand_typed_path};

// Scrolling

/// Scrolls the window by `rows` rows, forward if positive. Point moves only
/// if it would otherwise leave the window.
pub(crate) fn scroll_rows(ed: &mut Editor, rows: i64) -> CmdResult {
    let width = ed.text_width(ed.selected());
    let tab = ed.window_flow(ed.selected());
    let start = ed.buffer().marker(ed.window().start);
    let count = rows.unsigned_abs() as usize;
    let new_start = if rows >= 0 {
        rows_forward(ed.text(), start, count, width, tab)
    } else {
        rows_back(ed.text(), start, count, width, tab)
    };
    let marker = ed.window().start;
    ed.buffer_mut().set_marker(marker, new_start);
    // Keep point on a row that is shown.
    let tab = ed.window_flow(ed.selected());
    let height = ed.window().text_rows().max(1);
    let last = rows_forward(ed.text(), new_start, height - 1, width, tab);
    let point = ed.point();
    if point < new_start {
        ed.set_point(new_start);
    } else if point >= rows_forward(ed.text(), last, 1, width, tab) && last != new_start {
        let beyond = rows_forward(ed.text(), last, 1, width, tab);
        if beyond > last {
            ed.set_point(last);
        }
    }
    Ok(())
}

pub fn scroll_up_line(ed: &mut Editor) -> CmdResult {
    scroll_rows(ed, ed.arg())
}

pub fn scroll_down_line(ed: &mut Editor) -> CmdResult {
    scroll_rows(ed, -ed.arg())
}

// Lines

/// The lines that a command about lines applies to: those the active
/// region touches, or the line point is on.
fn target_lines(ed: &Editor) -> Range<usize> {
    let text = ed.text();
    match ed.region() {
        Ok(region) if ed.buffer().mark_active && !region.is_empty() => {
            let last = text.line_of(region.end);
            // A region that ends at the start of a line leaves that line out.
            let whole = text.line_start(last) == region.end;
            text.line_of(region.start)..last + usize::from(!whole)
        }
        _ => {
            let line = text.line_of(ed.point());
            line..line + 1
        }
    }
}

fn indentation_end(text: &Text, line: usize) -> usize {
    let start = text.line_start(line);
    start
        + text
            .chars_from(start)
            .take_while(|c| matches!(c, ' ' | '\t'))
            .count()
}

fn is_blank(text: &Text, line: usize) -> bool {
    indentation_end(text, line) == text.line_end(line)
}

/// Comments out the lines of the region, or of point, or removes their
/// comment marks if every one of them has one.
pub fn comment_or_uncomment(ed: &mut Editor) -> CmdResult {
    let language = ed.buffer().language();
    let mark = language
        .and_then(|language| language.line_comment)
        .ok_or_else(|| CmdError::msg("This buffer's language has no line comments"))?;
    // Lisp comments on lines of their own are written with two marks.
    let mark = if mark == ";" { ";;" } else { mark };
    let lines = target_lines(ed);
    let text = ed.text();
    let with_text: Vec<usize> = lines
        .clone()
        .filter(|line| !is_blank(text, *line))
        .collect();
    if with_text.is_empty() {
        return Ok(());
    }
    let commented = |line: usize| {
        let start = indentation_end(text, line);
        text.slice(start..text.line_end(line)).starts_with(mark)
    };
    let mark_len = mark.chars().count();
    if with_text.iter().all(|line| commented(*line)) {
        for line in with_text.into_iter().rev() {
            let start = indentation_end(ed.text(), line);
            let space = ed.text().char_at(start + mark_len) == Some(' ');
            ed.delete(start..start + mark_len + usize::from(space))?;
        }
    } else {
        // All marks in one column: that of the least indented line.
        let column = with_text
            .iter()
            .map(|line| indentation_end(text, *line) - text.line_start(*line))
            .min()
            .unwrap_or(0);
        for line in with_text.into_iter().rev() {
            let pos = ed.text().line_start(line) + column;
            ed.buffer_mut().insert(pos, &format!("{mark} "))?;
        }
    }
    ed.buffer_mut().mark_active = false;
    Ok(())
}

/// Moves the lines of the region, or the line of point, up or down a line.
fn move_lines(ed: &mut Editor, down: bool) -> CmdResult {
    let lines = target_lines(ed);
    let text = ed.text();
    let last_line = text.len_lines() - 1;
    if (!down && lines.start == 0) || (down && lines.end > last_line) {
        return Err(CmdError::msg(if down {
            "Already at the bottom"
        } else {
            "Already at the top"
        }));
    }
    // The neighbouring line changes sides.
    let neighbour = if down { lines.end } else { lines.start - 1 };
    let neighbour_text = text.slice(text.line_start(neighbour)..text.line_end(neighbour));
    let point = ed.point();
    let mark = ed.buffer().mark();
    let (active, shift) = (
        ed.buffer().mark_active,
        neighbour_text.chars().count() as i64 + 1,
    );
    if down {
        if neighbour == last_line {
            // No newline after the last line: take the one before it.
            let start = ed.text().line_end(neighbour - 1);
            let end = ed.text().line_end(neighbour);
            ed.delete(start..end)?;
        } else {
            let start = ed.text().line_start(neighbour);
            ed.delete(start..start + shift as usize)?;
        }
        let target = ed.text().line_start(lines.start);
        ed.buffer_mut()
            .insert(target, &format!("{neighbour_text}\n"))?;
    } else {
        let start = ed.text().line_start(neighbour);
        ed.delete(start..start + shift as usize)?;
        let last = lines.end - 2;
        let end = ed.text().line_end(last);
        ed.buffer_mut()
            .insert(end, &format!("\n{neighbour_text}"))?;
    }
    let moved = |pos: usize| (pos as i64 + if down { shift } else { -shift }) as usize;
    ed.set_point(moved(point));
    if let Some(mark) = mark {
        let buffer = ed.buffer_mut();
        buffer.set_mark(moved(mark));
        // The same lines stay marked, to move them again.
        buffer.mark_active = active;
    }
    ed.keep_mark = active;
    Ok(())
}

pub fn move_text_up(ed: &mut Editor) -> CmdResult {
    move_lines(ed, false)
}

pub fn move_text_down(ed: &mut Editor) -> CmdResult {
    move_lines(ed, true)
}

pub fn back_to_indentation(ed: &mut Editor) -> CmdResult {
    let text = ed.text();
    let pos = indentation_end(text, text.line_of(ed.point()));
    ed.set_point(pos);
    Ok(())
}

/// The spaces and tabs around point.
fn blanks_around(ed: &Editor) -> Range<usize> {
    let (text, point) = (ed.text(), ed.point());
    let blank = |c: &char| matches!(c, ' ' | '\t');
    let before = text.chars_before(point).take_while(blank).count();
    let after = text.chars_from(point).take_while(blank).count();
    point - before..point + after
}

pub fn delete_horizontal_space(ed: &mut Editor) -> CmdResult {
    let range = blanks_around(ed);
    let start = range.start;
    ed.delete(range)?;
    ed.set_point(start);
    Ok(())
}

pub fn just_one_space(ed: &mut Editor) -> CmdResult {
    let range = blanks_around(ed);
    let start = range.start;
    ed.delete(range)?;
    ed.set_point(start);
    ed.insert(" ")
}

/// Joins the line to the one above, with one space between them.
pub fn delete_indentation(ed: &mut Editor) -> CmdResult {
    let text = ed.text();
    let line = text.line_of(ed.point());
    if line == 0 {
        return Err(CmdError::msg("No line above to join to"));
    }
    let end = indentation_end(text, line);
    let above = text.line_end(line - 1);
    let blanks = text
        .chars_before(above)
        .take_while(|c| matches!(c, ' ' | '\t'))
        .count();
    let start = above - blanks;
    // No space after an opening bracket or before a closing one.
    let before = start.checked_sub(1).and_then(|pos| text.char_at(pos));
    let after = text.char_at(end);
    let space = before.is_some_and(|c| !matches!(c, '(' | '[' | '{'))
        && after.is_some_and(|c| !matches!(c, ')' | ']' | '}' | '\n'));
    ed.delete(start..end)?;
    ed.set_point(start);
    if space {
        ed.insert(" ")?;
        ed.set_point(start);
    }
    Ok(())
}

// Letter case

fn change_case(ed: &mut Editor, range: Range<usize>, change: impl Fn(&str) -> String) -> CmdResult {
    let old = ed.text().slice(range.clone());
    let new = change(&old);
    if new != old {
        ed.delete(range.clone())?;
        ed.buffer_mut().insert(range.start, &new)?;
    }
    Ok(())
}

fn capitalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_word = false;
    for c in text.chars() {
        if c.is_alphanumeric() {
            if in_word {
                out.extend(c.to_lowercase());
            } else {
                out.extend(c.to_uppercase());
            }
            in_word = true;
        } else {
            in_word = false;
            out.push(c);
        }
    }
    out
}

fn case_region(ed: &mut Editor, change: impl Fn(&str) -> String) -> CmdResult {
    let region = ed.region()?;
    let point = ed.point();
    change_case(ed, region, change)?;
    ed.set_point(point);
    Ok(())
}

fn case_word(ed: &mut Editor, change: impl Fn(&str) -> String) -> CmdResult {
    let start = ed.point();
    let end = super::motion::forward_word_pos(ed.text(), start);
    change_case(ed, start..end, change)?;
    ed.set_point(end);
    Ok(())
}

pub fn upcase_region(ed: &mut Editor) -> CmdResult {
    case_region(ed, str::to_uppercase)
}

pub fn downcase_region(ed: &mut Editor) -> CmdResult {
    case_region(ed, str::to_lowercase)
}

pub fn upcase_word(ed: &mut Editor) -> CmdResult {
    case_word(ed, str::to_uppercase)
}

pub fn downcase_word(ed: &mut Editor) -> CmdResult {
    case_word(ed, str::to_lowercase)
}

pub fn capitalize_word(ed: &mut Editor) -> CmdResult {
    case_word(ed, capitalize)
}

pub fn read_only_mode(ed: &mut Editor) -> CmdResult {
    let buffer = ed.buffer_mut();
    buffer.read_only = !buffer.read_only;
    let state = if buffer.read_only {
        "Buffer is now read-only"
    } else {
        "Buffer can be edited"
    };
    ed.message(state);
    Ok(())
}

// Replacing

/// The next place at or after `from` where `needle` occurs.
fn find(text: &Text, needle: &str, from: usize) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    let haystack = text.slice(from..text.len());
    let byte = haystack.find(needle)?;
    Some(from + haystack[..byte].chars().count())
}

/// What is looked for when replacing: a text as it is, or a regular
/// expression.
#[derive(Clone)]
enum Pattern {
    Text(String),
    Regexp(std::rc::Rc<regex::Regex>),
}

/// Where the next match at or after `from` is, and what replaces it.
fn next_match(
    text: &Text,
    pattern: &Pattern,
    to: &str,
    from: usize,
) -> Option<(std::ops::Range<usize>, String)> {
    match pattern {
        Pattern::Text(needle) => {
            let start = find(text, needle, from)?;
            Some((start..start + needle.chars().count(), to.to_owned()))
        }
        Pattern::Regexp(regex) => {
            let whole = text.to_string();
            let found = crate::regexp::search(regex, &whole, from)?;
            Some((found.range(0)?, found.expand(to)))
        }
    }
}

/// Asks, for each place the text occurs after point, whether to replace it.
pub fn query_replace(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    ed.read_string("Query replace: ", "", Completion::None, |ed, from| {
        if from.is_empty() {
            return Ok(());
        }
        let prompt = format!("Query replace {from} with: ");
        ed.read_string(prompt, "", Completion::None, move |ed, to| {
            let point = ed.point();
            replace_step(ed, Pattern::Text(from), to, 0, point);
            Ok(())
        })
    })
}

/// Asks, for each match of a regular expression after point, whether to
/// replace it.
pub fn query_replace_regexp(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    ed.read_string(
        "Query replace regexp: ",
        "",
        Completion::None,
        |ed, from| {
            if from.is_empty() {
                return Ok(());
            }
            let regex = crate::regexp::compile(&from)?;
            let prompt = format!("Query replace regexp {from} with: ");
            ed.read_string(prompt, "", Completion::None, move |ed, to| {
                let point = ed.point();
                replace_step(ed, Pattern::Regexp(regex), to, 0, point);
                Ok(())
            })
        },
    )
}

/// Shows the next match at or after `from` and asks what to do with it.
fn replace_step(ed: &mut Editor, pattern: Pattern, to: String, done: usize, from: usize) {
    let Some((found, replacement)) = next_match(ed.text(), &pattern, &to, from) else {
        ed.buffer_mut().mark_active = false;
        ed.message(format!("Replaced {done} occurrences"));
        return;
    };
    // The match is shown as the region.
    ed.set_point(found.end);
    let buffer = ed.buffer_mut();
    buffer.set_mark(found.start);
    buffer.mark_active = true;
    let question = format!("Replace with {replacement}? (y, n, ! for all, q to stop) ");
    ed.ask_key(question, move |ed, key| {
        // Puts the replacement in place of a match, and says where to look
        // for the next: after it, and a char further on if nothing was
        // matched, so that the same place is not found again.
        let replace =
            |ed: &mut Editor, found: std::ops::Range<usize>, with: &str| -> CmdResult<usize> {
                let empty = found.is_empty();
                ed.delete(found.clone())?;
                ed.buffer_mut().insert(found.start, with)?;
                let after = found.start + with.chars().count();
                ed.set_point(after);
                Ok(after + usize::from(empty))
            };
        match key.self_insert_char() {
            Some('y' | ' ') => {
                let next = replace(ed, found, &replacement)?;
                replace_step(ed, pattern, to, done + 1, next);
            }
            Some('n') => {
                let next = found.end + usize::from(found.is_empty());
                replace_step(ed, pattern, to, done, next);
            }
            Some('!') => {
                let mut count = done;
                let mut next = Some((found, replacement));
                while let Some((found, replacement)) = next {
                    let from = replace(ed, found, &replacement)?;
                    count += 1;
                    next = next_match(ed.text(), &pattern, &to, from);
                }
                ed.buffer_mut().mark_active = false;
                ed.message(format!("Replaced {count} occurrences"));
            }
            _ => {
                ed.buffer_mut().mark_active = false;
                ed.set_point(found.start);
                ed.message(format!("Replaced {done} occurrences"));
                if key == Key::ctrl('g') {
                    return Err(CmdError::Quit);
                }
            }
        }
        Ok(())
    });
}

// The symbol at point

/// Where the symbol at point occurs in the buffer, as whole symbols.
fn symbol_occurrences(ed: &Editor) -> CmdResult<(String, Vec<usize>)> {
    let buffer = ed.buffer();
    let range = ed.symbol_range_at(buffer, ed.point());
    if range.is_empty() {
        return Err(CmdError::msg("No symbol at point"));
    }
    let symbol = buffer.text().slice(range);
    let lisp = buffer
        .language()
        .is_some_and(|language| language.indent == amacs_treesit::IndentStyle::Lisp);
    let is_symbol = |c: char| crate::decorate::is_symbol_char(c, lisp);
    let text = buffer.text();
    let len = symbol.chars().count();
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(start) = find(text, &symbol, from) {
        let before = start.checked_sub(1).and_then(|pos| text.char_at(pos));
        let after = text.char_at(start + len);
        if !before.is_some_and(is_symbol) && !after.is_some_and(is_symbol) {
            found.push(start);
        }
        from = start + len;
    }
    Ok((symbol, found))
}

fn symbol_jump(ed: &mut Editor, forward: bool) -> CmdResult {
    let (symbol, found) = symbol_occurrences(ed)?;
    let range = ed.symbol_range_at(ed.buffer(), ed.point());
    let offset = ed.point() - range.start;
    let index = found.iter().position(|start| *start == range.start);
    let count = found.len();
    let target = match index {
        Some(index) if count > 1 => found[(index + if forward { 1 } else { count - 1 }) % count],
        _ => return Err(CmdError::msg(format!("No other {symbol}"))),
    };
    ed.set_point(target + offset);
    let position = found.iter().position(|start| *start == target).unwrap_or(0) + 1;
    ed.message(format!("{symbol}: {position} of {count}"));
    Ok(())
}

pub fn symbol_next(ed: &mut Editor) -> CmdResult {
    symbol_jump(ed, true)
}

pub fn symbol_previous(ed: &mut Editor) -> CmdResult {
    symbol_jump(ed, false)
}

/// Renames the symbol at point everywhere in the buffer.
pub fn symbol_rename(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    let (symbol, found) = symbol_occurrences(ed)?;
    let prompt = format!("Rename {symbol} in this buffer to: ");
    let initial = symbol.clone();
    ed.read_string(prompt, &initial, Completion::None, move |ed, new_name| {
        if new_name.is_empty() || new_name == symbol {
            return Ok(());
        }
        let len = symbol.chars().count();
        let point = ed.point();
        for start in found.iter().rev() {
            ed.delete(*start..*start + len)?;
            ed.buffer_mut().insert(*start, &new_name)?;
        }
        ed.set_point(point.min(ed.buffer().len()));
        ed.message(format!("Renamed {} occurrences", found.len()));
        Ok(())
    })
}

// Bookmarks

/// The bookmarks: name, file and position.
fn bookmarks(ed: &Editor) -> Vec<(String, PathBuf, usize)> {
    ed.history
        .entries("bookmark")
        .iter()
        .rev()
        .filter_map(|entry| {
            let mut parts = entry.splitn(3, '\u{1f}');
            let (name, pos, file) = (parts.next()?, parts.next()?, parts.next()?);
            Some((name.to_owned(), PathBuf::from(file), pos.parse().ok()?))
        })
        .collect()
}

fn bookmark_candidates(ed: &Editor) -> CmdResult<Completion> {
    let candidates: Vec<Candidate> = bookmarks(ed)
        .into_iter()
        .map(|(name, file, _)| Candidate::annotated(name, abbreviate_path(&file)))
        .collect();
    if candidates.is_empty() {
        return Err(CmdError::msg("No bookmarks have been set"));
    }
    Ok(Completion::Items {
        candidates,
        category: "bookmark-name".to_owned(),
    })
}

pub fn bookmark_set(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    let file = ed
        .buffer()
        .file
        .clone()
        .ok_or_else(|| CmdError::msg("This buffer does not visit a file"))?;
    let name = ed.buffer().name.clone();
    let pos = ed.point();
    ed.read_string(
        "Set bookmark named: ",
        &name,
        Completion::None,
        move |ed, name| {
            if name.is_empty() {
                return Ok(());
            }
            let prefix = format!("{name}\u{1f}");
            ed.history
                .remove("bookmark", |entry| entry.starts_with(&prefix));
            let entry = format!("{name}\u{1f}{pos}\u{1f}{}", file.display());
            ed.history.add("bookmark", entry);
            ed.message(format!("Bookmark {name} set"));
            Ok(())
        },
    )
}

pub fn bookmark_jump(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    let completion = bookmark_candidates(ed)?;
    ed.read_string("Jump to bookmark: ", "", completion, |ed, name| {
        let (_, file, pos) = bookmarks(ed)
            .into_iter()
            .find(|(bookmark, _, _)| *bookmark == name)
            .ok_or_else(|| CmdError::msg(format!("No bookmark named {name}")))?;
        ed.find_file(&file)?;
        ed.set_point(pos);
        Ok(())
    })
}

pub fn bookmark_delete(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    let completion = bookmark_candidates(ed)?;
    ed.read_string("Delete bookmark: ", "", completion, |ed, name| {
        let prefix = format!("{name}\u{1f}");
        ed.history
            .remove("bookmark", |entry| entry.starts_with(&prefix));
        ed.message(format!("Bookmark {name} deleted"));
        Ok(())
    })
}

// Shell commands

/// Runs a shell command with `input` on its standard input. Returns what
/// it wrote, to standard output and then standard error, and whether it
/// succeeded.
fn run_shell(ed: &mut Editor, command: &str, input: String) -> CmdResult<(String, bool)> {
    let directory = ed.local_directory()?;
    let environment = match ed.environment(&directory) {
        Some(EnvState::Ready(environment)) => environment,
        _ => Vec::new(),
    };
    let mut shell = Command::new("sh");
    shell.arg("-c").arg(command);
    if directory.is_dir() {
        shell.current_dir(&directory);
    }
    for (name, value) in &environment {
        match value {
            Some(value) => shell.env(name, value),
            None => shell.env_remove(name),
        };
    }
    let mut child = shell
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| CmdError::msg(format!("Cannot run the shell: {error}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        std::thread::spawn(move || {
            let _ = stdin.write_all(input.as_bytes());
        });
    }
    let output = child
        .wait_with_output()
        .map_err(|error| CmdError::msg(format!("The command failed: {error}")))?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok((text, output.status.success()))
}

/// Shows the output of a command: in the echo area if it is a line, else
/// in a buffer of its own.
fn show_output(ed: &mut Editor, output: &str, succeeded: bool) -> CmdResult {
    let trimmed = output.trim_end();
    if trimmed.is_empty() {
        ed.message(if succeeded {
            "(The command wrote nothing)"
        } else {
            "The command failed and wrote nothing"
        });
        Ok(())
    } else if trimmed.lines().count() == 1 {
        ed.message(trimmed.to_owned());
        Ok(())
    } else {
        const NAME: &str = "*Shell Command Output*";
        let id = match ed.find_buffer(NAME) {
            Some(id) => id,
            None => {
                let mut buffer = Buffer::new(NAME);
                buffer.read_only = true;
                ed.add_buffer(buffer)
            }
        };
        ed.buffers[id].reset(Text::from(output));
        ed.buffers[id].save_view(0, 0);
        ed.show_in_other_window(id)
    }
}

pub fn shell_command(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    ed.read_string("Shell command: ", "", Completion::None, |ed, command| {
        let (output, succeeded) = run_shell(ed, &command, String::new())?;
        show_output(ed, &output, succeeded)
    })
}

/// Runs a shell command on the region. With a prefix argument, its output
/// replaces the region.
pub fn shell_command_on_region(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    if ed.is_secret(ed.window().buffer) {
        return Err(CmdError::msg(
            "This buffer is secret: its text is not given to programs",
        ));
    }
    let region = ed.region()?;
    let replace = ed.raw_prefix().is_some();
    ed.read_string(
        "Shell command on region: ",
        "",
        Completion::None,
        move |ed, command| {
            let region = region.start.min(ed.buffer().len())..region.end.min(ed.buffer().len());
            let input = ed.text().slice(region.clone());
            let (output, succeeded) = run_shell(ed, &command, input)?;
            if !replace {
                return show_output(ed, &output, succeeded);
            }
            if !succeeded {
                return Err(CmdError::msg(format!(
                    "The command failed: {}",
                    output.lines().next().unwrap_or("")
                )));
            }
            ed.delete(region.clone())?;
            ed.buffer_mut().insert(region.start, &output)?;
            ed.set_point(region.start + output.chars().count());
            Ok(())
        },
    )
}

// Help

/// Reads a key sequence and says what it runs.
pub fn describe_key(ed: &mut Editor) -> CmdResult {
    read_key_sequence(ed, Vec::new());
    Ok(())
}

fn read_key_sequence(ed: &mut Editor, mut keys: Vec<Key>) {
    let typed: Vec<String> = keys.iter().map(Key::to_string).collect();
    let prompt = format!("Describe key: {}", typed.join(" "));
    ed.ask_key(prompt, move |ed, key| {
        keys.push(key);
        let described: Vec<String> = keys.iter().map(Key::to_string).collect();
        let described = described.join(" ");
        match ed.key_lookup(&keys) {
            KeyMeaning::Prefix => read_key_sequence(ed, keys),
            KeyMeaning::Command(name) => {
                let doc = ed
                    .command(&name)
                    .map(|command| crate::editor::doc_summary(&command.doc))
                    .unwrap_or_default();
                ed.message(format!("{described} runs {name}: {doc}"));
            }
            KeyMeaning::Unbound if keys.len() == 1 && key.self_insert_char().is_some() => {
                ed.message(format!("{described} inserts itself"));
            }
            KeyMeaning::Unbound => ed.message(format!("{described} is not bound")),
        }
        Ok(())
    });
}

/// Says what a command does and which keys run it.
pub fn describe_command(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    ed.read_string("Describe command: ", "", Completion::Command, |ed, name| {
        let command = ed
            .command(&name)
            .ok_or_else(|| CmdError::msg(format!("No command named {name}")))?;
        let doc = command.doc.clone();
        let keys = ed.keys_for(&name);
        let bound = if keys.is_empty() {
            "It is not bound to a key here.".to_owned()
        } else {
            format!("Keys: {}", keys.join(", "))
        };
        ed.show_text("*Help*", &format!("{name}\n\n{doc}\n\n{bound}\n"))
    })
}

pub fn find_file_other_window(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    let dir = crate::minibuffer::default_directory(ed);
    let base = ed.working_directory().to_owned();
    ed.read_string(
        "Find file in other window: ",
        &dir,
        Completion::File,
        move |ed, input| {
            super::windows::select_other_window(ed)?;
            ed.find_file_asking(&expand_typed_path(&input, &base))
        },
    )
}

pub fn switch_to_buffer_other_window(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    ed.read_string(
        "Buffer in other window: ",
        "",
        Completion::Buffer,
        |ed, name| {
            let id = ed
                .find_buffer(&name)
                .ok_or_else(|| CmdError::msg(format!("No such buffer: {name}")))?;
            super::windows::select_other_window(ed)?;
            ed.switch_to_buffer(id);
            Ok(())
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_capitalised() {
        assert_eq!(capitalize("hello WORLD"), "Hello World");
        assert_eq!(capitalize("åäö-x1"), "Åäö-X1");
    }

    #[test]
    fn text_is_found_by_char_position() {
        let text = Text::from("åäö abc åäö");
        assert_eq!(find(&text, "åäö", 0), Some(0));
        assert_eq!(find(&text, "åäö", 1), Some(8));
        assert_eq!(find(&text, "abc", 0), Some(4));
        assert_eq!(find(&text, "zzz", 0), None);
        assert_eq!(find(&text, "", 0), None);
    }
}
