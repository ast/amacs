//! The server for language models, driving a real amacs server the way a
//! model's client drives it: one message of the protocol at a time.

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use amacs_mcp::Mcp;
use serde_json::{Value, json};

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

    fn restart(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
        self.child = Self::spawn(self.dir.path());
        self.wait_until_listening();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Calls a tool and gives its text and whether it reported an error.
fn call(mcp: &mut Mcp, tool: &str, arguments: Value) -> (String, bool) {
    let reply = mcp
        .handle(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": tool, "arguments": arguments },
        }))
        .expect("a call gets a reply");
    let result = &reply["result"];
    let text = result["content"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_owned();
    (text, result["isError"] == true)
}

/// Calls a tool that is expected to work.
fn ok(mcp: &mut Mcp, tool: &str, arguments: Value) -> String {
    let (text, failed) = call(mcp, tool, arguments);
    assert!(!failed, "{tool} failed: {text}");
    text
}

#[test]
fn a_model_evaluates_code_and_gets_errors_with_their_trace() {
    let server = Server::start();
    let mut mcp = Mcp::new(server.socket());
    assert_eq!(ok(&mut mcp, "eval", json!({ "code": "(+ 1 2)" })), "3");
    assert_eq!(
        ok(
            &mut mcp,
            "eval",
            json!({ "code": "(define (twice x) (* 2 x))" })
        ),
        "(no value)"
    );
    assert_eq!(
        ok(
            &mut mcp,
            "eval",
            json!({ "code": "(display \"working\") (twice 21)" })
        ),
        "42\n--- written by the code ---\nworking"
    );
    let (text, failed) = call(
        &mut mcp,
        "eval",
        json!({ "code": "(define (inner x) (car x))\n(define (outer) (list (inner 5)))\n(outer)" }),
    );
    assert!(failed);
    assert_eq!(
        text,
        "error: car: expected a pair, got 5\n  at eval:1:19\n  in inner, at eval:1:19\n  in outer, at eval:2:23\n  in top level, at eval:3:1"
    );
    // The connection, and the editor, are fine afterwards.
    assert_eq!(ok(&mut mcp, "eval", json!({ "code": "(twice 4)" })), "8");
}

#[test]
fn a_model_presses_keys_and_reads_the_screen() {
    let server = Server::start();
    let mut mcp = Mcp::new(server.socket());
    let screen = ok(&mut mcp, "type", json!({ "text": "hello\nworld" }));
    assert!(
        screen.starts_with("100x30 frame, cursor at row 1, column 5"),
        "{screen}"
    );
    assert!(screen.contains("\n 0|hello\n 1|world\n"), "{screen}");
    assert!(
        screen.contains("28| **  *scratch*   L2 C5  (Fundamental)"),
        "{screen}"
    );
    let screen = ok(&mut mcp, "keys", json!({ "keys": "C-a C-k M-x" }));
    assert!(screen.contains("M-x"), "{screen}");
    let screen = ok(&mut mcp, "type", json!({ "text": "forward-ch" }));
    assert!(screen.contains("forward-char"), "{screen}");
    let screen = ok(&mut mcp, "keys", json!({ "keys": "C-g" }));
    assert!(screen.trim_end().ends_with("29|Quit"), "{screen}");
    // Keys that are not keys are refused with a reason.
    let (text, failed) = call(&mut mcp, "keys", json!({ "keys": "C-" }));
    assert!(failed && text.contains("C-"), "{text}");

    // What a command does after waiting for a program is waited for.
    ok(
        &mut mcp,
        "eval",
        json!({ "code": "(define-command (slow) \"Insert after running a program.\" (insert (cadr (run-process \"sh\" '(\"-c\" \"sleep 0.2; echo done-now\")))))" }),
    );
    let screen = ok(&mut mcp, "keys", json!({ "keys": "M-x" }));
    assert!(screen.contains("M-x"), "{screen}");
    let screen = ok(
        &mut mcp,
        "type",
        json!({ "text": "slow\n", "wait_for": "done-now" }),
    );
    assert!(screen.contains(" 1|done-now"), "{screen}");
    let screen = ok(
        &mut mcp,
        "screen",
        json!({ "wait_for": "never there", "timeout_ms": 50 }),
    );
    assert!(
        screen.ends_with("(the screen did not come to show \"never there\" within 50 ms)"),
        "{screen}"
    );

    let screen = ok(&mut mcp, "resize", json!({ "cols": 40, "rows": 10 }));
    assert!(screen.starts_with("40x10 frame"), "{screen}");
}

