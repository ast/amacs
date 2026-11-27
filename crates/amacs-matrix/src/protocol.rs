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

/// The parameters of the methods that are about one room, or one thread in
/// it.
#[derive(Debug, Deserialize)]
pub struct RoomParams {
    pub room: String,
    #[serde(default)]
    pub thread: Option<String>,
}

/// The parameters of "paginate-back".
#[derive(Debug, Deserialize)]
pub struct PaginateParams {
    pub room: String,
    #[serde(default)]
    pub thread: Option<String>,
    #[serde(default)]
    pub count: Option<u64>,
}

/// The parameters of "send".
#[derive(Debug, Deserialize)]
pub struct SendParams {
    pub room: String,
    #[serde(default)]
    pub thread: Option<String>,
    pub body: String,
}

/// The parameters of the methods that are about one message: "redact" and
/// "download". The message is one of the room's list, or of the thread's
/// when the editor asks from the buffer of a thread.
#[derive(Debug, Deserialize)]
pub struct MessageParams {
    pub room: String,
    #[serde(default)]
    pub thread: Option<String>,
    pub id: String,
}

/// The parameters of "reply" and "edit".
#[derive(Debug, Deserialize)]
pub struct BodyParams {
    pub room: String,
    #[serde(default)]
    pub thread: Option<String>,
    pub id: String,
    pub body: String,
}

/// The parameters of "react".
#[derive(Debug, Deserialize)]
pub struct ReactParams {
    pub room: String,
    #[serde(default)]
    pub thread: Option<String>,
    pub id: String,
    pub key: String,
}

/// The parameters of "upload".
#[derive(Debug, Deserialize)]
pub struct UploadParams {
    pub room: String,
    #[serde(default)]
    pub thread: Option<String>,
    pub path: String,
}

/// The parameters of "verify-confirm".
#[derive(Debug, Deserialize)]
pub struct ConfirmParams {
    #[serde(rename = "match")]
    pub matches: bool,
}

/// The parameters of "recover". There is no `Debug` here: the key is a
/// secret.
#[derive(Deserialize)]
pub struct RecoverParams {
    pub key: String,
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
    /// The ids of the spaces that the room is in, of those that the user
    /// has joined, in order.
    pub parents: Vec<String>,
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

/// The message that another answers. The editor reads the name and the
/// body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReplyTo {
    pub event_id: String,
    pub sender: Option<String>,
    pub sender_name: Option<String>,
    pub body: String,
}

/// One reaction to a message with the number of people who gave it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Reaction {
    pub key: String,
    pub count: u64,
    pub mine: bool,
}

/// The answers that a message has in a thread of its own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ThreadInfo {
    pub count: u64,
}

/// The file of a message that is a picture or another file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FileInfo {
    pub name: String,
    /// A picture on disk to show for the message, once it is fetched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
    /// The name under which the picture to show is fetched and kept. It is
    /// for this program only.
    #[serde(skip)]
    pub thumbnail_key: Option<String>,
}

/// A message or another thing that happened in a room.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Item {
    /// The name of the item for "update" and "remove", and for the methods
    /// that are about one message. It is the SDK's own name of the item in
    /// its list, which stays when a message that is being sent becomes the
    /// message that the server has.
    pub id: String,
    pub sender: String,
    pub sender_name: Option<String>,
    /// Seconds since 1970.
    pub time: u64,
    pub kind: Kind,
    pub body: String,
    pub edited: bool,
    /// Whether the user of this program sent it.
    pub own: bool,
    // The fields from here are left out when they have nothing to say.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<ReplyTo>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reactions: Vec<Reaction>,
    /// On the message that a thread starts with, when it has answers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread: Option<ThreadInfo>,
    /// On an answer in a thread: what to give as `thread` to see the
    /// thread.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<FileInfo>,
    // These two are more than the editor reads.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    /// "sending" or "failed", for a message that the server does not have
    /// yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_state: Option<&'static str>,
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

/// The "state" event. `verified` is left out while it is not known whether
/// the device is verified, which the editor takes as no reason to say
/// anything.
pub fn state_event(state: State, message: Option<&str>, verified: Option<bool>) -> Value {
    let mut event = json!({"event": "state", "state": state});
    if let Some(message) = message {
        event["message"] = json!(message);
    }
    if let Some(verified) = verified {
        event["verified"] = json!(verified);
    }
    event
}

