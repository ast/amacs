//! Completion at point: a list of ways to finish the word being typed,
//! shown under it.
//!
//! The candidates come from the buffer's language server if it has one,
//! and otherwise from the words of the buffer; the names of the snippets
//! that apply are added to either. The list appears by itself shortly
//! after a couple of chars are typed, and narrows as more are.

use std::time::{Duration, Instant};

use amacs_proto::{DisplayLine, FaceId};
use amacs_text::MarkerId;
use serde_json::Value as Json;
use unicode_width::UnicodeWidthChar;

use crate::buffer::BufferId;
use crate::display::{column_of, overlay_cells, str_cells};
use crate::editor::Editor;
use crate::error::{CmdError, CmdResult};
use crate::faces;
use crate::lsp::Pending;
use crate::script::Value;
use crate::snippet::Template;

pub const POPUP_MAP: &str = "completion";
/// Rows of the list.
const VISIBLE: usize = 10;
const MAX_WIDTH: usize = 60;
/// Chars of the buffer searched for words, on each side of point.
const WORD_SCAN: usize = 200_000;

/// What accepting a candidate does besides inserting its text.
#[derive(Debug, Clone, PartialEq)]
enum Extra {
    None,
    /// Edits elsewhere in the buffer, such as an import.
    Edits(Vec<Json>, amacs_lsp::Encoding),
    /// Expand the snippet of this name in place of the text.
    Snippet(String),
}

#[derive(Debug, Clone, PartialEq)]
struct Item {
    label: String,
    /// Shown beside the label.
    detail: String,
    /// What the typed text is matched against.
    filter: String,
    sort: String,
    insert: String,
    /// The text to insert is a snippet in the protocol's notation.
    is_snippet: bool,
    /// Where the text being replaced starts, if the candidate says.
    replace_from: Option<usize>,
    extra: Extra,
}

/// The list of candidates while it is showing.
pub(crate) struct Popup {
    buffer: BufferId,
    /// Start of the text being completed.
    start: MarkerId,
    items: Vec<Item>,
    /// The items that match what has been typed, best first.
    matches: Vec<usize>,
    selected: usize,
    scroll: usize,
    /// The server held candidates back, so typing more means asking again
    /// rather than narrowing what there is.
    incomplete: bool,
}

/// When and for what a list is wanted.
#[derive(Default)]
pub(crate) struct CompletionState {
    pub popup: Option<Popup>,
    /// When to ask for candidates, after typing has paused.
    pub due: Option<Instant>,
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// How well `filter` matches the typed `prefix`: lower is better, `None`
/// is no match. Case is ignored.
fn rank(filter: &str, prefix: &str) -> Option<u8> {
    if prefix.is_empty() {
        return Some(0);
    }
    let (filter, prefix) = (filter.to_lowercase(), prefix.to_lowercase());
    if filter.starts_with(&prefix) {
        Some(0)
    } else if filter.contains(&prefix) {
        Some(1)
    } else {
        // The typed chars in order, from the first char on: what servers
        // that match loosely send back.
        let mut chars = filter.chars();
        let same_start = filter.chars().next() == prefix.chars().next();
        (same_start && prefix.chars().all(|wanted| chars.any(|c| c == wanted))).then_some(2)
    }
}

fn kind_name(kind: &Json) -> &'static str {
    const KINDS: [&str; 26] = [
        "",
        "text",
        "method",
        "function",
        "constructor",
        "field",
        "variable",
        "class",
        "interface",
        "module",
        "property",
        "unit",
        "value",
        "enum",
        "keyword",
        "snippet",
        "color",
        "file",
        "reference",
        "folder",
        "enum member",
        "constant",
        "struct",
        "event",
        "operator",
        "type parameter",
    ];
    kind.as_u64()
        .and_then(|kind| KINDS.get(kind as usize))
        .copied()
        .unwrap_or("")
}

impl Editor {
    fn option_int(&self, name: &str, default: i64) -> i64 {
        match self.option(name) {
            Ok(Value::Int(value)) => value,
            _ => default,
        }
    }

