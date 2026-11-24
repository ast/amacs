//! Language servers: starting them, keeping them told about the text, and
//! taking in what they say.
//!
//! A buffer that visits a file can be attached to a server for its
//! language. Servers are shared by the buffers of a project. Everything is
//! asynchronous: a request is sent and the editor goes on; the answer is
//! handled when it arrives, in [`Editor::poll`].

mod edits;
mod features;

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use amacs_lsp::{Client, Encoding, Incoming, from_units, path_to_uri, to_units, uri_to_path};
use amacs_text::Text;
use serde_json::{Value as Json, json};

use crate::buffer::{BufferId, NewDiagnostic};
use crate::editor::Editor;
use crate::env::EnvState;
use crate::error::{CmdError, CmdResult};
use crate::faces::Severity;
use crate::files::abbreviate_path;
use crate::script::Value;

pub(crate) use edits::char_range;

/// Lines kept in the `*lsp-log*` buffer.
const LOG_BUFFER: &str = "*lsp-log*";

/// The position of char `pos`, as a server counts: line and offset in it.
pub(crate) fn lsp_position(text: &Text, pos: usize, encoding: Encoding) -> (usize, usize) {
    let line = text.line_of(pos);
    let prefix = text.slice(text.line_start(line)..pos);
    (line, to_units(&prefix, usize::MAX, encoding))
}

/// The char at a server's position. A position past the end of its line or
/// of the text is the end of that line or of the text.
pub(crate) fn lsp_offset(text: &Text, line: usize, units: usize, encoding: Encoding) -> usize {
    if line >= text.len_lines() {
        return text.len();
    }
    let start = text.line_start(line);
    let content = text.slice(start..text.line_end(line));
    start + from_units(&content, units, encoding)
}

/// One change to a document, with its range as it was before the change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextChange {
    pub start: (usize, usize),
    pub end: (usize, usize),
    pub text: String,
}

/// A buffer's link to a language server.
#[derive(Debug)]
pub(crate) struct Document {
    pub server: u64,
    pub uri: String,
    pub version: i64,
    pub encoding: Encoding,
    /// Whether the server has been sent the document.
    pub open: bool,
    /// Changes the server has not been told about yet.
    pub changes: Vec<TextChange>,
    /// The text was replaced wholesale, so the whole of it is sent next.
    pub resend: bool,
}

/// How to run the server for some languages.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerConfig {
    pub languages: Vec<String>,
    pub program: String,
    pub args: Vec<String>,
    /// Sent to the server as `initializationOptions`.
    pub options: Json,
}

/// What a request was for, so that its answer can be put to use.
#[derive(Debug)]
pub(crate) enum Pending {
    Initialize,
    /// A request made only to learn that the server has caught up.
    Barrier,
    Completion {
        buffer: BufferId,
        point: usize,
        manual: bool,
    },
    Hover {
        buffer: BufferId,
        point: usize,
        /// Asked for by the user, rather than shown in passing.
        explicit: bool,
    },
    Definition,
    References,
    Rename,
    /// A command the server was asked to run; only failure is of interest.
    Command,
    CodeActions,
    DocumentSymbols,
    WorkspaceSymbols,
    /// The answer does not matter.
    Ignore,
}

pub(crate) struct Server {
    pub id: u64,
    pub config: ServerConfig,
    pub root: PathBuf,
    client: Client,
    /// Whether the handshake is done.
    pub ready: bool,
    pub capabilities: Json,
    pub encoding: Encoding,
    pending: HashMap<i64, Pending>,
    /// What the server says it is busy with, such as indexing.
    progress: Option<String>,
}

impl Server {
    pub fn name(&self) -> &str {
        Path::new(&self.config.program)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&self.config.program)
    }

    /// Whether the server offers a capability such as `hoverProvider`.
    pub fn provides(&self, capability: &str) -> bool {
        !matches!(
            &self.capabilities[capability],
            Json::Null | Json::Bool(false)
        )
    }

    /// Whether changes are sent as whole documents.
    fn wants_full_text(&self) -> bool {
        let sync = &self.capabilities["textDocumentSync"];
        let kind = sync.as_i64().or_else(|| sync["change"].as_i64());
        kind != Some(2)
    }

    /// The characters after which the server wants to be asked to complete.
    pub fn completion_triggers(&self) -> Vec<char> {
        self.capabilities["completionProvider"]["triggerCharacters"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|trigger| trigger.as_str()?.chars().next())
            .collect()
    }

    pub fn request(&mut self, method: &str, params: Json, pending: Pending) {
        let id = self.client.request(method, params);
        self.pending.insert(id, pending);
    }

    pub fn notify(&self, method: &str, params: Json) {
        self.client.notify(method, params);
    }
}