#[test]
fn a_model_gets_a_picture_of_the_screen() {
    let server = Server::start();
    let mut mcp = Mcp::new(server.socket());
    ok(&mut mcp, "resize", json!({ "cols": 30, "rows": 6 }));
    ok(&mut mcp, "type", json!({ "text": "pixels" }));
    let reply = mcp
        .handle(&json!({
            "jsonrpc": "2.0", "id": 7, "method": "tools/call",
            "params": { "name": "screen", "arguments": { "image": true } },
        }))
        .unwrap();
    let content = &reply["result"]["content"][0];
    assert_eq!(content["type"], "image", "{reply}");
    assert_eq!(content["mimeType"], "image/png");
    let data = content["data"].as_str().unwrap();
    // The signature of a PNG file, in base64, and more than an empty one.
    assert!(
        data.starts_with("iVBORw0KGgo"),
        "{}",
        &data[..data.len().min(40)]
    );
    assert!(data.len() > 1000, "{}", data.len());
    // The text of the screen is still there to ask for.
    assert!(ok(&mut mcp, "screen", json!({})).contains(" 0|pixels"));
}

#[test]
fn a_model_finds_out_what_the_editor_has() {
    let server = Server::start();
    let mut mcp = Mcp::new(server.socket());
    let text = ok(
        &mut mcp,
        "describe",
        json!({ "name": "add-hook!", "source": true }),
    );
    assert!(
        text.starts_with(
            "add-hook! is a procedure: (add-hook! name f)\nAdds a procedure to the end of a hook"
        ),
        "{text}"
    );
    assert!(
        text.contains("Defined at runtime/core/hooks.scm:"),
        "{text}"
    );
    assert!(text.contains("(define (add-hook! name f)\n"), "{text}");
    let text = ok(&mut mcp, "describe", json!({ "name": "save-buffer" }));
    assert!(
        text.contains("It is a command, built in: Save the current buffer in its file."),
        "{text}"
    );
    let text = ok(&mut mcp, "describe", json!({ "name": "tab-width" }));
    assert!(
        text.contains("It is an option; its value here is 8"),
        "{text}"
    );
    let text = ok(
        &mut mcp,
        "describe",
        json!({ "name": "car", "source": true }),
    );
    assert!(
        text.contains("(no source: it is not a procedure written in Scheme)"),
        "{text}"
    );

    let text = ok(&mut mcp, "apropos", json!({ "pattern": "hook" }));
    assert!(
        text.contains("procedure add-hook! — Adds a procedure to the end of a hook"),
        "{text}"
    );
    let text = ok(&mut mcp, "apropos", json!({ "pattern": "rectangle" }));
    assert!(
        text.contains("command   kill-rectangle — Kill the rectangle"),
        "{text}"
    );
    assert_eq!(
        ok(&mut mcp, "apropos", json!({ "pattern": "zzzz" })),
        "nothing has \"zzzz\" in its name"
    );
}

#[test]
fn a_model_follows_what_goes_wrong_in_commands() {
    let server = Server::start();
    let mut mcp = Mcp::new(server.socket());
    assert_eq!(
        ok(&mut mcp, "messages", json!({})),
        "(the message log is empty)"
    );
    ok(
        &mut mcp,
        "eval",
        json!({ "code": "(define (helper x) (vector-ref x 3))\n(define-command (broken) \"Fail.\" (list (helper (vector 1))))\n(global-set-key \"C-c x\" 'broken)" }),
    );
    // The echo area has the message; the log has where and how.
    let screen = ok(&mut mcp, "keys", json!({ "keys": "C-c x" }));
    assert!(
        screen
            .trim_end()
            .ends_with("29|vector-ref: there is no item number 3"),
        "{screen}"
    );
    let log = ok(&mut mcp, "messages", json!({}));
    assert!(log.contains("error: vector-ref: there is no item number 3\n  at eval:1:20\n  in helper, at eval:1:20\n  in broken, at eval:2:40"), "{log}");

    // Tracing shows the calls that lead there.
    ok(
        &mut mcp,
        "eval",
        json!({ "code": "(trace! 'helper) (guard (e (#t #f)) (helper (vector 1 2 3 4))) (guard (e (#t #f)) (helper 5))" }),
    );
    let log = ok(&mut mcp, "messages", json!({ "lines": 4 }));
    assert_eq!(
        log,
        "trace: (helper #(1 2 3 4))\ntrace: helper => 4\ntrace: (helper 5)\ntrace: helper raised vector-ref: expected a vector, got 5"
    );
    assert_eq!(
        ok(&mut mcp, "eval", json!({ "code": "(traced)" })),
        "(helper)"
    );
    ok(
        &mut mcp,
        "eval",
        json!({ "code": "(untrace! 'helper) (helper (vector 1 2 3 4))" }),
    );
    assert_eq!(
        ok(&mut mcp, "messages", json!({ "lines": 1 })),
        "trace: helper raised vector-ref: expected a vector, got 5"
    );
}

