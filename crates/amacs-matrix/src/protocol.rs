//! The lines of JSON that the editor and this program exchange.
//!
//! The editor writes requests, one object on a line of standard input:
//! `{"id": n, "method": "...", "params": {...}}`. This program answers each
//! with `{"id": n, "result": ...}` or `{"id": n, "error": "text"}`, and tells
//! what happens meanwhile with `{"event": "...", ...}`. Standard output
//! carries nothing else: what is for a person to read goes to standard error,
//! which the editor logs.

use std::io::{self, Write};
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// A request of the editor. The id goes back as it came, whatever it is, so
/// that the editor finds the answer's request.
#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    pub id: Value,
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

impl Request {
    /// The request of a line, or why the line is none.
    pub fn parse(line: &str) -> Result<Self, String> {
        let value: Value = serde_json::from_str(line).map_err(|error| error.to_string())?;
        if !value.is_object() {
            return Err("a request is an object".to_owned());
        }
        serde_json::from_value(value).map_err(|error| error.to_string())
    }

    /// The parameters as a method wants them, or a sentence for the editor.
    pub fn params<T: for<'de> Deserialize<'de>>(&self) -> Result<T, String> {
        // A request without parameters may leave them out.
        let params = if self.params.is_null() {
            json!({})
        } else {
            self.params.clone()
        };
        serde_json::from_value(params).map_err(|error| {
            format!(
                "The parameters of “{}” are not as they must be: {error}.",
                self.method
            )
        })
    }
}

/// The parameters of "start".
#[derive(Deserialize)]
pub struct StartParams {
    pub user: String,
    #[serde(default)]
    pub homeserver: Option<String>,
    #[serde(default)]
    pub passphrase: String,
}

/// The parameters of the methods that are about one room.
#[derive(Debug, Deserialize)]
pub struct RoomParams {
    pub room: String,
}

/// The parameters of "paginate-back".
#[derive(Debug, Deserialize)]
pub struct PaginateParams {
    pub room: String,
    #[serde(default)]
    pub count: Option<u64>,
}

/// The parameters of "send".
#[derive(Debug, Deserialize)]
pub struct SendParams {
    pub room: String,
    pub body: String,
}

/// What this program is doing, as the "state" event says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    // The editor sets "starting" itself when it starts this program; the
    // name is here because it is one of the protocol's.
    #[allow(dead_code)]
    Starting,
    Login,
    Syncing,
    Offline,
    Error,
}

/// A room, as the "room" event tells of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RoomInfo {
    pub id: String,
    pub name: String,
    pub unread: u64,
    pub mentions: u64,
    /// The seconds since 1970 of the latest message, and 0 while none is
    /// known.
    pub activity: u64,
    pub direct: bool,
    pub space: bool,
}

/// What a message is, which decides how the editor shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Text,
    Notice,
    Emote,
    Image,
    File,
    Redacted,
    Undecryptable,
    Membership,
    Other,
}

/// The message that another answers. The editor does not read this yet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReplyTo {
    pub event_id: String,
    pub sender: Option<String>,
    pub sender_name: Option<String>,
    pub body: Option<String>,
}

/// One reaction to a message with the number of people who gave it. The
/// editor does not read this yet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Reaction {
    pub key: String,
    pub count: u64,
    pub mine: bool,
}

/// A message or another thing that happened in a room.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Item {
    /// The name of the item for "update" and "remove". It is the SDK's own
    /// name of the item in its list, which stays when a message that is
    /// being sent becomes the message that the server has.
    pub id: String,
    pub sender: String,
    pub sender_name: Option<String>,
    /// Seconds since 1970.
    pub time: u64,
    pub kind: Kind,
    pub body: String,
    pub edited: bool,
    // The fields from here are more than the editor reads now. They are
    // left out when they have nothing to say.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    /// "sending" or "failed", for a message that the server does not have
    /// yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_state: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<ReplyTo>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reactions: Vec<Reaction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_root: Option<String>,
}

/// A change to the list of messages that the editor keeps of a room.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    /// These are all the messages now.
    Reset(Vec<Item>),
    /// These come after the last.
    Append(Vec<Item>),
    /// These come before the first.
    Prepend(Vec<Item>),
    /// These take the place of the messages with their ids.
    Update(Vec<Item>),
    /// The messages with these ids are no longer there.
    Remove(Vec<String>),
}

