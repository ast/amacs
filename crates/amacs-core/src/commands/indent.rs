//! Indentation commands.

use amacs_treesit::{IndentStyle, indent_from_previous_line, line_indentation};

use crate::editor::Editor;
use crate::error::{CmdError, CmdResult};
use crate::sexp::{SexpSyntax, lisp_indent};

/// What the buffer's language says about the indentation of a line.
pub(crate) enum Indent {
    Column(usize),
    /// The line is inside a string or comment and is left as it is.
    Keep,
    /// The language has no indentation rules.
    NoRule,
}

/// Position of the first char of `line` that is not a space or tab.
fn indentation_end(ed: &Editor, line: usize) -> usize {
    let text = ed.text();
    let start = text.line_start(line);
    start
        + text
            .chars_from(start)
            .take_while(|c| matches!(c, ' ' | '\t'))
            .count()
}

fn line_is_blank(ed: &Editor, line: usize) -> bool {
    indentation_end(ed, line) == ed.text().line_end(line)
}

/// The indentation that `line` should have.
pub(crate) fn compute(ed: &mut Editor, line: usize) -> Indent {
    let buffer = ed.buffer_mut();
    buffer.update_syntax();
    let Some(language) = buffer.language() else {
        return Indent::NoRule;
    };
    let offside = match language.indent {
        IndentStyle::None => return Indent::NoRule,
        IndentStyle::Lisp => {
            let syntax = SexpSyntax::of(buffer);
            return lisp_indent(buffer.text(), &syntax, line, buffer.tab_width)
                .map_or(Indent::Keep, Indent::Column);
        }
        IndentStyle::Brackets(_) => false,
        IndentStyle::Offside => true,
    };
    match buffer.syntax_indent(line) {
        Some(column) => Indent::Column(column),
        None if buffer.has_syntax() => Indent::Keep,
        // No grammar: go by the line above.
        None => Indent::Column(indent_from_previous_line(
            buffer.text(),
            line,
            buffer.indent_params(),
            offside,
        )),
    }
}

/// Makes the indentation of `line` reach `column`. Point stays on the text
/// it was on, or moves to the text if it was in the indentation.
pub(crate) fn set_indentation(ed: &mut Editor, line: usize, column: usize) -> CmdResult {
    let buffer = ed.buffer();
    let want = if buffer.indent_tabs {
        let tab = buffer.tab_width;
        format!("{}{}", "\t".repeat(column / tab), " ".repeat(column % tab))
    } else {
        " ".repeat(column)
    };
    let start = ed.text().line_start(line);
    let end = indentation_end(ed, line);
    let point = ed.point();
    if ed.text().slice(start..end) != want {
        ed.delete(start..end)?;
        ed.buffer_mut().insert(start, &want)?;
    }
    if (start..=end).contains(&point) {
        ed.set_point(start + want.chars().count());
    }
    Ok(())
}

/// Indents `line` as its language says, if it says anything.
pub(crate) fn indent_line(ed: &mut Editor, line: usize) -> CmdResult {
    match compute(ed, line) {
        Indent::Column(column) => set_indentation(ed, line, column),
        Indent::Keep | Indent::NoRule => Ok(()),
    }
}

/// Indents the line that a newline has just started: as its language says,
/// or like the line above where the language says nothing.
pub(crate) fn indent_new_line(ed: &mut Editor) -> CmdResult {
    let line = ed.text().line_of(ed.point());
    match compute(ed, line) {
        Indent::Column(column) => set_indentation(ed, line, column),
        Indent::Keep => Ok(()),
        Indent::NoRule if line > 0 => {
            let tab = ed.buffer().tab_width;
            let (column, _) = line_indentation(ed.text(), line - 1, tab);
            set_indentation(ed, line, column)
        }
        Indent::NoRule => Ok(()),
    }
}

/// Indents every line that has text on it between two positions.
pub(crate) fn indent_lines(ed: &mut Editor, start: usize, end: usize) -> CmdResult {
    let first = ed.text().line_of(start);
    let last = ed.text().line_of(end.min(ed.text().len()));
    for line in first..=last {
        if !line_is_blank(ed, line) {
            indent_line(ed, line)?;
        }
    }
    Ok(())
}

fn is_offside(ed: &Editor) -> bool {
    ed.buffer()
        .language()
        .is_some_and(|language| language.indent == IndentStyle::Offside)
}

pub fn indent_for_tab_command(ed: &mut Editor) -> CmdResult {
    if ed.buffer().mark_active && ed.region().is_ok_and(|region| !region.is_empty()) {
        return indent_region(ed);
    }
    let line = ed.text().line_of(ed.point());
    let (offset, tab) = (ed.buffer().indent_offset, ed.buffer().tab_width);
    match compute(ed, line) {
        Indent::Column(column) if is_offside(ed) => {
            // Where layout is syntax, the line above only says how far in
            // the line may be. Pressing TAB again steps back out.
            let (current, _) = line_indentation(ed.text(), line, tab);
            let repeated = ed.last_command() == Some("indent-for-tab-command");
            let fresh = line_is_blank(ed, line) && !repeated;
            let target = if current > column || fresh || (repeated && current == 0) {
                column
            } else if repeated {
                current.saturating_sub(offset)
            } else {
                current
            };
            set_indentation(ed, line, target)
        }
        Indent::Column(column) => set_indentation(ed, line, column),
        Indent::Keep => Ok(()),
        Indent::NoRule if ed.buffer().indent_tabs => ed.insert("\t"),
        Indent::NoRule => {
            let text = ed.text();
            let point = ed.point();
            let column = crate::display::column_of(text, point, tab);
            ed.insert(&" ".repeat(offset - column % offset))
        }
    }
}

pub fn indent_region(ed: &mut Editor) -> CmdResult {
    let region = ed.region()?;
    if is_offside(ed) {
        return Err(CmdError::msg(
            "Indentation is part of the syntax here; indent the lines by hand",
        ));
    }
    // A region that ends at the start of a line does not include that line.
    let end = if region.end > region.start
        && ed.text().line_start(ed.text().line_of(region.end)) == region.end
    {
        region.end - 1
    } else {
        region.end
    };
    indent_lines(ed, region.start, end)?;
    ed.buffer_mut().mark_active = false;
    Ok(())
}

/// After a closing bracket is typed as the first thing on its line, the
/// line is indented again, since the bracket belongs one level out.
pub(crate) fn after_closing_bracket(ed: &mut Editor) -> CmdResult {
    let brackets = ed
        .buffer()
        .language()
        .is_some_and(|language| matches!(language.indent, IndentStyle::Brackets(_)));
    let point = ed.point();
    let line = ed.text().line_of(point);
    if brackets && indentation_end(ed, line) + 1 == point {
        indent_line(ed, line)?;
    }
    Ok(())
}
