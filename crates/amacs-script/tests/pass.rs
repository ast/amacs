//! The password store: `runtime/lib/pass.scm`, and the secrets that other
//! code gets from it, `runtime/lib/secrets.scm`. Each test makes a store
//! and a key of its own.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::Harness;
use common::crypt::{FakeClipboard, GpgHome, quoted, wait_for};

/// A password store with a few entries, in a git repository, and an
/// editor that uses it.
struct Store {
    home: GpgHome,
    dir: tempfile::TempDir,
    clipboard: FakeClipboard,
    h: Harness,
}

impl Store {
    fn new() -> Self {
        let home = GpgHome::new();
        home.key("Test Person <test@example.org>", "");
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("store");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join(".gpg-id"), "test@example.org\n").unwrap();
        let mut h = Harness::new(100, 16);
        home.used_by(&mut h);
        h.eval("(set-option! 'dired-icons #f)");
        h.eval(&format!(
            "(set-option! 'pass-store-directory {})",
            quoted(&root)
        ));
        let clipboard = FakeClipboard::for_editor(&mut h);
        let mut store = Self {
            home,
            dir,
            clipboard,
            h,
        };
        store.add(
            "mail/work",
            "pw-work\\nuser: me@work.example\\nURL: https://mail.example\\notpauth://totp/x?secret=ABC\\n",
        );
        store.add("web/site", "pw-site\\nlogin: someone\\n");
        for arguments in [
            vec!["init", "--quiet"],
            vec!["config", "user.email", "test@example.org"],
            vec!["config", "user.name", "Test"],
            vec!["config", "commit.gpgsign", "false"],
            vec!["add", "--all"],
            vec!["commit", "--quiet", "--message", "Start."],
        ] {
            assert!(store.git().args(arguments).status().unwrap().success());
        }
        store
    }

    fn root(&self) -> PathBuf {
        self.dir.path().join("store")
    }

    fn git(&self) -> Command {
        let mut command = Command::new("git");
        command.arg("-C").arg(self.root());
        command
    }

    /// Writes an entry: the text is as Scheme reads it in a string.
    fn add(&mut self, name: &str, text: &str) {
        let wrote = wait_for(&mut self.h, &format!("(%pass-write \"{name}\" \"{text}\")"));
        assert!(!wrote.contains("error"), "{wrote}");
    }

    fn text_of(&mut self, name: &str) -> String {
        wait_for(&mut self.h, &format!("(pass-entry-text \"{name}\")"))
    }

    /// The message of the last commit, and how many commits there are.
    fn last_commit(&self) -> (String, usize) {
        let log = self.git().args(["log", "--format=%s"]).output().unwrap();
        let log = String::from_utf8(log.stdout).unwrap();
        (
            log.lines().next().unwrap_or("").to_owned(),
            log.lines().count(),
        )
    }

    /// Waits for a commit with this message to be the last one.
    fn wait_for_commit(&mut self, message: &str) {
        let root = self.root();
        let message = message.to_owned();
        self.h.wait_until(move |_| {
            let log = Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["log", "-1", "--format=%s"])
                .output()
                .unwrap();
            String::from_utf8_lossy(&log.stdout).trim() == message
        });
        let clean = self.git().args(["status", "--porcelain"]).output().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&clean.stdout),
            "",
            "all is committed"
        );
    }

    fn file(&self, name: &str) -> PathBuf {
        self.root().join(format!("{name}.gpg"))
    }
}

fn exists(path: &Path) -> bool {
    path.exists()
}

#[test]
fn the_store_is_read_as_it_is() {
    let mut store = Store::new();
    assert_eq!(
        store.h.eval("(pass-entries)"),
        "(\"mail/work\" \"web/site\")"
    );
    assert_eq!(
        store.h.eval("(pass-entry-file \"mail/work\")"),
        format!("{:?}", store.file("mail/work").display().to_string())
    );
    assert_eq!(
        wait_for(&mut store.h, "(pass-password \"mail/work\")"),
        "\"pw-work\""
    );
    // The lines after the first say more; an address is no field.
    assert_eq!(
        wait_for(
            &mut store.h,
            "(pass-text-fields (pass-entry-text \"mail/work\"))"
        ),
        "((\"user\" . \"me@work.example\") (\"url\" . \"https://mail.example\"))"
    );
    assert_eq!(
        wait_for(&mut store.h, "(pass-field \"web/site\" \"user\")"),
        "\"someone\"",
        "login is the user too"
    );
    assert_eq!(
        wait_for(&mut store.h, "(pass-field \"web/site\" \"url\")"),
        "#f"
    );
    assert_eq!(
        wait_for(&mut store.h, "(pass-password \"no/such\")"),
        "(error \"no/such is not in the password store\")"
    );
}