/// The answer to a request: its result, or a sentence that says what went
/// wrong.
pub fn answer(id: &Value, outcome: &Result<Value, String>) -> Value {
    match outcome {
        Ok(result) => json!({"id": id, "result": result}),
        Err(error) => json!({"id": id, "error": error}),
    }
}

/// The "state" event.
pub fn state_event(state: State, message: Option<&str>) -> Value {
    match message {
        Some(message) => json!({"event": "state", "state": state, "message": message}),
        None => json!({"event": "state", "state": state}),
    }
}

/// The "timeline" event of a change to the messages of a room.
pub fn timeline_event(room: &str, op: &Op) -> Value {
    let (name, key, value) = match op {
        Op::Reset(items) => ("reset", "items", json!(items)),
        Op::Append(items) => ("append", "items", json!(items)),
        Op::Prepend(items) => ("prepend", "items", json!(items)),
        Op::Update(items) => ("update", "items", json!(items)),
        Op::Remove(ids) => ("remove", "ids", json!(ids)),
    };
    json!({"event": "timeline", "room": room, "op": name, key: value})
}

/// Where the lines for the editor go. All who write share one, so that each
/// line is written whole and the lines of one writer keep their order.
#[derive(Clone)]
pub struct Output(Arc<Mutex<Box<dyn Write + Send>>>);

impl Output {
    /// The output that the editor reads.
    pub fn stdout() -> Self {
        Self::to(Box::new(io::stdout()))
    }

    /// An output to somewhere else, which tests read.
    pub fn to(writer: Box<dyn Write + Send>) -> Self {
        Self(Arc::new(Mutex::new(writer)))
    }

    fn line(&self, message: &Value) {
        let mut out = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        // When the editor is gone there is nobody to tell. The end of
        // standard input stops the program then.
        let _ = writeln!(out, "{message}").and_then(|()| out.flush());
    }

    pub fn answer(&self, id: &Value, outcome: &Result<Value, String>) {
        self.line(&answer(id, outcome));
    }

    pub fn state(&self, state: State, message: Option<&str>) {
        self.line(&state_event(state, message));
    }

    pub fn login_url(&self, url: &str) {
        self.line(&json!({"event": "login-url", "url": url}));
    }

    pub fn room(&self, room: &RoomInfo) {
        self.line(&json!({"event": "room", "room": room}));
    }

    pub fn room_removed(&self, id: &str) {
        self.line(&json!({"event": "room-removed", "id": id}));
    }

    pub fn timeline(&self, room: &str, op: &Op) {
        self.line(&timeline_event(room, op));
    }
}

/// A sentence for the editor from what was being done and an error of the
/// SDK. The SDK's errors name no tokens and no passphrase.
pub fn explain(what: &str, error: impl std::fmt::Display) -> String {
    let error = error.to_string();
    let error = error.trim().trim_end_matches('.');
    format!("{what}: {error}.")
}

