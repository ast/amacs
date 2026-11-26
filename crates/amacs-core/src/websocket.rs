//! WebSocket servers that Scheme code runs: a port on this machine that
//! programs connect to, such as an extension of a web browser, and talk
//! with the editor in text messages for as long as they like.
//!
//! A server listens on the loopback addresses only, so nothing reaches it
//! from another machine. A web page can still ask a browser to connect to
//! a port of the machine it runs on, and a WebSocket is not held back by
//! the rule that keeps pages from reading other sites. So a server names
//! the origins it serves, and a request that a browser marks with another
//! origin is refused before it becomes a connection.
//!
//! Each server has a thread that accepts, and each connection a thread
//! that does the handshake and then both reads and writes: it sleeps in
//! `poll` until the socket has something or the editor has something for
//! it. The threads push onto one bounded channel and wake the editor,
//! which takes it all in at [`Editor::poll`], as it does for processes.

use std::collections::BTreeMap;
use std::io::{self, Cursor, Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use tungstenite::protocol::WebSocket;
use tungstenite::{Error as WsError, Message};

use crate::completion::Waker;
use crate::editor::Editor;
use crate::error::{CmdError, CmdResult};
use crate::script::{ScriptCall, ScriptFn, ScriptTarget, Value};

/// How many messages may wait for the editor before the readers are held.
const BACKLOG: usize = 256;
/// The most that the head of a request may take.
const MAX_HEAD: usize = 16 * 1024;
/// How long a client has to send its request and finish the handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the other side has to answer when this side closes.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);

/// What the threads report.
enum Incoming {
    /// A connection was made to a server: the server, the connection and
    /// the origin that the request gave, with the way to write to it.
    Opened(u64, u64, Option<String>, Outgoing),
    /// A text message arrived on a connection.
    Message(u64, String),
    /// A connection ended, whichever side ended it.
    Closed(u64),
}

/// What the editor asks of a connection's thread.
enum Command {
    Send(String),
    Close,
}

/// A way to interrupt a thread that sleeps in `poll`: a byte written here
/// makes the other end readable.
struct Wake(UnixStream);

impl Wake {
    /// The end that wakes and the end that is polled.
    fn pair() -> io::Result<(Wake, UnixStream)> {
        let (wake, woken) = UnixStream::pair()?;
        wake.set_nonblocking(true)?;
        woken.set_nonblocking(true)?;
        Ok((Wake(wake), woken))
    }

    fn wake(&self) {
        // A full pipe means the thread has been told already.
        let _ = (&self.0).write(&[1]);
    }
}

/// Takes what [`Wake::wake`] wrote, so that `poll` sleeps again.
fn drain(mut woken: &UnixStream) {
    let mut bytes = [0; 64];
    while matches!(woken.read(&mut bytes), Ok(n) if n > 0) {}
}

/// The editor's end of a connection.
struct Outgoing {
    commands: Sender<Command>,
    wake: Wake,
}

impl Outgoing {
    fn send(&self, command: Command) -> bool {
        let sent = self.commands.send(command).is_ok();
        self.wake.wake();
        sent
    }
}

struct Connection {
    server: u64,
    outgoing: Outgoing,
}

struct Server {
    port: u16,
    stop: Arc<AtomicBool>,
    wake: Wake,
    reply: Arc<Mutex<Option<String>>>,
    on_open: Option<ScriptFn>,
    on_message: Option<ScriptFn>,
    on_close: Option<ScriptFn>,
}

impl Server {
    fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        self.wake.wake();
    }
}

/// What a server's threads need to know about it.
struct Shared {
    server: u64,
    /// What to answer a plain `GET` with, as JSON, if anything.
    reply: Arc<Mutex<Option<String>>>,
    /// The starts of the origins that are served.
    origins: Vec<String>,
    stop: Arc<AtomicBool>,
    next_connection: Arc<std::sync::atomic::AtomicU64>,
    sender: SyncSender<Incoming>,
    waker: Waker,
}

pub(crate) struct WebSocketState {
    next_server: u64,
    /// Counted by the accepting threads, so that a connection has its
    /// number before the editor hears of it.
    next_connection: Arc<std::sync::atomic::AtomicU64>,
    servers: BTreeMap<u64, Server>,
    connections: BTreeMap<u64, Connection>,
    sender: SyncSender<Incoming>,
    incoming: Receiver<Incoming>,
}

