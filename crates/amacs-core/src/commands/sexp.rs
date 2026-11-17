//! Commands on balanced expressions: moving over them, killing them, and
//! the structural editing of paredit, which keeps brackets balanced.

use crate::commands::edit::{self, kill};
use crate::commands::indent;
use crate::commands::motion::{backward_word_pos, forward_word_pos};
use crate::editor::Editor;
use crate::error::{CmdError, CmdResult};
use crate::kill_ring::KillDirection;
use crate::sexp::{Kind, Node, SexpSyntax, Tree, closer_of};

pub const PAREDIT_MODE: &str = "paredit-mode";

fn tree(ed: &Editor) -> Tree {
    Tree::around(ed.text(), &SexpSyntax::of(ed.buffer()), ed.point())
}

fn is_lisp(ed: &Editor) -> bool {
    SexpSyntax::of(ed.buffer()).lisp
}

/// Where point ends up after moving forward over one expression.
fn forward_pos(ed: &Editor) -> CmdResult<usize> {
    let (tree, point) = (tree(ed), ed.point());
    if let Some(string) = tree.string_at(point) {
        // Inside a string, its words are the expressions.
        let limit = string.close_pos().unwrap_or(string.end);
        let target = forward_word_pos(ed.text(), point).min(limit);
        return if target > point {
            Ok(target)
        } else {
            Err(CmdError::msg("End of string"))
        };
    }
    tree.next(point)
        .map(|node| node.end)
        .ok_or_else(|| CmdError::msg("No expression follows"))
}

/// Where point ends up after moving backward over one expression.
fn backward_pos(ed: &Editor) -> CmdResult<usize> {
    let (tree, point) = (tree(ed), ed.point());
    if let Some(string) = tree.string_at(point) {
        let target = backward_word_pos(ed.text(), point).max(string.start + 1);
        return if target < point {
            Ok(target)
        } else {
            Err(CmdError::msg("Beginning of string"))
        };
    }
    tree.previous(point)
        .map(|node| node.prefix_start)
        .ok_or_else(|| CmdError::msg("No expression precedes"))
}

/// Moves over `count` expressions, backward if it is negative.
fn move_sexps(ed: &mut Editor, count: i64) -> CmdResult {
    for _ in 0..count.unsigned_abs() {
        let target = if count > 0 {
            forward_pos(ed)?
        } else {
            backward_pos(ed)?
        };
        ed.set_point(target);
    }
    Ok(())
}

pub fn forward_sexp(ed: &mut Editor) -> CmdResult {
    move_sexps(ed, ed.arg())
}

pub fn backward_sexp(ed: &mut Editor) -> CmdResult {
    move_sexps(ed, -ed.arg())
}

/// Where the expressions that `count` spans from point end.
fn span_end(ed: &mut Editor, count: i64) -> CmdResult<usize> {
    let point = ed.point();
    let moved = move_sexps(ed, count);
    let target = ed.point();
    ed.set_point(point);
    moved.map(|()| target)
}

pub fn kill_sexp(ed: &mut Editor) -> CmdResult {
    let (point, target) = (ed.point(), span_end(ed, ed.arg())?);
    if target >= point {
        kill(ed, point, target, KillDirection::Forward)
    } else {
        kill(ed, target, point, KillDirection::Backward)
    }
}

pub fn backward_kill_sexp(ed: &mut Editor) -> CmdResult {
    let (point, target) = (ed.point(), span_end(ed, -ed.arg())?);
    if target <= point {
        kill(ed, target, point, KillDirection::Backward)
    } else {
        kill(ed, point, target, KillDirection::Forward)
    }
}

pub fn mark_sexp(ed: &mut Editor) -> CmdResult {
    let target = span_end(ed, ed.arg())?;
    let buffer = ed.buffer_mut();
    buffer.set_mark(target);
    buffer.mark_active = true;
    Ok(())
}

/// Moves out of the enclosing list or string, to its start.
pub fn backward_up_list(ed: &mut Editor) -> CmdResult {
    let (tree, point) = (tree(ed), ed.point());
    let target = match tree.string_at(point) {
        Some(string) => string.prefix_start,
        None => tree
            .enclosing_list(point)
            .map(|list| list.prefix_start)
            .ok_or_else(|| CmdError::msg("At top level"))?,
    };
    ed.set_point(target);
    Ok(())
}

/// Moves out of the enclosing list or string, to just after its end.
pub fn up_list(ed: &mut Editor) -> CmdResult {
    let (tree, point) = (tree(ed), ed.point());
    let enclosing = tree.string_at(point).or_else(|| tree.enclosing_list(point));
    match enclosing {
        Some(node) if node.is_closed() => {
            let end = node.end;
            ed.set_point(end);
            Ok(())
        }
        Some(_) => Err(CmdError::msg("Unbalanced brackets")),
        None => Err(CmdError::msg("At top level")),
    }
}