/// A line for the editor's log, on standard error.
pub fn log(text: impl std::fmt::Display) {
    eprintln!("{text}");
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// An item with nothing but an id and a body, for tests.
    pub fn item(id: &str, body: &str) -> Item {
        Item {
            id: id.to_owned(),
            sender: "@ada:example.org".to_owned(),
            sender_name: Some("Ada".to_owned()),
            time: 3600,
            kind: Kind::Text,
            body: body.to_owned(),
            edited: false,
            event_id: None,
            send_state: None,
            reply_to: None,
            reactions: Vec::new(),
            thread_root: None,
        }
    }

    #[test]
    fn a_request_is_read() {
        let request =
            Request::parse(r#"{"id":3,"method":"send","params":{"room":"!a:b","body":"hi"}}"#)
                .unwrap();
        assert_eq!(request.id, json!(3));
        assert_eq!(request.method, "send");
        let params: SendParams = request.params().unwrap();
        assert_eq!((params.room.as_str(), params.body.as_str()), ("!a:b", "hi"));
    }

    #[test]
    fn start_takes_null_for_the_homeserver() {
        let request = Request::parse(
            r#"{"id":0,"method":"start","params":{"user":"@a:b","homeserver":null,"passphrase":"p"}}"#,
        )
        .unwrap();
        let params: StartParams = request.params().unwrap();
        assert_eq!(params.user, "@a:b");
        assert_eq!(params.homeserver, None);
        assert_eq!(params.passphrase, "p");
    }

    #[test]
    fn what_is_no_request_is_refused() {
        assert!(Request::parse("not json").is_err());
        assert!(Request::parse("[1, 2]").is_err());
        assert!(Request::parse(r#"{"id":1,"method":7}"#).is_err());
    }

    #[test]
    fn parameters_may_be_left_out_or_wrong() {
        let request = Request::parse(r#"{"id":1,"method":"login"}"#).unwrap();
        assert!(request.params::<Value>().is_ok());
        let request = Request::parse(r#"{"id":1,"method":"send","params":{"room":1}}"#).unwrap();
        let error = request.params::<SendParams>().unwrap_err();
        assert!(error.starts_with("The parameters of “send”"), "{error}");
    }

    #[test]
    fn answers_carry_the_id() {
        assert_eq!(
            answer(&json!(4), &Ok(json!(true))),
            json!({"id": 4, "result": true})
        );
        assert_eq!(
            answer(&json!(5), &Ok(Value::Null)),
            json!({"id": 5, "result": null})
        );
        assert_eq!(
            answer(&json!(6), &Err("No.".to_owned())),
            json!({"id": 6, "error": "No."})
        );
        // One object on one line, whatever is in it.
        let line = answer(&json!(7), &Err("two\nlines".to_owned())).to_string();
        assert!(!line.contains('\n'));
    }

    #[test]
    fn a_state_has_a_message_only_when_there_is_one() {
        assert_eq!(
            state_event(State::Login, None),
            json!({"event": "state", "state": "login"})
        );
        assert_eq!(
            state_event(State::Offline, Some("the server does not answer")),
            json!({"event": "state", "state": "offline", "message": "the server does not answer"})
        );
        assert_eq!(state_event(State::Starting, None)["state"], "starting");
        assert_eq!(state_event(State::Syncing, None)["state"], "syncing");
        assert_eq!(state_event(State::Error, None)["state"], "error");
    }

    #[test]
    fn an_item_is_as_the_editor_reads_it() {
        // The fields that the editor reads are all there, and those that it
        // does not read are left out while they are empty.
        assert_eq!(
            json!(item("7", "Is the kettle on?")),
            json!({
                "id": "7", "sender": "@ada:example.org", "sender_name": "Ada", "time": 3600,
                "kind": "text", "body": "Is the kettle on?", "edited": false,
            })
        );
        let mut more = item("8", "");
        more.kind = Kind::Undecryptable;
        more.sender_name = None;
        more.reactions = vec![Reaction {
            key: "+1".to_owned(),
            count: 2,
            mine: true,
        }];
        let value = json!(more);
        assert_eq!(value["kind"], "undecryptable");
        assert_eq!(value["sender_name"], Value::Null);
        assert_eq!(
            value["reactions"],
            json!([{"key": "+1", "count": 2, "mine": true}])
        );
    }

    #[test]
    fn timeline_events_have_items_or_ids() {
        let event = timeline_event("!a:b", &Op::Append(vec![item("1", "a")]));
        assert_eq!(event["event"], "timeline");
        assert_eq!(event["room"], "!a:b");
        assert_eq!(event["op"], "append");
        assert_eq!(event["items"][0]["id"], "1");
        for (op, name) in [
            (Op::Reset(Vec::new()), "reset"),
            (Op::Prepend(Vec::new()), "prepend"),
            (Op::Update(Vec::new()), "update"),
        ] {
            let event = timeline_event("!a:b", &op);
            assert_eq!(event["op"], name);
            assert_eq!(event["items"], json!([]));
        }
        let event = timeline_event("!a:b", &Op::Remove(vec!["1".to_owned()]));
        assert_eq!(
            event,
            json!({"event": "timeline", "room": "!a:b", "op": "remove", "ids": ["1"]})
        );
    }

    #[test]
    fn a_room_event_is_as_the_editor_reads_it() {
        let room = RoomInfo {
            id: "!tea:example.org".to_owned(),
            name: "Tea".to_owned(),
            unread: 3,
            mentions: 1,
            activity: 200,
            direct: false,
            space: false,
        };
        assert_eq!(
            json!(room),
            json!({
                "id": "!tea:example.org", "name": "Tea", "unread": 3, "mentions": 1,
                "activity": 200, "direct": false, "space": false,
            })
        );
    }

    #[test]
    fn an_explanation_is_one_sentence() {
        assert_eq!(
            explain("The login failed", "no route to host. "),
            "The login failed: no route to host."
        );
    }
}
