//! `amacsclient` as the helper that `ssh`, `sudo` and git run to ask the
//! user something: it takes the question to the editor's socket for
//! questions, with the ticket that the editor gave the program, and
//! prints the answer.

use std::os::unix::net::UnixListener;
use std::path::Path;
use std::process::{Command, Output};

use amacs_proto::{Answer, Ask, read_msg, write_msg};

/// Runs the helper with a question, against an editor that gives one
/// answer. Returns what the helper did and what it asked.
fn asked(program: &Path, args: &[&str], kind: Option<&str>, answer: Answer) -> (Output, Ask) {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("askpass.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let editor = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let ask = read_msg::<_, Ask>(&stream).unwrap().unwrap();
        write_msg(&stream, &answer).unwrap();
        ask
    });
    let mut command = Command::new(program);
    command
        .args(args)
        .env("AMACS_ASKPASS_TICKET", "ticket-1")
        .env("AMACS_ASKPASS_SOCKET", &socket)
        .env_remove("SSH_ASKPASS_PROMPT");
    if let Some(kind) = kind {
        command.env("SSH_ASKPASS_PROMPT", kind);
    }
    let output = command.output().unwrap();
    (output, editor.join().unwrap())
}

#[test]
fn the_helper_takes_a_question_to_the_editor_and_prints_the_answer() {
    let client = Path::new(env!("CARGO_BIN_EXE_amacsclient"));
    let (output, ask) = asked(
        client,
        &["--askpass", "me@evo's password: "],
        None,
        Answer::Given("hunter2".into()),
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "hunter2\n");
    assert_eq!(
        ask,
        Ask {
            ticket: "ticket-1".into(),
            prompt: "me@evo's password: ".into(),
            kind: String::new(),
        }
    );

    // ssh and sudo run a program by its path alone, so the editor gives
    // them a link whose name says what the program is to do.
    let dir = tempfile::tempdir().unwrap();
    let link = dir.path().join("askpass-4711");
    std::os::unix::fs::symlink(client, &link).unwrap();
    let (output, ask) = asked(
        &link,
        &["Allow use of key?"],
        Some("confirm"),
        Answer::Given("yes".into()),
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "yes\n");
    assert_eq!(
        (ask.prompt.as_str(), ask.kind.as_str()),
        ("Allow use of key?", "confirm")
    );
}

#[test]
fn no_answer_and_a_refusal_are_failures_that_print_nothing() {
    let client = Path::new(env!("CARGO_BIN_EXE_amacsclient"));
    let (output, _) = asked(client, &["--askpass", "Password: "], None, Answer::None);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());

    let (output, _) = asked(
        client,
        &["--askpass", "Password: "],
        None,
        Answer::Refused("this ticket is not known to the editor".into()),
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("this ticket is not known"),
        "{output:?}"
    );

    // Run by hand, not by a program that the editor started.
    let output = Command::new(client)
        .args(["--askpass", "Password: "])
        .env_remove("AMACS_ASKPASS_TICKET")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("AMACS_ASKPASS_TICKET is not set"),
        "{output:?}"
    );
}