/// The "timeline" event of a change to the messages of a room, or of a
/// thread in it.
pub fn timeline_event(room: &str, thread: Option<&str>, op: &Op) -> Value {
    let (name, key, value) = match op {
        Op::Reset(items) => ("reset", "items", json!(items)),
        Op::Append(items) => ("append", "items", json!(items)),
        Op::Prepend(items) => ("prepend", "items", json!(items)),
        Op::Update(items) => ("update", "items", json!(items)),
        Op::Remove(ids) => ("remove", "ids", json!(ids)),
    };
    let mut event = json!({"event": "timeline", "room": room, "op": name, key: value});
    if let Some(thread) = thread {
        event["thread"] = json!(thread);
    }
    event
}

/// One of the emojis that two devices compare.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Emoji {
    pub symbol: String,
    pub description: String,
}

/// What the "state" events say, kept so that the last one can be said again
/// when only the verification changes.
#[derive(Default)]
struct Told {
    state: Option<(State, Option<String>)>,
    verified: Option<bool>,
}

struct Shared {
    writer: Mutex<Box<dyn Write + Send>>,
    told: Mutex<Told>,
}

/// Where the lines for the editor go. All who write share one, so that each
/// line is written whole and the lines of one writer keep their order.
#[derive(Clone)]
pub struct Output(Arc<Shared>);

impl Output {
    /// The output that the editor reads.
    pub fn stdout() -> Self {
        Self::to(Box::new(io::stdout()))
    }

    /// An output to somewhere else, which tests read.
    pub fn to(writer: Box<dyn Write + Send>) -> Self {
        Self(Arc::new(Shared {
            writer: Mutex::new(writer),
            told: Mutex::default(),
        }))
    }

    fn line(&self, message: &Value) {
        let mut out = self.0.writer.lock().unwrap_or_else(PoisonError::into_inner);
        // When the editor is gone there is nobody to tell. The end of
        // standard input stops the program then.
        let _ = writeln!(out, "{message}").and_then(|()| out.flush());
    }

    pub fn answer(&self, id: &Value, outcome: &Result<Value, String>) {
        self.line(&answer(id, outcome));
    }

    /// Tells the state. Whether the device is verified goes with it.
    pub fn state(&self, state: State, message: Option<&str>) {
        let mut told = self.0.told.lock().unwrap_or_else(PoisonError::into_inner);
        told.state = Some((state, message.map(ToOwned::to_owned)));
        // The line is written under the lock, so that the states reach the
        // editor in the order in which they were set.
        self.line(&state_event(state, message, told.verified));
    }

    /// Notes whether the device is verified, or that this is not known.
    /// When that is news, the editor is told with the state that it was
    /// told last.
    pub fn set_verified(&self, verified: Option<bool>) {
        let mut told = self.0.told.lock().unwrap_or_else(PoisonError::into_inner);
        if told.verified == verified {
            return;
        }
        told.verified = verified;
        if let (Some(_), Some((state, message))) = (verified, &told.state) {
            self.line(&state_event(*state, message.as_deref(), verified));
        }
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

    pub fn timeline(&self, room: &str, thread: Option<&str>, op: &Op) {
        self.line(&timeline_event(room, thread, op));
    }

    pub fn notify(&self, room: &str, sender_name: Option<&str>, body: &str) {
        self.line(&json!({
            "event": "notify", "room": room, "sender_name": sender_name, "body": body,
        }));
    }

    pub fn verify_request(&self) {
        self.line(&json!({"event": "verify-request"}));
    }

    pub fn verify_emojis(&self, emojis: &[Emoji]) {
        self.line(&json!({"event": "verify-emojis", "emojis": emojis}));
    }

    pub fn verify_done(&self, ok: bool, message: Option<&str>) {
        let mut event = json!({"event": "verify-done", "ok": ok});
        if let Some(message) = message {
            event["message"] = json!(message);
        }
        self.line(&event);
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
            own: false,
            reply_to: None,
            reactions: Vec::new(),
            thread: None,
            thread_root: None,
            file: None,
            event_id: None,
            send_state: None,
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
            state_event(State::Login, None, None),
            json!({"event": "state", "state": "login"})
        );
        assert_eq!(
            state_event(State::Offline, Some("the server does not answer"), None),
            json!({"event": "state", "state": "offline", "message": "the server does not answer"})
        );
        assert_eq!(
            state_event(State::Starting, None, None)["state"],
            "starting"
        );
        assert_eq!(state_event(State::Syncing, None, None)["state"], "syncing");
        assert_eq!(state_event(State::Error, None, None)["state"], "error");
    }

    #[test]
    fn a_state_says_whether_the_device_is_verified_when_that_is_known() {
        assert_eq!(
            state_event(State::Syncing, None, Some(false)),
            json!({"event": "state", "state": "syncing", "verified": false})
        );
        assert_eq!(
            state_event(State::Syncing, None, Some(true))["verified"],
            true
        );
        assert!(
            state_event(State::Syncing, None, None)
                .get("verified")
                .is_none()
        );
    }