#[test]
fn a_password_is_found_copied_and_cleared_from_the_clipboard() {
    let mut store = Store::new();
    store.h.eval("(set-option! 'pass-clip-time 1)");
    store.h.keys("M-x").type_text("pass").keys("RET");
    store.h.type_text("mail").keys("RET");
    let screen = store.h.screen.text();
    assert!(screen.contains(" c   Copy the password"), "{screen}");
    assert!(screen.contains(" u   Copy the user"), "{screen}");
    store.h.keys("c");
    let clipboard = store.clipboard.clone();
    store.h.wait_until(|_| clipboard.text() == "pw-work");
    assert_eq!(
        store.h.echo(),
        "Copied the password of mail/work. The clipboard is cleared in 1 seconds."
    );
    store.h.wait_until(|_| clipboard.text().is_empty());
    store
        .h
        .wait_until(|h| h.echo() == "The clipboard was cleared");
    // Nor is it left on the kill ring.
    assert_ne!(store.h.eval("(current-kill)"), "\"pw-work\"");

    // The user, by the menu, and a field by its name.
    store.h.keys("M-x").type_text("pass").keys("RET");
    store.h.type_text("web/site").keys("RET u");
    store.h.wait_until(|_| clipboard.text() == "someone");
    store.h.keys("M-x").type_text("pass-copy-field").keys("RET");
    store.h.type_text("mail/work").keys("RET");
    store.h.wait_until(|h| h.prompt().contains("Field:"));
    store.h.type_text("url").keys("RET");
    store
        .h
        .wait_until(|_| clipboard.text() == "https://mail.example");
}

#[test]
fn entries_are_added_generated_and_committed_as_pass_does() {
    let mut store = Store::new();
    let clipboard = store.clipboard.clone();

    store.h.keys("M-x").type_text("pass-insert").keys("RET");
    store.h.type_text("new/one").keys("RET");
    store
        .h
        .wait_until(|h| h.prompt().starts_with("Password for new/one:"));
    store.h.type_text("s3cret");
    assert_eq!(store.h.prompt(), "Password for new/one: ******");
    store.h.keys("RET").type_text("s3cret").keys("RET");
    store.wait_for_commit("Add given password for new/one to store.");
    assert_eq!(store.text_of("new/one"), "\"s3cret\\n\"");
    assert_eq!(
        store.h.eval("(pass-entries)"),
        "(\"mail/work\" \"new/one\" \"web/site\")"
    );

    // Two that are not the same change nothing.
    let (_, commits) = store.last_commit();
    store.h.keys("M-x").type_text("pass-insert").keys("RET");
    store.h.type_text("new/two").keys("RET");
    store
        .h
        .type_text("a")
        .keys("RET")
        .type_text("b")
        .keys("RET");
    store
        .h
        .wait_until(|h| h.echo() == "The two are not the same");
    assert!(!exists(&store.file("new/two")));
    assert_eq!(store.last_commit().1, commits);

    // A generated one is of the length asked for, and is copied.
    store.h.keys("M-x").type_text("pass-generate").keys("RET");
    store.h.type_text("gen/x").keys("RET");
    store.h.wait_until(|h| h.prompt().starts_with("Length: 25"));
    store.h.keys("C-a C-k").type_text("40").keys("RET");
    store.wait_for_commit("Add generated password for gen/x.");
    store
        .h
        .wait_until(|_| clipboard.text().chars().count() == 40);
    let made = clipboard.text();
    assert_eq!(store.text_of("gen/x"), format!("{:?}", format!("{made}\n")));

    // In an entry that is there, the password is replaced and the rest
    // stays.
    store.h.keys("M-x").type_text("pass-generate").keys("RET");
    store.h.type_text("mail/work").keys("RET RET");
    store
        .h
        .wait_until(|h| h.prompt().contains("replace its password?"));
    store.h.keys("y");
    store.wait_for_commit("Replace generated password for mail/work.");
    let text = store.text_of("mail/work");
    assert!(
        text.ends_with("\\nuser: me@work.example\\nURL: https://mail.example\\notpauth://totp/x?secret=ABC\\n\""),
        "{text}"
    );
    assert!(!text.contains("pw-work"), "{text}");

    // What is generated is from the chars asked for.
    let plain = store.h.eval("(pass-generate-password 200 #f)");
    assert!(
        plain
            .trim_matches('"')
            .chars()
            .all(|c| c.is_ascii_alphanumeric()),
        "{plain}"
    );
    assert_eq!(plain.trim_matches('"').len(), 200);
    assert_ne!(plain, store.h.eval("(pass-generate-password 200 #f)"));
    let symbols = store
        .h
        .eval("(string->list (pass-generate-password 400 #t))");
    assert!(symbols.contains("#\\!") || symbols.contains("#\\~") || symbols.contains("#\\-"));
}