    /// Start of the word that ends at point.
    fn word_start(&self) -> usize {
        let point = self.point();
        point
            - self
                .text()
                .chars_before(point)
                .take_while(|c| is_word_char(*c))
                .count()
    }

    // When the list appears

    /// After a command: narrow the list, close it, or arrange for one.
    pub(crate) fn completion_after_command(&mut self, command: Option<&str>) {
        let typed = command == Some("self-insert-command");
        let deleted = command == Some("delete-backward-char");
        let own = matches!(
            command,
            Some(
                "completion-next"
                    | "completion-previous"
                    | "completion-at-point"
                    | "snippet-complete"
            )
        );
        self.completion.due = None;
        if self.completion.popup.is_some() {
            if typed || deleted {
                let incomplete = self
                    .completion
                    .popup
                    .as_ref()
                    .is_some_and(|popup| popup.incomplete);
                self.completion_narrow();
                // Still in the word: what was held back may match now, even
                // if nothing that was sent does.
                if incomplete && self.word_start() < self.point() {
                    self.completion.due = Some(Instant::now());
                }
            } else if !own {
                self.completion_close();
            }
            return;
        }
        if !typed || self.in_minibuffer() || self.buffer().read_only {
            return;
        }
        let delay = self.option_int("completion-delay", 100);
        if delay < 0 {
            return;
        }
        let point = self.point();
        let typed_char = point
            .checked_sub(1)
            .and_then(|pos| self.text().char_at(pos));
        let prefix = point - self.word_start();
        let triggered = typed_char.is_some_and(|c| {
            self.lsp_server_of(self.window().buffer)
                .is_some_and(|server| server.completion_triggers().contains(&c))
        });
        let enough = typed_char.is_some_and(is_word_char)
            && prefix as i64 >= self.option_int("completion-min-prefix", 2);
        if triggered || enough {
            self.completion.due = Some(Instant::now() + Duration::from_millis(delay as u64));
        }
    }

    pub(crate) fn completion_poll(&mut self, now: Instant) {
        if self.completion.due.is_some_and(|due| due <= now) {
            self.completion.due = None;
            self.completion_start(false);
        }
    }

    /// Asks for candidates for the text before point. `manual` says the
    /// user asked, who is then told if there are none.
    fn completion_start(&mut self, manual: bool) {
        let id = self.window().buffer;
        let point = self.point();
        let server = self
            .lsp_server_of(id)
            .filter(|server| server.provides("completionProvider"))
            .map(|server| server.id);
        match server {
            Some(server) => {
                self.lsp_flush(id);
                let mut params = self.lsp_position_params(id, point);
                params["context"] = serde_json::json!({"triggerKind": 1});
                let pending = Pending::Completion {
                    buffer: id,
                    point,
                    manual,
                };
                self.lsp_request(server, "textDocument/completion", params, pending);
            }
            None => {
                let items = self.word_items();
                self.completion_show(items, manual, false);
            }
        }
    }

    /// The words of the buffer as candidates, nearest to point first.
    fn word_items(&self) -> Vec<Item> {
        let text = self.text();
        let point = self.point();
        let start = self.word_start();
        let from = start.saturating_sub(WORD_SCAN);
        let mut words: Vec<(usize, String)> = Vec::new();
        let mut word = String::new();
        let mut word_start = from;
        let end = (point + WORD_SCAN).min(text.len());
        for (offset, c) in text
            .chars_from(from)
            .take(end - from)
            .chain(Some(' '))
            .enumerate()
        {
            if is_word_char(c) {
                if word.is_empty() {
                    word_start = from + offset;
                }
                word.push(c);
            } else if !word.is_empty() {
                let found = std::mem::take(&mut word);
                // Not the word being typed, and nothing too short to save typing.
                if word_start != start && found.chars().count() >= 3 {
                    words.push((word_start.abs_diff(point), found));
                }
            }
        }
        words.sort();
        let mut seen: Vec<&str> = Vec::new();
        let mut items = Vec::new();
        for (index, (_, word)) in words.iter().enumerate() {
            if seen.contains(&word.as_str()) {
                continue;
            }
            seen.push(word);
            items.push(Item {
                label: word.clone(),
                detail: String::new(),
                filter: word.clone(),
                sort: format!("{index:08}"),
                insert: word.clone(),
                is_snippet: false,
                replace_from: None,
                extra: Extra::None,
            });
        }
        items
    }