    /// An output and what was written to it, line by line.
    fn output() -> (Output, impl Fn() -> Vec<Value>) {
        #[derive(Clone, Default)]
        struct Lines(Arc<Mutex<Vec<u8>>>);
        impl Write for Lines {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let lines = Lines::default();
        let out = Output::to(Box::new(lines.clone()));
        let read = move || {
            String::from_utf8(lines.0.lock().unwrap().clone())
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect()
        };
        (out, read)
    }

    #[test]
    fn the_state_is_said_again_when_the_verification_changes() {
        let (out, lines) = output();
        // Before any state there is nothing to say again.
        out.set_verified(Some(false));
        assert_eq!(lines(), Vec::<Value>::new());
        out.state(State::Syncing, None);
        out.set_verified(Some(false));
        out.state(State::Offline, Some("the server does not answer"));
        out.set_verified(Some(true));
        out.set_verified(Some(true));
        // When it is no longer known, nothing is said: the next state says
        // nothing of it.
        out.set_verified(None);
        out.state(State::Login, None);
        assert_eq!(
            lines(),
            vec![
                json!({"event": "state", "state": "syncing", "verified": false}),
                json!({"event": "state", "state": "offline", "verified": false,
                       "message": "the server does not answer"}),
                json!({"event": "state", "state": "offline", "verified": true,
                       "message": "the server does not answer"}),
                json!({"event": "state", "state": "login"}),
            ]
        );
    }

    #[test]
    fn the_other_events_are_as_the_editor_reads_them() {
        let (out, lines) = output();
        out.notify("!a:b", Some("Ada"), "me: the tea is ready");
        out.verify_request();
        out.verify_emojis(&[Emoji {
            symbol: "D".to_owned(),
            description: "Dog".to_owned(),
        }]);
        out.verify_done(true, None);
        out.verify_done(false, Some("the emojis are not the same"));
        out.login_url("https://example.org/login");
        out.room_removed("!a:b");
        assert_eq!(
            lines(),
            vec![
                json!({"event": "notify", "room": "!a:b", "sender_name": "Ada",
                       "body": "me: the tea is ready"}),
                json!({"event": "verify-request"}),
                json!({"event": "verify-emojis",
                       "emojis": [{"symbol": "D", "description": "Dog"}]}),
                json!({"event": "verify-done", "ok": true}),
                json!({"event": "verify-done", "ok": false,
                       "message": "the emojis are not the same"}),
                json!({"event": "login-url", "url": "https://example.org/login"}),
                json!({"event": "room-removed", "id": "!a:b"}),
            ]
        );
    }

    #[test]
    fn an_item_is_as_the_editor_reads_it() {
        // The fields that the editor always reads are all there, and the
        // others are left out while they have nothing to say.
        assert_eq!(
            json!(item("7", "Is the kettle on?")),
            json!({
                "id": "7", "sender": "@ada:example.org", "sender_name": "Ada", "time": 3600,
                "kind": "text", "body": "Is the kettle on?", "edited": false, "own": false,
            })
        );
        let mut more = item("8", "kettle.jpg");
        more.kind = Kind::Image;
        more.sender_name = None;
        more.own = true;
        more.reactions = vec![Reaction {
            key: "+1".to_owned(),
            count: 2,
            mine: true,
        }];
        more.reply_to = Some(ReplyTo {
            event_id: "$a".to_owned(),
            sender: Some("@ada:example.org".to_owned()),
            sender_name: Some("Ada".to_owned()),
            body: "Is the kettle on?".to_owned(),
        });
        more.thread = Some(ThreadInfo { count: 2 });
        more.thread_root = Some("3.1".to_owned());
        more.file = Some(FileInfo {
            name: "kettle.jpg".to_owned(),
            thumbnail: None,
            thumbnail_key: Some("example.org%2Fabc".to_owned()),
        });
        let value = json!(more);
        assert_eq!(value["kind"], "image");
        assert_eq!(value["sender_name"], Value::Null);
        assert_eq!(value["own"], true);
        assert_eq!(
            value["reactions"],
            json!([{"key": "+1", "count": 2, "mine": true}])
        );
        assert_eq!(value["reply_to"]["sender_name"], "Ada");
        assert_eq!(value["reply_to"]["body"], "Is the kettle on?");
        assert_eq!(value["thread"], json!({"count": 2}));
        assert_eq!(value["thread_root"], "3.1");
        // The key of the picture is for this program only, and there is no
        // picture before it is fetched.
        assert_eq!(value["file"], json!({"name": "kettle.jpg"}));
        more.file.as_mut().unwrap().thumbnail = Some("/c/k.png".to_owned());
        assert_eq!(
            json!(more)["file"],
            json!({"name": "kettle.jpg", "thumbnail": "/c/k.png"})
        );
    }

    #[test]
    fn timeline_events_have_items_or_ids() {
        let event = timeline_event("!a:b", None, &Op::Append(vec![item("1", "a")]));
        assert_eq!(event["event"], "timeline");
        assert_eq!(event["room"], "!a:b");
        assert_eq!(event["op"], "append");
        assert_eq!(event["items"][0]["id"], "1");
        assert!(event.get("thread").is_none());
        for (op, name) in [
            (Op::Reset(Vec::new()), "reset"),
            (Op::Prepend(Vec::new()), "prepend"),
            (Op::Update(Vec::new()), "update"),
        ] {
            let event = timeline_event("!a:b", None, &op);
            assert_eq!(event["op"], name);
            assert_eq!(event["items"], json!([]));
        }
        let event = timeline_event("!a:b", None, &Op::Remove(vec!["1".to_owned()]));
        assert_eq!(
            event,
            json!({"event": "timeline", "room": "!a:b", "op": "remove", "ids": ["1"]})
        );
        // The events of a thread say which.
        let event = timeline_event("!a:b", Some("2.7"), &Op::Reset(Vec::new()));
        assert_eq!(
            event,
            json!({"event": "timeline", "room": "!a:b", "thread": "2.7", "op": "reset", "items": []})
        );
    }

    #[test]
    fn the_parameters_of_the_methods_are_read() {
        let params = |line: &str| Request::parse(line).unwrap();
        let open: RoomParams =
            params(r#"{"id":1,"method":"timeline-open","params":{"room":"!a:b","thread":"2.7"}}"#)
                .params()
                .unwrap();
        assert_eq!(
            (open.room.as_str(), open.thread.as_deref()),
            ("!a:b", Some("2.7"))
        );
        let open: RoomParams =
            params(r#"{"id":1,"method":"timeline-open","params":{"room":"!a:b"}}"#)
                .params()
                .unwrap();
        assert_eq!(open.thread, None);
        let reply: BodyParams =
            params(r#"{"id":1,"method":"reply","params":{"room":"!a:b","id":"1.4","body":"yes"}}"#)
                .params()
                .unwrap();
        assert_eq!(
            (reply.id.as_str(), reply.body.as_str(), reply.thread),
            ("1.4", "yes", None)
        );
        let react: ReactParams =
            params(r#"{"id":1,"method":"react","params":{"room":"!a:b","thread":"$e","id":"2.1","key":"+1"}}"#)
                .params()
                .unwrap();
        assert_eq!(
            (react.key.as_str(), react.thread.as_deref()),
            ("+1", Some("$e"))
        );
        let redact: MessageParams =
            params(r#"{"id":1,"method":"redact","params":{"room":"!a:b","id":"1.4"}}"#)
                .params()
                .unwrap();
        assert_eq!(redact.id, "1.4");
        let upload: UploadParams =
            params(r#"{"id":1,"method":"upload","params":{"room":"!a:b","path":"/tmp/a.png"}}"#)
                .params()
                .unwrap();
        assert_eq!(upload.path, "/tmp/a.png");
        let confirm: ConfirmParams =
            params(r#"{"id":1,"method":"verify-confirm","params":{"match":true}}"#)
                .params()
                .unwrap();
        assert!(confirm.matches);
        let recover: RecoverParams =
            params(r#"{"id":1,"method":"recover","params":{"key":"EsTc 1234"}}"#)
                .params()
                .unwrap();
        assert_eq!(recover.key, "EsTc 1234");
        // What is missing is said, in the way of every method.
        let error = params(r#"{"id":1,"method":"react","params":{"room":"!a:b","id":"1"}}"#)
            .params::<ReactParams>()
            .unwrap_err();
        assert!(
            error.contains("“react”") && error.contains("key"),
            "{error}"
        );
        assert!(
            params(r#"{"id":1,"method":"verify-confirm","params":{}}"#)
                .params::<ConfirmParams>()
                .is_err()
        );
    }

    #[test]
    fn a_room_event_is_as_the_editor_reads_it() {
        let room = RoomInfo {
            id: "!roses:example.org".to_owned(),
            name: "Roses".to_owned(),
            unread: 3,
            mentions: 1,
            activity: 200,
            direct: false,
            space: false,
            parents: vec!["!space:example.org".to_owned()],
        };
        assert_eq!(
            json!(room),
            json!({
                "id": "!roses:example.org", "name": "Roses", "unread": 3, "mentions": 1,
                "activity": 200, "direct": false, "space": false,
                "parents": ["!space:example.org"],
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
