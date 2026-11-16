//! The editor as a server. Clients connect over a Unix socket to open
//! frames in their terminals, to visit files and to evaluate Scheme.
//!
//! One thread owns the editor. Each connection has a thread that reads its
//! messages into the main thread's queue and a thread that writes replies,
//! so a slow or stuck client cannot stall the editor.

use std::collections::HashMap;
use std::io::{BufReader, BufWriter};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use amacs_core::{EditorEvent, FrameId};
use amacs_proto::{
    ClientMsg, FileSpec, FrameRequest, InputEvent, Key, PROTOCOL_VERSION, ServerMsg, read_msg,
    socket_path, write_msg,
};
use amacs_script::{InterruptHandle, Session};
use anyhow::{Context, Result, bail};

use crate::{AUTO_SAVE_IDLE, AUTO_SAVE_INTERVAL, Options, start_session};

/// The base frame is never shown, so its size only matters to code that
/// asks for it.
const HEADLESS_SIZE: (u16, u16) = (80, 24);
/// A client that does not take what it is sent for this long is dropped.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

type ConnId = u64;

enum Event {
    Connected(ConnId, UnixStream),
    Message(ConnId, ClientMsg),
    Disconnected(ConnId),
    /// A signal asked the server to stop.
    Shutdown,
}

struct Conn {
    /// Messages for the writer thread. Dropping it ends the connection once
    /// everything queued has been written.
    out: Sender<ServerMsg>,
    writer: JoinHandle<()>,
    greeted: bool,
    frame: Option<FrameId>,
    /// Buffers this client is still waiting for, if it waits at all.
    waiting: Option<usize>,
}

struct Server {
    session: Session,
    conns: HashMap<ConnId, Conn>,
}

/// The listening socket, removed again on drop if this process created it.
struct Listener {
    listener: UnixListener,
    created: Option<PathBuf>,
}

impl Drop for Listener {
    fn drop(&mut self) {
        if let Some(path) = &self.created {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Takes the socket that systemd passed in, or binds one.
fn listen() -> Result<Listener> {
    let mut passed = listenfd::ListenFd::from_env();
    if let Some(listener) = passed
        .take_unix_listener(0)
        .context("bad socket from systemd")?
    {
        return Ok(Listener {
            listener,
            created: None,
        });
    }

    let path = socket_path();
    if let Some(dir) = path.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .with_context(|| format!("cannot create {}", dir.display()))?;
    }
    if path.exists() {
        if UnixStream::connect(&path).is_ok() {
            bail!("a server is already listening on {}", path.display());
        }
        // Left behind by a server that did not exit cleanly.
        std::fs::remove_file(&path).with_context(|| format!("cannot remove {}", path.display()))?;
    }
    let listener = UnixListener::bind(&path)
        .with_context(|| format!("cannot listen on {}", path.display()))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    Ok(Listener {
        listener,
        created: Some(path),
    })
}

fn spawn_reader(id: ConnId, stream: UnixStream, events: Sender<Event>, interrupt: InterruptHandle) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        while let Ok(Some(message)) = read_msg::<_, ClientMsg>(&mut reader) {
            // C-g must get through even while the main thread is busy
            // running Scheme code and not reading its queue.
            if message == ClientMsg::Input(InputEvent::Key(Key::ctrl('g'))) {
                interrupt.interrupt();
            }
            if events.send(Event::Message(id, message)).is_err() {
                return;
            }
        }
        let _ = events.send(Event::Disconnected(id));
    });
}

fn spawn_writer(stream: UnixStream) -> (Sender<ServerMsg>, JoinHandle<()>) {
    let (sender, messages) = mpsc::channel::<ServerMsg>();
    let handle = std::thread::spawn(move || {
        let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
        let mut writer = BufWriter::new(&stream);
        for message in messages {
            if write_msg(&mut writer, &message).is_err() {
                break;
            }
        }
        drop(writer);
        // Closing our side is what tells the client, and our reader, that
        // the connection is over.
        let _ = stream.shutdown(std::net::Shutdown::Both);
    });
    (sender, handle)
}

