//! A Model Context Protocol server for amacs.
//!
//! A language model that works on or with the editor gets what a person
//! at the keyboard has, and what a programmer at a REPL has: it presses
//! keys in a frame of its own and reads the screen, as text or as a
//! picture; it evaluates Scheme in the running editor and gets values,
//! printed output, and errors with their place and the calls that led to
//! them; it asks what a name means and where it is defined, and loads
//! changed code without a restart.
//!
//! [`Mcp::handle`] takes one JSON-RPC message and gives the reply, so the
//! server can be driven over standard input and output or from a test.

mod driver;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

use driver::{Driver, Evaluated, eval_for_user};

/// The frame that the model drives, when it does not ask for another size.
const DEFAULT_SIZE: (u16, u16) = (100, 30);

/// Said before the answer of the first call after the server was replaced.
const RECONNECTED: &str = "(The amacs server was started anew since the last call. This is a \
new frame, and what was only defined or opened in the old server is gone.)";

const INSTRUCTIONS: &str = "\
amacs is an Emacs-like editor with a Rust core, extended in its own Scheme. This server \
connects you to a running amacs. You have a frame of your own, which no person sees, with \
the same buffers as the user's frames.

To use the editor: `keys` presses keys in Emacs notation and returns the screen; `type` \
types text; `screen` shows the frame again, as text or as a picture.

To inspect and change it: `eval` runs Scheme in the running editor. Everything the editor \
can do is a Scheme procedure or a command. `apropos` finds names, `describe` says what a \
name means, with its documentation, where it is defined and its source. A definition \
evaluated with `eval` takes effect at once for everything that uses the name; `load` does \
the same for a file. Nothing written in Scheme needs a rebuild or a restart.

When something goes wrong: an error from `eval` comes with its location and the calls \
that led to it. Errors in commands run by keys show in the echo area, the last row of the \
screen, and in full in `messages`. `(trace! 'name)` logs every call of a procedure with \
its arguments and result to `messages`; `(untrace! 'name)` stops that.

Strings are written with double quotes, symbols with a leading quote: (describe 'find-file). \
Positions in buffers count chars from 0; lines count from 1. Code that waits for the user \
or for a program, such as (read-string \"...\") or (run-process ...), makes `eval` answer \
at once without a value; look at the screen or at `messages` afterwards.";

pub struct Mcp {
    socket: PathBuf,
    size: (u16, u16),
    driver: Option<Driver>,
}

/// What a tool gives back: text, or a picture.
enum Answer {
    Text(String),
    /// A PNG image.
    Image(Vec<u8>),
}

fn text_of(args: &Value, name: &str) -> Result<String> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("the argument `{name}` is missing; it must be a string"))
}

fn number_of(args: &Value, name: &str) -> Option<u64> {
    args.get(name).and_then(Value::as_u64)
}

fn flag(args: &Value, name: &str) -> bool {
    args.get(name).and_then(Value::as_bool).unwrap_or(false)
}

