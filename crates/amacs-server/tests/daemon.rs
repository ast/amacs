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
        Self::start_with(|_, _| {})
    }

    /// Starts a server after `prepare` has had its directory and the
    /// command that starts it.
    fn start_with(prepare: impl FnOnce(&Path, &mut Command)) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut command = Self::command(dir.path());
        prepare(dir.path(), &mut command);
        let child = command.spawn().unwrap();
        let server = Self { child, dir };
        server.wait_until_listening();
        server
    }

    fn spawn(dir: &Path) -> Child {
        Self::command(dir).spawn().unwrap()
    }

    fn command(dir: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_amacs"));
        command
            .arg("--daemon")
            .env("AMACS_SOCKET", dir.join("sock"))
            .env("XDG_STATE_HOME", dir.join("state"))
            .env("XDG_CONFIG_HOME", dir.join("config"))
            .env("HOME", dir)
            // A checkout names an init file for the servers started in it.
            .env_remove("AMACS_INIT")
            // The server must not reach the clipboard of whoever runs the tests.
            .env_remove("WAYLAND_DISPLAY")
            .stdin(Stdio::null())
            .stderr(Stdio::piped());
        command
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
    /// client has applied the resulting draws.
    fn sync(&mut self) {
        self.send(ClientMsg::Sync);
        match self.receive() {
            Some(ServerMsg::Synced) => {}
            other => panic!("unexpected reply: {other:?}"),
        }
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
fn a_frame_is_told_whether_to_draw_with_the_gpu() {
    let server = Server::start();
    let frame = FrameRequest::New { cols: 40, rows: 10 };
    let gpu = |reply| match reply {
        ServerMsg::Attached { gpu, .. } => gpu,
        other => panic!("unexpected reply: {other:?}"),
    };

    let mut first = Client::connect(&server);
    assert!(!gpu(first.edit(&[], false, frame)));

    first.eval("(set-option! 'gpu-renderer #t)").unwrap();
    let mut second = Client::connect(&server);
    assert!(gpu(second.edit(&[], false, frame)));

    let refused = first.eval("(set-option! 'gpu-renderer \"gpu\")");
    assert!(refused.unwrap_err().contains("must be #t or #f"));
}

#[test]
fn the_environment_can_name_the_init_file() {
    let init = |dir: &Path, name: &str, width: u8| {
        let path = dir.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, format!("(set-option! 'tab-width {width})")).unwrap();
        path
    };
    // The file that AMACS_INIT names is loaded, not the one in the
    // directory for configuration.
    let server = Server::start_with(|dir, command| {
        init(dir, "config/amacs/init.scm", 3);
        command.env("AMACS_INIT", init(dir, "other.scm", 5));
    });
    let mut client = Client::connect(&server);
    assert_eq!(client.eval("(option 'tab-width)").unwrap(), "5");

    // A name of no file is passed over.
    let server = Server::start_with(|dir, command| {
        init(dir, "config/amacs/init.scm", 3);
        command.env("AMACS_INIT", "no/such/init.scm");
    });
    let mut client = Client::connect(&server);
    assert_eq!(client.eval("(option 'tab-width)").unwrap(), "3");
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
fn a_text_field_of_a_browser_gets_a_window_that_closes_when_it_is_done() {
    use tungstenite::Message;
    use tungstenite::client::IntoClientRequest;

    let server = Server::start();
    let mut first = Client::attach(&server, 40, 6, &[], false);
    first.type_text("at work");
    // The test opens the window itself, where the editor would start a
    // program that does.
    let port: u16 = first
        .eval(
            "(set-option! 'ghost-text-port 0)
             (set-option! 'ghost-text-new-window #t)
             (define (start-process . ignored) #t)
             (ghost-text-start)
             (websocket-server-port %ghost-server)",
        )
        .unwrap()
        .parse()
        .unwrap();

    let stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let mut request = format!("ws://localhost:{port}/")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("Origin", "moz-extension://test".parse().unwrap());
    let (mut browser, _) = tungstenite::client(request, stream).unwrap();
    let field = serde_json::json!({
        "title": "A page", "url": "example.com", "syntax": "",
        "text": "from the page", "selections": [{ "start": 13, "end": 13 }],
    });
    browser.send(Message::text(field.to_string())).unwrap();
    let start = Instant::now();
    while first.eval("(ghost-text-fields)").unwrap() == "()" {
        assert!(start.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(5));
    }

    let mut window = Client::attach(&server, 40, 6, &[], false);
    assert_eq!(window.screen.row(0), "from the page");
    // The frame that was in use is left as it was.
    first.sync();
    assert_eq!(first.screen.row(0), "at work");

    window.type_text("!");
    window.send(ClientMsg::Input(InputEvent::Key("C-c".parse().unwrap())));
    window.send(ClientMsg::Input(InputEvent::Key("C-c".parse().unwrap())));
    assert_eq!(
        window.receive(),
        Some(ServerMsg::Exit {
            message: None,
            status: 0
        })
    );
    let sent: serde_json::Value = match browser.read().unwrap() {
        Message::Text(text) => serde_json::from_str(text.as_str()).unwrap(),
        other => panic!("unexpected message: {other:?}"),
    };
    assert_eq!(sent["text"], "from the page!");
    assert!(matches!(browser.read(), Ok(Message::Close(_))));
    first.sync();
    assert_eq!(first.screen.row(0), "at work");
}

/// The socket on which a server takes the questions of programs.
fn askpass_socket(server: &Server) -> PathBuf {
    let start = Instant::now();
    loop {
        let found = fs::read_dir(server.dir.path())
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .find(|path| {
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                name.starts_with("askpass-") && name.ends_with(".sock")
            });
        if let Some(found) = found {
            return found;
        }
        assert!(start.elapsed() < Duration::from_secs(20), "no socket");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn what_a_program_asks_is_answered_in_the_frame_of_a_person() {
    use amacs_proto::{Answer, Ask};

    let server = Server::start();
    let socket = askpass_socket(&server);
    // Beside the socket is the helper, under a name of this server's own.
    let link = socket.with_extension("");
    assert!(link.is_symlink(), "{}", link.display());
    let ask = |ticket: &str, prompt: &str| {
        let stream = UnixStream::connect(&socket).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        write_msg(
            &stream,
            &Ask {
                ticket: ticket.to_owned(),
                prompt: prompt.to_owned(),
                kind: String::new(),
            },
        )
        .unwrap();
        stream
    };
    let answer = |stream: &UnixStream| read_msg::<_, Answer>(stream).unwrap().unwrap();

    // Nobody asks without a ticket, and a program that evaluates code is
    // given none.
    let refused = answer(&ask("made-up", "Password: "));
    assert!(matches!(refused, Answer::Refused(_)), "{refused:?}");
    let mut program = Client::connect(&server);
    let error = program
        .eval("(askpass-environment \"a program\")")
        .unwrap_err();
    assert!(error.contains("a program is not given a ticket"), "{error}");

    // A person starts something that may ask, in a terminal of theirs.
    let mut person = Client::attach(&server, 70, 10, &[], false);
    let mut other = Client::attach(&server, 70, 10, &[], false);
    person
        .keys("M-:")
        .type_text("(define environment (askpass-environment \"git push\"))")
        .keys("RET");
    let ticket = program
        .eval("(cadr (assoc \"AMACS_ASKPASS_TICKET\" environment))")
        .unwrap();
    let ticket = ticket.trim_matches('"');
    let helper = program
        .eval("(cadr (assoc \"SSH_ASKPASS\" environment))")
        .unwrap();
    assert_eq!(Path::new(helper.trim_matches('"')), link);

    // The other frame was used since: the question still comes where the
    // program was started.
    other.keys("C-f");
    let waiting = ask(ticket, "Password for 'https://example.org': ");
    let start = Instant::now();
    while !person.echo().starts_with("git push Password for") {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "{}",
            person.echo()
        );
        std::thread::sleep(Duration::from_millis(10));
        person.sync();
    }
    other.sync();
    assert!(!other.echo().contains("Password"), "{}", other.echo());
    person.type_text("hunter2");
    assert!(person.echo().ends_with(": *******"), "{}", person.echo());
    person.keys("RET");
    assert_eq!(answer(&waiting), Answer::Given("hunter2".to_owned()));

    // C-g is no answer.
    let waiting = ask(ticket, "Password: ");
    let start = Instant::now();
    while !person.echo().starts_with("git push Password:") {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "{}",
            person.echo()
        );
        std::thread::sleep(Duration::from_millis(10));
        person.sync();
    }
    person.keys("C-g");
    assert_eq!(answer(&waiting), Answer::None);

    // A helper that goes away, as with an ssh that waited too long,
    // takes its question off the screen.
    let waiting = ask(ticket, "Passphrase: ");
    let start = Instant::now();
    while !person.echo().starts_with("git push Passphrase:") {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "{}",
            person.echo()
        );
        std::thread::sleep(Duration::from_millis(10));
        person.sync();
    }
    drop(waiting);
    let start = Instant::now();
    while person.echo().contains("Passphrase") {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "{}",
            person.echo()
        );
        std::thread::sleep(Duration::from_millis(10));
        person.sync();
    }
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
        "work.txt has changes that were not saved; use M-x recover-this-file"
    );
    client
        .keys("M-x")
        .type_text("recover-this-file")
        .keys("RET");
    assert_eq!(client.screen.row(1), "unsaved");
}

/// The journal of the one file whose undo history the server keeps.
fn undo_journal(server: &Server) -> Option<PathBuf> {
    let entries = fs::read_dir(server.dir.path().join("state/amacs/undo")).ok()?;
    entries
        .flatten()
        .map(|entry| entry.path())
        // A journal is written under a temporary name first, which starts
        // with a dot.
        .find(|path| !path.file_name().unwrap().to_string_lossy().starts_with('.'))
}

/// Waits until the journal has been written to since `since`: that takes
/// an idle second.
fn wait_for_undo_journal(server: &Server, since: std::time::SystemTime) {
    let start = Instant::now();
    let written = || {
        undo_journal(server)
            .and_then(|path| fs::metadata(path).ok()?.modified().ok())
            .is_some_and(|modified| modified >= since)
    };
    while !written() {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "the undo history was not written"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn what_was_typed_a_second_ago_survives_a_power_failure_with_its_history() {
    let mut server = Server::start();
    let file = server.file("work.txt", "saved\n");
    let mut client = Client::attach(&server, 70, 6, &[&file], false);
    client.keys("M->").type_text("one").keys("RET");
    client.type_text("two");
    let typed = std::time::SystemTime::now();
    wait_for_undo_journal(&server, typed);
    assert!(
        !server.dir.path().join("state/amacs/auto-save").exists(),
        "this is sooner than auto-save"
    );

    // Killed outright, the server writes nothing more.
    server.crash_and_restart();
    let mut client = Client::attach(&server, 70, 6, &[&file], false);
    assert_eq!(
        client.echo(),
        "work.txt has changes that were not saved; use M-x recover-this-file"
    );
    assert_eq!(client.screen.row(1), "", "the file is as it was saved");
    client
        .keys("M-x")
        .type_text("recover-this-file")
        .keys("RET");
    assert_eq!(
        client.echo(),
        "Recovered the unsaved changes; save the buffer to keep them"
    );
    assert_eq!(client.screen.row(1), "one");
    assert_eq!(client.screen.row(2), "two");
    // Each step that was typed is there to undo, not one change for all.
    client.keys("C-/");
    assert_eq!(client.screen.row(1), "one");
    assert_eq!(client.screen.row(2), "");
    client.keys("C-?");
    assert_eq!(client.screen.row(2), "two");

    // Saved, the history is of the file as it is now, and is there after
    // the server was stopped and started in the ordinary way too.
    client.keys("C-x C-s");
    assert_eq!(fs::read_to_string(&file).unwrap(), "saved\none\ntwo");
    wait_for_undo_journal(&server, std::time::SystemTime::now());
    server.crash_and_restart();
    let mut client = Client::attach(&server, 70, 6, &[&file], false);
    assert_eq!(client.echo(), "", "nothing is left to recover");
    assert_eq!(client.screen.row(2), "two");
    client.keys("C-/");
    assert_eq!(client.screen.row(1), "one");
    assert_eq!(client.screen.row(2), "");

    // A file that something else changed has no history any more.
    drop(client);
    fs::write(&file, "changed elsewhere\n").unwrap();
    wait_for_undo_journal(&server, std::time::SystemTime::now());
    server.crash_and_restart();
    let mut client = Client::attach(&server, 70, 6, &[&file], false);
    assert_eq!(client.screen.row(0), "changed elsewhere");
    client.keys("C-/");
    assert_eq!(client.screen.row(0), "changed elsewhere");
    assert_eq!(client.echo(), "No further undo information");
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
        "work.txt has changes that were not saved; use M-x recover-this-file"
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