type Inbox = Arc<Mutex<VecDeque<(u64, Incoming)>>>;

/// The state of all language servers.
#[derive(Default)]
pub(crate) struct LspState {
    configs: Vec<ServerConfig>,
    pub servers: Vec<Server>,
    next_id: u64,
    inbox: Inbox,
    /// Buffers whose server starts once their environment is known.
    waiting: Vec<BufferId>,
    /// When to say what is under point, after it has rested there.
    pub idle_due: Option<Instant>,
    /// Where `xref-go-back` returns to, oldest first.
    pub history: Vec<(BufferId, usize)>,
}

/// The name a language has in the protocol.
fn language_id(name: &str) -> &str {
    match name {
        "bash" => "shellscript",
        "tsx" => "typescriptreact",
        "hcl" => "terraform",
        "elisp" => "emacs-lisp",
        "make" => "makefile",
        other => other,
    }
}

fn client_capabilities() -> Json {
    json!({
        "general": {"positionEncodings": ["utf-8", "utf-32", "utf-16"]},
        "workspace": {
            "applyEdit": true,
            "workspaceEdit": {"documentChanges": true},
            "configuration": true,
            "workspaceFolders": true,
            "symbol": {},
            "executeCommand": {},
        },
        "textDocument": {
            "synchronization": {"didSave": true},
            "publishDiagnostics": {"versionSupport": true},
            "completion": {
                "completionItem": {
                    "snippetSupport": true,
                    "documentationFormat": ["plaintext", "markdown"],
                },
                "contextSupport": true,
            },
            "hover": {"contentFormat": ["plaintext", "markdown"]},
            "definition": {"linkSupport": true},
            "references": {},
            "rename": {},
            "codeAction": {
                "codeActionLiteralSupport": {"codeActionKind": {"valueSet": [
                    "quickfix", "refactor", "refactor.extract", "refactor.inline",
                    "refactor.rewrite", "source", "source.organizeImports",
                ]}},
            },
            "documentSymbol": {"hierarchicalDocumentSymbolSupport": true},
        },
        "window": {"workDoneProgress": true},
    })
}

/// The first line of text worth showing from a server's prose, which may be
/// Markdown: not a code fence, a rule or blank.
pub(crate) fn first_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("```") && *line != "---")
        .unwrap_or("")
}

/// The line of a hover that says most in the least space. Servers such as
/// rust-analyzer lead with a block that says where a thing lives and
/// follow it with one that says what it is; the second is the one wanted.
pub(crate) fn summary_line(text: &str) -> &str {
    // The first line of each fenced block.
    let mut firsts: Vec<&str> = Vec::new();
    let (mut in_block, mut taken) = (false, false);
    for line in text.lines().map(str::trim) {
        if line.starts_with("```") {
            in_block = !in_block;
            taken = false;
        } else if in_block && !taken && !line.is_empty() {
            firsts.push(line);
            taken = true;
        }
    }
    match firsts.get(1) {
        Some(second) => second,
        None => first_line(text),
    }
}

impl Editor {
    // Configuration

    /// Says which program serves some languages. A later definition for a
    /// language replaces an earlier one.
    pub fn lsp_define_server(&mut self, config: ServerConfig) {
        for existing in &mut self.lsp.configs {
            existing
                .languages
                .retain(|language| !config.languages.contains(language));
        }
        self.lsp
            .configs
            .retain(|existing| !existing.languages.is_empty());
        self.lsp.configs.push(config);
    }

    fn lsp_config_for(&self, language: &str) -> Option<&ServerConfig> {
        self.lsp
            .configs
            .iter()
            .find(|config| config.languages.iter().any(|name| name == language))
    }

    // Lifecycle

    /// Attaches the current buffer to the language server for its language,
    /// starting the server if need be. Does nothing for a buffer without a
    /// file or a language without a server.
    pub fn lsp_ensure(&mut self) -> CmdResult {
        let id = self.window().buffer;
        self.lsp_attach(id)
    }

    /// Like [`Editor::lsp_ensure`], for the user: says what happened.
    pub fn lsp_ensure_command(&mut self) -> CmdResult {
        self.lsp_ensure()?;
        let id = self.window().buffer;
        if self.buffers[id].document.is_none() && !self.lsp.waiting.contains(&id) {
            return Err(CmdError::msg(
                "No language server is set up for this buffer",
            ));
        }
        let description = self.lsp_describe();
        self.message(description);
        Ok(())
    }

