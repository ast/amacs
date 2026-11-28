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

mod media;
mod notify;
mod protocol;
mod rooms;
mod session;
mod timeline;
mod verify;

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use matrix_sdk::Client;
use matrix_sdk::event_handler::EventHandlerDropGuard;
use matrix_sdk::ruma::{EventId, OwnedEventId, OwnedRoomId, OwnedUserId, RoomId, UserId};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use media::{Cache, Thumbnailer};
use protocol::{
    BodyParams, ConfirmParams, MessageParams, Output, PaginateParams, ReactParams, RecoverParams,
    Request, RoomParams, SendParams, StartParams, State, UploadParams, log,
};
use session::{Keeper, Paths, SessionEnd, Stored};
use timeline::OpenTimeline;
use verify::Verifier;

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
    /// Where the files that are fetched are kept, or why there is no such
    /// place.
    cache_root: Result<PathBuf, String>,
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
    /// The lists that are open, by room and by the name that the editor
    /// has for the thread, if it is the list of a thread.
    timelines: HashMap<View, OpenTimeline>,
    /// Counts the lists that were opened: each has the number at the start
    /// of the ids of its items, so that an id names one item of one list
    /// for as long as the program runs.
    opened: u64,
    verifier: Verifier,
    thumbnailer: Thumbnailer,
    /// Where the files of this user are kept, or why there is no such
    /// place.
    cache: Result<Cache, String>,
    _tasks: Vec<Task>,
    /// Removes the event handlers that hold the client, so that it goes
    /// with this.
    _handlers: Vec<EventHandlerDropGuard>,
}

/// A room, or a thread in it by the name that the editor has for it.
type View = (OwnedRoomId, Option<String>);

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
        cache_root: media::cache_root_of_environment(),
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
        "send" => send(app, &request).await,
        "reply" => reply(app, &request).await,
        "edit" => edit(app, &request).await,
        "react" => react(app, &request).await,
        "redact" => redact(app, &request).await,
        // These wait for the server, which can take long, and the user may
        // want to write a message meanwhile: they run beside the rest.
        "paginate-back" | "mark-read" | "upload" | "download" | "verify-start"
        | "verify-accept" | "verify-cancel" | "verify-confirm" | "recover" => {
            let app = app.clone();
            tokio::spawn(async move {
                let outcome = beside(&app, &request).await;
                app.out.answer(&request.id, &outcome);
            });
            return;
        }
        other => Err(format!("This program has no method “{other}”.")),
    };
    app.out.answer(&request.id, &outcome);
}