impl Default for WebSocketState {
    fn default() -> Self {
        let (sender, incoming) = mpsc::sync_channel(BACKLOG);
        Self {
            next_server: 0,
            next_connection: Arc::default(),
            servers: BTreeMap::new(),
            connections: BTreeMap::new(),
            sender,
            incoming,
        }
    }
}

/// However the editor ends, its ports are let go.
impl Drop for WebSocketState {
    fn drop(&mut self) {
        for server in self.servers.values() {
            server.stop();
        }
        for connection in self.connections.values() {
            connection.outgoing.send(Command::Close);
        }
    }
}

/// The head of a request, as far as deciding what to do with it needs.
#[derive(Debug, PartialEq, Eq)]
struct Head {
    method: String,
    /// Whether the client asks for a WebSocket.
    upgrade: bool,
    origin: Option<String>,
}

/// Reads the first line and the headers this module acts on. Whether a
/// request for a WebSocket is a proper one is for the handshake to say.
fn parse_head(head: &[u8]) -> Option<Head> {
    let text = std::str::from_utf8(head).ok()?;
    let mut lines = text.split("\r\n");
    let mut request = lines.next()?.split(' ');
    let method = request.next().filter(|method| !method.is_empty())?;
    request.next()?;
    if !request.next()?.starts_with("HTTP/") {
        return None;
    }
    let (mut upgrade, mut origin) = (false, None);
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("upgrade") {
            upgrade = value
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case("websocket"));
        } else if name.eq_ignore_ascii_case("origin") {
            origin = Some(value.to_owned());
        }
    }
    Some(Head {
        method: method.to_owned(),
        upgrade,
        origin,
    })
}

/// Whether a request with this origin is served. A program that is not a
/// browser gives none, and runs on this machine as the user already.
fn origin_allowed(origin: Option<&str>, origins: &[String]) -> bool {
    match origin {
        None => true,
        Some(origin) => origins
            .iter()
            .any(|start| origin.starts_with(start.as_str())),
    }
}

/// An answer to a request that does not become a connection.
fn http_response(status: &str, origin: Option<&str>, body: &str) -> Vec<u8> {
    // A browser only lets the code that asked read the answer if the
    // answer names where that code is from.
    let cors = origin
        .filter(|origin| !origin.contains(['\r', '\n']))
        .map(|origin| format!("Access-Control-Allow-Origin: {origin}\r\nVary: Origin\r\n"))
        .unwrap_or_default();
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         {cors}Connection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

/// Reads a request up to the empty line that ends its head.
fn read_head(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut head = Vec::new();
    let mut chunk = [0; 1024];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        if head.len() > MAX_HEAD {
            return Err(io::Error::other("the request is too long"));
        }
        match stream.read(&mut chunk)? {
            0 => return Err(io::ErrorKind::UnexpectedEof.into()),
            n => head.extend_from_slice(&chunk[..n]),
        }
    }
    Ok(head)
}

/// A stream that first gives again what was read from it already: the
/// head of the request, which the handshake wants to read itself.
struct Replay {
    head: Cursor<Vec<u8>>,
    stream: TcpStream,
}

impl Read for Replay {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self.head.read(buffer)? {
            0 => self.stream.read(buffer),
            n => Ok(n),
        }
    }
}

impl Write for Replay {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.stream.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

fn would_block(error: &WsError) -> bool {
    matches!(error, WsError::Io(error) if error.kind() == io::ErrorKind::WouldBlock)
}

/// Serves one client: answers a plain request, or makes the connection and
/// carries its messages until either side ends it.
fn serve(mut stream: TcpStream, shared: &Shared) {
    let _ = stream.set_nodelay(true);
    if stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT)).is_err()
        || stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT)).is_err()
    {
        return;
    }
    let Ok(head) = read_head(&mut stream) else {
        return;
    };
    let Some(parsed) = parse_head(&head) else {
        let _ = stream.write_all(&http_response("400 Bad Request", None, ""));
        return;
    };
    let origin = parsed.origin.as_deref();
    if !origin_allowed(origin, &shared.origins) {
        let _ = stream.write_all(&http_response("403 Forbidden", None, ""));
        return;
    }
    if !parsed.upgrade {
        let reply = shared.reply.lock().map_or(None, |reply| reply.clone());
        let response = match (reply, parsed.method.as_str()) {
            (Some(reply), "GET") => http_response("200 OK", origin, &reply),
            _ => http_response("404 Not Found", origin, ""),
        };
        let _ = stream.write_all(&response);
        return;
    }
    let replay = Replay {
        head: Cursor::new(head),
        stream,
    };
    let Ok(socket) = tungstenite::accept(replay) else {
        return;
    };
    let stream = &socket.get_ref().stream;
    if stream.set_nonblocking(true).is_err() {
        return;
    }
    let Ok((wake, woken)) = Wake::pair() else {
        return;
    };
    let (commands, received) = mpsc::channel();
    let id = shared.next_connection.fetch_add(1, Ordering::Relaxed) + 1;
    let opened = Incoming::Opened(
        shared.server,
        id,
        parsed.origin,
        Outgoing { commands, wake },
    );
    if shared.sender.send(opened).is_err() {
        return;
    }
    (shared.waker)();
    carry(socket, &received, &woken, id, shared);
    if shared.sender.send(Incoming::Closed(id)).is_ok() {
        (shared.waker)();
    }
}

