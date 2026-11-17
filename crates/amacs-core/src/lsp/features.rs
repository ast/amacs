//! What language servers are asked for: where things are defined and used,
//! what they are, renaming, code actions and symbols.

use std::collections::HashMap;
use std::path::PathBuf;

use amacs_lsp::{Encoding, uri_to_path};
use serde_json::{Value as Json, json};

use super::{Pending, char_range, lsp_offset, lsp_position, summary_line};
use crate::buffer::{Buffer, BufferId};
use crate::completion::{Candidate, Completion};
use crate::editor::Editor;
use crate::error::{CmdError, CmdResult};
use crate::files::abbreviate_path;
use crate::minibuffer::Accepted;

const HELP_BUFFER: &str = "*lsp-help*";

/// A place in a file, as a server gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Location {
    pub path: PathBuf,
    pub line: usize,
    /// Offset in the line, in the server's units.
    pub units: usize,
    pub encoding: Encoding,
}

/// Reads a `Location`, a `LocationLink`, or an array of either.
fn parse_locations(result: &Json, encoding: Encoding) -> Vec<Location> {
    let one = |value: &Json| {
        let uri = value["uri"].as_str().or(value["targetUri"].as_str())?;
        let range = value
            .get("range")
            .or(value.get("targetSelectionRange"))
            .or(value.get("targetRange"))?;
        Some(Location {
            path: uri_to_path(uri)?,
            line: range["start"]["line"].as_u64()? as usize,
            units: range["start"]["character"].as_u64()? as usize,
            encoding,
        })
    };
    match result {
        Json::Array(items) => items.iter().filter_map(one).collect(),
        Json::Null => Vec::new(),
        single => one(single).into_iter().collect(),
    }
}