/// The handlers that run beside the rest.
async fn beside(app: &Arc<App>, request: &Request) -> Outcome {
    match request.method.as_str() {
        "paginate-back" => paginate_back(app, request).await,
        "mark-read" => mark_read(app, request).await,
        "upload" => upload(app, request).await,
        "download" => download(app, request).await,
        "verify-start" => verifier(app).await?.request().await,
        "verify-accept" => verifier(app).await?.accept().await,
        "verify-cancel" => verifier(app).await?.cancel().await,
        "verify-confirm" => {
            let params: ConfirmParams = request.params()?;
            verifier(app).await?.confirm(params.matches).await
        }
        "recover" => {
            let params: RecoverParams = request.params()?;
            verifier(app).await?.recover(&params.key).await
        }
        other => Err(format!("This program has no method “{other}”.")),
    }
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
    // Whether the device is verified is known, as far as it is, before
    // the first "syncing" is said, which carries it.
    let (verifier, verified, verify_requests) = Verifier::start(&client, &app.out);
    notify::start(&client, &app.out).await;

    let cache = match (&app.cache_root, client.user_id()) {
        (Ok(root), Some(user)) => Ok(Cache::new(root, user)),
        (Err(error), _) => Err(error.clone()),
        (_, None) => Err(NOT_LOGGED_IN.to_owned()),
    };
    let (thumbnailer, thumbnails) = match &cache {
        Ok(cache) => {
            let (thumbnailer, task) = Thumbnailer::start(client.clone(), cache.clone());
            (thumbnailer, Some(task))
        }
        // Without a place for them, messages have no pictures.
        Err(_) => (Thumbnailer::idle(), None),
    };

    let syncing = rooms::start(&client, &app.out).await?;
    let mut tasks = vec![session, verified];
    tasks.extend(thumbnails);
    Ok(Live {
        client,
        syncing,
        timelines: HashMap::new(),
        opened: 0,
        verifier,
        thumbnailer,
        cache,
        _tasks: tasks,
        _handlers: vec![verify_requests],
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
    // What was known of the device is of a session that is no more.
    app.out.set_verified(None);
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

/// The event that a thread starts with, from the name that the editor has
/// for the thread: the id of that event, or the id of its item in a list
/// that is open (see `timeline`).
async fn thread_root(
    live: &Live,
    room: &OwnedRoomId,
    thread: &str,
) -> Result<OwnedEventId, String> {
    if thread.starts_with('$') {
        return EventId::parse(thread)
            .map_err(|_| format!("“{thread}” is not the id of an event."));
    }
    for ((of_room, _), open) in &live.timelines {
        if of_room == room
            && let Some(event_id) = timeline::event_id(&open.timeline(), thread).await
        {
            return Ok(event_id);
        }
    }
    Err("The message that the thread starts with is not in a list that is open.".to_owned())
}

/// The list of a room or of a thread, which must be open.
fn open_timeline(
    live: &Live,
    room: OwnedRoomId,
    thread: Option<String>,
) -> Result<Arc<matrix_sdk_ui::Timeline>, String> {
    let view = (room, thread);
    match live.timelines.get(&view) {
        Some(open) => Ok(open.timeline()),
        None if view.1.is_some() => Err("This thread is not open.".to_owned()),
        None => Err("This room is not open.".to_owned()),
    }
}

/// The list that a request names, without the state held: what is done with
/// it may wait for the server.
async fn timeline_of(
    app: &Arc<App>,
    room: &str,
    thread: Option<String>,
) -> Result<(Client, Arc<matrix_sdk_ui::Timeline>), String> {
    let room = room_id(room)?;
    let mut guard = app.account.lock().await;
    let live = live(&mut guard)?;
    Ok((live.client.clone(), open_timeline(live, room, thread)?))
}

/// "timeline-open": the editor wants the messages of a room or of a thread,
/// now and as they change.
async fn timeline_open(app: &Arc<App>, request: &Request) -> Outcome {
    let params: RoomParams = request.params()?;
    let room = room_id(&params.room)?;
    let mut guard = app.account.lock().await;
    let live = live(&mut guard)?;
    let root = match &params.thread {
        Some(thread) => Some(thread_root(live, &room, thread).await?),
        None => None,
    };
    // A list that is open already is opened anew, so that the editor gets
    // its "reset" and both sides agree.
    let view = (room, params.thread);
    live.timelines.remove(&view);
    live.opened += 1;
    let target = timeline::Target {
        room: &view.0,
        thread: view.1.as_deref().zip(root),
        id_prefix: format!("{}.", live.opened),
    };
    let open = timeline::open(
        &live.client,
        &live.syncing.room_list(),
        target,
        &live.thumbnailer,
        &app.out,
    )
    .await?;
    live.timelines.insert(view, open);
    Ok(Value::Null)
}

/// "timeline-close": the editor no longer shows the room or the thread.
async fn timeline_close(app: &Arc<App>, request: &Request) -> Outcome {
    let params: RoomParams = request.params()?;
    let room = room_id(&params.room)?;
    let mut guard = app.account.lock().await;
    let live = live(&mut guard)?;
    let view = (room, params.thread);
    let closed = live.timelines.remove(&view).is_some();
    // The server is asked for all of the room for as long as one of its
    // lists is open.
    if closed && !live.timelines.keys().any(|(room, _)| room == &view.0) {
        timeline::close(&live.syncing.room_list(), &view.0);
    }
    Ok(Value::Null)
}

/// "paginate-back": the editor wants earlier messages of an open list.
async fn paginate_back(app: &Arc<App>, request: &Request) -> Outcome {
    let params: PaginateParams = request.params()?;
    let (_, timeline) = timeline_of(app, &params.room, params.thread).await?;
    timeline::paginate_back(&timeline, params.count).await
}

/// "send": a message of plain text to a room, or to a thread whose list is
/// open.
async fn send(app: &Arc<App>, request: &Request) -> Outcome {
    let params: SendParams = request.params()?;
    let room = room_id(&params.room)?;
    let (client, open) = {
        let mut guard = app.account.lock().await;
        let live = live(&mut guard)?;
        let open = match params.thread {
            // Only the list of a thread knows how to answer in the thread.
            Some(thread) => Some(open_timeline(live, room.clone(), Some(thread))?),
            None => live
                .timelines
                .get(&(room.clone(), None))
                .map(OpenTimeline::timeline),
        };
        (live.client.clone(), open)
    };
    timeline::send(&client, open, &room, &params.body).await
}

/// "reply": an answer to a message of an open list.
async fn reply(app: &Arc<App>, request: &Request) -> Outcome {
    let params: BodyParams = request.params()?;
    let (_, timeline) = timeline_of(app, &params.room, params.thread).await?;
    timeline::reply(&timeline, &params.id, &params.body).await
}

/// "edit": another text for a message of the user's own.
async fn edit(app: &Arc<App>, request: &Request) -> Outcome {
    let params: BodyParams = request.params()?;
    let (_, timeline) = timeline_of(app, &params.room, params.thread).await?;
    timeline::edit(&timeline, &params.id, &params.body).await
}

/// "react": a reaction to a message, or the end of the user's own.
async fn react(app: &Arc<App>, request: &Request) -> Outcome {
    let params: ReactParams = request.params()?;
    if params.key.trim().is_empty() {
        return Err("A reaction can not be empty.".to_owned());
    }
    let (_, timeline) = timeline_of(app, &params.room, params.thread).await?;
    timeline::react(&timeline, &params.id, &params.key).await
}

/// "redact": deletes a message for everybody.
async fn redact(app: &Arc<App>, request: &Request) -> Outcome {
    let params: MessageParams = request.params()?;
    let (_, timeline) = timeline_of(app, &params.room, params.thread).await?;
    timeline::redact(&timeline, &params.id).await
}

/// "mark-read": the user has read the room, or a thread of it, to its end.
async fn mark_read(app: &Arc<App>, request: &Request) -> Outcome {
    let params: RoomParams = request.params()?;
    let (_, timeline) = timeline_of(app, &params.room, params.thread).await?;
    timeline::mark_read(&timeline).await
}

/// "upload": sends a file to a room or a thread whose list is open.
async fn upload(app: &Arc<App>, request: &Request) -> Outcome {
    let params: UploadParams = request.params()?;
    let (_, timeline) = timeline_of(app, &params.room, params.thread).await?;
    timeline::upload(&timeline, &params.path).await
}

/// "download": fetches the file of a message, unless it is on disk. The
/// result is where it is.
async fn download(app: &Arc<App>, request: &Request) -> Outcome {
    let params: MessageParams = request.params()?;
    let room = room_id(&params.room)?;
    let (client, timeline, cache) = {
        let mut guard = app.account.lock().await;
        let live = live(&mut guard)?;
        (
            live.client.clone(),
            open_timeline(live, room, params.thread)?,
            live.cache.clone()?,
        )
    };
    let attachment = timeline::attachment(&timeline, &params.id).await?;
    let path = media::download(&client, &cache, &attachment).await?;
    Ok(Value::String(path.to_string_lossy().into_owned()))
}

/// The verifier of the user who is logged in.
async fn verifier(app: &Arc<App>) -> Result<Verifier, String> {
    let mut guard = app.account.lock().await;
    Ok(live(&mut guard)?.verifier.clone())
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
            state_root: Ok(dir.join("state")),
            cache_root: Ok(dir.join("cache")),
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
        for (number, method) in [
            "reply",
            "edit",
            "react",
            "redact",
            "mark-read",
            "upload",
            "download",
            "paginate-back",
            "timeline-open",
            "timeline-close",
        ]
        .into_iter()
        .enumerate()
        {
            let id = 100 + number as u64;
            let params = json!({"room": "!a:example.org", "id": "1.0", "body": "x", "key": "+1", "path": "/a"});
            request(&app, json!({"id": id, "method": method, "params": params})).await;
            assert_eq!(lines.answer(id).await["error"], NOT_LOGGED_IN, "{method}");
        }
        for (number, method) in ["verify-start", "verify-accept", "verify-cancel"]
            .into_iter()
            .enumerate()
        {
            let id = 200 + number as u64;
            request(&app, json!({"id": id, "method": method})).await;
            assert_eq!(lines.answer(id).await["error"], NOT_LOGGED_IN, "{method}");
        }
        request(
            &app,
            json!({"id": 210, "method": "recover", "params": {"key": "EsTc 1234"}}),
        )
        .await;
        assert_eq!(lines.answer(210).await["error"], NOT_LOGGED_IN);
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
        let paths = Paths::make(&scratch.0.join("state"), user).unwrap();
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

        // The methods that are about a message or a file need an open
        // list, and say so.
        let not_open = [
            json!({"method": "reply", "params": {"room": "!a:example.org", "id": "1.0", "body": "x"}}),
            json!({"method": "edit", "params": {"room": "!a:example.org", "id": "1.0", "body": "x"}}),
            json!({"method": "react", "params": {"room": "!a:example.org", "id": "1.0", "key": "+1"}}),
            json!({"method": "redact", "params": {"room": "!a:example.org", "id": "1.0"}}),
            json!({"method": "mark-read", "params": {"room": "!a:example.org"}}),
            json!({"method": "upload", "params": {"room": "!a:example.org", "path": "/nowhere/a.png"}}),
            json!({"method": "download", "params": {"room": "!a:example.org", "id": "1.0"}}),
        ];
        for (number, mut line) in not_open.into_iter().enumerate() {
            let id = 100 + number as u64;
            line["id"] = json!(id);
            request(&app, line.clone()).await;
            assert_eq!(
                lines.answer(id).await["error"],
                "This room is not open.",
                "{line}"
            );
        }
        let in_thread = [
            json!({"method": "send", "params": {"room": "!a:example.org", "thread": "1.0", "body": "x"}}),
            json!({"method": "paginate-back", "params": {"room": "!a:example.org", "thread": "$e", "count": 3}}),
            json!({"method": "upload", "params": {"room": "!a:example.org", "thread": "1.0", "path": "/a"}}),
        ];
        for (number, mut line) in in_thread.into_iter().enumerate() {
            let id = 200 + number as u64;
            line["id"] = json!(id);
            request(&app, line.clone()).await;
            assert_eq!(
                lines.answer(id).await["error"],
                "This thread is not open.",
                "{line}"
            );
        }
        // A thread is named by an item of an open list, or by an event.
        request(&app, json!({"id": 300, "method": "timeline-open", "params": {"room": "!a:example.org", "thread": "1.0"}})).await;
        assert_eq!(
            lines.answer(300).await["error"],
            "The message that the thread starts with is not in a list that is open."
        );
        request(&app, json!({"id": 301, "method": "timeline-open", "params": {"room": "!a:example.org", "thread": "$root"}})).await;
        assert_eq!(lines.answer(301).await["error"], "This room is not known.");
        request(&app, json!({"id": 302, "method": "timeline-close", "params": {"room": "!a:example.org", "thread": "$root"}})).await;
        assert_eq!(lines.answer(302).await, json!({"id": 302, "result": null}));

        // What is wrong with the parameters is said before anything else.
        request(&app, json!({"id": 310, "method": "react", "params": {"room": "!a:example.org", "id": "1.0", "key": " "}})).await;
        assert_eq!(
            lines.answer(310).await["error"],
            "A reaction can not be empty."
        );
        request(&app, json!({"id": 311, "method": "reply", "params": {"room": "!a:example.org", "body": "x"}})).await;
        assert!(
            lines.answer(311).await["error"]
                .as_str()
                .unwrap()
                .starts_with("The parameters of “reply”")
        );
        request(
            &app,
            json!({"id": 312, "method": "download", "params": {"room": "nothing", "id": "1.0"}}),
        )
        .await;
        assert_eq!(
            lines.answer(312).await["error"],
            "“nothing” is not the id of a room."
        );

        // No verification is going on.
        request(&app, json!({"id": 320, "method": "verify-accept"})).await;
        assert_eq!(
            lines.answer(320).await["error"],
            "No device asks to verify this one."
        );
        request(
            &app,
            json!({"id": 321, "method": "verify-confirm", "params": {"match": true}}),
        )
        .await;
        assert_eq!(
            lines.answer(321).await["error"],
            "No emojis are waiting for an answer."
        );
        request(
            &app,
            json!({"id": 322, "method": "verify-confirm", "params": {}}),
        )
        .await;
        assert!(
            lines.answer(322).await["error"]
                .as_str()
                .unwrap()
                .starts_with("The parameters of “verify-confirm”")
        );
        request(&app, json!({"id": 323, "method": "verify-cancel"})).await;
        assert_eq!(lines.answer(323).await, json!({"id": 323, "result": null}));
        request(
            &app,
            json!({"id": 324, "method": "recover", "params": {"key": "  "}}),
        )
        .await;
        assert_eq!(
            lines.answer(324).await["error"],
            "The recovery key is empty."
        );
        request(&app, json!({"id": 325, "method": "recover", "params": {}})).await;
        let error = lines.answer(325).await["error"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(error.starts_with("The parameters of “recover”"), "{error}");

        // The server does not answer, which the editor is told.
        lines
            .wait_for(|value| value["event"] == "state" && value["state"] == "offline")
            .await;

        // With another passphrase the session stays shut.
        let (other, other_lines, _) = {
            let lines = Lines::default();
            let app = Arc::new(App {
                out: Output::to(Box::new(lines.clone())),
                state_root: Ok(scratch.0.join("state")),
                cache_root: Ok(scratch.0.join("cache")),
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