/// Moves into the next list, to just after its opening bracket.
pub fn down_list(ed: &mut Editor) -> CmdResult {
    let (tree, point) = (tree(ed), ed.point());
    let target = tree
        .level(point)
        .sexps()
        .find(|node| node.is_list() && node.start >= point)
        .map(|list| list.start + 1)
        .ok_or_else(|| CmdError::msg("No list follows"))?;
    ed.set_point(target);
    Ok(())
}

// Paredit

fn char_before(ed: &Editor, pos: usize) -> Option<char> {
    pos.checked_sub(1).and_then(|pos| ed.text().char_at(pos))
}

/// Inserts an opening and a closing delimiter around point, with the
/// spaces that keep them apart from their neighbours.
fn insert_pair(ed: &mut Editor, open: char, close: char) -> CmdResult {
    let point = ed.point();
    let space_before = char_before(ed, point).is_some_and(|c| {
        !c.is_whitespace() && !matches!(c, '(' | '[' | '{' | '\'' | '`' | ',' | '@' | '#')
    });
    let space_after = ed
        .text()
        .char_at(point)
        .is_some_and(|c| !c.is_whitespace() && !matches!(c, ')' | ']' | '}'));
    let mut text = String::new();
    if space_before {
        text.push(' ');
    }
    text.push(open);
    text.push(close);
    if space_after {
        text.push(' ');
    }
    ed.insert(&text)?;
    ed.set_point(point + usize::from(space_before) + 1);
    Ok(())
}

fn paredit_open(ed: &mut Editor, open: char) -> CmdResult {
    if tree(ed).in_text(ed.point()) {
        return ed.insert(&open.to_string());
    }
    insert_pair(ed, open, closer_of(open))
}

pub fn paredit_open_round(ed: &mut Editor) -> CmdResult {
    paredit_open(ed, '(')
}

pub fn paredit_open_square(ed: &mut Editor) -> CmdResult {
    paredit_open(ed, '[')
}

/// Moves past the closing bracket of the enclosing list, removing the
/// whitespace before it. In a string or comment, inserts the typed char.
pub fn paredit_close(ed: &mut Editor) -> CmdResult {
    let (tree, point) = (tree(ed), ed.point());
    if tree.in_text(point) {
        return edit::self_insert_plain(ed);
    }
    let list = tree
        .enclosing_list(point)
        .ok_or_else(|| CmdError::msg("At top level"))?;
    let close = list
        .close_pos()
        .ok_or_else(|| CmdError::msg("Unbalanced brackets"))?;
    // Whitespace before the bracket goes, unless a comment is in the way.
    let last = list.children.last();
    let keep_from = match last {
        Some(node) if node.kind == Kind::Comment => close,
        Some(node) => node.end,
        None => list.start + 1,
    };
    ed.delete(keep_from..close)?;
    ed.set_point(keep_from + 1);
    Ok(())
}

/// Inserts a pair of double quotes, steps over the closing quote of a
/// string, or inserts an escaped quote inside one.
pub fn paredit_doublequote(ed: &mut Editor) -> CmdResult {
    let (tree, point) = (tree(ed), ed.point());
    if tree.in_comment(point) {
        return ed.insert("\"");
    }
    match tree.string_at(point) {
        Some(string) if string.close_pos() == Some(point) => {
            ed.set_point(point + 1);
            Ok(())
        }
        Some(_) => ed.insert("\\\""),
        None => insert_pair(ed, '"', '"'),
    }
}

/// Whether the char before `pos` is escaped by a backslash.
fn is_escaped(ed: &Editor, pos: usize) -> bool {
    let backslashes = ed
        .text()
        .chars_before(pos)
        .take_while(|c| *c == '\\')
        .count();
    backslashes % 2 == 1
}

/// Deletes the char before point unless that would unbalance the text:
/// a delimiter is stepped over, and deleted only with its partner when
/// there is nothing between them.
pub fn paredit_backward_delete(ed: &mut Editor) -> CmdResult {
    let (tree, point) = (tree(ed), ed.point());
    if point == 0 {
        return Err(CmdError::msg("Beginning of buffer"));
    }
    if tree.in_comment(point) {
        return delete(ed, point - 1, point);
    }
    if let Some(string) = tree.string_at(point) {
        if point == string.start + 1 {
            // After the opening quote.
            return if string.end == string.start + 2 {
                delete(ed, string.start, string.end)
            } else {
                Ok(())
            };
        }
        // An escaped char goes together with its backslash.
        if point >= 2 && is_escaped(ed, point - 1) {
            return delete(ed, point - 2, point);
        }
        return delete(ed, point - 1, point);
    }
    let level = tree.level(point);
    // Just after a list or string: step into it.
    if let Some(node) = level.children.iter().find(|node| node.end == point)
        && (node.is_list() || matches!(node.kind, Kind::Str { .. }))
        && node.is_closed()
    {
        ed.set_point(point - 1);
        return Ok(());
    }
    // Just after the opening bracket of the enclosing list.
    if let Some(list) = tree.enclosing_list(point)
        && list.start + 1 == point
    {
        return if list.end == list.start + 2 && list.is_closed() {
            delete(ed, list.start, list.end)
        } else {
            Ok(())
        };
    }
    delete(ed, point - 1, point)
}