    /// The snippets that apply here, as candidates.
    fn snippet_items(&self) -> Vec<Item> {
        self.snippets_here()
            .into_iter()
            .map(|(name, _)| Item {
                label: name.to_owned(),
                detail: "snippet".to_owned(),
                filter: name.to_owned(),
                // After everything a server or the buffer offers.
                sort: format!("\u{10ffff}{name}"),
                insert: String::new(),
                is_snippet: false,
                replace_from: None,
                extra: Extra::Snippet(name.to_owned()),
            })
            .collect()
    }

    pub(crate) fn lsp_completion_response(
        &mut self,
        server: u64,
        buffer: BufferId,
        point: usize,
        manual: bool,
        result: &Json,
    ) {
        // The answer is for where point was. More of the same word may have
        // been typed since, which narrows it; anything else makes it stale.
        let now = self.point();
        let stale = self.window().buffer != buffer
            || now < point
            || !self.text().slice(point..now).chars().all(is_word_char);
        if stale || self.in_minibuffer() {
            return;
        }
        let encoding = self
            .lsp_server(server)
            .map_or(amacs_lsp::Encoding::default(), |server| server.encoding);
        let text = self.text();
        let items = result.get("items").unwrap_or(result);
        let items: Vec<Item> = items
            .as_array()
            .into_iter()
            .flatten()
            .map(|item| {
                let label = item["label"].as_str().unwrap_or("").to_owned();
                let edit = &item["textEdit"];
                let range = edit.get("range").or(edit.get("insert"));
                let insert = edit["newText"]
                    .as_str()
                    .or(item["insertText"].as_str())
                    .unwrap_or(&label)
                    .to_owned();
                let detail = item["detail"]
                    .as_str()
                    .or(item["labelDetails"]["detail"].as_str())
                    .map_or_else(|| kind_name(&item["kind"]).to_owned(), str::to_owned);
                let additional = item["additionalTextEdits"].as_array().cloned();
                Item {
                    filter: item["filterText"].as_str().unwrap_or(&label).to_owned(),
                    sort: item["sortText"].as_str().unwrap_or(&label).to_owned(),
                    is_snippet: item["insertTextFormat"].as_i64() == Some(2),
                    replace_from: range
                        .map(|range| crate::lsp::char_range(text, range, encoding).start),
                    extra: match additional {
                        Some(edits) if !edits.is_empty() => Extra::Edits(edits, encoding),
                        _ => Extra::None,
                    },
                    label,
                    detail: detail.lines().next().unwrap_or("").to_owned(),
                    insert,
                }
            })
            .collect();
        let incomplete = result["isIncomplete"].as_bool().unwrap_or(false);
        self.completion_show(items, manual, incomplete);
    }

    /// Shows candidates for the word before point, with the snippets that
    /// apply added to them.
    fn completion_show(&mut self, mut items: Vec<Item>, manual: bool, incomplete: bool) {
        items.extend(self.snippet_items());
        let start = self.word_start();
        let buffer = self.window().buffer;
        self.completion_close();
        let start = self.buffer_mut().create_marker_with(start, false);
        self.completion.popup = Some(Popup {
            buffer,
            start,
            items,
            matches: Vec::new(),
            selected: 0,
            scroll: 0,
            incomplete,
        });
        self.completion_narrow();
        if self.completion.popup.is_none() && manual {
            self.message("No completions");
        }
    }

