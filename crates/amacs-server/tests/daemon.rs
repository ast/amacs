//! Tests of the real server process, driven over its socket the way
//! `amacsclient` drives it.

use std::fs;
use std::io::{BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use amacs_proto::{
    ClientMsg, FileSpec, FrameRequest, InputEvent, PROTOCOL_VERSION, ServerMsg, parse_keys,
    read_msg, write_msg,
};
use amacs_testkit::Screen;

/// A server process with its own socket, state and (empty) configuration.
struct Server {
    child: Child,
    dir: tempfile::TempDir,
}

impl Server {
    fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let child = Self::spawn(dir.path());
        let server = Self { child, dir };
        server.wait_until_listening();
        server
    }

    fn spawn(dir: &Path) -> Child {
        Command::new(env!("CARGO_BIN_EXE_amacs"))
            .arg("--daemon")
            .env("AMACS_SOCKET", dir.join("sock"))
            .env("XDG_STATE_HOME", dir.join("state"))
            .env("XDG_CONFIG_HOME", dir.join("config"))
            .env("HOME", dir)
            // The server must not reach the clipboard of whoever runs the tests.
            .env_remove("WAYLAND_DISPLAY")
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    }

    fn socket(&self) -> PathBuf {
        self.dir.path().join("sock")
    }

    fn wait_until_listening(&self) {
        let start = Instant::now();
        while UnixStream::connect(self.socket()).is_err() {
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "the server did not start"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Kills the server without giving it a chance to clean up, and starts
    /// another on the same socket and state.
    fn crash_and_restart(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
        self.child = Self::spawn(self.dir.path());
        self.wait_until_listening();
    }

    fn file(&self, name: &str, content: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        fs::write(&path, content).unwrap();
        path
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    screen: Screen,
}

impl Client {
    fn connect(server: &Server) -> Self {
        let stream = UnixStream::connect(server.socket()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        let mut client = Self {
            reader: BufReader::new(stream.try_clone().unwrap()),
            writer: stream,
            screen: Screen::default(),
        };
        client.send(ClientMsg::Hello {
            version: PROTOCOL_VERSION,
        });
        client
    }

    fn send(&mut self, message: ClientMsg) {
        write_msg(&mut self.writer, &message).unwrap();
    }

    /// The next message that is not a draw. Draws are applied to the screen.
    fn receive(&mut self) -> Option<ServerMsg> {
        loop {
            match read_msg::<_, ServerMsg>(&mut self.reader).unwrap()? {
                ServerMsg::Draw(ops) => self.screen.apply(&ops),
                other => return Some(other),
            }
        }
    }

    fn eval(&mut self, source: &str) -> Result<String, String> {
        self.send(ClientMsg::Eval {
            source: source.to_owned(),
        });
        match self.receive() {
            Some(ServerMsg::EvalResult { ok: true, text, .. }) => Ok(text),
            Some(ServerMsg::EvalResult {
                ok: false, text, ..
            }) => Err(text),
            other => panic!("unexpected reply: {other:?}"),
        }
    }

    /// Waits until the server has handled everything sent so far and this
    /// client has applied the resulting draws. The server draws after each
    /// batch of messages, so the draw for earlier input is sure to precede
    /// the reply to a second request.
    fn sync(&mut self) {
        self.eval("1").unwrap();
        self.eval("1").unwrap();
    }

    fn edit(&mut self, files: &[&Path], wait: bool, frame: FrameRequest) -> ServerMsg {
        let files = files
            .iter()
            .map(|path| FileSpec {
                path: path.to_path_buf(),
                line: None,
                column: None,
            })
            .collect();
        self.send(ClientMsg::Edit { files, wait, frame });
        self.receive().expect("a reply to the edit request")
    }

    /// Opens a frame of the given size, as `amacsclient -t` does.
    fn attach(server: &Server, cols: u16, rows: u16, files: &[&Path], wait: bool) -> Self {
        let mut client = Self::connect(server);
        let reply = client.edit(files, wait, FrameRequest::New { cols, rows });
        assert!(matches!(reply, ServerMsg::Attached { .. }), "{reply:?}");
        client.sync();
        client
    }

    fn keys(&mut self, keys: &str) -> &mut Self {
        for key in parse_keys(keys).unwrap() {
            self.send(ClientMsg::Input(InputEvent::Key(key)));
        }
        self.sync();
        self
    }

    fn type_text(&mut self, text: &str) -> &mut Self {
        for c in text.chars() {
            self.send(ClientMsg::Input(InputEvent::Key(amacs_proto::Key::char(c))));
        }
        self.sync();
        self
    }

    fn echo(&self) -> String {
        self.screen.row(self.screen.size().1 - 1)
    }
}

#[test]
fn programs_get_errors_in_parts_and_what_the_code_wrote() {
    let server = Server::start();
    let library = server.file(
        "library.scm",
        "(define (first-word words)\n  (display \"looking\")\n  (car words))\n",
    );
    let mut client = Client::connect(&server);
    client.eval(&format!("(load {library:?})")).unwrap();
    let library = library.display();

    client.send(ClientMsg::Eval {
        source: "(display \"hello \") (write 'there) (+ 1 2)".to_owned(),
    });
    let Some(ServerMsg::EvalResult {
        ok: true,
        text,
        output,
        failure: None,
    }) = client.receive()
    else {
        panic!("expected a value");
    };
    assert_eq!((text.as_str(), output.as_str()), ("3", "hello there"));

    client.send(ClientMsg::Eval {
        source: "(define (count-words text) (list (first-word text)))\n(count-words 5)".to_owned(),
    });
    let Some(ServerMsg::EvalResult {
        ok: false,
        text,
        output,
        failure: Some(failure),
    }) = client.receive()
    else {
        panic!("expected an error");
    };
    assert_eq!(output, "looking");
    assert_eq!(failure.message, "car: expected a pair, got 5");
    assert_eq!(failure.location, Some(format!("{library}:3:3")));
    assert_eq!(
        failure.trace,
        vec![
            (
                Some("first-word".to_owned()),
                Some(format!("{library}:3:3"))
            ),
            (Some("count-words".to_owned()), Some("eval:1:34".to_owned())),
            (None, Some("eval:2:1".to_owned())),
        ]
    );
    assert!(
        text.starts_with("error: car: expected a pair, got 5\n  at "),
        "{text}"
    );
}

#[test]
fn evaluates_scheme_for_programs() {
    let server = Server::start();
    let file = server.file("notes.txt", "first\nsecond\n");
    let mut client = Client::connect(&server);

    // The operations that the Emacs MCP server performs.
    assert_eq!(client.eval("(+ 1 2)").unwrap(), "3");
    assert_eq!(client.eval(&format!("(find-file {file:?})")).unwrap(), "");
    assert_eq!(
        client.eval("(buffer-list)").unwrap(),
        "(\"notes.txt\" \"*scratch*\")"
    );
    assert_eq!(
        client.eval("(buffer-string \"notes.txt\")").unwrap(),
        "\"first\\nsecond\\n\""
    );
    client.eval("(goto-char 8)").unwrap();
    assert_eq!(
        client
            .eval("(list (point) (line-number-at-pos) (current-column))")
            .unwrap(),
        "(8 2 2)"
    );

    let error = client.eval("(car 1)").unwrap_err();
    assert!(error.contains("car: expected a pair, got 1"), "{error}");
    assert_eq!(
        client.eval("(+ 2 2)").unwrap(),
        "4",
        "an error does not end the connection"
    );
}

#[test]
fn two_frames_share_buffers_but_not_prompts() {
    let server = Server::start();
    let mut one = Client::attach(&server, 40, 6, &[], false);
    let mut two = Client::attach(&server, 30, 5, &[], false);
    assert_eq!(one.screen.size(), (40, 6));
    assert_eq!(two.screen.size(), (30, 5));

    one.type_text("typed in one");
    two.sync();
    assert_eq!(two.screen.row(0), "typed in one");

    two.keys("M-g g").type_text("12");
    one.sync();
    assert_eq!(two.echo(), "Goto line: 12");
    assert_eq!(one.echo(), "");
    two.keys("C-g");

    // Closing one client's terminal leaves the other, and the server, alone.
    one.keys("C-x 2");
    one.send(ClientMsg::Input(InputEvent::Key("C-x".parse().unwrap())));
    one.send(ClientMsg::Input(InputEvent::Key("C-c".parse().unwrap())));
    assert_eq!(
        one.receive(),
        Some(ServerMsg::Exit {
            message: None,
            status: 0
        })
    );
    two.type_text("!");
    assert_eq!(two.screen.row(0), "!typed in one");
}

#[test]
fn a_waiting_client_returns_when_its_file_is_finished() {
    let server = Server::start();
    let commit = server.file("COMMIT_EDITMSG", "");
    let mut frame = Client::attach(&server, 60, 6, &[], false);

    // What `EDITOR=amacsclient git commit` does from another terminal.
    let mut waiter = Client::connect(&server);
    let reply = waiter.edit(
        &[&commit],
        true,
        FrameRequest::NewIfNone { cols: 80, rows: 24 },
    );
    assert_eq!(
        reply,
        ServerMsg::Opened,
        "a frame exists, so the file opens there"
    );

    frame.sync();
    assert_eq!(
        frame.screen.row(4),
        " --  COMMIT_EDITMSG   L1 C0  (Fundamental)"
    );
    frame.type_text("Fix the bug").keys("C-x C-s C-x #");
    assert_eq!(
        waiter.receive(),
        Some(ServerMsg::Exit {
            message: None,
            status: 0
        })
    );
    assert_eq!(fs::read_to_string(&commit).unwrap(), "Fix the bug");
    assert_eq!(frame.screen.row(4), " --  *scratch*   L1 C0  (Fundamental)");
}

#[test]
fn a_waiting_frame_closes_when_its_files_are_finished() {
    let server = Server::start();
    let file = server.file("a.txt", "a");
    let mut client = Client::attach(&server, 60, 6, &[&file], true);
    assert_eq!(client.screen.row(0), "a");
    client.send(ClientMsg::Input(InputEvent::Key("C-x".parse().unwrap())));
    client.send(ClientMsg::Input(InputEvent::Key("#".parse().unwrap())));
    assert_eq!(
        client.receive(),
        Some(ServerMsg::Exit {
            message: None,
            status: 0
        })
    );

    // Without a terminal and without waiting, there is nothing to stay for.
    let mut quick = Client::connect(&server);
    assert_eq!(
        quick.edit(&[&file], false, FrameRequest::None),
        ServerMsg::Opened
    );
    assert_eq!(
        quick.receive(),
        Some(ServerMsg::Exit {
            message: None,
            status: 0
        })
    );
}

#[test]
fn a_vanished_client_takes_its_frame_with_it() {
    let server = Server::start();
    let mut stays = Client::attach(&server, 60, 6, &[], false);
    let leaves = Client::attach(&server, 60, 6, &[], false);
    drop(leaves);
    // The server notices the closed socket and carries on.
    stays.type_text("still here");
    assert_eq!(stays.screen.row(0), "still here");
}

#[test]
fn bad_clients_are_turned_away_without_harm() {
    let server = Server::start();

    let mut old = Client {
        reader: BufReader::new(UnixStream::connect(server.socket()).unwrap()),
        writer: UnixStream::connect(server.socket()).unwrap(),
        screen: Screen::default(),
    };
    old.reader = BufReader::new(old.writer.try_clone().unwrap());
    old.send(ClientMsg::Hello {
        version: PROTOCOL_VERSION + 1,
    });
    match old.receive() {
        Some(ServerMsg::Exit {
            message: Some(message),
            status: 1,
        }) => {
            assert!(message.contains("amacsclient --kill-server"), "{message}");
        }
        other => panic!("unexpected reply: {other:?}"),
    }

    let mut garbage = UnixStream::connect(server.socket()).unwrap();
    garbage.write_all(&[0xff; 64]).unwrap();
    drop(garbage);

    let mut rude = Client {
        reader: BufReader::new(UnixStream::connect(server.socket()).unwrap()),
        writer: UnixStream::connect(server.socket()).unwrap(),
        screen: Screen::default(),
    };
    rude.reader = BufReader::new(rude.writer.try_clone().unwrap());
    rude.send(ClientMsg::Eval { source: "1".into() });
    assert!(matches!(
        rude.receive(),
        Some(ServerMsg::Exit { status: 1, .. })
    ));

    assert_eq!(Client::connect(&server).eval("(+ 1 1)").unwrap(), "2");
}

#[test]
fn a_second_server_refuses_to_take_over_the_socket() {
    let server = Server::start();
    let output = Server::spawn(server.dir.path()).wait_with_output().unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("a server is already listening"), "{stderr}");
    assert_eq!(
        Client::connect(&server).eval("1").unwrap(),
        "1",
        "the first is unharmed"
    );
}

#[test]
fn exiting_the_editor_stops_the_server_and_its_clients() {
    let mut server = Server::start();
    // The server says which process it is, for stopping it from outside.
    let pid_file = amacs_proto::pid_path(&server.socket());
    assert_eq!(
        fs::read_to_string(&pid_file).unwrap().trim(),
        server.child.id().to_string()
    );
    let mut frame = Client::attach(&server, 40, 6, &[], false);
    let mut other = Client::connect(&server);
    assert_eq!(other.eval("(exit-editor)").unwrap(), "");
    assert_eq!(
        frame.receive(),
        Some(ServerMsg::Exit {
            message: None,
            status: 0
        })
    );
    assert!(server.child.wait().unwrap().success());
    assert!(
        !server.socket().exists(),
        "the socket is removed on a clean exit"
    );
    assert!(!pid_file.exists(), "and so is the note of the process id");
}

#[test]
fn a_termination_signal_auto_saves_unsaved_work() {
    let mut server = Server::start();
    let file = server.file("work.txt", "saved\n");
    let mut client = Client::attach(&server, 70, 6, &[&file], false);
    client.keys("M->").type_text("unsaved");

    let pid = server.child.id().to_string();
    assert!(
        Command::new("kill")
            .args(["-TERM", &pid])
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        client.receive(),
        Some(ServerMsg::Exit {
            message: None,
            status: 0
        })
    );
    assert!(server.child.wait().unwrap().success());
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "saved\n",
        "the file is not written"
    );

    server.child = Server::spawn(server.dir.path());
    server.wait_until_listening();
    let mut client = Client::attach(&server, 70, 6, &[&file], false);
    assert_eq!(
        client.echo(),
        "work.txt has auto-save data; use M-x recover-this-file"
    );
    client
        .keys("M-x")
        .type_text("recover-this-file")
        .keys("RET");
    assert_eq!(client.screen.row(1), "unsaved");
}

#[test]
fn work_survives_the_server_being_killed() {
    let mut server = Server::start();
    let file = server.file("work.txt", "saved\n");
    let mut client = Client::attach(&server, 70, 6, &[&file], false);
    client.keys("M->").type_text("typed before the crash");

    // Auto-save runs once the editor has been idle for five seconds.
    let auto_save_dir = server.dir.path().join("state/amacs/auto-save");
    let start = Instant::now();
    // The data is written under a temporary name first, which starts with a dot.
    let auto_saved = || {
        fs::read_dir(&auto_save_dir).is_ok_and(|entries| {
            entries
                .flatten()
                .any(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        })
    };
    while !auto_saved() {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "nothing was auto-saved"
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    server.crash_and_restart();
    assert_eq!(
        client.receive(),
        None,
        "the client sees the connection close"
    );

    let mut client = Client::attach(&server, 70, 6, &[&file], false);
    assert_eq!(
        client.echo(),
        "work.txt has auto-save data; use M-x recover-this-file"
    );
    client
        .keys("M-x")
        .type_text("recover-this-file")
        .keys("RET");
    assert_eq!(client.screen.row(1), "typed before the crash");
    client.keys("C-x C-s");
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "saved\ntyped before the crash"
    );
    assert_eq!(fs::read_dir(&auto_save_dir).unwrap().count(), 0);
}
