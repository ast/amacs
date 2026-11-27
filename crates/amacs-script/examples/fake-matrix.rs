//! The program that speaks Matrix, for tests: it speaks to the editor as
//! `amacs-matrix` does, in lines of JSON on standard input and output, and
//! to no server. It has two rooms, a space with a room in it, and a talk
//! with one person. The first message of a room has a thread, and the last
//! is a picture, whose file is the one that the environment variable
//! `FAKE_MATRIX_PICTURE` names. The user "@new:example.org" is on a device
//! that is not verified, until the emojis are said to match or a recovery
//! key is given. A message that is sent comes
//! back as one of the room, and "paginate-back" gives one earlier message,
//! once. A message of the user's own can be changed, and any can be
//! answered, reacted to and deleted; to "ping me" it tells of a message
//! that names the user. To "start" with the user "@nobody:example.org" it says that
//! nobody is logged in, and "login" then logs in.

use std::io::{self, BufRead, Write};

use serde_json::{Value, json};

fn send(message: &Value) {
    let mut out = io::stdout().lock();
    writeln!(out, "{message}").expect("the editor is reading");
    out.flush().expect("the editor is reading");
}

fn room(id: &str, name: &str, unread: u64, mentions: u64, activity: u64) {
    send(&json!({"event": "room", "room": {
        "id": id, "name": name, "unread": unread, "mentions": mentions,
        "activity": activity, "direct": false, "space": false,
    }}));
}

fn item(id: &str, sender: &str, time: u64, kind: &str, body: &str) -> Value {
    json!({
        "id": id, "sender": format!("@{}:example.org", sender.to_lowercase()),
        "sender_name": sender, "time": time, "kind": kind, "body": body, "edited": false,
    })
}

fn rooms(verified: bool) {
    send(&json!({"event": "state", "state": "syncing", "verified": verified}));
    room("!tea:example.org", "Tea", 0, 0, 100);
    room("!work:example.org", "Work", 3, 1, 200);
    send(&json!({"event": "room", "room": {
        "id": "!space:example.org", "name": "Garden club", "unread": 0, "mentions": 0,
        "activity": 300, "direct": false, "space": true,
    }}));
    send(&json!({"event": "room", "room": {
        "id": "!roses:example.org", "name": "Roses", "unread": 0, "mentions": 0,
        "activity": 50, "direct": false, "space": false,
        "parents": ["!elsewhere:example.org", "!space:example.org"],
    }}));
    send(&json!({"event": "room", "room": {
        "id": "!ada:example.org", "name": "Ada", "unread": 0, "mentions": 0,
        "activity": 10, "direct": true, "space": false, "parents": [],
    }}));
}