pub fn run(options: &Options) -> Result<ExitCode> {
    let listener = listen()?;
    let mut session = start_session(options, HEADLESS_SIZE.0, HEADLESS_SIZE.1)?;
    session.editor_mut().set_daemon(true);

    let (sender, events) = mpsc::channel();
    {
        let sender = sender.clone();
        let listener = listener
            .listener
            .try_clone()
            .context("cannot share the socket")?;
        std::thread::spawn(move || {
            for (id, stream) in listener.incoming().flatten().enumerate() {
                if sender.send(Event::Connected(id as ConnId, stream)).is_err() {
                    break;
                }
            }
        });
    }
    {
        use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
        let sender = sender.clone();
        let mut signals = signal_hook::iterator::Signals::new([SIGHUP, SIGINT, SIGTERM])
            .context("cannot install signal handlers")?;
        std::thread::spawn(move || {
            if signals.forever().next().is_some() {
                let _ = sender.send(Event::Shutdown);
            }
        });
    }
    // Tells systemd that the server is ready; does nothing otherwise.
    let _ = sd_notify::notify(&[sd_notify::NotifyState::Ready]);

    let interrupt = session.interrupt_handle();
    let mut server = Server {
        session,
        conns: HashMap::new(),
    };
    let mut last_auto_save = Instant::now();
    let mut crashed = false;
    'serve: loop {
        let first = match events.recv_timeout(AUTO_SAVE_IDLE) {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => {
                server.session.editor_mut().auto_save(false);
                last_auto_save = Instant::now();
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        // A bug that panics must not take the user's unsaved work with it.
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let mut next = Some(first);
            while let Some(event) = next {
                match event {
                    Event::Connected(id, stream) => {
                        server.connect(id, stream, sender.clone(), interrupt.clone());
                    }
                    Event::Message(id, message) => server.handle_message(id, message),
                    Event::Disconnected(id) => server.disconnect(id),
                    Event::Shutdown => return false,
                }
                next = events.try_recv().ok();
            }
            server.after_events();
            !server.session.should_exit()
        }));
        match outcome {
            Ok(true) => {}
            Ok(false) => break 'serve,
            Err(_) => {
                crashed = true;
                break 'serve;
            }
        }
        if last_auto_save.elapsed() >= AUTO_SAVE_INTERVAL {
            server.session.editor_mut().auto_save(false);
            last_auto_save = Instant::now();
        }
    }

    // Whatever the reason for stopping, unsaved changes go to the auto-save
    // directory first. After a panic the editor may be in a bad state, so
    // this too is guarded.
    let saved = catch_unwind(AssertUnwindSafe(|| {
        server.session.editor_mut().auto_save(true)
    }));
    let message = if crashed {
        Some("amacs: the server crashed; unsaved changes were auto-saved".to_owned())
    } else {
        None
    };
    server.close_all(message, u8::from(crashed));
    drop(listener);
    if crashed {
        bail!(
            "internal error; {}",
            match saved {
                Ok(count) => format!("{count} modified buffers were auto-saved"),
                Err(_) => "auto-saving failed too".to_owned(),
            }
        );
    }
    Ok(ExitCode::SUCCESS)
}

impl Server {
    fn connect(
        &mut self,
        id: ConnId,
        stream: UnixStream,
        events: Sender<Event>,
        interrupt: InterruptHandle,
    ) {
        let Ok(read_half) = stream.try_clone() else {
            return;
        };
        spawn_reader(id, read_half, events, interrupt);
        let (out, writer) = spawn_writer(stream);
        self.conns.insert(
            id,
            Conn {
                out,
                writer,
                greeted: false,
                frame: None,
                waiting: None,
            },
        );
    }

    fn send(&self, id: ConnId, message: ServerMsg) {
        if let Some(conn) = self.conns.get(&id) {
            // If the writer is gone, the reader will report the disconnect.
            let _ = conn.out.send(message);
        }
    }

    /// Tells a client to exit and forgets it. Returns the thread that is
    /// still writing to it.
    fn finish(
        &mut self,
        id: ConnId,
        message: Option<String>,
        status: u8,
    ) -> Option<JoinHandle<()>> {
        let conn = self.conns.remove(&id)?;
        let _ = conn.out.send(ServerMsg::Exit { message, status });
        let mut editor = self.session.editor_mut();
        editor.remove_waiters(id);
        if let Some(frame) = conn.frame {
            // The base frame is never a client's, so this cannot fail.
            let _ = editor.delete_frame(frame);
        }
        Some(conn.writer)
    }

    fn disconnect(&mut self, id: ConnId) {
        self.finish(id, None, 0);
    }

