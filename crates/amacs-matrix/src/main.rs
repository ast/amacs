//! The program through which amacs talks to Matrix.
//!
//! The editor starts this program and talks to it in lines of JSON on
//! standard input and output (see `protocol`). This program holds the
//! Matrix SDK: the login (`session`), the sync and the rooms (`rooms`), and
//! the messages of the rooms that are open (`timeline`). It stops when
//! standard input ends.
//!
//! A request names a method. [`dispatch`] finds the method's handler, which
//! works on the state that all handlers share, [`App`]. A new method is a
//! new arm there and a new handler. Most handlers are short and are waited
//! for, so that requests take effect in the order in which they came. The
//! two that wait for something far away, a login in the browser and a page
//! of earlier messages, run beside the rest.

// The futures of the SDK are nested more deeply than the compiler follows
// by default. The SDK's own crates set the same limit.
#![recursion_limit = "256"]

mod protocol;
mod rooms;
mod session;
mod timeline;

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use matrix_sdk::Client;
use matrix_sdk::ruma::{OwnedRoomId, OwnedUserId, RoomId, UserId};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use protocol::{Output, PaginateParams, Request, RoomParams, SendParams, StartParams, State, log};
use session::{Keeper, Paths, SessionEnd, Stored};
use timeline::OpenTimeline;

/// A task that stops when its owner lets go of it.
pub struct Task(JoinHandle<()>);

impl Task {
    pub fn spawn(future: impl Future<Output = ()> + Send + 'static) -> Self {
        Self(tokio::spawn(future))
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// What all handlers share.
struct App {
    out: Output,
    /// Where the state of all users is kept, or why there is no such place.
    state_root: Result<PathBuf, String>,
    /// The user that "start" named, and nothing before that.
    account: Mutex<Option<Account>>,
}

/// The user of this run.
struct Account {
    user: OwnedUserId,
    homeserver: Option<String>,
    passphrase: String,
    paths: Paths,
    /// The login that waits for the browser, if one does.
    login: Option<Task>,
    /// Counts the logins, so that one that was given up does not take what
    /// belongs to the one that came after it.
    login_serial: u64,
    /// What there is once the user is logged in.
    live: Option<Live>,
}

/// A client that is logged in, with its sync and its open rooms. All of it
/// stops when this is dropped.
struct Live {
    client: Client,
    syncing: rooms::Syncing,
    timelines: HashMap<OwnedRoomId, OpenTimeline>,
    _session: Task,
}

type Outcome = Result<Value, String>;

const NOT_STARTED: &str = "Matrix is not started.";
const NOT_LOGGED_IN: &str = "You are not logged in. Use M-x matrix-login.";

fn main() {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            log(format!("amacs-matrix can not start: {error}"));
            std::process::exit(1);
        }
    };
    runtime.block_on(serve());
    // Tasks that wait for the server are not waited for: what must be on
    // disk is written when it happens, not at the end.
    runtime.shutdown_background();
}