    /// Narrows the list to what matches the text typed so far, and closes
    /// it when nothing does or point has left the word.
    fn completion_narrow(&mut self) {
        let Some(popup) = &self.completion.popup else {
            return;
        };
        let here = self.window().buffer == popup.buffer;
        let start = self.buffers[popup.buffer].marker(popup.start);
        let point = self.point();
        if !here || point < start || self.word_start() > start {
            self.completion_close();
            return;
        }
        let prefix = self.text().slice(start..point);
        let Some(popup) = &mut self.completion.popup else {
            return;
        };
        let mut ranked: Vec<(u8, &str, &str, usize)> = popup
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                // A candidate that is exactly what was typed finishes nothing.
                let done = item.insert == prefix && item.extra == Extra::None;
                let rank = rank(&item.filter, &prefix).filter(|_| !done)?;
                Some((rank, item.sort.as_str(), item.label.as_str(), index))
            })
            .collect();
        ranked.sort();
        popup.matches = ranked.into_iter().map(|(_, _, _, index)| index).collect();
        popup.selected = 0;
        popup.scroll = 0;
        if popup.matches.is_empty() {
            self.completion_close();
        }
    }

    pub(crate) fn completion_close(&mut self) {
        if let Some(popup) = self.completion.popup.take()
            && let Some(buffer) = self.buffers.get_mut(popup.buffer)
        {
            buffer.remove_marker(popup.start);
        }
    }

    /// Whether the list is showing in the current buffer.
    pub(crate) fn completion_active(&self) -> bool {
        self.completion
            .popup
            .as_ref()
            .is_some_and(|popup| popup.buffer == self.window().buffer && !self.in_minibuffer())
    }

    fn completion_move(&mut self, delta: i64) {
        if let Some(popup) = &mut self.completion.popup {
            // Moving past either end comes round to the other.
            let count = popup.matches.len() as i64;
            popup.selected = (popup.selected as i64 + delta).rem_euclid(count) as usize;
            if popup.selected < popup.scroll {
                popup.scroll = popup.selected;
            } else if popup.selected >= popup.scroll + VISIBLE {
                popup.scroll = popup.selected + 1 - VISIBLE;
            }
        }
    }

    /// Replaces the word being typed with the selected candidate.
    fn completion_accept(&mut self) -> CmdResult {
        let Some(popup) = &self.completion.popup else {
            return Ok(());
        };
        let item = popup.items[popup.matches[popup.selected]].clone();
        let start = self.buffers[popup.buffer].marker(popup.start);
        self.completion_close();
        let point = self.point();
        let from = item.replace_from.map_or(start, |from| from.min(point));
        // What the candidate adds elsewhere goes in first, while positions
        // are still what the server meant; markers carry point across.
        let end = self.buffer_mut().create_marker_with(point, true);
        let begin = self.buffer_mut().create_marker_with(from, false);
        if let Extra::Edits(edits, encoding) = &item.extra {
            let id = self.window().buffer;
            self.lsp_apply_text_edits(id, edits, *encoding)?;
        }
        let (from, to) = (self.buffer().marker(begin), self.buffer().marker(end));
        self.buffer_mut().remove_marker(begin);
        self.buffer_mut().remove_marker(end);
        self.delete(from..to)?;
        self.set_point(from);
        match &item.extra {
            Extra::Snippet(name) => self.expand_snippet_named(name),
            _ if item.is_snippet => self.expand_snippet(&Template::parse_lsp(&item.insert)),
            _ => self.insert(&item.insert),
        }
    }

    // Drawing

    /// Draws the list over the rows of the selected window. `cursor` is
    /// where point is in the window, and `gutter` the columns left of the
    /// text.
    pub(crate) fn overlay_popup(
        &self,
        lines: &mut [DisplayLine],
        cursor: Option<(u16, u16)>,
        width: usize,
    ) {
        let (Some(popup), Some((row, col))) = (&self.completion.popup, cursor) else {
            return;
        };
        let (row, col) = (usize::from(row), usize::from(col));
        let buffer = &self.buffers[popup.buffer];
        let start = buffer.marker(popup.start);
        let point = self.point();
        if point < start {
            return;
        }
        let end = (popup.scroll + VISIBLE).min(popup.matches.len());
        let shown = &popup.matches[popup.scroll..end];
        // Below point if there is room, else above, else as much as fits below.
        let (below, above) = (lines.len().saturating_sub(row + 1), row);
        let (first_row, count) = if below >= shown.len() || below >= above {
            (row + 1, shown.len().min(below))
        } else {
            let count = shown.len().min(above);
            (row - count, count)
        };
        if count == 0 || width < 4 {
            return;
        }

        let label_cells = shown
            .iter()
            .map(|index| str_cells(&popup.items[*index].label))
            .max()
            .unwrap_or(0);
        let detail_cells = shown
            .iter()
            .map(|index| str_cells(&popup.items[*index].detail))
            .max()
            .unwrap_or(0);
        let wanted = label_cells
            + if detail_cells > 0 {
                detail_cells + 2
            } else {
                0
            }
            + 2;
        let box_width = wanted.clamp(8, MAX_WIDTH).min(width);
        // The labels line up with the text being completed.
        let typed = column_of(buffer.text(), point, buffer.tab_width).saturating_sub(column_of(
            buffer.text(),
            start,
            buffer.tab_width,
        ));
        let left = col
            .saturating_sub(typed + 1)
            .min(width.saturating_sub(box_width));

        for (offset, index) in shown.iter().take(count).enumerate() {
            let item = &popup.items[*index];
            let selected = popup.scroll + offset == popup.selected;
            let (face, detail_face) = if selected {
                (faces::POPUP_SELECTED, faces::POPUP_SELECTED)
            } else {
                (faces::POPUP, faces::POPUP_ANNOTATION)
            };
            let mut cells: Vec<(char, FaceId)> = Vec::with_capacity(box_width);
            cells.push((' ', face));
            cells.extend(item.label.chars().map(|c| (c, face)));
            let detail: Vec<char> = item.detail.chars().collect();
            let used: usize = cells.iter().map(|(c, _)| c.width().unwrap_or(1)).sum();
            let detail_width = str_cells(&item.detail);
            if detail_width > 0 && used + 2 + detail_width < box_width {
                let gap = box_width - 1 - detail_width - used;
                cells.extend(std::iter::repeat_n((' ', face), gap));
                cells.extend(detail.into_iter().map(|c| (c, detail_face)));
            }
            overlay_cells(
                &mut lines[first_row + offset],
                left,
                box_width,
                &cells,
                face,
            );
        }
    }
}

