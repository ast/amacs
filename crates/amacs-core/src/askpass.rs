//! Questions that programs put to the user through the editor: the
//! password of a host, the passphrase of a key, whether a host is the
//! right one.
//!
//! `ssh`, `sudo` and git do not have to be watched for what looks like a
//! question. Each of them runs a helper program if it is told of one
//! (`SSH_ASKPASS`, `SUDO_ASKPASS`, `GIT_ASKPASS`), with the question as
//! its argument, and reads the answer from what the helper prints. The
//! helper of the editor is `amacsclient`, which brings the question to
//! the editor over a socket of its own; Scheme (`runtime/lib/askpass.scm`)
//! finds the answer or asks for it in the minibuffer.
//!
//! Not every program may make the editor ask for a password. Before the
//! editor starts a program that may, it makes a ticket for it, which the
//! program has in its environment and the helper shows with the question.
//! A question without a ticket that is known is refused, and what the
//! user is asked always starts with whom the ticket was made for.
//!
//! This is the one place that all of it goes through. It is not part of
//! an [`Editor`], because the programs are started and the questions come
//! in on other threads than the editor's.

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use crate::completion::Waker;
use crate::frame::FrameId;
use crate::script::{ScriptCall, ScriptTarget, Value};
use crate::{CmdError, CmdResult, Editor};

/// The variable that holds the ticket of a program.
pub const TICKET_VARIABLE: &str = "AMACS_ASKPASS_TICKET";
/// The variable that holds the socket that the helper asks on.
pub const SOCKET_VARIABLE: &str = "AMACS_ASKPASS_SOCKET";

/// Told `true` when a question of a ticket is put to the user and
/// `false` when it is answered.
pub type Asking = Arc<dyn Fn(bool) + Send + Sync>;

/// Whom a ticket was made for.
struct Ticket {
    /// Who asks, in a few words that start the prompt: `/ssh:nina:`.
    who: String,
    host: Option<String>,
    user: Option<String>,
    /// The frame that the questions are shown in, if it is still there.
    frame: Option<FrameId>,
    /// How many questions were asked with this ticket.
    asked: u64,
    asking: Option<Asking>,
}

/// A question that the editor has not taken in yet.
struct Question {
    id: u64,
    ticket: String,
    prompt: String,
    kind: String,
    /// How many questions the same ticket asked before this one.
    before: u64,
}

/// Where the helper is and how the editor is told of a question.
struct Served {
    helper: PathBuf,
    socket: PathBuf,
    waker: Waker,
}

#[derive(Default)]
struct Desk {
    served: Option<Served>,
    tickets: HashMap<String, Ticket>,
    pending: Vec<Question>,
    /// Who waits for the answer to a question, and its ticket.
    open: HashMap<u64, (String, Sender<Option<String>>)>,
    next: u64,
}

fn desk() -> MutexGuard<'static, Desk> {
    static DESK: OnceLock<Mutex<Desk>> = OnceLock::new();
    DESK.get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Says that questions are answered from now on: `helper` is the program
/// that brings them, `socket` is where it brings them, and `waker` makes
/// the editor look at what came in. From now on the programs that reach
/// other hosts are not told to ask nothing.
pub fn serve(helper: PathBuf, socket: PathBuf, waker: Waker) {
    desk().served = Some(Served {
        helper,
        socket,
        waker,
    });
    amacs_remote::Connections::global().set_asker(Arc::new(|remote, asking| {
        let last = remote.hops.last();
        let ticket = ticket(
            &remote.host(),
            last.map(|hop| hop.host.clone())
                .filter(|host| !host.is_empty()),
            last.and_then(|hop| hop.user.clone()),
            None,
            Some(asking),
        )?;
        let environment = environment(&ticket);
        Some(amacs_remote::Asked {
            environment,
            done: Box::new(move || {
                release(&ticket);
            }),
        })
    }));
}

/// Whether questions are answered: see [`serve`].
pub fn is_served() -> bool {
    desk().served.is_some()
}

