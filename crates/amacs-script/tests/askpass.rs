//! What programs ask the user through the editor:
//! `runtime/lib/askpass.scm` over `amacs_core::askpass`. The helper
//! program and the socket that it asks on are tested with the server;
//! here the questions come in the way the socket's thread brings them.

mod common;

use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use amacs_core::askpass;
use common::Harness;

/// An editor that answers questions. There is one desk for the questions
/// of a process, so the tests take turns at it.
struct Asked {
    h: Harness,
    _turn: MutexGuard<'static, ()>,
}

impl Asked {
    fn new() -> Self {
        static TURN: Mutex<()> = Mutex::new(());
        let turn = TURN.lock().unwrap_or_else(PoisonError::into_inner);
        askpass::serve("/helper".into(), "/socket".into(), Arc::new(|| {}));
        Self {
            h: Harness::new(70, 8),
            _turn: turn,
        }
    }

    /// A ticket as a module of Scheme gets one for a program.
    fn ticket(&mut self, arguments: &str) -> String {
        let made = self.h.eval(&format!(
            "(cadr (assoc \"AMACS_ASKPASS_TICKET\" (askpass-environment {arguments})))"
        ));
        made.trim_matches('"').to_owned()
    }

    /// Asks as the helper of a program does, and lets the editor see it.
    fn ask(&mut self, ticket: &str, prompt: &str, kind: &str) -> Receiver<Option<String>> {
        let answer = askpass::ask(ticket, prompt, kind).unwrap();
        self.h.redisplay();
        answer
    }
}

fn answer(answer: &Receiver<Option<String>>) -> Option<String> {
    answer.recv_timeout(Duration::from_secs(10)).unwrap()
}

#[test]
fn a_program_with_a_ticket_asks_in_the_minibuffer() {
    let mut asked = Asked::new();
    assert_eq!(asked.h.eval("(askpass-served?)"), "#t");
    let ticket = asked.ticket("\"git push\"");
    let variables = asked.h.eval("(map car (askpass-environment \"other\"))");
    for name in [
        "SSH_ASKPASS",
        "SSH_ASKPASS_REQUIRE",
        "SUDO_ASKPASS",
        "GIT_ASKPASS",
    ] {
        assert!(variables.contains(name), "{variables}");
    }

    let password = asked.ask(&ticket, "Password for 'https://example.org': ", "");
    asked.h.type_text("hunter2");
    assert_eq!(
        asked.h.prompt(),
        "git push Password for 'https://example.org': *******"
    );
    assert!(password.try_recv().is_err(), "not before RET");
    asked.h.keys("RET");
    assert_eq!(answer(&password).as_deref(), Some("hunter2"));

    // C-g is no answer, and the program gives up.
    let quit = asked.ask(&ticket, "Password: ", "");
    assert_eq!(asked.h.prompt(), "git push Password:");
    asked.h.keys("C-g");
    assert_eq!(answer(&quit), None);

    // With the program done, its ticket asks nothing more.
    assert_eq!(
        asked.h.eval(&format!(
            "(askpass-done! '((\"AMACS_ASKPASS_TICKET\" \"{ticket}\")))"
        )),
        "#t"
    );
    assert!(askpass::ask(&ticket, "Password: ", "").is_err());
}