/// Reads requests until standard input ends.
async fn serve() {
    let app = Arc::new(App {
        out: Output::stdout(),
        state_root: session::state_root_of_environment(),
        account: Mutex::new(None),
    });
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    loop {
        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(error) => {
                log(format!("standard input can not be read: {error}"));
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        match Request::parse(&line) {
            Ok(request) => dispatch(&app, request).await,
            // There is no id to answer to. The line itself may hold the
            // passphrase, so it is not logged.
            Err(error) => log(format!("a line that is no request was ignored: {error}")),
        }
    }
    // Let go of the client in an orderly way: the sync and the stores.
    app.account.lock().await.take();
}

/// Runs the handler of a request's method, and answers the request.
async fn dispatch(app: &Arc<App>, request: Request) {
    let outcome = match request.method.as_str() {
        "start" => start(app, &request).await,
        "login" => match begin_login(app, request.id.clone()).await {
            // The answer comes when the user has finished in the browser.
            Ok(()) => return,
            Err(error) => Err(error),
        },
        "timeline-open" => timeline_open(app, &request).await,
        "timeline-close" => timeline_close(app, &request).await,
        "paginate-back" => {
            // The server can take long to answer, and the user may want to
            // write a message meanwhile.
            let app = app.clone();
            tokio::spawn(async move {
                let outcome = paginate_back(&app, &request).await;
                app.out.answer(&request.id, &outcome);
            });
            return;
        }
        "send" => send(app, &request).await,
        other => Err(format!("This program has no method “{other}”.")),
    };
    app.out.answer(&request.id, &outcome);
}

/// "start": names the user and opens the store. With a session from an
/// earlier run the sync starts; without one the editor is told to log in.
async fn start(app: &Arc<App>, request: &Request) -> Outcome {
    // A second "start" changes nothing, so it must not change the state
    // that the editor shows either.
    if app.account.lock().await.is_some() {
        return Err("Matrix is started already.".to_owned());
    }
    let outcome = start_inner(app, request).await;
    if let Err(error) = &outcome {
        // The list of rooms shows the state, so the reason goes there too.
        app.out.state(State::Error, Some(error));
    }
    outcome
}

async fn start_inner(app: &Arc<App>, request: &Request) -> Outcome {
    let params: StartParams = request.params()?;
    if params.passphrase.is_empty() {
        return Err("The store needs a passphrase.".to_owned());
    }
    let user = UserId::parse(&params.user).map_err(|_| {
        format!(
            "“{}” is not a Matrix address such as @name:example.org.",
            params.user
        )
    })?;

    let mut account = app.account.lock().await;
    if account.is_some() {
        return Err("Matrix is started already.".to_owned());
    }
    let paths = Paths::make(app.state_root.as_ref().map_err(Clone::clone)?, &user)?;

    // The passphrase is stretched to open the session, which takes a
    // moment in which other tasks must not stand still.
    let session = {
        let path = paths.session();
        let passphrase = params.passphrase.clone();
        tokio::task::spawn_blocking(move || Keeper::open(&path, &passphrase))
            .await
            .map_err(|error| protocol::explain("The session could not be read", error))??
    };

    let mut new = Account {
        user,
        homeserver: params.homeserver.filter(|server| !server.trim().is_empty()),
        passphrase: params.passphrase,
        paths,
        login: None,
        login_serial: 0,
        live: None,
    };
    match session {
        None => {
            new.paths.clear_for_login();
            *account = Some(new);
            app.out.state(State::Login, None);
        }
        Some((keeper, stored)) => {
            let client = session::restore(&new.paths, &new.passphrase, &stored, &new.user).await?;
            new.live = Some(go_live(app, client, keeper, stored).await?);
            *account = Some(new);
        }
    }
    Ok(Value::Null)
}

/// Starts what runs while the user is logged in: the sync, and the task
/// that keeps the session on disk up to date.
async fn go_live(
    app: &Arc<App>,
    client: Client,
    keeper: Keeper,
    stored: Stored,
) -> Result<Live, String> {
    // The watch starts before anything asks the server, so that it sees the
    // first new tokens.
    let changes = client.subscribe_to_session_changes();
    let session = {
        let app = app.clone();
        let client = client.clone();
        Task::spawn(async move {
            if let SessionEnd::Revoked =
                session::keep_session(client, keeper, stored, changes).await
            {
                // The teardown drops this very task, so it runs in another.
                tokio::spawn(async move { session_revoked(&app).await });
            }
        })
    };
    let syncing = rooms::start(&client, &app.out).await?;
    Ok(Live {
        client,
        syncing,
        timelines: HashMap::new(),
        _session: session,
    })
}

/// The server no longer knows the session: stop, put the session away, and
/// tell the editor that a login is needed.
async fn session_revoked(app: &Arc<App>) {
    let mut account = app.account.lock().await;
    let Some(account) = account.as_mut() else {
        return;
    };
    if account.live.take().is_none() {
        return;
    }
    log("the server ended the session");
    account.paths.retire();
    app.out.state(
        State::Login,
        Some("The server ended the session. Log in again."),
    );
}

/// "login": starts a login through the browser. The request is answered
/// when the login has finished or failed.
async fn begin_login(app: &Arc<App>, id: Value) -> Result<(), String> {
    let mut guard = app.account.lock().await;
    let account = guard.as_mut().ok_or(NOT_STARTED)?;
    if account.live.is_some() {
        return Err("You are logged in already.".to_owned());
    }
    // A login that still waits for the browser is given up: the user asked
    // again, so that page is gone. Its request stays without an answer,
    // which the editor does not mind.
    account.login = None;
    account.login_serial += 1;
    let serial = account.login_serial;

    let (paths, passphrase, user, homeserver) = (
        account.paths.clone(),
        account.passphrase.clone(),
        account.user.clone(),
        account.homeserver.clone(),
    );
    let app_for_task = app.clone();
    account.login = Some(Task::spawn(async move {
        let app = app_for_task;
        let outcome =
            session::login(&paths, &passphrase, &user, homeserver.as_deref(), &app.out).await;
        let outcome = match outcome {
            Ok((client, keeper, stored)) => {
                finish_login(&app, serial, client, keeper, stored).await
            }
            Err(error) => Err(error),
        };
        app.out.answer(&id, &outcome);
    }));
    Ok(())
}

/// Takes a client that has just logged in into use.
async fn finish_login(
    app: &Arc<App>,
    serial: u64,
    client: Client,
    keeper: Keeper,
    stored: Stored,
) -> Outcome {
    let mut guard = app.account.lock().await;
    let account = guard.as_mut().ok_or(NOT_STARTED)?;
    if account.login_serial != serial || account.live.is_some() {
        return Err("Another login took the place of this one.".to_owned());
    }
    account.live = Some(go_live(app, client, keeper, stored).await?);
    Ok(Value::Null)
}

fn room_id(room: &str) -> Result<OwnedRoomId, String> {
    RoomId::parse(room).map_err(|_| format!("“{room}” is not the id of a room."))
}

/// "timeline-open": the editor wants the messages of a room, now and as
/// they change.
async fn timeline_open(app: &Arc<App>, request: &Request) -> Outcome {
    let params: RoomParams = request.params()?;
    let room = room_id(&params.room)?;
    let mut guard = app.account.lock().await;
    let live = live(&mut guard)?;
    // A room that is open already is opened anew, so that the editor gets
    // its "reset" and both sides agree.
    live.timelines.remove(&room);
    let open = timeline::open(&live.client, &live.syncing.room_list(), &room, &app.out).await?;
    live.timelines.insert(room, open);
    Ok(Value::Null)
}

/// "timeline-close": the editor no longer shows the room.
async fn timeline_close(app: &Arc<App>, request: &Request) -> Outcome {
    let params: RoomParams = request.params()?;
    let room = room_id(&params.room)?;
    let mut guard = app.account.lock().await;
    let live = live(&mut guard)?;
    if live.timelines.remove(&room).is_some() {
        timeline::close(&live.syncing.room_list(), &room);
    }
    Ok(Value::Null)
}

/// "paginate-back": the editor wants earlier messages of an open room.
async fn paginate_back(app: &Arc<App>, request: &Request) -> Outcome {
    let params: PaginateParams = request.params()?;
    let room = room_id(&params.room)?;
    let timeline = {
        let mut guard = app.account.lock().await;
        let live = live(&mut guard)?;
        live.timelines
            .get(&room)
            .ok_or("This room is not open.")?
            .timeline()
    };
    // The state is free again while the server is asked.
    timeline::paginate_back(&timeline, params.count).await
}

/// "send": a message of plain text to a room.
async fn send(app: &Arc<App>, request: &Request) -> Outcome {
    let params: SendParams = request.params()?;
    let room = room_id(&params.room)?;
    let (client, open) = {
        let mut guard = app.account.lock().await;
        let live = live(&mut guard)?;
        (
            live.client.clone(),
            live.timelines.get(&room).map(OpenTimeline::timeline),
        )
    };
    timeline::send(&client, open, &room, &params.body).await
}

/// What there is of a user who is logged in, or why there is nothing.
fn live(account: &mut Option<Account>) -> Result<&mut Live, String> {
    let account = account.as_mut().ok_or(NOT_STARTED)?;
    account
        .live
        .as_mut()
        .ok_or_else(|| NOT_LOGGED_IN.to_owned())
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::Mutex as StdMutex;
    use std::time::{Duration, Instant};

    use matrix_sdk::authentication::oauth::{ClientId, OAuthSession, UserSession};
    use matrix_sdk::ruma::{device_id, user_id};
    use matrix_sdk::{SessionMeta, SessionTokens};
    use serde_json::json;

    use super::*;
    use crate::session::SlidingSync;

    /// An output that keeps its lines, for the test to read.
    #[derive(Clone, Default)]
    struct Lines(Arc<StdMutex<Vec<u8>>>);

    impl Write for Lines {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Lines {
        fn values(&self) -> Vec<Value> {
            String::from_utf8(self.0.lock().unwrap().clone())
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).expect("one object on each line"))
                .collect()
        }

        /// Waits until a line is as wanted, for as long as a slow machine
        /// may need.
        async fn wait_for(&self, wanted: impl Fn(&Value) -> bool) -> Value {
            let start = Instant::now();
            loop {
                if let Some(value) = self.values().into_iter().find(|value| wanted(value)) {
                    return value;
                }
                assert!(
                    start.elapsed() < Duration::from_secs(120),
                    "{:?}",
                    self.values()
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }

        async fn answer(&self, id: u64) -> Value {
            self.wait_for(|value| value["id"] == json!(id)).await
        }
    }

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn app(name: &str) -> (Arc<App>, Lines, Scratch) {
        let dir =
            std::env::temp_dir().join(format!("amacs-matrix-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let lines = Lines::default();
        let app = Arc::new(App {
            out: Output::to(Box::new(lines.clone())),
            state_root: Ok(dir.clone()),
            account: Mutex::new(None),
        });
        (app, lines, Scratch(dir))
    }

    async fn request(app: &Arc<App>, line: Value) {
        dispatch(app, Request::parse(&line.to_string()).unwrap()).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn without_a_session_the_editor_is_told_to_log_in() {
        let (app, lines, _scratch) = app("login");
        // Nothing works before "start".
        request(&app, json!({"id": 0, "method": "login"})).await;
        assert_eq!(lines.answer(0).await["error"], NOT_STARTED);
        request(
            &app,
            json!({"id": 1, "method": "start", "params": {"user": "nobody", "passphrase": "p"}}),
        )
        .await;
        assert!(
            lines.answer(1).await["error"]
                .as_str()
                .unwrap()
                .contains("not a Matrix address")
        );
        request(
            &app,
            json!({"id": 2, "method": "start", "params": {"user": "@nobody:example.org"}}),
        )
        .await;
        assert_eq!(
            lines.answer(2).await["error"],
            "The store needs a passphrase."
        );

        let start = json!({"id": 3, "method": "start", "params": {
            "user": "@nobody:example.org", "homeserver": null, "passphrase": "p",
        }});
        request(&app, start.clone()).await;
        assert_eq!(lines.answer(3).await, json!({"id": 3, "result": null}));
        // The state comes before the answer, and it is the last state.
        let values = lines.values();
        let state = values
            .iter()
            .rposition(|value| value["event"] == "state")
            .unwrap();
        assert_eq!(values[state], json!({"event": "state", "state": "login"}));
        assert!(state < values.iter().position(|value| value["id"] == 3).unwrap());

        request(
            &app,
            json!({"id": 4, "method": "send", "params": {"room": "!a:example.org", "body": "hi"}}),
        )
        .await;
        assert_eq!(lines.answer(4).await["error"], NOT_LOGGED_IN);
        request(&app, json!({"id": 5, "method": "fly"})).await;
        assert_eq!(
            lines.answer(5).await["error"],
            "This program has no method “fly”."
        );
        // A second start is refused and leaves the state as it is.
        let before = lines.values().len();
        request(&app, start).await;
        assert_eq!(lines.values().len(), before + 1);
    }

    /// A start with a session of an earlier run, against a server that is
    /// not there: the session is taken up from disk, the sync starts, and
    /// the requests that need a room are answered. Nothing leaves this
    /// machine: the server's address is a port of this machine on which
    /// nothing listens.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn with_a_session_the_sync_starts() {
        let (app, lines, scratch) = app("restore");
        let user = user_id!("@nobody:example.org");
        let paths = Paths::make(&scratch.0, user).unwrap();
        let stored = Stored {
            homeserver: "http://127.0.0.1:9/".to_owned(),
            sliding_sync: SlidingSync::Unstable,
            session: OAuthSession {
                client_id: ClientId::new("client".to_owned()),
                user: UserSession {
                    meta: SessionMeta {
                        user_id: user.to_owned(),
                        device_id: device_id!("DEVICE").to_owned(),
                    },
                    tokens: SessionTokens {
                        access_token: "access".to_owned(),
                        refresh_token: Some("refresh".to_owned()),
                    },
                },
            },
        };
        Keeper::create(&paths.session(), "p")
            .unwrap()
            .save(&stored)
            .unwrap();

        request(
            &app,
            json!({"id": 0, "method": "start", "params": {
                "user": user, "homeserver": null, "passphrase": "p",
            }}),
        )
        .await;
        assert_eq!(lines.answer(0).await, json!({"id": 0, "result": null}));
        lines
            .wait_for(|value| value["event"] == "state" && value["state"] == "syncing")
            .await;
        assert!(!lines.values().iter().any(|value| value["state"] == "login"));

        request(
            &app,
            json!({"id": 1, "method": "timeline-open", "params": {"room": "!a:example.org"}}),
        )
        .await;
        assert_eq!(lines.answer(1).await["error"], "This room is not known.");
        request(&app, json!({"id": 2, "method": "paginate-back", "params": {"room": "!a:example.org", "count": 30}})).await;
        assert_eq!(lines.answer(2).await["error"], "This room is not open.");
        request(
            &app,
            json!({"id": 3, "method": "send", "params": {"room": "!a:example.org", "body": "hi"}}),
        )
        .await;
        assert_eq!(lines.answer(3).await["error"], "This room is not known.");
        request(
            &app,
            json!({"id": 4, "method": "timeline-close", "params": {"room": "not a room"}}),
        )
        .await;
        assert_eq!(
            lines.answer(4).await["error"],
            "“not a room” is not the id of a room."
        );
        request(
            &app,
            json!({"id": 5, "method": "timeline-close", "params": {"room": "!a:example.org"}}),
        )
        .await;
        assert_eq!(lines.answer(5).await, json!({"id": 5, "result": null}));
        request(&app, json!({"id": 6, "method": "login"})).await;
        assert_eq!(lines.answer(6).await["error"], "You are logged in already.");

        // The server does not answer, which the editor is told.
        lines
            .wait_for(|value| value["event"] == "state" && value["state"] == "offline")
            .await;

        // With another passphrase the session stays shut.
        let (other, other_lines, _) = {
            let lines = Lines::default();
            let app = Arc::new(App {
                out: Output::to(Box::new(lines.clone())),
                state_root: Ok(scratch.0.clone()),
                account: Mutex::new(None),
            });
            (app, lines, ())
        };
        request(
            &other,
            json!({"id": 0, "method": "start", "params": {"user": user, "passphrase": "q"}}),
        )
        .await;
        assert_eq!(
            other_lines.answer(0).await["error"],
            "The passphrase does not open the Matrix store."
        );
        assert_eq!(other_lines.values()[0]["state"], "error");
    }
}