    fn handle_message(&mut self, id: ConnId, message: ClientMsg) {
        let Some(conn) = self.conns.get_mut(&id) else {
            return;
        };
        if !conn.greeted {
            match message {
                ClientMsg::Hello { version } if version == PROTOCOL_VERSION => conn.greeted = true,
                ClientMsg::Hello { version } => {
                    let message = format!(
                        "amacsclient speaks protocol {version} but the server speaks {PROTOCOL_VERSION}; restart the server"
                    );
                    self.finish(id, Some(message), 1);
                }
                _ => {
                    self.finish(
                        id,
                        Some("protocol error: expected a greeting".to_owned()),
                        1,
                    );
                }
            }
            return;
        }
        match message {
            ClientMsg::Hello { .. } => {}
            ClientMsg::Eval { source } => {
                let reply = match self.session.eval_quietly(&source) {
                    Ok(text) => ServerMsg::EvalResult { ok: true, text },
                    Err(error) => ServerMsg::EvalResult {
                        ok: false,
                        text: error.report.trim_end().to_owned(),
                    },
                };
                self.send(id, reply);
            }
            ClientMsg::Edit { files, wait, frame } => self.edit(id, &files, wait, frame),
            ClientMsg::Input(event) => {
                if let Some(frame) = conn.frame {
                    self.session.handle_frame_event(frame, event);
                }
            }
        }
    }

    fn edit(&mut self, id: ConnId, files: &[FileSpec], wait: bool, request: FrameRequest) {
        let has_client_frame = self.conns.values().any(|conn| conn.frame.is_some());
        let size = match request {
            FrameRequest::New { cols, rows } => Some((cols, rows)),
            FrameRequest::NewIfNone { cols, rows } if !has_client_frame => Some((cols, rows)),
            FrameRequest::NewIfNone { .. } | FrameRequest::None => None,
        };
        let frame = size.map(|(cols, rows)| {
            let mut editor = self.session.editor_mut();
            let frame = editor.create_frame(cols, rows);
            editor.select_frame(frame);
            frame
        });
        let reply = if frame.is_some() {
            let (font_family, font_size) = self.session.editor().frame_font();
            ServerMsg::Attached {
                font_family,
                font_size,
            }
        } else {
            ServerMsg::Opened
        };
        self.send(id, reply);

        let mut waiting = 0;
        {
            let mut editor = self.session.editor_mut();
            for file in files {
                match editor.find_file(&file.path) {
                    Ok(()) => {
                        if let Some(line) = file.line {
                            editor.goto_line(line as usize, file.column.unwrap_or(0) as usize);
                        }
                        if wait {
                            let buffer = editor.window().buffer;
                            editor.add_waiter(buffer, id);
                            waiting += 1;
                        }
                    }
                    Err(error) => editor.message(error.to_string()),
                }
            }
        }
        self.session.run_pending();

        let Some(conn) = self.conns.get_mut(&id) else {
            return;
        };
        conn.frame = frame;
        conn.waiting = (wait && waiting > 0).then_some(waiting);
        // A client with neither a frame nor anything to wait for is done.
        if conn.frame.is_none() && conn.waiting.is_none() {
            self.finish(id, None, 0);
        }
    }

    /// Acts on what the editor did while handling events, then brings every
    /// client's display up to date.
    fn after_events(&mut self) {
        let events = self.session.editor_mut().take_events();
        for event in events {
            match event {
                EditorEvent::WaiterReleased(id) => {
                    let Some(conn) = self.conns.get_mut(&id) else {
                        continue;
                    };
                    if let Some(waiting) = &mut conn.waiting {
                        *waiting = waiting.saturating_sub(1);
                        if *waiting == 0 {
                            self.finish(id, None, 0);
                        }
                    }
                }
                EditorEvent::FrameClosed(frame) => {
                    let owner = self
                        .conns
                        .iter()
                        .find(|(_, conn)| conn.frame == Some(frame));
                    if let Some((id, _)) = owner {
                        let id = *id;
                        self.finish(id, None, 0);
                    }
                }
            }
        }
        // Finishing a client can release others' waiters only through the
        // editor, which reports that on the next round.
        let frames: Vec<(ConnId, FrameId)> = self
            .conns
            .iter()
            .filter_map(|(id, conn)| Some((*id, conn.frame?)))
            .collect();
        for (id, frame) in frames {
            let ops = self.session.redisplay_frame(frame);
            if !ops.is_empty() {
                self.send(id, ServerMsg::Draw(ops));
            }
        }
    }

    /// Tells every client to exit and waits until they have been told.
    fn close_all(&mut self, message: Option<String>, status: u8) {
        let ids: Vec<ConnId> = self.conns.keys().copied().collect();
        let writers: Vec<JoinHandle<()>> = ids
            .into_iter()
            .filter_map(|id| self.finish(id, message.clone(), status))
            .collect();
        for writer in writers {
            let _ = writer.join();
        }
    }
}
