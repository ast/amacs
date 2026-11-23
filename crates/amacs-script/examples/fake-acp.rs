//! A coding agent for tests. It speaks the Agent Client Protocol as the
//! editor uses it: lines of JSON-RPC on standard input and output.
//!
//! It answers `initialize` and `session/new`, and to every prompt it gives
//! the same turn: a thought, a message that repeats the prompt, a tool call
//! that changes a file and shows the change, a plan, and the end.

use std::io::{self, BufRead, Write};

use serde_json::{Value, json};

fn send(message: &Value) {
    let mut out = io::stdout().lock();
    writeln!(out, "{message}").expect("the editor is reading");
    out.flush().expect("the editor is reading");
}

fn update(update: &Value) {
    send(&json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {"sessionId": "fake", "update": update},
    }));
}

fn text(kind: &str, text: &str) {
    update(&json!({"sessionUpdate": kind, "content": {"type": "text", "text": text}}));
}

fn turn(prompt: &str) {
    text("agent_thought_chunk", "Let me look ");
    text("agent_thought_chunk", "at the notes.");
    text("agent_message_chunk", "You said: ");
    text(
        "agent_message_chunk",
        &format!("{prompt}\nI will change the notes."),
    );
    update(&json!({
        "sessionUpdate": "tool_call",
        "toolCallId": "call-1",
        "title": "Edit notes.txt",
        "kind": "edit",
        "status": "pending",
        "locations": [{"path": "notes.txt", "line": 2}],
    }));
    update(&json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": "call-1",
        "status": "completed",
        "content": [{
            "type": "diff",
            "path": "notes.txt",
            "oldText": "one\ntwo\nthree\n",
            "newText": "one\n2\nthree\n",
        }],
    }));
    update(&json!({
        "sessionUpdate": "plan",
        "entries": [
            {"content": "Read the notes", "priority": "high", "status": "completed"},
            {"content": "Tell about it", "priority": "low", "status": "pending"},
        ],
    }));
    text("agent_message_chunk", "Done.");
    update(&json!({"sessionUpdate": "usage_update", "used": 28389, "size": 1_000_000}));
}

fn main() {
    for line in io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(id) = message.get("id").cloned() else {
            // A notification, such as session/cancel: nothing to answer.
            continue;
        };
        let result = match message["method"].as_str().unwrap_or("") {
            "initialize" => json!({
                "protocolVersion": 1,
                "agentCapabilities": {},
                "authMethods": [],
            }),
            "session/new" => json!({
                "sessionId": "fake",
                "modes": {"currentModeId": "default"},
                "configOptions": [
                    {"id": "mode", "category": "mode", "currentValue": "default"},
                    {"id": "model", "category": "model", "currentValue": "fake-model"},
                ],
            }),
            "session/prompt" => {
                let prompt = message["params"]["prompt"][0]["text"]
                    .as_str()
                    .unwrap_or("");
                turn(prompt);
                json!({"stopReason": "end_turn"})
            }
            method => {
                send(&json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {"code": -32601, "message": format!("no {method} here")},
                }));
                continue;
            }
        };
        send(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }
}