/// Carries the messages of a connection both ways until it ends.
fn carry(
    mut socket: WebSocket<Replay>,
    commands: &Receiver<Command>,
    woken: &UnixStream,
    id: u64,
    shared: &Shared,
) {
    // When this side asked to close, and so by when the other must answer.
    let mut closing: Option<Instant> = None;
    loop {
        loop {
            let written = match commands.try_recv() {
                Ok(Command::Send(text)) if closing.is_none() => socket.write(Message::text(text)),
                Ok(Command::Send(_)) => Ok(()),
                // The editor forgot the connection: the same as closing it.
                Ok(Command::Close) | Err(TryRecvError::Disconnected) => {
                    if closing.is_some() {
                        break;
                    }
                    closing = Some(Instant::now() + CLOSE_TIMEOUT);
                    socket.close(None)
                }
                Err(TryRecvError::Empty) => break,
            };
            match written {
                Ok(()) => {}
                // The message is kept and goes out when there is room.
                Err(error) if would_block(&error) => {}
                Err(_) => return,
            }
        }
        let unsent = match socket.flush() {
            Ok(()) => false,
            Err(error) if would_block(&error) => true,
            Err(_) => return,
        };
        loop {
            match socket.read() {
                Ok(Message::Text(text)) => {
                    if shared
                        .sender
                        .send(Incoming::Message(id, text.as_str().to_owned()))
                        .is_err()
                    {
                        return;
                    }
                    (shared.waker)();
                }
                // Pings are answered by the library; the rest is not text.
                Ok(_) => {}
                Err(error) if would_block(&error) => break,
                Err(_) => return,
            }
        }
        let timeout = match closing {
            Some(deadline) => {
                let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                    return;
                };
                Some(Timespec {
                    tv_sec: left.as_secs() as i64,
                    tv_nsec: left.subsec_nanos().into(),
                })
            }
            None => None,
        };
        let mut wanted = PollFlags::IN;
        if unsent {
            wanted |= PollFlags::OUT;
        }
        let mut fds = [
            PollFd::new(&socket.get_ref().stream, wanted),
            PollFd::new(woken, PollFlags::IN),
        ];
        match poll(&mut fds, timeout.as_ref()) {
            Ok(_) => {}
            Err(rustix::io::Errno::INTR) => {}
            Err(_) => return,
        }
        drain(woken);
    }
}

/// Accepts connections until the server is stopped.
fn listen(listeners: &[TcpListener], woken: &UnixStream, shared: &Arc<Shared>) {
    while !shared.stop.load(Ordering::Relaxed) {
        for listener in listeners {
            while let Ok((stream, _)) = listener.accept() {
                // The listener does not block, and on some systems what it
                // accepts then does not either.
                if stream.set_nonblocking(false).is_err() {
                    continue;
                }
                let shared = shared.clone();
                std::thread::spawn(move || serve(stream, &shared));
            }
        }
        let mut fds: Vec<PollFd<'_>> = listeners
            .iter()
            .map(|listener| PollFd::new(listener, PollFlags::IN))
            .collect();
        fds.push(PollFd::new(woken, PollFlags::IN));
        match poll(&mut fds, None) {
            Ok(_) | Err(rustix::io::Errno::INTR) => {}
            Err(_) => return,
        }
        drain(woken);
    }
}

