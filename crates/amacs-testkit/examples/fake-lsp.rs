//! A language server for tests. It speaks the real protocol and keeps its
//! own copy of every open document, so that the editor's side of document
//! synchronisation is checked by everything it answers.
//!
//! Its "language" is words: `BUG`, `WARN` and `NOTE` are diagnostics, a line
//! starting with `fn NAME` defines a symbol, and every word refers to its
//! first occurrence.
//!
//! Arguments: `--utf8` to negotiate UTF-8 offsets, `--full-sync` to ask for
//! whole documents on every change.

use std::collections::BTreeMap;
use std::io::{self, BufReader};

use amacs_lsp::{Encoding, from_units, read_message, to_units, write_message};
use serde_json::{Value, json};

struct Server {
    out: io::Stdout,
    encoding: Encoding,
    docs: BTreeMap<String, String>,
    next_request: i64,
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Every word of `text` with its line and char column.
fn words(text: &str) -> Vec<(usize, usize, String)> {
    let mut found = Vec::new();
    for (line_index, line) in text.split('\n').enumerate() {
        let chars: Vec<char> = line.chars().collect();
        let mut col = 0;
        while col < chars.len() {
            if is_word_char(chars[col]) {
                let start = col;
                while col < chars.len() && is_word_char(chars[col]) {
                    col += 1;
                }
                found.push((line_index, start, chars[start..col].iter().collect()));
            } else {
                col += 1;
            }
        }
    }
    found
}

impl Server {
    fn send(&mut self, message: Value) {
        write_message(&mut self.out, &message).expect("the editor is reading");
    }