/// The name of a symbol kind.
fn symbol_kind(kind: &Json) -> &'static str {
    const KINDS: [&str; 27] = [
        "",
        "file",
        "module",
        "namespace",
        "package",
        "class",
        "method",
        "property",
        "field",
        "constructor",
        "enum",
        "interface",
        "function",
        "variable",
        "constant",
        "string",
        "number",
        "boolean",
        "array",
        "object",
        "key",
        "null",
        "enum member",
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

/// The text of a hover: Markdown, a marked string, or several of them.
fn hover_text(contents: &Json) -> String {
    match contents {
        Json::String(text) => text.clone(),
        Json::Array(parts) => parts
            .iter()
            .map(hover_text)
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
        Json::Object(_) => contents["value"].as_str().unwrap_or("").to_owned(),
        _ => String::new(),
    }
}

/// Prose without its Markdown code fences.
fn without_fences(text: &str) -> String {
    text.lines()
        .filter(|line| !line.trim_start().starts_with("```"))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

impl Editor {
    /// The word around point, which commands offer as their default.
    fn symbol_at_point(&self) -> String {
        let (text, point) = (self.text(), self.point());
        let is_symbol = |c: &char| c.is_alphanumeric() || *c == '_';
        let before = text.chars_before(point).take_while(is_symbol).count();
        let after = text.chars_from(point).take_while(is_symbol).count();
        text.slice(point - before..point + after)
    }

    fn server_encoding(&self, server: u64) -> Encoding {
        self.lsp_server(server)
            .map_or(Encoding::default(), |server| server.encoding)
    }

    // Places

    /// Remembers where point is, for `xref-go-back`.
    fn push_place(&mut self) {
        let place = (self.window().buffer, self.point());
        if self.lsp.history.last() != Some(&place) {
            self.lsp.history.push(place);
            let excess = self.lsp.history.len().saturating_sub(100);
            self.lsp.history.drain(..excess);
        }
    }

    fn goto_location(&mut self, location: &Location) -> CmdResult {
        self.push_place();
        self.find_file(&location.path)?;
        let pos = lsp_offset(
            self.text(),
            location.line,
            location.units,
            location.encoding,
        );
        self.set_point(pos);
        Ok(())
    }

    /// Lets the user pick one of several places and goes there.
    fn choose_location(
        &mut self,
        prompt: &str,
        server: u64,
        locations: Vec<Location>,
    ) -> CmdResult {
        let root = self.lsp_server(server).map(|server| server.root.clone());
        let mut lines: HashMap<PathBuf, Vec<String>> = HashMap::new();
        let mut candidates = Vec::with_capacity(locations.len());
        for (index, location) in locations.iter().enumerate() {
            let content = lines.entry(location.path.clone()).or_insert_with(|| {
                let open = self
                    .buffers
                    .values()
                    .find(|buffer| buffer.file.as_deref() == Some(&location.path));
                let text = match open {
                    Some(buffer) => buffer.text().to_string(),
                    None => std::fs::read_to_string(&location.path).unwrap_or_default(),
                };
                text.lines().map(str::to_owned).collect()
            });
            let line = content.get(location.line).map_or("", |line| line.trim());
            let shown = root
                .as_deref()
                .and_then(|root| location.path.strip_prefix(root).ok())
                .map_or_else(
                    || abbreviate_path(&location.path),
                    |relative| relative.display().to_string(),
                );
            candidates.push(Candidate {
                text: format!("{shown}:{}: {line}", location.line + 1),
                annotation: String::new(),
                value: Some(index.to_string()),
            });
        }
        let completion = Completion::Items {
            candidates,
            category: "location".to_owned(),
        };
        self.read_candidate(prompt, "", completion, move |ed, accepted| {
            let Accepted::Candidate(candidate) = accepted else {
                return Err(CmdError::msg("No match selected"));
            };
            let location = candidate
                .value()
                .parse::<usize>()
                .ok()
                .and_then(|index| locations.get(index))
                .ok_or_else(|| CmdError::msg("No match selected"))?;
            ed.goto_location(location)
        })
    }

    /// Goes to the one place found, or lets the user pick among several.
    fn visit_locations(&mut self, what: &str, server: u64, locations: Vec<Location>) {
        let result = match locations.len() {
            0 => Err(CmdError::msg(format!("No {what} found"))),
            1 => self.goto_location(&locations[0]),
            _ => {
                let prompt = format!("{} {what}s: ", locations.len());
                self.choose_location(&prompt, server, locations)
            }
        };
        self.report(result);
    }

    pub fn xref_find_definitions(&mut self) -> CmdResult {
        let server = self.lsp_current_server("definitionProvider")?;
        let params = self.lsp_position_params(self.window().buffer, self.point());
        self.lsp_request(
            server,
            "textDocument/definition",
            params,
            Pending::Definition,
        );
        Ok(())
    }

    pub(crate) fn lsp_definition_response(&mut self, server: u64, result: &Json) {
        let locations = parse_locations(result, self.server_encoding(server));
        self.visit_locations("definition", server, locations);
    }

    pub fn xref_find_references(&mut self) -> CmdResult {
        let server = self.lsp_current_server("referencesProvider")?;
        let mut params = self.lsp_position_params(self.window().buffer, self.point());
        params["context"] = json!({"includeDeclaration": true});
        self.lsp_request(
            server,
            "textDocument/references",
            params,
            Pending::References,
        );
        Ok(())
    }

    pub(crate) fn lsp_references_response(&mut self, server: u64, result: &Json) {
        let locations = parse_locations(result, self.server_encoding(server));
        // One reference is still listed: it says there are no others.
        let result = if locations.is_empty() {
            Err(CmdError::msg("No references found"))
        } else {
            let prompt = format!("{} references: ", locations.len());
            self.choose_location(&prompt, server, locations)
        };
        self.report(result);
    }

    /// Returns to where the last jump to a definition or reference started.
    pub fn xref_go_back(&mut self) -> CmdResult {
        loop {
            let (buffer, pos) = self
                .lsp
                .history
                .pop()
                .ok_or_else(|| CmdError::msg("No earlier place to go back to"))?;
            if self.buffers.contains_key(buffer) {
                self.switch_to_buffer(buffer);
                self.set_point(pos);
                return Ok(());
            }
        }
    }

    // Hover

    pub fn lsp_hover(&mut self) -> CmdResult {
        let server = self.lsp_current_server("hoverProvider")?;
        let (buffer, point) = (self.window().buffer, self.point());
        let params = self.lsp_position_params(buffer, point);
        let pending = Pending::Hover {
            buffer,
            point,
            explicit: true,
        };
        self.lsp_request(server, "textDocument/hover", params, pending);
        Ok(())
    }

    pub(crate) fn lsp_hover_response(
        &mut self,
        buffer: BufferId,
        point: usize,
        explicit: bool,
        result: &Json,
    ) {
        let text = hover_text(&result["contents"]);
        if explicit {
            let plain = without_fences(&text);
            if plain.is_empty() {
                self.message("Nothing is known about this");
            } else if plain.lines().count() <= 1 {
                self.message(plain);
            } else {
                self.show_help(&plain);
            }
            return;
        }
        // Said in passing: only if the user is still there and nothing
        // else has been said meanwhile.
        let still_there = self.window().buffer == buffer && self.point() == point;
        let line = summary_line(&text);
        if still_there
            && self.frame().echo.is_none()
            && self.minibuffer.is_none()
            && !line.is_empty()
        {
            self.message(line.to_owned());
        }
    }

    /// Shows text in the help buffer, in another window.
    fn show_help(&mut self, text: &str) {
        let shown = self.show_text(HELP_BUFFER, text);
        self.report(shown);
    }

    /// Shows text in a read-only buffer of the given name, in another window.
    pub(crate) fn show_text(&mut self, name: &str, text: &str) -> CmdResult {
        let id = match self.find_buffer(name) {
            Some(id) => id,
            None => {
                let mut buffer = Buffer::new(name);
                buffer.read_only = true;
                self.add_buffer(buffer)
            }
        };
        self.buffers[id].reset(amacs_text::Text::from(text));
        self.buffers[id].save_view(0, 0);
        self.show_in_other_window(id)
    }

    // Rename

    pub fn lsp_rename(&mut self) -> CmdResult {
        let server = self.lsp_current_server("renameProvider")?;
        let symbol = self.symbol_at_point();
        if symbol.is_empty() {
            return Err(CmdError::msg("No symbol at point"));
        }
        let params = self.lsp_position_params(self.window().buffer, self.point());
        let prompt = format!("Rename {symbol} to: ");
        let initial = symbol.clone();
        self.read_string(prompt, &initial, Completion::None, move |ed, new_name| {
            if new_name.is_empty() || new_name == symbol {
                return Ok(());
            }
            let mut params = params;
            params["newName"] = Json::String(new_name);
            ed.lsp_request(server, "textDocument/rename", params, Pending::Rename);
            Ok(())
        })
    }

    pub(crate) fn lsp_rename_response(&mut self, server: u64, result: &Json) {
        let applied = self.lsp_apply_workspace_edit(result, self.server_encoding(server));
        self.commit_undo();
        match applied {
            Ok(0) => self.message("Nothing to rename"),
            Ok(1) => self.message("Renamed in 1 buffer"),
            Ok(count) => self.message(format!("Renamed in {count} buffers; save them with C-x s")),
            Err(reason) => self.message(format!("Rename failed: {reason}")),
        }
    }

    // Code actions

    pub fn lsp_code_actions(&mut self) -> CmdResult {
        let server = self.lsp_current_server("codeActionProvider")?;
        let id = self.window().buffer;
        let range = match self.region() {
            Ok(region) if self.buffer().mark_active => region,
            _ => self.point()..self.point(),
        };
        let buffer = &self.buffers[id];
        let Some(document) = &buffer.document else {
            return Ok(());
        };
        let position = |pos: usize| {
            let (line, character) = lsp_position(buffer.text(), pos, document.encoding);
            json!({"line": line, "character": character})
        };
        let diagnostics: Vec<Json> = buffer
            .diagnostics()
            .filter(|(found, _)| found.start <= range.end && range.start <= found.end)
            .map(|(_, diagnostic)| diagnostic.raw.clone())
            .collect();
        let params = json!({
            "textDocument": {"uri": document.uri},
            "range": {"start": position(range.start), "end": position(range.end)},
            "context": {"diagnostics": diagnostics, "triggerKind": 1},
        });
        self.lsp_request(
            server,
            "textDocument/codeAction",
            params,
            Pending::CodeActions,
        );
        Ok(())
    }

    pub(crate) fn lsp_code_actions_response(&mut self, server: u64, result: &Json) {
        let actions: Vec<Json> = result
            .as_array()
            .into_iter()
            .flatten()
            .filter(|action| action["disabled"].is_null())
            .cloned()
            .collect();
        if actions.is_empty() {
            self.message("No code actions here");
            return;
        }
        let candidates = actions
            .iter()
            .enumerate()
            .map(|(index, action)| Candidate {
                text: action["title"].as_str().unwrap_or("(untitled)").to_owned(),
                annotation: action["kind"].as_str().unwrap_or("").to_owned(),
                value: Some(index.to_string()),
            })
            .collect();
        let completion = Completion::Items {
            candidates,
            category: "code-action".to_owned(),
        };
        let asked = self.read_candidate("Code action: ", "", completion, move |ed, accepted| {
            let Accepted::Candidate(candidate) = accepted else {
                return Err(CmdError::msg("No such action"));
            };
            let action = candidate
                .value()
                .parse::<usize>()
                .ok()
                .and_then(|index| actions.get(index))
                .ok_or_else(|| CmdError::msg("No such action"))?;
            ed.lsp_run_action(server, action)
        });
        self.report(asked);
    }

    fn lsp_run_action(&mut self, server: u64, action: &Json) -> CmdResult {
        if !action["edit"].is_null() {
            self.lsp_apply_workspace_edit(&action["edit"], self.server_encoding(server))
                .map_err(CmdError::msg)?;
        }
        // An action is a command itself, or carries one to run after its edit.
        let command = if action["command"].is_string() {
            action
        } else {
            &action["command"]
        };
        if let Some(name) = command["command"].as_str() {
            let params = json!({"command": name, "arguments": command["arguments"]});
            self.lsp_request(server, "workspace/executeCommand", params, Pending::Command);
        }
        Ok(())
    }

    // Symbols

    pub fn lsp_document_symbols(&mut self) -> CmdResult {
        let server = self.lsp_current_server("documentSymbolProvider")?;
        let uri = self.buffer().document.as_ref().map(|d| d.uri.clone());
        let params = json!({"textDocument": {"uri": uri}});
        self.lsp_request(
            server,
            "textDocument/documentSymbol",
            params,
            Pending::DocumentSymbols,
        );
        Ok(())
    }

    pub(crate) fn lsp_document_symbols_response(&mut self, server: u64, result: &Json) {
        let encoding = self.server_encoding(server);
        // Name, kind and position of each symbol, outer ones first.
        fn collect(
            symbols: &Json,
            prefix: &str,
            text: &amacs_text::Text,
            encoding: Encoding,
            out: &mut Vec<(String, &'static str, usize)>,
        ) {
            for symbol in symbols.as_array().into_iter().flatten() {
                let name = symbol["name"].as_str().unwrap_or("");
                let name = match prefix {
                    "" => name.to_owned(),
                    prefix => format!("{prefix} / {name}"),
                };
                let range = symbol
                    .get("selectionRange")
                    .or(symbol.get("range"))
                    .unwrap_or(&symbol["location"]["range"]);
                let pos = char_range(text, range, encoding).start;
                out.push((name.clone(), symbol_kind(&symbol["kind"]), pos));
                collect(&symbol["children"], &name, text, encoding, out);
            }
        }
        let mut symbols = Vec::new();
        collect(result, "", self.text(), encoding, &mut symbols);
        if symbols.is_empty() {
            self.message("No symbols in this buffer");
            return;
        }
        let point = self.point();
        let nearest = symbols
            .iter()
            .rposition(|(_, _, pos)| *pos <= point)
            .unwrap_or(0);
        let candidates = symbols
            .iter()
            .map(|(name, kind, pos)| Candidate {
                text: name.clone(),
                annotation: (*kind).to_owned(),
                value: Some(pos.to_string()),
            })
            .collect();
        let completion = Completion::Items {
            candidates,
            category: "symbol".to_owned(),
        };
        let origin = (point, self.buffer().marker(self.window().start));
        let restore = move |ed: &mut Editor| {
            ed.set_point(origin.0);
            let start = ed.window().start;
            ed.buffer_mut().set_marker(start, origin.1);
        };
        let go_to = |ed: &mut Editor, candidate: &Candidate| {
            if let Ok(pos) = candidate.value().parse::<usize>() {
                ed.set_point(pos);
            }
        };
        let asked =
            self.read_candidate(
                "Go to symbol: ",
                "",
                completion,
                move |ed, accepted| match accepted {
                    Accepted::Candidate(candidate) => {
                        ed.buffer_mut().set_mark(origin.0);
                        go_to(ed, &candidate);
                        Ok(())
                    }
                    Accepted::Input(_) => {
                        restore(ed);
                        Err(CmdError::msg("No such symbol"))
                    }
                },
            );
        if asked.is_ok() {
            self.set_prompt_preview(
                move |ed, candidate| {
                    if let Some(candidate) = candidate {
                        go_to(ed, candidate);
                    }
                },
                restore,
            );
            self.select_candidate(nearest);
        }
        self.report(asked);
    }

    pub fn lsp_workspace_symbols(&mut self) -> CmdResult {
        let server = self.lsp_current_server("workspaceSymbolProvider")?;
        let symbol = self.symbol_at_point();
        self.read_string(
            "Find symbol: ",
            &symbol,
            Completion::None,
            move |ed, query| {
                let params = json!({"query": query});
                ed.lsp_request(
                    server,
                    "workspace/symbol",
                    params,
                    Pending::WorkspaceSymbols,
                );
                Ok(())
            },
        )
    }

    pub(crate) fn lsp_workspace_symbols_response(&mut self, server: u64, result: &Json) {
        let encoding = self.server_encoding(server);
        let locations: Vec<Location> = result
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|symbol| parse_locations(&symbol["location"], encoding))
            .collect();
        self.visit_locations("symbol", server, locations);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations_come_in_several_shapes() {
        let range =
            json!({"start": {"line": 2, "character": 5}, "end": {"line": 2, "character": 9}});
        let plain = json!({"uri": "file:///a/b.rs", "range": range});
        let link = json!({"targetUri": "file:///a/c.rs", "targetRange": range,
                          "targetSelectionRange": range});
        let expected = |path: &str| Location {
            path: PathBuf::from(path),
            line: 2,
            units: 5,
            encoding: Encoding::Utf16,
        };
        assert_eq!(
            parse_locations(&plain, Encoding::Utf16),
            [expected("/a/b.rs")]
        );
        assert_eq!(
            parse_locations(&json!([plain, link]), Encoding::Utf16),
            [expected("/a/b.rs"), expected("/a/c.rs")]
        );
        assert!(parse_locations(&Json::Null, Encoding::Utf16).is_empty());
        assert!(
            parse_locations(
                &json!({"uri": "untitled:1", "range": range}),
                Encoding::Utf16
            )
            .is_empty()
        );
    }

    #[test]
    fn hover_contents_come_in_several_shapes() {
        assert_eq!(hover_text(&json!("plain")), "plain");
        assert_eq!(
            hover_text(&json!({"kind": "markdown", "value": "**b**"})),
            "**b**"
        );
        assert_eq!(
            hover_text(&json!([{"language": "rust", "value": "fn f()"}, "docs", ""])),
            "fn f()\n\ndocs"
        );
        assert_eq!(
            without_fences("```rust\nfn f()\n```\n\ndocs\n"),
            "fn f()\n\ndocs"
        );
    }
}
