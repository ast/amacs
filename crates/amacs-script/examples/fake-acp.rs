//! A coding agent for tests. It speaks the Agent Client Protocol as the
//! editor uses it: lines of JSON-RPC on standard input and output.
//!
//! It answers `initialize` and `session/new`, tells of two slash commands,
//! and lets its mode and its model be set. To a prompt that starts with
//! "ask" it asks leave for a tool call and says which answer it got; to one
//! that starts with "run" it has the editor run a program and says how that
//! ended. To every other prompt it gives the same turn: a thought, a
//! message that repeats the prompt, a tool call that changes a file and
//! shows the change, a plan, and the end.

use std::io::{self, BufRead, Lines, StdinLock, Write};

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

/// Asks the editor for something and waits for its answer, the `result` or
/// the `error` of it. What else the editor sends meanwhile is dropped.
fn ask(input: &mut Lines<StdinLock>, method: &str, params: &Value) -> Value {
    let id = format!("agent-{method}");
    send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    for line in input.by_ref() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if message["id"] == json!(id) && message.get("method").is_none() {
            return message;
        }
    }
    Value::Null
}

/// A turn in which leave is asked for a tool call.
fn asking(input: &mut Lines<StdinLock>) {
    update(&json!({
        "sessionUpdate": "tool_call",
        "toolCallId": "call-ask",
        "title": "Remove the build",
        "kind": "execute",
        "status": "pending",
    }));
    let answer = ask(
        input,
        "session/request_permission",
        &json!({
            "sessionId": "fake",
            "toolCall": {"toolCallId": "call-ask"},
            "options": [
                {"optionId": "allow-once", "name": "Yes", "kind": "allow_once"},
                {"optionId": "allow-always", "name": "Yes, and do not ask again", "kind": "allow_always"},
                {"optionId": "allow-here", "name": "Yes, for this directory", "kind": "allow_always"},
                {"optionId": "reject", "name": "No", "kind": "reject_once"},
            ],
        }),
    );
    let outcome = &answer["result"]["outcome"];
    let chosen = outcome["optionId"].as_str().unwrap_or("nothing");
    let allowed = chosen.starts_with("allow");
    update(&json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": "call-ask",
        "status": if allowed { "completed" } else { "failed" },
    }));
    text("agent_message_chunk", &format!("You chose: {chosen}"));
}

/// A turn in which the editor runs a program for the agent.
fn running(input: &mut Lines<StdinLock>) {
    let created = ask(
        input,
        "terminal/create",
        &json!({
            "sessionId": "fake",
            "command": "sh",
            "args": ["-c", "echo \"$GREETING from $(basename \"$PWD\")\"; echo oops >&2; exit 3"],
            "env": [{"name": "GREETING", "value": "hello"}],
            "outputByteLimit": 1000,
        }),
    );
    let Some(terminal) = created["result"]["terminalId"].as_str() else {
        text("agent_message_chunk", &format!("No terminal: {created}"));
        return;
    };
    update(&json!({
        "sessionUpdate": "tool_call",
        "toolCallId": "call-run",
        "title": "Run the greeting",
        "kind": "execute",
        "status": "in_progress",
        "content": [{"type": "terminal", "terminalId": terminal}],
    }));
    let params = json!({"sessionId": "fake", "terminalId": terminal});
    let exit = ask(input, "terminal/wait_for_exit", &params);
    let output = ask(input, "terminal/output", &params);
    update(&json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": "call-run",
        "status": "failed",
    }));
    ask(input, "terminal/release", &params);
    let gone = ask(input, "terminal/output", &params);
    text(
        "agent_message_chunk",
        &format!(
            "It ended with {} and wrote {} lines. Truncated: {}. Released: {}.",
            exit["result"]["exitCode"],
            output["result"]["output"]
                .as_str()
                .unwrap_or("")
                .lines()
                .count(),
            output["result"]["truncated"],
            gone.get("error").is_some(),
        ),
    );
}

fn options(mode: &str, model: &str) -> Value {
    json!([
        {
            "id": "mode", "name": "Mode", "category": "mode", "type": "select",
            "currentValue": mode,
            "options": [
                {"value": "default", "name": "Default", "description": "Asks before it changes anything"},
                {"value": "plan", "name": "Plan", "description": "Plans, and changes nothing"},
            ],
        },
        {
            "id": "model", "name": "Model", "category": "model", "type": "select",
            "currentValue": model,
            "options": [
                {"group": "fake", "name": "Fakes", "options": [
                    {"value": "fake-model", "name": "Fake"},
                    {"value": "faker-model", "name": "Faker"},
                ]},
            ],
        },
    ])
}

fn main() {
    let mut mode = String::from("default");
    let mut model = String::from("fake-model");
    let mut input = io::stdin().lock().lines();
    while let Some(line) = input.next() {
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
            "session/new" => {
                send(&json!({"jsonrpc": "2.0", "id": id, "result": {
                    "sessionId": "fake",
                    "modes": {"currentModeId": mode},
                    "configOptions": options(&mode, &model),
                }}));
                update(&json!({
                    "sessionUpdate": "available_commands_update",
                    "availableCommands": [
                        {"name": "compact", "description": "Make the conversation shorter"},
                        {"name": "review", "description": "Review a change",
                         "input": {"hint": "which change"}},
                    ],
                }));
                continue;
            }
            "session/set_config_option" => {
                let value = message["params"]["value"].as_str().unwrap_or("").to_owned();
                match message["params"]["configId"].as_str() {
                    Some("mode") => mode = value,
                    Some("model") => model = value,
                    _ => {}
                }
                json!({"configOptions": options(&mode, &model)})
            }
            "session/prompt" => {
                let prompt = message["params"]["prompt"][0]["text"]
                    .as_str()
                    .unwrap_or("");
                if prompt.starts_with("ask") {
                    asking(&mut input);
                } else if prompt.starts_with("run") {
                    running(&mut input);
                } else {
                    turn(prompt);
                }
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