/// A string as Scheme reads it.
fn quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let group = chunk.iter().enumerate().fold(0u32, |group, (index, byte)| {
            group | u32::from(*byte) << (16 - 8 * index)
        });
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(ALPHABET[(group >> (18 - 6 * index) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// What an evaluation came to, for a model to read.
fn report(evaluated: &Evaluated) -> Result<String> {
    let mut out = evaluated.text.clone();
    if !evaluated.output.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("--- written by the code ---\n");
        out.push_str(evaluated.output.trim_end());
    }
    if evaluated.ok {
        Ok(if out.is_empty() {
            "(no value)".to_owned()
        } else {
            out
        })
    } else {
        Err(anyhow!(out))
    }
}

fn tools() -> Value {
    let no_arguments = json!({ "type": "object", "properties": {} });
    json!([
        {
            "name": "eval",
            "description": "Evaluate Scheme in the running editor and return the value of the last form, as `write` shows it, with whatever the code wrote with `display`. An error comes back with its message, its location as file:line:column, and the calls that led to it. Definitions take effect at once. By default the code runs in your own frame; with `in_user_frame` it runs in the frame that the user last used, so that (buffer-name), (point) and the like are about what the user is looking at.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "code": { "type": "string", "description": "One or more Scheme forms." },
                    "in_user_frame": { "type": "boolean", "description": "Run in the user's frame instead of yours. Default false." }
                },
                "required": ["code"]
            }
        },
        {
            "name": "keys",
            "description": "Press keys in your frame, in Emacs notation separated by spaces, and return the screen afterwards. Examples: \"C-x C-f\", \"M-x\", \"RET\", \"C-g\", \"<down>\", \"a b c\". Use `type` for text. What a command does after waiting for a program (git, a formatter) may not be on the screen yet: give `wait_for` to wait until the screen shows some text.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "keys": { "type": "string" },
                    "wait_for": { "type": "string", "description": "Text that the screen must show before returning." },
                    "timeout_ms": { "type": "integer", "description": "How long to wait for `wait_for`. Default 3000." }
                },
                "required": ["keys"]
            }
        },
        {
            "name": "type",
            "description": "Type text in your frame, a key for each char (a newline is RET), and return the screen afterwards. For filling in the minibuffer or inserting into a buffer as a user would.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": { "type": "string" },
                    "wait_for": { "type": "string", "description": "Text that the screen must show before returning." },
                    "timeout_ms": { "type": "integer" }
                },
                "required": ["text"]
            }
        },
        {
            "name": "screen",
            "description": "Show your frame as it is now: each row of text with its number, and where the cursor is. The last row is the echo area, where messages and prompts appear; the row above it is the mode line of the lowest window. With `image`, return a picture of the frame as a window would draw it, with colours and faces, instead of text.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "image": { "type": "boolean", "description": "Return a PNG picture instead of text. Default false." },
                    "wait_for": { "type": "string", "description": "Text that the screen must show before returning." },
                    "timeout_ms": { "type": "integer" }
                }
            }
        },
        {
            "name": "resize",
            "description": "Change the size of your frame, in columns and rows of characters. It starts at 100 by 30.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "cols": { "type": "integer" },
                    "rows": { "type": "integer" }
                },
                "required": ["cols", "rows"]
            }
        },
        {
            "name": "describe",
            "description": "Say what a name means in the editor: a procedure with how it is called, its documentation and where it is defined; a command; a macro; a variable with its value; an option with its value and documentation. With `source`, also the text of its definition, if it is written in Scheme.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "source": { "type": "boolean", "description": "Include the source text. Default false." }
                },
                "required": ["name"]
            }
        },
        {
            "name": "apropos",
            "description": "Find what the editor has by part of a name: procedures, macros, variables and commands whose names contain the text, each with the first line of its documentation.",
            "inputSchema": {
                "type": "object",
                "properties": { "pattern": { "type": "string" } },
                "required": ["pattern"]
            }
        },
        {
            "name": "messages",
            "description": "The end of the editor's message log: what was shown in the echo area, the full reports of errors in commands with their traces, what code wrote with `display`, and the output of `trace!`.",
            "inputSchema": {
                "type": "object",
                "properties": { "lines": { "type": "integer", "description": "How many lines from the end. Default 40." } }
            }
        },
        {
            "name": "buffers",
            "description": "List the buffers: name, major mode, the file it visits if any, and whether it has unsaved changes.",
            "inputSchema": no_arguments
        },
        {
            "name": "read_buffer",
            "description": "The text of a buffer, or some of its lines, with line numbers.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "The buffer's name, as `buffers` lists it." },
                    "start_line": { "type": "integer", "description": "First line, counting from 1. Default 1." },
                    "end_line": { "type": "integer", "description": "Last line. Default: the last of the buffer." }
                },
                "required": ["name"]
            }
        },
        {
            "name": "open_file",
            "description": "Visit a file in your frame, optionally going to a line, and return the screen. With `in_user_frame`, show it to the user instead.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "line": { "type": "integer" },
                    "in_user_frame": { "type": "boolean" }
                },
                "required": ["path"]
            }
        },
        {
            "name": "load",
            "description": "Read a file of Scheme and run everything in it in the running editor, as after editing a file under runtime/ or a file of your own. Definitions replace the ones in force; nothing needs a restart. Returns the last value, or the error with its place in the file.",
            "inputSchema": {
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }
        },
        {
            "name": "run_tests",
            "description": "Run tests written in Scheme with `deftest`. With `path`, load that file of tests first. Returns the number of tests that passed, and the name and message of each that failed.",
            "inputSchema": {
                "type": "object",
                "properties": { "path": { "type": "string" } }
            }
        }
    ])
}

impl Mcp {
    /// A server for the amacs that listens on a socket. Nothing is
    /// connected until a tool needs it.
    pub fn new(socket: PathBuf) -> Self {
        Self {
            socket,
            size: DEFAULT_SIZE,
            driver: None,
        }
    }