/// Listens on a port of this machine, for this machine only. Port 0 takes
/// one that is free. The address of IPv6 is taken too where there is one,
/// since a browser may try it first for `localhost`.
fn bind(port: u16) -> io::Result<(Vec<TcpListener>, u16)> {
    let first = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))?;
    let port = first.local_addr()?.port();
    let mut listeners = vec![first];
    if let Ok(second) = TcpListener::bind(SocketAddr::from((Ipv6Addr::LOCALHOST, port))) {
        listeners.push(second);
    }
    for listener in &listeners {
        listener.set_nonblocking(true)?;
    }
    Ok((listeners, port))
}

impl Editor {
    /// Starts a server on a port of this machine and returns its number.
    /// A request for a WebSocket whose origin starts with one of `origins`,
    /// or that has no origin, becomes a connection; a plain `GET` is
    /// answered with `reply`, as JSON. `on_open` is called with a new
    /// connection's number and its origin, `on_message` with the connection
    /// and each text message, `on_close` with the connection when it has
    /// ended. An error if the port cannot be had.
    pub fn websocket_listen(
        &mut self,
        port: u16,
        reply: Option<String>,
        origins: Vec<String>,
        on_open: Option<ScriptFn>,
        on_message: Option<ScriptFn>,
        on_close: Option<ScriptFn>,
    ) -> CmdResult<u64> {
        let failed =
            |error: io::Error| CmdError::msg(format!("cannot listen on port {port}: {error}"));
        let (listeners, port) = bind(port).map_err(failed)?;
        let (wake, woken) = Wake::pair().map_err(failed)?;
        self.websockets.next_server += 1;
        let id = self.websockets.next_server;
        let stop = Arc::new(AtomicBool::new(false));
        let reply = Arc::new(Mutex::new(reply));
        let shared = Arc::new(Shared {
            server: id,
            reply: reply.clone(),
            origins,
            stop: stop.clone(),
            next_connection: self.websockets.next_connection.clone(),
            sender: self.websockets.sender.clone(),
            waker: self.waker.clone(),
        });
        std::thread::spawn(move || listen(&listeners, &woken, &shared));
        self.websockets.servers.insert(
            id,
            Server {
                port,
                stop,
                wake,
                reply,
                on_open,
                on_message,
                on_close,
            },
        );
        Ok(id)
    }

    /// The numbers of the servers that listen, oldest first.
    pub fn websocket_server_list(&self) -> Vec<u64> {
        self.websockets.servers.keys().copied().collect()
    }

    /// The port a server listens on.
    pub fn websocket_server_port(&self, id: u64) -> CmdResult<u16> {
        self.websockets
            .servers
            .get(&id)
            .map(|server| server.port)
            .ok_or_else(|| CmdError::msg(format!("There is no WebSocket server {id}")))
    }

    /// Sets what a server answers a plain `GET` with, as JSON, or that it
    /// answers none: for an answer that tells the port the server got.
    pub fn set_websocket_server_reply(&mut self, id: u64, reply: Option<String>) -> CmdResult {
        let server = self
            .websockets
            .servers
            .get(&id)
            .ok_or_else(|| CmdError::msg(format!("There is no WebSocket server {id}")))?;
        if let Ok(mut current) = server.reply.lock() {
            *current = reply;
        }
        Ok(())
    }

    /// Stops a server and closes its connections, without word to its
    /// procedures.
    pub fn delete_websocket_server(&mut self, id: u64) -> CmdResult {
        let server = self
            .websockets
            .servers
            .remove(&id)
            .ok_or_else(|| CmdError::msg(format!("There is no WebSocket server {id}")))?;
        server.stop();
        self.websockets.connections.retain(|_, connection| {
            if connection.server == id {
                connection.outgoing.send(Command::Close);
            }
            connection.server != id
        });
        Ok(())
    }

    /// The connections of a server that are open, oldest first.
    pub fn websocket_connections(&self, server: u64) -> Vec<u64> {
        self.websockets
            .connections
            .iter()
            .filter(|(_, connection)| connection.server == server)
            .map(|(id, _)| *id)
            .collect()
    }

    /// Whether a connection is open.
    pub fn websocket_live(&self, id: u64) -> bool {
        self.websockets.connections.contains_key(&id)
    }