    fn lsp_attach(&mut self, id: BufferId) -> CmdResult {
        let buffer = &self.buffers[id];
        if buffer.document.is_some() {
            return Ok(());
        }
        let (Some(file), Some(language)) = (buffer.file.clone(), buffer.language()) else {
            return Ok(());
        };
        // A server for a file on another host would have to run there.
        if crate::file_system::is_remote(&file) {
            return Ok(());
        }
        let Some(config) = self.lsp_config_for(language.name).cloned() else {
            return Ok(());
        };
        let directory = file.parent().unwrap_or(Path::new("/")).to_owned();
        let root = self.project_root(&directory);

        let existing = self
            .lsp
            .servers
            .iter()
            .find(|server| server.config == config && server.root == root)
            .map(|server| server.id);
        let server = match existing {
            Some(server) => server,
            None => {
                let environment = match self.environment(&directory) {
                    Some(EnvState::Loading) => {
                        // Come back when the environment is known.
                        if !self.lsp.waiting.contains(&id) {
                            self.lsp.waiting.push(id);
                        }
                        return Ok(());
                    }
                    Some(EnvState::Ready(environment)) => environment,
                    Some(EnvState::Failed(_)) | None => Vec::new(),
                };
                self.lsp_spawn(config, root, &environment)?
            }
        };
        let (ready, encoding) = self
            .lsp_server(server)
            .map_or((false, Encoding::default()), |server| {
                (server.ready, server.encoding)
            });
        self.buffers[id].document = Some(Document {
            server,
            uri: path_to_uri(&file),
            version: 0,
            encoding,
            open: false,
            changes: Vec::new(),
            resend: false,
        });
        if ready {
            self.lsp_did_open(id);
        }
        Ok(())
    }

    fn lsp_spawn(
        &mut self,
        config: ServerConfig,
        root: PathBuf,
        environment: &[(String, Option<String>)],
    ) -> CmdResult<u64> {
        self.lsp.next_id += 1;
        let id = self.lsp.next_id;
        let (inbox, waker) = (self.lsp.inbox.clone(), self.waker.clone());
        let client = Client::spawn(
            &config.program,
            &config.args,
            &root,
            environment,
            move |incoming| {
                if let Ok(mut inbox) = inbox.lock() {
                    inbox.push_back((id, incoming));
                }
                waker();
            },
        )
        .map_err(|error| {
            CmdError::msg(format!(
                "Cannot start the language server {}: {error}",
                config.program
            ))
        })?;
        let uri = path_to_uri(&root);
        let name = root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let params = json!({
            "processId": std::process::id(),
            "clientInfo": {"name": "amacs", "version": env!("CARGO_PKG_VERSION")},
            "rootPath": root.display().to_string(),
            "rootUri": uri,
            "workspaceFolders": [{"uri": uri, "name": name}],
            "initializationOptions": config.options,
            "capabilities": client_capabilities(),
        });
        let mut server = Server {
            id,
            config,
            root,
            client,
            ready: false,
            capabilities: Json::Null,
            encoding: Encoding::default(),
            pending: HashMap::new(),
            progress: None,
        };
        server.request("initialize", params, Pending::Initialize);
        self.lsp.servers.push(server);
        Ok(id)
    }

    pub(crate) fn lsp_server(&self, id: u64) -> Option<&Server> {
        self.lsp.servers.iter().find(|server| server.id == id)
    }

    pub(crate) fn lsp_server_mut(&mut self, id: u64) -> Option<&mut Server> {
        self.lsp.servers.iter_mut().find(|server| server.id == id)
    }

    /// The server of a buffer, once it is ready and has the buffer's text.
    pub(crate) fn lsp_server_of(&self, buffer: BufferId) -> Option<&Server> {
        let document = self.buffers[buffer].document.as_ref()?;
        self.lsp_server(document.server)
            .filter(|server| server.ready && document.open)
    }

    /// The server of the current buffer, or why there is none to ask.
    pub(crate) fn lsp_current_server(&mut self, capability: &str) -> CmdResult<u64> {
        let buffer = self.window().buffer;
        let Some(server) = self.lsp_server_of(buffer) else {
            return Err(CmdError::msg(if self.buffers[buffer].document.is_some() {
                "The language server is still starting"
            } else {
                "This buffer has no language server"
            }));
        };
        if !server.provides(capability) {
            return Err(CmdError::msg(format!("{} cannot do that", server.name())));
        }
        let id = server.id;
        // Whatever is asked, it is asked about the text as it is now.
        self.lsp_flush(buffer);
        Ok(id)
    }