#[test]
fn a_model_works_with_buffers_and_files_and_reloads_code() {
    let server = Server::start();
    let dir = server.dir.path().to_owned();
    let mut mcp = Mcp::new(server.socket());
    let notes = dir.join("notes.txt");
    std::fs::write(&notes, "one\ntwo\nthree\nfour\n").unwrap();
    let screen = ok(
        &mut mcp,
        "open_file",
        json!({ "path": notes.display().to_string(), "line": 3 }),
    );
    assert!(
        screen.contains("cursor at row 2, column 0") && screen.contains(" 2|three"),
        "{screen}"
    );
    ok(&mut mcp, "type", json!({ "text": "x" }));
    let buffers = ok(&mut mcp, "buffers", json!({}));
    assert!(
        buffers.contains(&format!(
            "notes.txt  (text-mode)  {}  [modified]",
            notes.display()
        )),
        "{buffers}"
    );
    assert!(
        buffers.contains("*scratch*  (fundamental-mode)"),
        "{buffers}"
    );
    assert_eq!(
        ok(
            &mut mcp,
            "read_buffer",
            json!({ "name": "notes.txt", "start_line": 2, "end_line": 3 })
        ),
        "    2|two\n    3|xthree"
    );
    let (text, failed) = call(&mut mcp, "read_buffer", json!({ "name": "nope" }));
    assert!(failed && text.contains("No such buffer: nope"), "{text}");

    // Code in a file: loaded, changed, loaded again, with errors placed.
    let library = dir.join("library.scm");
    std::fs::write(&library, "(define (answer) 1)\n(answer)\n").unwrap();
    let path = library.display().to_string();
    assert_eq!(ok(&mut mcp, "load", json!({ "path": path })), "1");
    std::fs::write(&library, "(define (answer) 2)\n(deftest answers (check-equal? (answer) 2))\n(deftest fails (check-equal? (answer) 3))\n").unwrap();
    ok(&mut mcp, "load", json!({ "path": path }));
    assert_eq!(ok(&mut mcp, "eval", json!({ "code": "(answer)" })), "2");
    let report = ok(&mut mcp, "run_tests", json!({}));
    assert!(
        report.starts_with(
            "1 passed, 1 failed\nFAILED fails: check-equal? failed: got 2 but expected 3"
        ),
        "{report}"
    );
    std::fs::write(&library, "(define (answer) 3)\n(car (answer))\n").unwrap();
    let (text, failed) = call(&mut mcp, "load", json!({ "path": path }));
    assert!(failed && text.contains(&format!("at {path}:2:1")), "{text}");
}

#[test]
fn a_models_frame_stays_out_of_the_users_way() {
    let mut server = Server::start();
    let mut mcp = Mcp::new(server.socket());
    let file = server.dir.path().join("mine.txt");
    std::fs::write(&file, "mine\n").unwrap();
    ok(
        &mut mcp,
        "open_file",
        json!({ "path": file.display().to_string() }),
    );
    assert_eq!(
        ok(&mut mcp, "eval", json!({ "code": "(buffer-name)" })),
        "\"mine.txt\""
    );
    // The user's frame, here the server's first, still shows what it did,
    // and that is where code runs when asked to.
    assert_eq!(
        ok(
            &mut mcp,
            "eval",
            json!({ "code": "(buffer-name)", "in_user_frame": true })
        ),
        "\"*scratch*\""
    );
    ok(&mut mcp, "keys", json!({ "keys": "C-x b" }));
    assert_eq!(
        ok(
            &mut mcp,
            "eval",
            json!({ "code": "(in-minibuffer?)", "in_user_frame": true })
        ),
        "#f",
        "a prompt in the model's frame is not one in the user's"
    );
    ok(&mut mcp, "keys", json!({ "keys": "C-g" }));

    // When the server is started anew, as after a rebuild, the next call
    // finds it again, and says so.
    server.restart();
    let answer = ok(&mut mcp, "eval", json!({ "code": "(+ 1 1)" }));
    assert!(answer.contains("started anew"), "{answer}");
    assert!(answer.ends_with("\n2"), "{answer}");
    assert_eq!(ok(&mut mcp, "eval", json!({ "code": "(+ 1 1)" })), "2");
}