/// Deletes the char after point unless that would unbalance the text.
pub fn paredit_forward_delete(ed: &mut Editor) -> CmdResult {
    let (tree, point) = (tree(ed), ed.point());
    if point >= ed.buffer().len() {
        return Err(CmdError::msg("End of buffer"));
    }
    if tree.in_comment(point) {
        return delete(ed, point, point + 1);
    }
    if let Some(string) = tree.string_at(point) {
        if string.close_pos() == Some(point) {
            // Before the closing quote.
            return if string.end == string.start + 2 {
                delete(ed, string.start, string.end)
            } else {
                Ok(())
            };
        }
        if ed.text().char_at(point) == Some('\\') && point + 2 <= string.end {
            return delete(ed, point, point + 2);
        }
        return delete(ed, point, point + 1);
    }
    let level = tree.level(point);
    // Just before a list or string: step into it.
    if let Some(node) = level.children.iter().find(|node| node.start == point)
        && (node.is_list() || matches!(node.kind, Kind::Str { .. }))
    {
        ed.set_point(point + 1);
        return Ok(());
    }
    // Just before the closing bracket of the enclosing list.
    if let Some(list) = tree.enclosing_list(point)
        && list.close_pos() == Some(point)
    {
        return if list.end == list.start + 2 {
            delete(ed, list.start, list.end)
        } else {
            Ok(())
        };
    }
    delete(ed, point, point + 1)
}

fn delete(ed: &mut Editor, start: usize, end: usize) -> CmdResult {
    ed.delete(start..end)?;
    ed.set_point(start);
    Ok(())
}

/// Kills to the end of the line, but whole expressions only, so that the
/// brackets stay balanced.
pub fn paredit_kill(ed: &mut Editor) -> CmdResult {
    let (tree, point) = (tree(ed), ed.point());
    let text = ed.text();
    let eol = text.line_end(text.line_of(point));
    let rest_is_blank = text
        .chars_from(point)
        .take(eol - point)
        .all(char::is_whitespace);
    if tree.in_comment(point) || rest_is_blank {
        return edit::kill_line_plain(ed);
    }
    if let Some(string) = tree.string_at(point) {
        let end = string.close_pos().unwrap_or(string.end).min(eol);
        return kill(ed, point, end, KillDirection::Forward);
    }
    let level = tree.level(point);
    let mut end = point;
    let mut to_eol = true;
    for node in level.children.iter().filter(|node| node.end > point) {
        if node.prefix_start.max(point) >= eol {
            break;
        }
        if node.kind == Kind::Comment {
            end = eol;
            break;
        }
        end = node.end;
        if node.end > eol {
            // An expression that starts on this line goes whole.
            to_eol = false;
            break;
        }
    }
    if to_eol {
        // Up to the end of the line, or to the bracket that closes the
        // enclosing list if that comes first.
        end = match tree.enclosing_list(point).and_then(Node::close_pos) {
            Some(close) if close < eol => close,
            _ => eol,
        };
    }
    kill(ed, point, end, KillDirection::Forward)
}

/// The enclosing list with both brackets present.
fn enclosing(tree: &Tree, point: usize) -> CmdResult<&Node> {
    tree.enclosing_list(point)
        .filter(|list| list.is_closed())
        .ok_or_else(|| CmdError::msg("Not inside a balanced list"))
}

/// Indents the lines between two positions again, in Lisp buffers.
fn reindent(ed: &mut Editor, start: usize, end: usize) -> CmdResult {
    if is_lisp(ed) {
        indent::indent_lines(ed, start, end)?;
    }
    Ok(())
}

/// Wraps the following expression in round brackets.
pub fn paredit_wrap_round(ed: &mut Editor) -> CmdResult {
    let (tree, point) = (tree(ed), ed.point());
    if tree.in_text(point) {
        return Err(CmdError::msg("Not in a string or comment"));
    }
    let Some(next) = tree.next(point).filter(|node| node.kind != Kind::Stray) else {
        return insert_pair(ed, '(', ')');
    };
    let (start, end) = (next.prefix_start.max(point), next.end);
    ed.buffer_mut().insert(end, ")")?;
    ed.buffer_mut().insert(start, "(")?;
    ed.set_point(start + 1);
    reindent(ed, start, end + 2)
}