/// A word that nobody can guess.
fn random_word() -> Option<String> {
    let mut bytes = [0; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .ok()?;
    Some(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Makes a ticket for a program that is about to be started: who it is,
/// in the words that the user is to see, the host and the user that it
/// may ask a password for, if any, and the frame to ask in. Returns
/// nothing if questions are not answered.
pub fn ticket(
    who: &str,
    host: Option<String>,
    user: Option<String>,
    frame: Option<FrameId>,
    asking: Option<Asking>,
) -> Option<String> {
    let mut desk = desk();
    desk.served.as_ref()?;
    let name = random_word()?;
    desk.tickets.insert(
        name.clone(),
        Ticket {
            who: who.to_owned(),
            host,
            user,
            frame,
            asked: 0,
            asking,
        },
    );
    Some(name)
}

/// The variables that a program with a ticket is started with: they lead
/// `ssh`, `sudo -A` and git to the helper, and the helper to the editor.
/// `ssh` is told to use the helper even where it has a terminal or no
/// display.
pub fn environment(ticket: &str) -> Vec<(String, String)> {
    let desk = desk();
    let Some(served) = &desk.served else {
        return Vec::new();
    };
    let helper = served.helper.display().to_string();
    vec![
        ("SSH_ASKPASS".to_owned(), helper.clone()),
        ("SSH_ASKPASS_REQUIRE".to_owned(), "force".to_owned()),
        ("SUDO_ASKPASS".to_owned(), helper.clone()),
        ("GIT_ASKPASS".to_owned(), helper),
        (TICKET_VARIABLE.to_owned(), ticket.to_owned()),
        (
            SOCKET_VARIABLE.to_owned(),
            served.socket.display().to_string(),
        ),
    ]
}

/// Ends a ticket: its program is done. Returns whether it was known.
pub fn release(ticket: &str) -> bool {
    desk().tickets.remove(ticket).is_some()
}

/// Brings in a question from the helper of a program: its ticket, the
/// question in the program's words, and the kind (`confirm` for yes or
/// no, `none` for words that want no answer, else empty: something to
/// type that is not shown). The answer comes on what is returned, or
/// nothing if the user gives none. A ticket that is not known is an
/// error: the program was not started by the editor to ask.
pub fn ask(ticket: &str, prompt: &str, kind: &str) -> Result<Receiver<Option<String>>, String> {
    let mut desk = desk();
    let Some(known) = desk.tickets.get_mut(ticket) else {
        return Err("this ticket is not known to the editor".to_owned());
    };
    let before = known.asked;
    known.asked += 1;
    let asking = known.asking.clone();
    let id = desk.next;
    desk.next += 1;
    let (answer, answers) = mpsc::channel();
    desk.open.insert(id, (ticket.to_owned(), answer));
    desk.pending.push(Question {
        id,
        ticket: ticket.to_owned(),
        prompt: prompt.to_owned(),
        kind: kind.to_owned(),
        before,
    });
    let waker = desk.served.as_ref().map(|served| served.waker.clone());
    drop(desk);
    if let Some(asking) = asking {
        asking(true);
    }
    if let Some(waker) = waker {
        waker();
    }
    Ok(answers)
}

/// Gives the answer to a question, or says that there is none. Returns
/// whether the question was open.
pub fn answer(id: u64, answer: Option<String>) -> bool {
    let mut desk = desk();
    let Some((ticket, waiting)) = desk.open.remove(&id) else {
        return false;
    };
    let still_asking = desk.open.values().any(|(other, _)| *other == ticket);
    let asking = desk
        .tickets
        .get(&ticket)
        .and_then(|ticket| ticket.asking.clone())
        .filter(|_| !still_asking);
    drop(desk);
    // The helper may be gone, with the program that ran it.
    let _ = waiting.send(answer);
    if let Some(asking) = asking {
        asking(false);
    }
    true
}

impl Editor {
    /// Makes a ticket for a program that Scheme is about to start: see
    /// [`ticket`]. Its questions are shown in the frame that is current
    /// now.
    pub fn askpass_ticket(
        &mut self,
        who: &str,
        host: Option<String>,
        user: Option<String>,
    ) -> CmdResult<String> {
        ticket(who, host, user, Some(self.current_frame()), None).ok_or_else(|| {
            CmdError::msg("Nobody can answer the questions of a program in this session")
        })
    }

    /// Hands the questions that came in to Scheme, each in the frame of
    /// its ticket, or else in the frame that a person used last. With
    /// only the frame of a program to ask in, there is no answer.
    pub(crate) fn askpass_poll(&mut self) {
        let pending = std::mem::take(&mut desk().pending);
        for question in pending {
            let facts = {
                let desk = desk();
                desk.tickets.get(&question.ticket).map(|ticket| {
                    (
                        ticket.who.clone(),
                        ticket.host.clone(),
                        ticket.user.clone(),
                        ticket.frame,
                    )
                })
            };
            let Some((who, host, user, frame)) = facts else {
                answer(question.id, None);
                continue;
            };
            let frame = frame
                .filter(|frame| self.has_frame(*frame))
                .unwrap_or_else(|| self.current_frame());
            let call = ScriptCall {
                target: ScriptTarget::Named("%askpass-asked"),
                args: vec![
                    Value::Int(question.id as i64),
                    who.into(),
                    host.into(),
                    user.into(),
                    question.prompt.into(),
                    Value::sym(if question.kind.is_empty() {
                        "secret"
                    } else {
                        question.kind.as_str()
                    }),
                    Value::Int(question.before as i64),
                ],
                command: None,
                echo_result: false,
                frame: Some(frame),
            };
            if self.frames[frame].program || self.queue_script(call).is_err() {
                answer(question.id, None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_question_needs_a_ticket_and_is_answered_once() {
        assert!(ask("no-such-ticket", "Password:", "").is_err());
        // The desk is shared by the tests of this program: serve as one
        // of them would, and look only at what is this test's.
        if !is_served() {
            desk().served = Some(Served {
                helper: "/helper".into(),
                socket: "/socket".into(),
                waker: Arc::new(|| {}),
            });
        }
        let told = Arc::new(Mutex::new(Vec::new()));
        let asking: Asking = {
            let told = Arc::clone(&told);
            Arc::new(move |open| told.lock().unwrap().push(open))
        };
        let ticket = ticket("ssh", Some("nina".into()), None, None, Some(asking)).unwrap();
        assert_eq!(ticket.len(), 32);
        let variables = environment(&ticket);
        assert!(variables.contains(&("SSH_ASKPASS_REQUIRE".to_owned(), "force".to_owned())));
        assert!(variables.contains(&(TICKET_VARIABLE.to_owned(), ticket.clone())));

        let first = ask(&ticket, "Password:", "").unwrap();
        let second = ask(&ticket, "Password:", "").unwrap();
        let (one, two) = {
            let desk = desk();
            let mine: Vec<&Question> = desk
                .pending
                .iter()
                .filter(|question| question.ticket == ticket)
                .collect();
            assert_eq!(mine.len(), 2);
            // Each knows how many came before it.
            assert_eq!((mine[0].before, mine[1].before), (0, 1));
            (mine[0].id, mine[1].id)
        };
        assert!(answer(one, Some("secret".into())));
        assert!(!answer(one, Some("again".into())));
        assert_eq!(first.recv().unwrap().as_deref(), Some("secret"));
        // Told that a question is open for each, and that none is only
        // after the last.
        assert_eq!(*told.lock().unwrap(), [true, true]);
        assert!(answer(two, None));
        assert_eq!(second.recv().unwrap(), None);
        assert_eq!(*told.lock().unwrap(), [true, true, false]);

        assert!(release(&ticket));
        assert!(!release(&ticket));
        assert!(ask(&ticket, "Password:", "").is_err());
        desk().pending.retain(|question| question.ticket != ticket);
    }
}