fn main() {
    let mut sent = 0;
    let mut earlier = true;
    for line in io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = message["id"].clone();
        let params = &message["params"];
        let room_id = params["room"].as_str().unwrap_or_default().to_owned();
        let mut result = Value::Null;
        match message["method"].as_str().unwrap_or_default() {
            "start" => {
                if params["passphrase"].as_str().is_none_or(str::is_empty) {
                    send(&json!({"id": id, "error": "The store needs a passphrase."}));
                    continue;
                }
                if params["user"] == "@nobody:example.org" {
                    send(&json!({"event": "state", "state": "login"}));
                } else {
                    rooms(params["user"] != "@new:example.org");
                }
            }
            "login" => {
                send(&json!({"event": "login-url", "url": "https://example.org/login"}));
                rooms(true);
            }
            "timeline-open" if params["thread"].is_string() => {
                let mut answer = item("$t1", "Brook", 3610, "text", "Which kettle?");
                answer["thread_root"] = params["thread"].clone();
                send(&json!({
                    "event": "timeline", "room": room_id, "thread": params["thread"],
                    "op": "reset",
                    "items": [item("$1", "Ada", 3600, "text", "Is the kettle on?"), answer],
                }));
            }
            "timeline-open" => {
                let mut first = item("$1", "Ada", 3600, "text", "Is the kettle on?");
                first["thread"] = json!({"count": 2});
                let mut picture = item("$4", "Brook", 3780, "image", "kettle.jpg");
                picture["file"] = json!({
                    "name": "kettle.jpg",
                    "thumbnail": std::env::var("FAKE_MATRIX_PICTURE").ok(),
                });
                send(&json!({
                    "event": "timeline", "room": room_id, "op": "reset",
                    "items": [
                        first,
                        item("$2", "Brook", 3660, "emote", "puts it on"),
                        item("$3", "Ada", 3720, "undecryptable", ""),
                        picture,
                    ],
                }));
            }
            "download" => result = json!(std::env::var("FAKE_MATRIX_PICTURE").ok()),
            "upload" => {
                let path = params["path"].as_str().unwrap_or_default();
                let name = path.rsplit('/').next().unwrap_or_default();
                let mut file = item("$file", "Me", 3900, "file", name);
                file["own"] = json!(true);
                file["file"] = json!({"name": name});
                send(&json!({
                    "event": "timeline", "room": room_id, "op": "append", "items": [file],
                }));
            }
            "verify-start" => send(&json!({"event": "verify-emojis", "emojis": [
                {"symbol": "D", "description": "Dog"}, {"symbol": "K", "description": "Key"},
            ]})),
            "verify-confirm" => {
                let ok = params["match"] == true;
                send(&json!({"event": "verify-done", "ok": ok, "message": if ok {
                    Value::Null
                } else {
                    json!("the emojis are not the same")
                }}));
                if ok {
                    send(&json!({"event": "state", "state": "syncing", "verified": true}));
                }
            }
            "recover" => {
                if params["key"] != "EsTc 1234" {
                    send(&json!({"id": id, "error": "The recovery key is not correct."}));
                    continue;
                }
                send(&json!({"event": "state", "state": "syncing", "verified": true}));
            }
            "send" => {
                sent += 1;
                let body = params["body"].as_str().unwrap_or_default();
                let mut own = item(&format!("$sent{sent}"), "Me", 3840, "text", body);
                own["own"] = json!(true);
                send(&json!({
                    "event": "timeline", "room": room_id, "thread": params["thread"],
                    "op": "append",
                    "items": [own],
                }));
                if body == "ping me" {
                    send(&json!({
                        "event": "notify", "room": room_id, "sender_name": "Ada",
                        "body": "me: the tea is ready",
                    }));
                }
                if body == "edit that" {
                    let mut edited = item("$1", "Ada", 3600, "text", "Is the kettle on yet?");
                    edited["edited"] = json!(true);
                    send(&json!({
                        "event": "timeline", "room": room_id, "op": "update", "items": [edited],
                    }));
                    send(&json!({
                        "event": "timeline", "room": room_id, "op": "remove", "ids": ["$2"],
                    }));
                }
                room(&room_id, "Tea", 0, 0, 400);
            }
            "paginate-back" => {
                if earlier {
                    send(&json!({
                        "event": "timeline", "room": room_id, "op": "prepend",
                        "items": [item("$0", "Brook", 60, "text", "Good morning")],
                    }));
                }
                result = json!(earlier);
                earlier = false;
            }
            "reply" => {
                let mut answer = item(
                    "$reply",
                    "Me",
                    3900,
                    "text",
                    params["body"].as_str().unwrap_or_default(),
                );
                answer["own"] = json!(true);
                answer["reply_to"] =
                    json!({"sender_name": "Ada", "body": "Is the kettle on?\nOr not?"});
                send(
                    &json!({"event": "timeline", "room": room_id, "op": "append", "items": [answer]}),
                );
            }
            "edit" => {
                let target = params["id"].as_str().unwrap_or_default();
                let mut changed = item(
                    target,
                    "Me",
                    3840,
                    "text",
                    params["body"].as_str().unwrap_or_default(),
                );
                changed["own"] = json!(true);
                changed["edited"] = json!(true);
                send(
                    &json!({"event": "timeline", "room": room_id, "op": "update", "items": [changed]}),
                );
            }
            "react" => {
                let target = params["id"].as_str().unwrap_or_default();
                let mut reacted = item(target, "Ada", 3600, "text", "Is the kettle on?");
                reacted["reactions"] = json!([
                    {"key": params["key"], "count": 2, "mine": true},
                    {"key": "tea", "count": 1, "mine": false},
                ]);
                send(
                    &json!({"event": "timeline", "room": room_id, "op": "update", "items": [reacted]}),
                );
            }
            "redact" => {
                let target = params["id"].as_str().unwrap_or_default();
                let gone = item(target, "Ada", 3600, "redacted", "");
                send(
                    &json!({"event": "timeline", "room": room_id, "op": "update", "items": [gone]}),
                );
            }
            "mark-read" => room(&room_id, "Work", 0, 0, 200),
            "timeline-close" => {}
            other => {
                send(&json!({"id": id, "error": format!("no method {other}")}));
                continue;
            }
        }
        send(&json!({"id": id, "result": result}));
    }
}