/// Removes the brackets of the enclosing list.
pub fn paredit_splice(ed: &mut Editor) -> CmdResult {
    let (tree, point) = (tree(ed), ed.point());
    let list = enclosing(&tree, point)?;
    let (start, end) = (list.start, list.end);
    ed.delete(end - 1..end)?;
    ed.delete(start..start + 1)?;
    ed.set_point(point - 1);
    reindent(ed, start, end - 2)
}

/// Replaces the enclosing list with the expression after point.
pub fn paredit_raise(ed: &mut Editor) -> CmdResult {
    let (tree, point) = (tree(ed), ed.point());
    let list = enclosing(&tree, point)?;
    let next = tree
        .next(point)
        .ok_or_else(|| CmdError::msg("No expression follows"))?;
    let (start, end) = (next.prefix_start, next.end);
    let (list_start, list_end) = (list.prefix_start, list.end);
    let raised = ed.text().slice(start..end);
    ed.delete(list_start..list_end)?;
    ed.buffer_mut().insert(list_start, &raised)?;
    ed.set_point(list_start);
    reindent(ed, list_start, list_start + raised.chars().count())
}

/// Moves the closing bracket of the enclosing list past the expression
/// that follows the list.
pub fn paredit_forward_slurp(ed: &mut Editor) -> CmdResult {
    let (tree, point) = (tree(ed), ed.point());
    let list = enclosing(&tree, point)?;
    let close = list.end - 1;
    let following = tree
        .parent_of(list)
        .sexps()
        .find(|node| node.start >= list.end)
        .ok_or_else(|| CmdError::msg("Nothing to slurp"))?;
    let target = following.end;
    let bracket = ed.text().slice(close..close + 1);
    let (start, empty) = (list.start, list.sexps().next().is_none());
    ed.buffer_mut().insert(target, &bracket)?;
    ed.delete(close..close + 1)?;
    // An empty list takes the expression right after its bracket.
    if empty {
        let gap = following.prefix_start - list.end;
        ed.delete(close..close + gap)?;
    }
    ed.set_point(point);
    reindent(ed, start, target)
}

/// Moves the closing bracket of the enclosing list to before its last
/// expression.
pub fn paredit_forward_barf(ed: &mut Editor) -> CmdResult {
    let (tree, point) = (tree(ed), ed.point());
    let list = enclosing(&tree, point)?;
    let mut inside = list.sexps().rev();
    let last = inside
        .next()
        .ok_or_else(|| CmdError::msg("Nothing to barf"))?;
    let target = inside.next().map_or(list.start + 1, |node| node.end);
    let close = list.end - 1;
    let bracket = ed.text().slice(close..close + 1);
    let (start, end) = (list.start, list.end);
    let needs_space = target == last.prefix_start;
    ed.delete(close..close + 1)?;
    ed.buffer_mut().insert(target, &bracket)?;
    if needs_space {
        ed.buffer_mut().insert(target + 1, " ")?;
    }
    ed.set_point(point.min(target));
    reindent(ed, start, end)
}

/// Moves the opening bracket of the enclosing list to before the
/// expression that precedes the list.
pub fn paredit_backward_slurp(ed: &mut Editor) -> CmdResult {
    let (tree, point) = (tree(ed), ed.point());
    let list = enclosing(&tree, point)?;
    let preceding = tree
        .parent_of(list)
        .sexps()
        .rev()
        .find(|node| node.end <= list.prefix_start)
        .ok_or_else(|| CmdError::msg("Nothing to slurp"))?;
    let open = list.start;
    let bracket = ed.text().slice(open..open + 1);
    let (target, end) = (preceding.prefix_start, list.end);
    ed.delete(open..open + 1)?;
    ed.buffer_mut().insert(target, &bracket)?;
    ed.set_point(point);
    reindent(ed, target, end)
}

/// Moves the opening bracket of the enclosing list to after its first
/// expression.
pub fn paredit_backward_barf(ed: &mut Editor) -> CmdResult {
    let (tree, point) = (tree(ed), ed.point());
    let list = enclosing(&tree, point)?;
    let mut inside = list.sexps();
    let first = inside
        .next()
        .ok_or_else(|| CmdError::msg("Nothing to barf"))?;
    let target = inside.next().map_or(list.end - 1, |node| node.prefix_start);
    let open = list.start;
    let bracket = ed.text().slice(open..open + 1);
    let (start, end) = (list.prefix_start, list.end);
    let needs_space = target == first.end;
    // Insert first: the target is after the bracket that then goes.
    ed.buffer_mut().insert(target, &bracket)?;
    if needs_space {
        ed.buffer_mut().insert(target, " ")?;
    }
    ed.delete(open..open + 1)?;
    let space = usize::from(needs_space);
    ed.set_point(if point > target {
        point + space
    } else {
        target + space
    });
    reindent(ed, start, end)
}