    fn line<'a>(&self, text: &'a str, line: usize) -> &'a str {
        text.split('\n').nth(line).unwrap_or("")
    }

    /// A protocol position for a line and char column.
    fn position(&self, text: &str, line: usize, col: usize) -> Value {
        let units = to_units(self.line(text, line), col, self.encoding);
        json!({"line": line, "character": units})
    }

    fn range(&self, text: &str, line: usize, col: usize, len: usize) -> Value {
        json!({
            "start": self.position(text, line, col),
            "end": self.position(text, line, col + len),
        })
    }

    /// The line and char column of a protocol position.
    fn char_position(&self, text: &str, position: &Value) -> (usize, usize) {
        let line = position["line"].as_u64().unwrap_or(0) as usize;
        let units = position["character"].as_u64().unwrap_or(0) as usize;
        (
            line,
            from_units(self.line(text, line), units, self.encoding),
        )
    }

    /// The byte offset of a protocol position.
    fn offset(&self, text: &str, position: &Value) -> usize {
        let (line, col) = self.char_position(text, position);
        let line_start: usize = text.split('\n').take(line).map(|l| l.len() + 1).sum();
        let within: usize = self
            .line(text, line)
            .chars()
            .take(col)
            .map(char::len_utf8)
            .sum();
        (line_start + within).min(text.len())
    }

    /// The word that a position is in or just after, with its start column.
    fn word_at(&self, text: &str, position: &Value) -> Option<(usize, usize, String)> {
        let (line, col) = self.char_position(text, position);
        words(text).into_iter().find(|(l, start, word)| {
            *l == line && *start <= col && col <= start + word.chars().count()
        })
    }

    fn publish_diagnostics(&mut self, uri: &str) {
        let text = self.docs.get(uri).cloned().unwrap_or_default();
        if text.contains("CRASH") {
            eprintln!("fake-lsp: crashing as asked");
            std::process::exit(1);
        }
        let diagnostics: Vec<Value> = words(&text)
            .into_iter()
            .filter_map(|(line, col, word)| {
                let severity = match word.as_str() {
                    "BUG" => 1,
                    "WARN" => 2,
                    "NOTE" => 3,
                    _ => return None,
                };
                Some(json!({
                    "range": self.range(&text, line, col, word.chars().count()),
                    "severity": severity,
                    "source": "fake",
                    "code": format!("E{severity}"),
                    "message": format!("{word} found\nsecond line of the message"),
                }))
            })
            .collect();
        self.send(json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": {"uri": uri, "diagnostics": diagnostics},
        }));
    }

    fn apply_change(&mut self, uri: &str, change: &Value) {
        let text = self.docs.get(uri).cloned().unwrap_or_default();
        let new_text = change["text"].as_str().unwrap_or("");
        let updated = match change.get("range") {
            Some(range) => {
                let start = self.offset(&text, &range["start"]);
                let end = self.offset(&text, &range["end"]).max(start);
                format!("{}{new_text}{}", &text[..start], &text[end..])
            }
            None => new_text.to_owned(),
        };
        self.docs.insert(uri.to_owned(), updated);
    }

    fn symbols(&self) -> Vec<(String, usize, String, usize)> {
        let mut found = Vec::new();
        for (uri, text) in &self.docs {
            for (index, line) in text.split('\n').enumerate() {
                if let Some(rest) = line.strip_prefix("fn ") {
                    let name: String = rest.chars().take_while(|c| is_word_char(*c)).collect();
                    found.push((uri.clone(), index, name, line.chars().count()));
                }
            }
        }
        found
    }

    fn completion(&self, uri: &str, position: &Value) -> Value {
        let text = self.docs.get(uri).cloned().unwrap_or_default();
        let (line, col) = self.char_position(&text, position);
        let chars: Vec<char> = self.line(&text, line).chars().collect();
        let mut start = col.min(chars.len());
        while start > 0 && is_word_char(chars[start - 1]) {
            start -= 1;
        }
        let prefix: String = chars[start..col.min(chars.len())].iter().collect();
        if start > 0 && chars[start - 1] == '.' {
            return json!({"isIncomplete": false, "items": [
                {"label": "len", "kind": 2, "detail": "fn len() -> usize"},
                {"label": "push", "kind": 2, "insertTextFormat": 2,
                 "insertText": "push(${1:value})$0"},
                {"label": "is_empty", "kind": 2},
            ]});
        }
        if text.contains("PARTIAL") {
            // A server that holds back: one candidate at a time.
            let first = words(&text)
                .into_iter()
                .map(|(_, _, word)| word)
                .find(|word| word.starts_with(&prefix) && *word != prefix);
            let items: Vec<Value> = first
                .into_iter()
                .map(|word| json!({"label": word}))
                .collect();
            return json!({"isIncomplete": true, "items": items});
        }
        let mut items: Vec<Value> = Vec::new();
        let mut seen = vec![prefix.clone()];
        for (_, _, word) in words(&text) {
            if !seen.contains(&word) {
                seen.push(word.clone());
                items.push(json!({"label": word, "kind": 1}));
            }
        }
        items.push(json!({
            "label": "println", "kind": 3, "detail": "fn println(fmt)",
            "insertTextFormat": 2, "insertText": "println(\"$1\")$0",
        }));
        items.push(json!({
            "label": "import_me", "detail": "auto import",
            "additionalTextEdits": [{
                "range": self.range(&text, 0, 0, 0), "newText": "use import_me;\n",
            }],
        }));
        items.push(json!({
            "label": "replace_whole",
            "textEdit": {
                "range": self.range(&text, line, start, prefix.chars().count()),
                "newText": "replaced_whole",
            },
        }));
        json!({"isIncomplete": false, "items": items})
    }

    fn occurrences(&self, word: &str) -> Vec<Value> {
        let mut found = Vec::new();
        for (uri, text) in &self.docs {
            for (line, col, other) in words(text) {
                if other == word {
                    found.push(json!({
                        "uri": uri,
                        "range": self.range(text, line, col, word.chars().count()),
                    }));
                }
            }
        }
        found
    }

    fn request(&mut self, method: &str, params: &Value) -> Result<Value, (i64, String)> {
        let uri = params["textDocument"]["uri"]
            .as_str()
            .unwrap_or("")
            .to_owned();
        let text = self.docs.get(&uri).cloned().unwrap_or_default();
        let word = self.word_at(&text, &params["position"]);
        match method {
            "initialize" => {
                let offered = params["capabilities"]["general"]["positionEncodings"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                let flag = |name: &str| std::env::args().any(|arg| arg == name);
                if flag("--utf8") && offered.contains(&json!("utf-8")) {
                    self.encoding = Encoding::Utf8;
                }
                let full = flag("--full-sync");
                let encoding = match self.encoding {
                    Encoding::Utf8 => "utf-8",
                    _ => "utf-16",
                };
                Ok(json!({
                    "capabilities": {
                        "positionEncoding": encoding,
                        "textDocumentSync": {
                            "openClose": true,
                            "change": if full { 1 } else { 2 },
                            "save": true,
                        },
                        "completionProvider": {"triggerCharacters": ["."]},
                        "hoverProvider": true,
                        "definitionProvider": true,
                        "referencesProvider": true,
                        "renameProvider": true,
                        "codeActionProvider": true,
                        "documentSymbolProvider": true,
                        "workspaceSymbolProvider": true,
                        "executeCommandProvider": {"commands": ["fake.note"]},
                    },
                    "serverInfo": {"name": "fake-lsp"},
                }))
            }
            "shutdown" => Ok(Value::Null),
            "textDocument/completion" => Ok(self.completion(&uri, &params["position"])),
            "textDocument/hover" => Ok(match word {
                Some((line, col, word)) => json!({
                    "contents": {
                        "kind": "markdown",
                        "value": format!("**{word}**\n\ndocumentation of {word}"),
                    },
                    "range": self.range(&text, line, col, word.chars().count()),
                }),
                None => Value::Null,
            }),
            "textDocument/definition" => Ok(match word {
                Some((_, _, word)) if word == "extern_fn" => {
                    let other = uri
                        .rsplit_once('/')
                        .map_or_else(String::new, |(dir, _)| format!("{dir}/other.txt"));
                    json!([{"uri": other, "range": {
                        "start": {"line": 1, "character": 3},
                        "end": {"line": 1, "character": 12},
                    }}])
                }
                Some((_, _, word)) => {
                    json!(self.occurrences(&word).into_iter().next())
                }
                None => Value::Null,
            }),
            "textDocument/references" => Ok(match word {
                Some((_, _, word)) => json!(self.occurrences(&word)),
                None => json!([]),
            }),
            "textDocument/rename" => {
                let Some((_, _, word)) = word else {
                    return Err((-32602, "no symbol here".to_owned()));
                };
                let new_name = params["newName"].as_str().unwrap_or("");
                let mut changes: Vec<Value> = Vec::new();
                for (uri, text) in &self.docs {
                    let edits: Vec<Value> = words(text)
                        .into_iter()
                        .filter(|(_, _, other)| *other == word)
                        .map(|(line, col, _)| {
                            json!({
                                "range": self.range(text, line, col, word.chars().count()),
                                "newText": new_name,
                            })
                        })
                        .collect();
                    if !edits.is_empty() {
                        changes.push(json!({
                            "textDocument": {"uri": uri, "version": null},
                            "edits": edits,
                        }));
                    }
                }
                Ok(json!({"documentChanges": changes}))
            }
            "textDocument/codeAction" => {
                let mut actions = Vec::new();
                for diagnostic in params["context"]["diagnostics"]
                    .as_array()
                    .into_iter()
                    .flatten()
                {
                    if diagnostic["message"]
                        .as_str()
                        .is_some_and(|m| m.starts_with("BUG"))
                    {
                        actions.push(json!({
                            "title": "Replace BUG with FIXED",
                            "kind": "quickfix",
                            "edit": {"changes": {uri.clone(): [
                                {"range": diagnostic["range"], "newText": "FIXED"},
                            ]}},
                        }));
                    }
                }
                actions.push(json!({
                    "title": "Leave a note",
                    "command": "fake.note",
                    "arguments": [uri],
                }));
                Ok(json!(actions))
            }
            "workspace/executeCommand" => {
                let target = params["arguments"][0].as_str().unwrap_or("").to_owned();
                let text = self.docs.get(&target).cloned().unwrap_or_default();
                self.next_request += 1;
                let edit = json!({
                    "jsonrpc": "2.0",
                    "id": format!("fake-{}", self.next_request),
                    "method": "workspace/applyEdit",
                    "params": {"edit": {"changes": {target: [
                        {"range": self.range(&text, 0, 0, 0), "newText": "// note\n"},
                    ]}}},
                });
                self.send(edit);
                Ok(Value::Null)
            }
            "textDocument/documentSymbol" => Ok(json!(
                self.symbols()
                    .into_iter()
                    .filter(|(symbol_uri, ..)| *symbol_uri == uri)
                    .map(|(_, line, name, len)| json!({
                        "name": name,
                        "kind": 12,
                        "range": self.range(&text, line, 0, len),
                        "selectionRange": self.range(&text, line, 3, name.chars().count()),
                    }))
                    .collect::<Vec<_>>()
            )),
            "workspace/symbol" => {
                let query = params["query"].as_str().unwrap_or("");
                let symbols = self.symbols();
                Ok(json!(
                    symbols
                        .iter()
                        .filter(|(_, _, name, _)| name.contains(query))
                        .map(|(uri, line, name, _)| json!({
                            "name": name,
                            "kind": 12,
                            "location": {
                                "uri": uri,
                                "range": self.range(&self.docs[uri], *line, 3, name.chars().count()),
                            },
                        }))
                        .collect::<Vec<_>>()
                ))
            }
            other => Err((-32601, format!("method not found: {other}"))),
        }
    }

    fn notification(&mut self, method: &str, params: &Value) {
        let uri = params["textDocument"]["uri"]
            .as_str()
            .unwrap_or("")
            .to_owned();
        match method {
            "initialized" => {
                // Ask for something, as real servers do, to see it answered.
                self.next_request += 1;
                let ask = json!({
                    "jsonrpc": "2.0",
                    "id": self.next_request,
                    "method": "workspace/configuration",
                    "params": {"items": [{"section": "fake"}]},
                });
                self.send(ask);
            }
            "textDocument/didOpen" => {
                let text = params["textDocument"]["text"].as_str().unwrap_or("");
                self.docs.insert(uri.clone(), text.to_owned());
                self.publish_diagnostics(&uri);
            }
            "textDocument/didChange" => {
                for change in params["contentChanges"].as_array().into_iter().flatten() {
                    self.apply_change(&uri, change);
                }
                self.publish_diagnostics(&uri);
            }
            "textDocument/didClose" => {
                self.docs.remove(&uri);
            }
            "exit" => std::process::exit(0),
            _ => {}
        }
    }
}

fn main() {
    let mut server = Server {
        out: io::stdout(),
        encoding: Encoding::Utf16,
        docs: BTreeMap::new(),
        next_request: 0,
    };
    let mut input = BufReader::new(io::stdin());
    while let Ok(Some(message)) = read_message(&mut input) {
        let method = message["method"].as_str().map(str::to_owned);
        let id = message.get("id").cloned();
        match (method, id) {
            (Some(method), Some(id)) => {
                let reply = match server.request(&method, &message["params"]) {
                    Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                    Err((code, text)) => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {"code": code, "message": text},
                    }),
                };
                server.send(reply);
            }
            (Some(method), None) => server.notification(&method, &message["params"]),
            // A response to something this server asked.
            _ => {}
        }
    }
}