    fn lsp_did_open(&mut self, id: BufferId) {
        let buffer = &mut self.buffers[id];
        let Some(document) = &mut buffer.document else {
            return;
        };
        let Some(server) = self
            .lsp
            .servers
            .iter()
            .find(|server| server.id == document.server)
        else {
            return;
        };
        document.encoding = server.encoding;
        document.open = true;
        document.changes.clear();
        document.resend = false;
        let (uri, version) = (document.uri.clone(), document.version);
        let language = buffer
            .language()
            .map_or("plaintext", |language| language.name);
        server.notify(
            "textDocument/didOpen",
            json!({"textDocument": {
                "uri": uri,
                "languageId": language_id(language),
                "version": version,
                "text": buffer.text().to_string(),
            }}),
        );
    }

    /// Tells the server of a buffer about the changes since it was last told.
    pub(crate) fn lsp_flush(&mut self, id: BufferId) {
        let buffer = &mut self.buffers[id];
        let Some(document) = buffer
            .document
            .as_mut()
            .filter(|document| document.open && (document.resend || !document.changes.is_empty()))
        else {
            return;
        };
        let Some(server) = self
            .lsp
            .servers
            .iter()
            .find(|server| server.id == document.server)
        else {
            return;
        };
        document.version += 1;
        let changes = std::mem::take(&mut document.changes);
        let whole = std::mem::take(&mut document.resend) || server.wants_full_text();
        let (uri, version) = (document.uri.clone(), document.version);
        let content: Vec<Json> = if whole {
            vec![json!({"text": buffer.text().to_string()})]
        } else {
            changes
                .into_iter()
                .map(|change| {
                    json!({
                        "range": {
                            "start": {"line": change.start.0, "character": change.start.1},
                            "end": {"line": change.end.0, "character": change.end.1},
                        },
                        "text": change.text,
                    })
                })
                .collect()
        };
        server.notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": uri, "version": version},
                "contentChanges": content,
            }),
        );
    }

    /// Tells every server about the changes to its buffers.
    pub(crate) fn lsp_flush_all(&mut self) {
        let changed: Vec<BufferId> = self
            .buffers
            .iter()
            .filter(|(_, buffer)| {
                buffer.document.as_ref().is_some_and(|document| {
                    document.open && (document.resend || !document.changes.is_empty())
                })
            })
            .map(|(id, _)| id)
            .collect();
        for id in changed {
            self.lsp_flush(id);
        }
    }

    pub(crate) fn lsp_did_save(&mut self, id: BufferId) {
        self.lsp_flush(id);
        if let Some(server) = self.lsp_server_of(id)
            && let Some(document) = &self.buffers[id].document
        {
            server.notify(
                "textDocument/didSave",
                json!({"textDocument": {"uri": document.uri}}),
            );
        }
    }

    /// Detaches a buffer from its server, as when the buffer is killed.
    pub(crate) fn lsp_detach(&mut self, id: BufferId) {
        if let Some(server) = self.lsp_server_of(id)
            && let Some(document) = &self.buffers[id].document
        {
            server.notify(
                "textDocument/didClose",
                json!({"textDocument": {"uri": document.uri}}),
            );
        }
        let buffer = &mut self.buffers[id];
        buffer.document = None;
        buffer.set_diagnostics(Vec::new());
        self.lsp.waiting.retain(|waiting| *waiting != id);
    }

    /// Stops a server and detaches its buffers. Returns those buffers.
    fn lsp_remove_server(&mut self, id: u64, politely: bool) -> Vec<BufferId> {
        let Some(index) = self.lsp.servers.iter().position(|server| server.id == id) else {
            return Vec::new();
        };
        let mut server = self.lsp.servers.remove(index);
        if politely {
            // The process is killed when the client is dropped, so a server
            // that ignores this is stopped all the same.
            server.request("shutdown", Json::Null, Pending::Ignore);
            server.notify("exit", Json::Null);
        }
        let attached: Vec<BufferId> = self
            .buffers
            .iter()
            .filter(|(_, buffer)| {
                buffer
                    .document
                    .as_ref()
                    .is_some_and(|document| document.server == id)
            })
            .map(|(buffer, _)| buffer)
            .collect();
        for buffer in &attached {
            let buffer = &mut self.buffers[*buffer];
            buffer.document = None;
            buffer.set_diagnostics(Vec::new());
        }
        attached
    }

    /// Stops every language server, as when the editor exits.
    pub fn lsp_shutdown_all(&mut self) {
        let ids: Vec<u64> = self.lsp.servers.iter().map(|server| server.id).collect();
        for id in ids {
            self.lsp_remove_server(id, true);
        }
    }

    /// Stops the current buffer's server.
    pub fn lsp_shutdown(&mut self) -> CmdResult {
        let server = self
            .buffer()
            .document
            .as_ref()
            .map(|document| document.server)
            .ok_or_else(|| CmdError::msg("This buffer has no language server"))?;
        self.lsp_remove_server(server, true);
        self.message("Language server stopped");
        Ok(())
    }

    /// Stops the current buffer's server and starts it again.
    pub fn lsp_restart(&mut self) -> CmdResult {
        let current = self.window().buffer;
        let attached = match self.buffers[current].document.as_ref() {
            Some(document) => self.lsp_remove_server(document.server, true),
            None => vec![current],
        };
        for buffer in attached {
            self.lsp_attach(buffer)?;
        }
        if self.buffers[current].document.is_none() && !self.lsp.waiting.contains(&current) {
            return Err(CmdError::msg(
                "No language server is set up for this buffer",
            ));
        }
        self.message("Language server restarted");
        Ok(())
    }

    // Incoming messages

    /// Takes in what the servers have sent, and does what has come due.
    pub(crate) fn lsp_poll(&mut self, now: Instant) {
        loop {
            let next = self
                .lsp
                .inbox
                .lock()
                .ok()
                .and_then(|mut inbox| inbox.pop_front());
            let Some((server, incoming)) = next else {
                break;
            };
            match incoming {
                Incoming::Message(message) => self.lsp_handle(server, &message),
                Incoming::Closed(reason) => self.lsp_closed(server, &reason),
            }
        }
        if self.lsp.idle_due.is_some_and(|due| due <= now) {
            self.lsp.idle_due = None;
            self.lsp_idle();
        }
    }

    /// Starts the servers that were waiting for their environment.
    pub(crate) fn lsp_start_waiting(&mut self) {
        for buffer in std::mem::take(&mut self.lsp.waiting) {
            if self.buffers.contains_key(buffer) {
                let started = self.lsp_attach(buffer);
                self.report(started);
            }
        }
    }

    fn lsp_closed(&mut self, id: u64, reason: &str) {
        let Some(server) = self.lsp_server(id) else {
            // It was stopped on purpose.
            return;
        };
        let name = server.name().to_owned();
        let tail = server.client.stderr_tail();
        self.lsp_remove_server(id, false);
        for line in tail.iter().rev().take(20).rev() {
            self.lsp_log(&format!("{name}: {line}"));
        }
        self.message(format!(
            "{name}: {reason}; see {LOG_BUFFER}, or M-x lsp-restart"
        ));
    }

    pub(crate) fn lsp_log(&mut self, line: &str) {
        let id = match self.find_buffer(LOG_BUFFER) {
            Some(id) => id,
            None => {
                let mut buffer = crate::buffer::Buffer::new(LOG_BUFFER);
                buffer.read_only = true;
                self.add_buffer(buffer)
            }
        };
        self.buffers[id].append_log(line);
    }

    fn lsp_handle(&mut self, server: u64, message: &Json) {
        let method = message["method"].as_str();
        let id = message.get("id").filter(|id| !id.is_null());
        match (method, id) {
            (Some(method), Some(id)) => {
                let result = self.lsp_server_request(server, method, &message["params"]);
                if let Some(server) = self.lsp_server(server) {
                    match result {
                        Ok(result) => server.client.respond(id.clone(), result),
                        Err(reason) => server.client.respond_error(id.clone(), -32601, &reason),
                    }
                }
            }
            (Some(method), None) => self.lsp_notification(server, method, &message["params"]),
            (None, Some(id)) => {
                let pending = self
                    .lsp_server_mut(server)
                    .and_then(|server| server.pending.remove(&id.as_i64()?));
                if let Some(pending) = pending {
                    self.lsp_response(server, pending, message);
                }
            }
            (None, None) => {}
        }
    }

    /// Answers a request made by a server.
    fn lsp_server_request(
        &mut self,
        server: u64,
        method: &str,
        params: &Json,
    ) -> Result<Json, String> {
        match method {
            "workspace/configuration" => {
                // No settings are kept here; servers fall back to defaults.
                let count = params["items"].as_array().map_or(0, Vec::len);
                Ok(Json::Array(vec![Json::Null; count]))
            }
            "client/registerCapability"
            | "client/unregisterCapability"
            | "window/workDoneProgress/create"
            | "window/showMessageRequest" => Ok(Json::Null),
            "workspace/workspaceFolders" => {
                let root = self
                    .lsp_server(server)
                    .map(|server| path_to_uri(&server.root));
                Ok(json!([{"uri": root, "name": ""}]))
            }
            "workspace/applyEdit" => {
                let encoding = self
                    .lsp_server(server)
                    .map_or(Encoding::default(), |server| server.encoding);
                let applied = self.lsp_apply_workspace_edit(&params["edit"], encoding);
                self.commit_undo();
                Ok(match applied {
                    Ok(_) => json!({"applied": true}),
                    Err(reason) => json!({"applied": false, "failureReason": reason}),
                })
            }
            other => Err(format!("amacs does not handle {other}")),
        }
    }

    fn lsp_notification(&mut self, server: u64, method: &str, params: &Json) {
        let name = self
            .lsp_server(server)
            .map_or(String::new(), |server| server.name().to_owned());
        match method {
            "textDocument/publishDiagnostics" => self.lsp_diagnostics(server, params),
            "window/showMessage" => {
                let text = params["message"].as_str().unwrap_or("");
                self.lsp_log(&format!("{name}: {text}"));
                // Errors and warnings are worth interrupting for.
                if params["type"].as_i64().is_some_and(|kind| kind <= 2) {
                    self.message(format!("{name}: {}", first_line(text)));
                }
            }
            "window/logMessage" => {
                let text = params["message"].as_str().unwrap_or("");
                self.lsp_log(&format!("{name}: {text}"));
            }
            "$/progress" => {
                let value = &params["value"];
                let progress = match value["kind"].as_str() {
                    Some("end") | None => None,
                    Some(_) => {
                        let title = value["title"].as_str();
                        let detail = value["message"].as_str();
                        let percent = value["percentage"].as_u64().map(|p| format!("{p}%"));
                        let parts: Vec<&str> = [title, detail, percent.as_deref()]
                            .into_iter()
                            .flatten()
                            .collect();
                        Some(parts.join(" ")).filter(|text| !text.is_empty())
                    }
                };
                if let Some(server) = self.lsp_server_mut(server) {
                    // A report without a title continues the one before.
                    if progress.is_some() || value["kind"].as_str() == Some("end") {
                        server.progress = progress;
                    }
                }
            }
            _ => {}
        }
    }

    fn lsp_diagnostics(&mut self, server: u64, params: &Json) {
        let Some(uri) = params["uri"].as_str() else {
            return;
        };
        let path = uri_to_path(uri);
        let found = self.buffers.iter().find(|(_, buffer)| {
            buffer.document.as_ref().is_some_and(|document| {
                document.server == server
                    && (document.uri == uri
                        || path.is_some() && buffer.file.as_deref() == path.as_deref())
            })
        });
        let Some((id, buffer)) = found else { return };
        let encoding = buffer
            .document
            .as_ref()
            .map_or(Encoding::default(), |document| document.encoding);
        let text = buffer.text();
        let diagnostics = params["diagnostics"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|diagnostic| {
                let severity = match diagnostic["severity"].as_i64() {
                    Some(1) | None => Severity::Error,
                    Some(2) => Severity::Warning,
                    Some(_) => Severity::Note,
                };
                NewDiagnostic {
                    range: edits::char_range(text, &diagnostic["range"], encoding),
                    severity,
                    message: diagnostic["message"].as_str().unwrap_or("").to_owned(),
                    source: diagnostic["source"].as_str().unwrap_or("").to_owned(),
                    raw: diagnostic.clone(),
                }
            })
            .collect();
        self.buffers[id].set_diagnostics(diagnostics);
    }

    fn lsp_response(&mut self, server: u64, pending: Pending, message: &Json) {
        let name = self
            .lsp_server(server)
            .map_or(String::new(), |server| server.name().to_owned());
        if let Some(error) = message.get("error").filter(|error| !error.is_null()) {
            let text = error["message"].as_str().unwrap_or("request failed");
            match pending {
                Pending::Initialize => {
                    self.lsp_remove_server(server, false);
                    self.message(format!("{name} did not start: {}", first_line(text)));
                }
                // Nobody is waiting for these.
                Pending::Barrier
                | Pending::Ignore
                | Pending::Completion { manual: false, .. }
                | Pending::Hover {
                    explicit: false, ..
                } => {}
                _ => self.message(format!("{name}: {}", first_line(text))),
            }
            return;
        }
        let result = &message["result"];
        match pending {
            Pending::Initialize => self.lsp_initialized(server, result),
            Pending::Barrier | Pending::Ignore | Pending::Command => {}
            Pending::Completion {
                buffer,
                point,
                manual,
            } => self.lsp_completion_response(server, buffer, point, manual, result),
            Pending::Hover {
                buffer,
                point,
                explicit,
            } => self.lsp_hover_response(buffer, point, explicit, result),
            Pending::Definition => self.lsp_definition_response(server, result),
            Pending::References => self.lsp_references_response(server, result),
            Pending::Rename => self.lsp_rename_response(server, result),
            Pending::CodeActions => self.lsp_code_actions_response(server, result),
            Pending::DocumentSymbols => self.lsp_document_symbols_response(server, result),
            Pending::WorkspaceSymbols => self.lsp_workspace_symbols_response(server, result),
        }
    }

    fn lsp_initialized(&mut self, id: u64, result: &Json) {
        let Some(server) = self.lsp_server_mut(id) else {
            return;
        };
        server.capabilities = result["capabilities"].clone();
        server.encoding = server.capabilities["positionEncoding"]
            .as_str()
            .and_then(Encoding::from_name)
            .unwrap_or_default();
        server.ready = true;
        server.notify("initialized", json!({}));
        let attached: Vec<BufferId> = self
            .buffers
            .iter()
            .filter(|(_, buffer)| {
                buffer
                    .document
                    .as_ref()
                    .is_some_and(|document| document.server == id)
            })
            .map(|(buffer, _)| buffer)
            .collect();
        for buffer in attached {
            self.lsp_did_open(buffer);
        }
    }

    // Requests

    /// The `textDocument` and `position` of a request about point.
    pub(crate) fn lsp_position_params(&self, buffer: BufferId, pos: usize) -> Json {
        let buffer = &self.buffers[buffer];
        let Some(document) = &buffer.document else {
            return Json::Null;
        };
        let (line, character) = lsp_position(buffer.text(), pos, document.encoding);
        json!({
            "textDocument": {"uri": document.uri},
            "position": {"line": line, "character": character},
        })
    }

    pub(crate) fn lsp_request(
        &mut self,
        server: u64,
        method: &str,
        params: Json,
        pending: Pending,
    ) {
        if let Some(server) = self.lsp_server_mut(server) {
            server.request(method, params, pending);
        }
    }

    // What the user sees

    /// What the mode line says about a buffer's server and diagnostics.
    pub(crate) fn lsp_mode_line(&self, id: BufferId) -> String {
        let buffer = &self.buffers[id];
        let mut parts = Vec::new();
        if let Some(document) = &buffer.document
            && let Some(server) = self.lsp_server(document.server)
        {
            let state = match (&server.progress, server.ready) {
                (_, false) => ": starting".to_owned(),
                (Some(progress), true) => format!(": {progress}"),
                (None, true) => String::new(),
            };
            parts.push(format!("[{}{state}]", server.name()));
        } else if self.lsp.waiting.contains(&id) {
            parts.push("[direnv…]".to_owned());
        }
        let count = |severity: Severity| {
            buffer
                .diagnostics()
                .filter(|(_, diagnostic)| diagnostic.severity == severity)
                .count()
        };
        let (errors, warnings) = (count(Severity::Error), count(Severity::Warning));
        if errors > 0 {
            parts.push(format!("E{errors}"));
        }
        if warnings > 0 {
            parts.push(format!("W{warnings}"));
        }
        parts.join(" ")
    }

    /// Says what is under point, now that it has rested there: a
    /// diagnostic, or else what the server knows about the symbol.
    fn lsp_idle(&mut self) {
        if self.frame().echo.is_some() || self.minibuffer.is_some() || self.in_minibuffer() {
            return;
        }
        let (id, point) = (self.window().buffer, self.point());
        if let Some(diagnostic) = self.buffers[id].diagnostics_at(point).first() {
            let source = match diagnostic.source.as_str() {
                "" => String::new(),
                source => format!("{source}: "),
            };
            let text = format!("{source}{}", first_line(&diagnostic.message));
            self.message(text);
            return;
        }
        if let Some(server) = self
            .lsp_server_of(id)
            .filter(|s| s.provides("hoverProvider"))
        {
            let server = server.id;
            self.lsp_flush(id);
            let params = self.lsp_position_params(id, point);
            let pending = Pending::Hover {
                buffer: id,
                point,
                explicit: false,
            };
            self.lsp_request(server, "textDocument/hover", params, pending);
        }
    }

    /// After a command: tell the servers what changed, and arrange to say
    /// what is under point if it stays there.
    pub(crate) fn lsp_after_command(&mut self) {
        self.lsp_flush_all();
        let buffer = self.buffer();
        let interesting = buffer.document.is_some() || buffer.diagnostics().next().is_some();
        let delay = match self.option("lsp-idle-delay") {
            Ok(Value::Int(delay)) => u64::try_from(delay).ok(),
            _ => Some(300),
        };
        self.lsp.idle_due = delay
            .filter(|_| interesting)
            .map(|delay| Instant::now() + Duration::from_millis(delay));
    }

    // For tests and scripts

    /// Whether anything is in flight: a server starting, a request without
    /// its answer, an environment being loaded, messages not yet handled.
    pub fn lsp_busy(&self) -> bool {
        self.environments.is_loading()
            || !self.lsp.waiting.is_empty()
            || self
                .lsp
                .servers
                .iter()
                .any(|server| !server.ready || !server.pending.is_empty())
            || self.lsp.inbox.lock().is_ok_and(|inbox| !inbox.is_empty())
    }

    /// Asks every server a question whose only point is its answer: once it
    /// comes, the server has dealt with everything sent before, and so has
    /// said everything it had to say about it.
    pub fn lsp_barrier(&mut self) {
        self.lsp_flush_all();
        for server in &mut self.lsp.servers {
            if server.ready {
                server.request("amacs/barrier", Json::Null, Pending::Barrier);
            }
        }
    }

    /// The diagnostics of the current buffer as Scheme data: lists of
    /// start, end, severity and message.
    pub fn diagnostics_value(&self) -> Value {
        Value::List(
            self.buffer()
                .diagnostics()
                .map(|(range, diagnostic)| {
                    let severity = match diagnostic.severity {
                        Severity::Error => "error",
                        Severity::Warning => "warning",
                        Severity::Note => "note",
                    };
                    Value::List(vec![
                        range.start.into(),
                        range.end.into(),
                        Value::sym(severity),
                        Value::str(diagnostic.message.clone()),
                    ])
                })
                .collect(),
        )
    }

    /// A short description of the current buffer's server, for messages.
    pub(crate) fn lsp_describe(&self) -> String {
        match self
            .buffer()
            .document
            .as_ref()
            .and_then(|document| self.lsp_server(document.server))
        {
            Some(server) => format!(
                "{} in {}{}",
                server.name(),
                abbreviate_path(&server.root),
                if server.ready { "" } else { " (starting)" }
            ),
            None => "No language server".to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_convert_both_ways() {
        let text = Text::from("aö😀b\nsecond\n");
        assert_eq!(lsp_position(&text, 3, Encoding::Utf16), (0, 4));
        assert_eq!(lsp_position(&text, 3, Encoding::Utf8), (0, 7));
        assert_eq!(lsp_position(&text, 7, Encoding::Utf16), (1, 2));
        assert_eq!(lsp_offset(&text, 0, 4, Encoding::Utf16), 3);
        assert_eq!(lsp_offset(&text, 0, 7, Encoding::Utf8), 3);
        assert_eq!(lsp_offset(&text, 1, 2, Encoding::Utf32), 7);
        assert_eq!(lsp_offset(&text, 0, 99, Encoding::Utf16), 4, "end of line");
        assert_eq!(lsp_offset(&text, 99, 0, Encoding::Utf16), text.len());
    }

    #[test]
    fn prose_is_reduced_to_a_line() {
        assert_eq!(first_line("```rust\nfn f()\n```\n---\ndocs"), "fn f()");
        assert_eq!(first_line("\n  plain  \nmore"), "plain");
        assert_eq!(first_line("```\n```"), "");
        let hover = "```rust\ncrate::m\n```\n\n```rust\nfn f(x: u8)\n    -> u8\n```\n---\ndocs";
        assert_eq!(summary_line(hover), "fn f(x: u8)");
        assert_eq!(summary_line("```c\nint x\n```\ntext"), "int x");
        assert_eq!(summary_line("just text"), "just text");
    }
}