    /// Handles one message of the protocol. Returns the reply, or `None`
    /// for a notification, which has none.
    pub fn handle(&mut self, message: &Value) -> Option<Value> {
        let id = message.get("id").cloned();
        let method = message.get("method").and_then(Value::as_str).unwrap_or("");
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => Ok(json!({
                "protocolVersion": params
                    .get("protocolVersion")
                    .cloned()
                    .unwrap_or_else(|| json!("2025-06-18")),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "amacs", "version": env!("CARGO_PKG_VERSION") },
                "instructions": INSTRUCTIONS,
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools() })),
            "tools/call" => Ok(self.call(&params)),
            _ => Err(format!("there is no method {method}")),
        };
        // A message without an id is a notification and gets no answer.
        let id = id?;
        Some(match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(message) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": message },
            }),
        })
    }

    /// Runs a tool and wraps what it gives as the protocol wants it.
    fn call(&mut self, params: &Value) -> Value {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        // A connection from an earlier call may be to a server that has
        // gone since, as after a restart. Asking it for nothing tells, and
        // the tool then connects again.
        let reconnected = self
            .driver
            .as_mut()
            .is_some_and(|driver| driver.sync().is_err());
        if reconnected {
            self.driver = None;
        }
        let answer = self.tool(name, &args);
        // A connection lost during the call is left for the next one.
        if self.driver.as_ref().is_some_and(|driver| driver.lost) {
            self.driver = None;
        }
        match answer {
            Ok(Answer::Text(text)) => json!({
                "content": [{
                    "type": "text",
                    "text": if reconnected { format!("{RECONNECTED}\n{text}") } else { text },
                }],
                "isError": false,
            }),
            Ok(Answer::Image(png)) => json!({
                "content": [{ "type": "image", "data": base64(&png), "mimeType": "image/png" }],
                "isError": false,
            }),
            Err(error) => json!({
                "content": [{ "type": "text", "text": format!("{error:#}") }],
                "isError": true,
            }),
        }
    }

    fn driver(&mut self) -> Result<&mut Driver> {
        if self.driver.is_none() {
            self.driver = Some(Driver::open(&self.socket, self.size.0, self.size.1)?);
        }
        Ok(self.driver.as_mut().expect("opened above"))
    }

    /// Evaluates Scheme whose value is a string, or that writes text, and
    /// gives the text as it is rather than as Scheme writes it.
    fn display(&mut self, source: &str) -> Result<String> {
        let evaluated = self.driver()?.eval(&format!("(display {source})"))?;
        if evaluated.ok {
            Ok(evaluated.output)
        } else {
            Err(anyhow!(evaluated.text))
        }
    }

    /// Waits for the screen to show a text, if the arguments ask for it,
    /// and gives the screen.
    fn screen_after(&mut self, args: &Value) -> Result<Answer> {
        let timeout = Duration::from_millis(number_of(args, "timeout_ms").unwrap_or(3000));
        let driver = self.driver()?;
        let mut note = String::new();
        if let Some(wanted) = args.get("wait_for").and_then(Value::as_str)
            && !driver.wait_for(wanted, timeout)?
        {
            note = format!(
                "\n(the screen did not come to show {wanted:?} within {} ms)",
                timeout.as_millis()
            );
        }
        Ok(Answer::Text(format!("{}{note}", driver.screen_text())))
    }

    fn tool(&mut self, name: &str, args: &Value) -> Result<Answer> {
        match name {
            "eval" => {
                let code = text_of(args, "code")?;
                let evaluated = if flag(args, "in_user_frame") {
                    eval_for_user(&self.socket, &code)?
                } else {
                    self.driver()?.eval(&code)?
                };
                report(&evaluated).map(Answer::Text)
            }
            "keys" => {
                let keys = text_of(args, "keys")?;
                self.driver()?.keys(&keys)?;
                self.screen_after(args)
            }
            "type" => {
                let text = text_of(args, "text")?;
                self.driver()?.type_text(&text)?;
                self.screen_after(args)
            }
            "screen" => {
                if flag(args, "image") {
                    let timeout =
                        Duration::from_millis(number_of(args, "timeout_ms").unwrap_or(3000));
                    let driver = self.driver()?;
                    if let Some(wanted) = args.get("wait_for").and_then(Value::as_str) {
                        driver.wait_for(wanted, timeout)?;
                    }
                    return driver.screen_png().map(Answer::Image);
                }
                self.driver()?.sync()?;
                self.screen_after(args)
            }
            "resize" => {
                let size = |name: &str| {
                    number_of(args, name)
                        .and_then(|n| u16::try_from(n).ok())
                        .filter(|n| (4..=500).contains(n))
                        .ok_or_else(|| anyhow!("`{name}` must be a number from 4 to 500"))
                };
                let (cols, rows) = (size("cols")?, size("rows")?);
                self.size = (cols, rows);
                self.driver()?.resize(cols, rows)?;
                self.screen_after(args)
            }
            "describe" => {
                let name = text_of(args, "name")?;
                let symbol = format!("(string->symbol {})", quoted(&name));
                let mut out = self.display(&format!("(describe-fully {symbol})"))?;
                if flag(args, "source") {
                    let source = self.display(&format!(
                        "(let ((value (global-ref {symbol} #f)))
                           (or (and (procedure? value) (procedure-source value)) \"\"))"
                    ))?;
                    if source.is_empty() {
                        out.push_str("\n\n(no source: it is not a procedure written in Scheme)");
                    } else {
                        out.push_str("\n\n");
                        out.push_str(&source);
                    }
                }
                Ok(Answer::Text(out))
            }
            "apropos" => {
                let pattern = text_of(args, "pattern")?;
                let found = self.display(&format!("(apropos-lines {})", quoted(&pattern)))?;
                Ok(Answer::Text(if found.is_empty() {
                    format!("nothing has {pattern:?} in its name")
                } else {
                    found
                }))
            }
            "messages" => {
                let lines = number_of(args, "lines").unwrap_or(40);
                let log = self.display(&format!("(messages {lines})"))?;
                Ok(Answer::Text(if log.is_empty() {
                    "(the message log is empty)".to_owned()
                } else {
                    log
                }))
            }
            "buffers" => self.display("(buffer-lines)").map(Answer::Text),
            "read_buffer" => {
                let name = text_of(args, "name")?;
                let text = self.display(&format!("(buffer-string {})", quoted(&name)))?;
                let first = number_of(args, "start_line").unwrap_or(1).max(1) as usize;
                let last = number_of(args, "end_line").map_or(usize::MAX, |n| n as usize);
                let numbered: Vec<String> = text
                    .lines()
                    .enumerate()
                    .map(|(index, line)| (index + 1, line))
                    .filter(|(number, _)| (first..=last).contains(number))
                    .map(|(number, line)| format!("{number:>5}|{line}"))
                    .collect();
                Ok(Answer::Text(if numbered.is_empty() {
                    "(no lines there)".to_owned()
                } else {
                    numbered.join("\n")
                }))
            }
            "open_file" => {
                let path = text_of(args, "path")?;
                let line = number_of(args, "line")
                    .map_or_else(String::new, |line| format!(" (goto-line {line})"));
                let code = format!("(find-file {}){line}", quoted(&path));
                if flag(args, "in_user_frame") {
                    let evaluated = eval_for_user(&self.socket, &code)?;
                    if !evaluated.ok {
                        bail!(evaluated.text);
                    }
                    return Ok(Answer::Text(format!("{path} is shown in the user's frame")));
                }
                let evaluated = self.driver()?.eval(&code)?;
                if !evaluated.ok {
                    bail!(evaluated.text);
                }
                self.driver()?.sync()?;
                self.screen_after(args)
            }
            "load" => {
                let path = text_of(args, "path")?;
                let evaluated = self.driver()?.eval(&format!("(load {})", quoted(&path)))?;
                report(&evaluated).map(Answer::Text)
            }
            "run_tests" => {
                if let Some(path) = args.get("path").and_then(Value::as_str) {
                    let loaded = self.driver()?.eval(&format!("(load {})", quoted(path)))?;
                    if !loaded.ok {
                        bail!(loaded.text);
                    }
                }
                self.display("(test-report)").map(Answer::Text)
            }
            other => bail!("there is no tool {other}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_become_base64() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xff, 0xfe, 0xfd, 0xfc]), "//79/A==");
    }

    #[test]
    fn strings_are_written_for_scheme() {
        assert_eq!(quoted("a \"b\"\n\\"), "\"a \\\"b\\\"\\n\\\\\"");
    }

    #[test]
    fn the_protocol_is_answered_without_an_editor() {
        let mut mcp = Mcp::new(PathBuf::from("/nonexistent/socket"));
        let reply = mcp
            .handle(&json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
                             "params": { "protocolVersion": "2025-03-26" } }))
            .unwrap();
        assert_eq!(reply["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(reply["result"]["serverInfo"]["name"], "amacs");
        assert!(
            reply["result"]["instructions"]
                .as_str()
                .unwrap()
                .contains("eval")
        );
        // Notifications get no answer.
        assert!(
            mcp.handle(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
                .is_none()
        );
        let reply = mcp
            .handle(&json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }))
            .unwrap();
        let tools = reply["result"]["tools"].as_array().unwrap();
        assert!(tools.len() >= 10);
        for tool in tools {
            assert!(tool["description"].as_str().unwrap().len() > 40, "{tool}");
            assert_eq!(tool["inputSchema"]["type"], "object");
        }
        let reply = mcp
            .handle(&json!({ "jsonrpc": "2.0", "id": 3, "method": "no/such" }))
            .unwrap();
        assert_eq!(reply["error"]["code"], -32601);
        // A tool that is asked for wrongly says so as its result.
        let reply = mcp
            .handle(&json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call",
                             "params": { "name": "eval", "arguments": {} } }))
            .unwrap();
        assert_eq!(reply["result"]["isError"], true);
        assert!(
            reply["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("`code` is missing")
        );
    }
}