#[test]
fn an_entry_is_edited_renamed_and_deleted() {
    let mut store = Store::new();
    store.h.keys("M-x").type_text("pass-edit").keys("RET");
    store.h.type_text("web/site").keys("RET");
    store.h.wait_until(|h| h.mode_line().contains("site.gpg"));
    assert_eq!(store.h.text(), "pw-site\nlogin: someone\n");
    assert_eq!(store.h.eval("(option 'buffer-secret)"), "#t");
    store
        .h
        .keys("M->")
        .type_text("note: changed")
        .keys("C-x C-s");
    store.wait_for_commit("Edit password for web/site using amacs.");
    assert_eq!(
        store.text_of("web/site"),
        "\"pw-site\\nlogin: someone\\nnote: changed\""
    );

    // An entry that is made by visiting its file is added.
    let made = store.file("web/made");
    store.h.eval(&format!("(find-file {})", quoted(&made)));
    store.h.wait_until(|h| h.mode_line().contains("made.gpg"));
    store.h.type_text("typed").keys("C-x C-s");
    store.wait_for_commit("Add given password for web/made to store.");

    // The command is about the entry that the buffer visits.
    store.h.keys("M-x").type_text("pass-rename").keys("RET");
    store
        .h
        .wait_until(|h| h.prompt().starts_with("Rename web/made to: web/made"));
    store.h.keys("C-a C-k").type_text("other/moved").keys("RET");
    store.wait_for_commit("Rename web/made to other/moved.");
    assert!(!exists(&made));
    assert_eq!(store.text_of("other/moved"), "\"typed\"");

    store.h.keys("M-x").type_text("pass-delete").keys("RET");
    store.h.type_text("other/moved").keys("RET");
    store.h.wait_until(|h| {
        h.prompt()
            .contains("Delete other/moved from the password store?")
    });
    store.h.keys("y");
    store.wait_for_commit("Remove other/moved from store.");
    assert_eq!(
        store.h.eval("(pass-entries)"),
        "(\"mail/work\" \"web/site\")"
    );
}

#[test]
fn the_store_is_browsed_in_the_file_manager() {
    let mut store = Store::new();
    let clipboard = store.clipboard.clone();
    store.h.keys("M-x").type_text("pass-browse").keys("RET");
    assert!(
        store.h.mode_line().contains("(Dired Pass)"),
        "{}",
        store.h.mode_line()
    );
    // Into a directory, the keys are there still; w copies the password
    // of the entry on the line.
    store.h.keys("RET");
    assert!(
        store.h.mode_line().contains("(Dired Pass)"),
        "{}",
        store.h.mode_line()
    );
    assert_eq!(
        store
            .h
            .eval("(file-name-nondirectory (dired-file-at-point))"),
        "\"work.gpg\""
    );
    store.h.keys("w");
    store.h.wait_until(|_| clipboard.text() == "pw-work");
    // ? lists them.
    store.h.keys("?");
    let help = store.h.text();
    assert!(help.contains("Keys of pass-store-mode"), "{help}");
    assert!(
        help.contains("Copy the password of an entry of the password store to the clipboard."),
        "{help}"
    );
    store.h.keys("q");
    // RET reads the entry.
    store.h.keys("RET");
    store.h.wait_until(|h| h.mode_line().contains("work.gpg"));
    assert!(store.h.text().starts_with("pw-work\n"));

    // Another directory has the file manager's own keys.
    let elsewhere = store.dir.path().to_owned();
    store
        .h
        .eval(&format!("(dired-open {})", quoted(&elsewhere)));
    assert!(
        !store.h.mode_line().contains("Pass"),
        "{}",
        store.h.mode_line()
    );
}

#[test]
fn other_code_gets_secrets_by_what_it_knows() {
    let mut store = Store::new();
    store.add("example.org", "pw-host\\n");
    store.add("example.org/alice", "pw-alice\\n");
    store.add("servers/bob@example.org", "pw-bob\\n");
    store.add("example.org:2222", "pw-port\\n");

    let names = |found: &str| -> String { found.to_owned() };
    assert_eq!(
        names(
            &store
                .h
                .eval("(map car (secret-search 'host \"example.org\"))")
        ),
        "(\"servers/bob@example.org\" \"example.org/alice\" \"example.org\")"
    );
    assert_eq!(
        store
            .h
            .eval("(map caddr (secret-search 'host \"example.org\"))"),
        "(\"bob\" \"alice\" #f)"
    );
    assert_eq!(
        wait_for(
            &mut store.h,
            "(secret-get 'host \"example.org\" 'user \"alice\")"
        ),
        "\"pw-alice\""
    );
    assert_eq!(
        wait_for(
            &mut store.h,
            "(secret-get 'host \"example.org\" 'user \"bob\")"
        ),
        "\"pw-bob\""
    );
    // A user that no name gives is the host's.
    assert_eq!(
        wait_for(
            &mut store.h,
            "(secret-get 'host \"example.org\" 'user \"carol\")"
        ),
        "\"pw-host\""
    );
    assert_eq!(
        wait_for(
            &mut store.h,
            "(secret-get 'host \"example.org\" 'port 2222 'user \"carol\")"
        ),
        "\"pw-port\""
    );
    assert_eq!(
        wait_for(&mut store.h, "(secret-get 'host \"nowhere.example\")"),
        "#f"
    );

    // Nothing of it is for a program that asks the editor.
    for asked in [
        "(pass-password \"mail/work\")",
        "(secret-get 'host \"example.org\")",
        &format!("(crypt-decrypt {})", quoted(&store.file("mail/work"))),
    ] {
        let refused = store.h.session.eval_quietly(asked).unwrap_err();
        assert!(
            refused.message.contains("a program is not given"),
            "{asked}: {}",
            refused.message
        );
    }
    let _ = &store.home;
}