#[test]
fn a_source_of_secrets_answers_the_first_question_and_then_the_user() {
    let mut asked = Asked::new();
    asked.h.eval(
        "(define looked '())
         (define-secret-source 'fake
           (lambda (host user port)
             (set! looked (cons (list host user) looked))
             (if (equal? host \"mail.example.org\")
                 (list (list \"mail\" host user (lambda () \"from-the-store\")))
                 '())))
         (set-option! 'secret-sources '(fake))",
    );
    let ticket = asked.ticket("\"mail\" 'host \"mail.example.org\" 'user \"me\"");
    let first = asked.ask(&ticket, "Password: ", "");
    assert_eq!(answer(&first).as_deref(), Some("from-the-store"));
    assert_eq!(asked.h.prompt(), "", "nothing was asked");
    assert_eq!(asked.h.eval("looked"), "((\"mail.example.org\" \"me\"))");

    // A program that asks again did not get in with that, or wants
    // something else. What it wants is not read out of its words: the
    // user answers from here on.
    let second = asked.ask(&ticket, "Password: ", "");
    assert_eq!(asked.h.prompt(), "mail Password:");
    asked.h.type_text("typed").keys("RET");
    assert_eq!(answer(&second).as_deref(), Some("typed"));
    assert_eq!(asked.h.eval("(length looked)"), "1");

    // Without a host there is nothing to look for.
    let ticket = asked.ticket("\"other\"");
    let question = asked.ask(&ticket, "Password: ", "");
    assert_eq!(asked.h.prompt(), "other Password:");
    asked.h.keys("C-g");
    assert_eq!(answer(&question), None);
    assert_eq!(asked.h.eval("(length looked)"), "1");
}

#[test]
fn questions_of_yes_or_no_and_words_without_a_question() {
    let mut asked = Asked::new();
    asked
        .h
        .session
        .handle_event(amacs_proto::InputEvent::Resize {
            cols: 100,
            rows: 10,
        });
    asked.h.screen = Default::default();
    asked.h.redisplay();
    let ticket = asked.ticket("\"ssh to evo\"");
    // What ssh asks about a host that it does not know, in its lines.
    let unknown = "The authenticity of host 'evo (10.0.0.2)' can't be established.\n\
                   ED25519 key fingerprint is SHA256:abc.\n\
                   Are you sure you want to continue connecting (yes/no/[fingerprint])? ";
    let host = asked.ask(&ticket, unknown, "");
    // The last line is the question, and the lines before it are under
    // it, each whole.
    let screen = asked.h.screen.text();
    let rows: Vec<&str> = screen.lines().map(str::trim_end).collect();
    let at = rows
        .iter()
        .position(|row| row.starts_with("ssh to evo Are you sure"))
        .unwrap_or_else(|| panic!("{screen}"));
    assert_eq!(
        rows[at + 1..at + 3],
        [
            "The authenticity of host 'evo (10.0.0.2)' can't be established.",
            "ED25519 key fingerprint is SHA256:abc."
        ],
        "{screen}"
    );
    // ssh takes yes, no or the fingerprint, so the answer is typed. It is
    // hidden as every answer is: ssh does not say which may be shown.
    asked.h.type_text("yes");
    assert!(asked.h.screen.text().contains("[fingerprint])? ***"));
    asked.h.keys("RET");
    assert_eq!(answer(&host).as_deref(), Some("yes"));

    let confirm = asked.ask(&ticket, "Allow use of key id?", "confirm");
    asked.h.keys("y");
    assert_eq!(answer(&confirm).as_deref(), Some("yes"));
    let confirm = asked.ask(&ticket, "Allow use of key id?", "confirm");
    asked.h.keys("n");
    assert_eq!(answer(&confirm), None);

    let told = asked.ask(&ticket, "Confirm user presence for key", "none");
    assert_eq!(answer(&told).as_deref(), Some(""));
    assert_eq!(asked.h.echo(), "ssh to evo Confirm user presence for key");
}

#[test]
fn a_hook_answers_first_and_an_open_prompt_is_not_broken_into() {
    let mut asked = Asked::new();
    asked.h.eval(
        "(add-hook! 'askpass-functions
                    (lambda (who host user prompt kind before)
                      (and (equal? prompt \"Code: \") \"123456\")))",
    );
    let ticket = asked.ticket("\"ssh to evo\"");
    let code = asked.ask(&ticket, "Code: ", "");
    assert_eq!(answer(&code).as_deref(), Some("123456"));

    // The user is in a prompt of their own when a question comes.
    asked.h.keys("M-x").type_text("forw");
    let password = asked.ask(&ticket, "Password: ", "");
    assert!(
        asked.h.prompt().contains("M-x forw"),
        "{}",
        asked.h.prompt()
    );
    assert!(password.try_recv().is_err());
    asked.h.keys("C-g");
    asked.h.wait_until(|h| h.prompt() == "ssh to evo Password:");
    asked.h.type_text("x").keys("RET");
    assert_eq!(answer(&password).as_deref(), Some("x"));
}

#[test]
fn a_command_that_waits_for_the_program_that_asks_does_not_keep_the_answer_back() {
    let mut asked = Asked::new();
    // As find-file waits for ssh to give a file, and ssh asks first.
    asked.h.eval(
        "(define done #f)
         (define-command (wait-long) \"Wait for a program.\"
           (run-process \"sleep\" '(\"3\"))
           (set! done #t))",
    );
    let ticket = asked.ticket("\"ssh to evo\"");
    asked.h.keys("M-x").type_text("wait-long").keys("RET");
    let password = asked.ask(&ticket, "Password: ", "");
    assert_eq!(asked.h.prompt(), "ssh to evo Password:");
    asked.h.type_text("abc");
    assert_eq!(asked.h.prompt(), "ssh to evo Password: ***");
    asked.h.keys("RET");
    assert_eq!(answer(&password).as_deref(), Some("abc"));
    assert_eq!(asked.h.eval("done"), "#f", "the command still waits");
    // And C-g in such a prompt is no answer, where it would else only
    // stop the waiting.
    let password = asked.ask(&ticket, "Password: ", "");
    asked.h.keys("C-g");
    assert_eq!(answer(&password), None);
}

#[test]
fn a_program_that_evaluates_code_gets_no_ticket_and_gives_no_answer() {
    let mut asked = Asked::new();
    let ticket = asked.ticket("\"real\"");
    let refused = asked
        .h
        .session
        .eval_quietly("(askpass-environment \"a program\")")
        .unwrap_err();
    assert!(
        refused.message.contains("a program is not given a ticket"),
        "{}",
        refused.message
    );
    let question = asked.ask(&ticket, "Password: ", "");
    assert_eq!(asked.h.prompt(), "real Password:");
    let refused = asked
        .h
        .session
        .eval_quietly("(%askpass-answer 0 \"mine\")")
        .unwrap_err();
    assert!(refused.message.contains("a program does not answer"));
    asked.h.keys("C-g");
    assert_eq!(answer(&question), None);
}