pub fn completion_at_point(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    ed.completion_start(true);
    Ok(())
}

pub fn completion_next(ed: &mut Editor) -> CmdResult {
    ed.completion_move(1);
    Ok(())
}

pub fn completion_previous(ed: &mut Editor) -> CmdResult {
    ed.completion_move(-1);
    Ok(())
}

pub fn completion_accept(ed: &mut Editor) -> CmdResult {
    ed.completion_accept()
}

pub fn completion_quit(ed: &mut Editor) -> CmdResult {
    ed.completion_close();
    Ok(())
}

/// Offers the snippets whose names start with the word before point, or
/// expands the one that the word names.
pub fn snippet_complete(ed: &mut Editor) -> CmdResult {
    ed.forbid_in_minibuffer()?;
    let start = ed.word_start();
    let prefix = ed.text().slice(start..ed.point());
    let exact = ed.snippets_here().iter().any(|(name, _)| *name == prefix);
    if exact {
        ed.delete(start..start + prefix.chars().count())?;
        ed.set_point(start);
        return ed.expand_snippet_named(&prefix);
    }
    let items = ed.snippet_items();
    if items.is_empty() {
        return Err(CmdError::msg("No snippets are defined for this mode"));
    }
    // Only the snippets, so none are added a second time.
    let buffer = ed.window().buffer;
    ed.completion_close();
    let marker = ed.buffer_mut().create_marker_with(start, false);
    ed.completion.popup = Some(Popup {
        buffer,
        start: marker,
        items,
        matches: Vec::new(),
        selected: 0,
        scroll: 0,
        incomplete: false,
    });
    ed.completion_narrow();
    if ed.completion.popup.is_none() {
        return Err(CmdError::msg(format!("No snippet starts with {prefix}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_are_ranked_by_how_they_match() {
        assert_eq!(rank("println", "pri"), Some(0));
        assert_eq!(rank("println", "PRI"), Some(0));
        assert_eq!(rank("eprintln", "pri"), Some(1));
        assert_eq!(rank("println", "ptl"), Some(2));
        assert_eq!(rank("println", "xyz"), None);
        assert_eq!(rank("replace_whole", "al"), None);
        assert_eq!(rank("anything", ""), Some(0));
    }
}