    /// Sends a text message on a connection. The text is handed to the
    /// connection's thread, so a client slow to read does not hold the
    /// editor. An error if the connection has ended.
    pub fn websocket_send(&mut self, id: u64, text: &str) -> CmdResult {
        let sent = self
            .websockets
            .connections
            .get(&id)
            .is_some_and(|connection| connection.outgoing.send(Command::Send(text.to_owned())));
        if sent {
            Ok(())
        } else {
            Err(CmdError::msg(format!(
                "WebSocket connection {id} has ended"
            )))
        }
    }

    /// Closes a connection, after what was sent on it has gone out. Its
    /// server's procedure for the end is called when it has ended. Nothing
    /// happens if it has ended already.
    pub fn websocket_close(&mut self, id: u64) {
        if let Some(connection) = self.websockets.connections.get(&id) {
            connection.outgoing.send(Command::Close);
        }
    }

    /// Takes in what the connections report and calls the procedures of
    /// their servers.
    pub(crate) fn websocket_poll(&mut self) {
        let mut calls = Vec::new();
        while let Ok(incoming) = self.websockets.incoming.try_recv() {
            let state = &mut self.websockets;
            match incoming {
                Incoming::Opened(server, id, origin, outgoing) => {
                    // A server deleted meanwhile takes its connections along.
                    let Some(handlers) = state.servers.get(&server) else {
                        outgoing.send(Command::Close);
                        continue;
                    };
                    state
                        .connections
                        .insert(id, Connection { server, outgoing });
                    if let Some(func) = &handlers.on_open {
                        calls.push((func.clone(), vec![Value::Int(id as i64), origin.into()]));
                    }
                }
                Incoming::Message(id, text) => {
                    let handlers = state
                        .connections
                        .get(&id)
                        .and_then(|connection| state.servers.get(&connection.server));
                    if let Some(func) = handlers.and_then(|server| server.on_message.as_ref()) {
                        calls.push((func.clone(), vec![Value::Int(id as i64), text.into()]));
                    }
                }
                Incoming::Closed(id) => {
                    let handlers = state
                        .connections
                        .remove(&id)
                        .and_then(|connection| state.servers.get(&connection.server));
                    if let Some(func) = handlers.and_then(|server| server.on_close.as_ref()) {
                        calls.push((func.clone(), vec![Value::Int(id as i64)]));
                    }
                }
            }
        }
        for (func, args) in calls {
            let _ = self.queue_script(ScriptCall {
                target: ScriptTarget::Func(func),
                args,
                command: None,
                echo_result: false,
                frame: None,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_head_of_a_request_says_what_is_asked_and_by_whom() {
        let plain = parse_head(b"GET / HTTP/1.1\r\nHost: localhost:4001\r\n\r\n").unwrap();
        assert_eq!(
            plain,
            Head {
                method: "GET".into(),
                upgrade: false,
                origin: None
            }
        );
        let socket = parse_head(
            b"GET / HTTP/1.1\r\nConnection: keep-alive, Upgrade\r\nUPGRADE: WebSocket\r\n\
              origin: moz-extension://abc\r\n\r\n",
        )
        .unwrap();
        assert!(socket.upgrade);
        assert_eq!(socket.origin.as_deref(), Some("moz-extension://abc"));
        assert_eq!(parse_head(b"nonsense\r\n\r\n"), None);
        assert_eq!(parse_head(b"\xff\xfe\r\n\r\n"), None);
    }

    #[test]
    fn only_the_origins_named_are_served() {
        let origins = vec!["moz-extension://".to_owned()];
        assert!(origin_allowed(None, &origins));
        assert!(origin_allowed(Some("moz-extension://abc"), &origins));
        assert!(!origin_allowed(Some("https://example.com"), &origins));
        assert!(!origin_allowed(Some("null"), &origins));
        assert!(!origin_allowed(Some("https://example.com"), &[]));
    }

    #[test]
    fn an_answer_names_the_origin_that_may_read_it() {
        let text =
            String::from_utf8(http_response("200 OK", Some("moz-extension://abc"), "{}")).unwrap();
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(text.contains("Access-Control-Allow-Origin: moz-extension://abc\r\n"));
        assert!(text.contains("Content-Length: 2\r\n"));
        assert!(text.ends_with("\r\n\r\n{}"));
        let plain = String::from_utf8(http_response("200 OK", None, "{}")).unwrap();
        assert!(!plain.contains("Access-Control"));
    }
}
