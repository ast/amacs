//! Files on other hosts, reached here through the `sftp-server` of this
//! machine: `/sftp-test::/path` is a file that is visited, saved and listed
//! as one on another host is, over SFTP and in the background.

mod common;

use std::fs;
use std::path::Path;

use common::Harness;

/// The name of a local file as that of a file on a host of the tests.
fn on(host: &str, path: &Path) -> String {
    let path = fs::canonicalize(path).unwrap_or_else(|_| {
        let parent = fs::canonicalize(path.parent().unwrap()).unwrap();
        parent.join(path.file_name().unwrap())
    });
    format!("/sftp-test:{host}:{}", path.display())
}

fn there(path: &Path) -> String {
    on("", path)
}

/// Visits a file by typing its name at the prompt, over what is there.
fn visit(h: &mut Harness, name: &str) {
    h.keys("C-x C-f")
        .type_text(&format!("/{name}"))
        .keys("M-RET");
}

fn file_name(h: &mut Harness) -> String {
    h.eval("(buffer-file-name)").trim_matches('"').to_owned()
}

#[test]
fn a_remote_file_is_visited_edited_and_saved() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.txt");
    fs::write(&path, "over there\n").unwrap();
    let name = there(&path);

    let mut h = Harness::new(100, 12);
    visit(&mut h, &name);
    h.wait_until(|h| h.text() == "over there\n");
    assert_eq!(file_name(&mut h), name);
    assert_eq!(h.eval("(buffer-name)"), "\"notes.txt\"");
    assert_eq!(h.eval("(file-remote? (buffer-file-name))"), "#t");
    assert_eq!(h.eval("(file-remote? \"/etc/hosts\")"), "#f");
    assert_eq!(h.echo(), "");

    h.type_text("Now ").keys("C-x C-s");
    h.wait_until(|h| h.echo().starts_with("Wrote"));
    assert_eq!(h.echo(), format!("Wrote {name}"));
    assert_eq!(fs::read_to_string(&path).unwrap(), "Now over there\n");
    assert_eq!(h.eval("(buffer-modified?)"), "#f");
    h.keys("C-x C-s");
    assert_eq!(h.echo(), "(No changes need to be saved)");

    // Visiting it again shows the buffer that is there.
    h.eval("(switch-to-buffer \"*scratch*\")");
    visit(&mut h, &name);
    h.wait_until(|h| h.text() == "Now over there\n");
    assert_eq!(
        h.eval(
            "(length (filter (lambda (name) (string-prefix? \"notes.txt\" name)) (buffer-list)))"
        ),
        "1"
    );

    // Under another name too, when both lead to the same file.
    std::os::unix::fs::symlink("notes.txt", dir.path().join("link")).unwrap();
    h.eval("(switch-to-buffer \"*scratch*\")");
    visit(&mut h, &there(&dir.path().join("link")));
    h.wait_until(|h| h.text() == "Now over there\n");
    assert_eq!(h.eval("(buffer-name)"), "\"notes.txt\"");
}

#[test]
fn what_is_typed_while_a_file_is_read_goes_into_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.txt");
    fs::write(&path, "there\n").unwrap();

    let mut h = Harness::new(100, 12);
    visit(&mut h, &there(&path));
    h.type_text("typed ");
    h.wait_until(|h| h.text() == "typed there\n");
    assert_eq!(h.eval("(buffer-name)"), "\"notes.txt\"");
}

#[test]
fn a_new_remote_file_is_made_by_saving() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new.txt");
    let name = there(&path);

    let mut h = Harness::new(100, 12);
    visit(&mut h, &name);
    h.wait_until(|h| h.echo() == "(New file)");
    assert_eq!(file_name(&mut h), name);
    h.type_text("made").keys("C-x C-s");
    h.wait_until(|h| h.echo().starts_with("Wrote"));
    assert_eq!(fs::read_to_string(&path).unwrap(), "made");

    // Writing a buffer to another host makes that its file.
    let copy = there(&dir.path().join("copy.txt"));
    h.keys("C-x C-w")
        .type_text(&format!("/{copy}"))
        .keys("M-RET");
    h.wait_until(|h| h.echo().starts_with("Wrote"));
    assert_eq!(file_name(&mut h), copy);
    assert_eq!(h.eval("(buffer-name)"), "\"copy.txt\"");
    assert_eq!(
        fs::read_to_string(dir.path().join("copy.txt")).unwrap(),
        "made"
    );
}

#[test]
fn scheme_visits_a_remote_file_and_goes_on_when_it_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.txt");
    fs::write(&path, "one\ntwo\n").unwrap();

    let mut h = Harness::new(100, 12);
    h.eval(&format!(
        "(define seen #f)
         (run-later (lambda ()
           (find-file {:?})
           (set! seen (list (buffer-name) (buffer-string)))))",
        there(&path)
    ));
    h.wait_until(|h| h.eval("seen") != "#f");
    assert_eq!(h.eval("seen"), "(\"data.txt\" \"one\\ntwo\\n\")");
}

#[test]
fn what_goes_wrong_is_said() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::new(120, 12);

    // A way to a host that is not there yet.
    // A method whose program is not there. It is not a new file.
    h.eval("(define-remote-method \"nowhere\" 'command '(\"amacs-no-such-program\" host))");
    visit(&mut h, "/nowhere:box:/etc/hosts");
    h.wait_until(|h| {
        h.echo()
            .contains("the program amacs-no-such-program was not found on this machine")
    });
    assert!(
        h.echo().contains("Cannot open /nowhere:box:/etc/hosts"),
        "{}",
        h.echo()
    );

    // A name that is not whole.
    visit(&mut h, "/ssh:nowhere");
    h.wait_until(|h| h.echo().contains("has no path"));

    // A file that is not text.
    fs::write(dir.path().join("blob"), [0, 159, 146, 150]).unwrap();
    visit(&mut h, &there(&dir.path().join("blob")));
    h.wait_until(|h| h.echo().contains("is not text (4)"));

    // A directory that is not there, on saving.
    let name = there(&dir.path().join("new.txt"));
    visit(&mut h, &name);
    h.wait_until(|h| h.echo() == "(New file)");
    fs::remove_dir_all(dir.path()).unwrap();
    h.type_text("text").keys("C-x C-s");
    h.wait_until(|h| h.echo().contains("Cannot write"));
    assert!(h.echo().contains("No such file"), "{}", h.echo());
    assert_eq!(h.eval("(buffer-modified?)"), "#t");
}

#[test]
fn a_large_remote_file_is_asked_about() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.txt");
    fs::write(&path, "x".repeat(3000)).unwrap();
    let name = there(&path);

    let mut h = Harness::new(120, 12);
    h.eval("(set-option! 'large-file-threshold 0)");
    visit(&mut h, &name);
    h.wait_until(|h| h.echo().contains("open it anyway?"));
    assert_eq!(
        h.echo(),
        format!("{name} is 2.9K; open it anyway? (y or n)")
    );
    h.keys("n");
    assert_eq!(h.eval("(buffer-name)"), "\"*scratch*\"");

    visit(&mut h, &name);
    h.wait_until(|h| h.echo().contains("open it anyway?"));
    h.keys("y");
    h.wait_until(|h| h.text().len() == 3000);

    // Code is not asked: it says so, or is told.
    h.eval("(kill-buffer)");
    h.eval(&format!(
        "(define said #f)
         (run-later (lambda ()
           (set! said (guard (e (#t (error-object-message e))) (find-file {name:?}) 'visited))))"
    ));
    h.wait_until(|h| h.eval("said") != "#f");
    assert_eq!(
        h.eval("said"),
        format!("\"{name} is 2.9K, above large-file-threshold\"")
    );
    h.eval(&format!("(run-later (lambda () (find-file {name:?} #t)))"));
    h.wait_until(|h| h.text().len() == 3000);
}

#[test]
fn a_remote_directory_is_shown_by_the_file_manager() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), "in a\n").unwrap();
    fs::create_dir(dir.path().join("sub")).unwrap();
    let name = there(dir.path());

    let mut h = Harness::new(100, 12);
    h.eval("(set-option! 'dired-icons #f)");
    visit(&mut h, &name);
    h.wait_until(|h| h.text().contains("a.txt"));
    assert!(h.text().starts_with(&format!("{name}:\n")), "{}", h.text());
    assert_eq!(
        h.eval("(map dired-row-name (vector->list (dired-state-rows (dired-state))))"),
        "(\"sub\" \"a.txt\")"
    );
    // And a file is visited from it.
    h.keys("n RET");
    h.wait_until(|h| h.text() == "in a\n");
    assert_eq!(file_name(&mut h), format!("{name}/a.txt"));
}

#[test]
fn a_remote_file_is_read_again_when_asked() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.txt");
    fs::write(&path, "first\n").unwrap();

    let mut h = Harness::new(100, 12);
    visit(&mut h, &there(&path));
    h.wait_until(|h| h.text() == "first\n");
    fs::write(&path, "second\n").unwrap();
    h.keys("M-x").type_text("revert-buffer").keys("RET");
    h.wait_until(|h| h.text() == "second\n");
    assert_eq!(h.echo(), "Reverted notes.txt");
    assert_eq!(h.eval("(buffer-modified?)"), "#f");
}

#[test]
fn saving_asks_when_the_remote_file_changed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shared.txt");
    fs::write(&path, "ours").unwrap();

    let mut h = Harness::new(120, 12);
    visit(&mut h, &there(&path));
    h.wait_until(|h| h.text() == "ours");
    fs::write(&path, "theirs").unwrap();
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    let file = fs::File::options().write(true).open(&path).unwrap();
    file.set_modified(later).unwrap();

    h.type_text("!").keys("C-x C-s");
    assert!(
        h.echo()
            .ends_with("changed on disk; overwrite it? (y or n)"),
        "{}",
        h.echo()
    );
    h.keys("y");
    h.wait_until(|h| h.echo().starts_with("Wrote"));
    assert_eq!(fs::read_to_string(&path).unwrap(), "!ours");
    // The time that the file has now is the one that is known.
    h.type_text("!").keys("C-x C-s");
    h.wait_until(|_| fs::read_to_string(&path).unwrap() == "!!ours");
}

#[test]
fn names_on_a_host_are_completed() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("alpha.txt"), "a\n").unwrap();
    fs::write(dir.path().join("beta.txt"), "b\n").unwrap();
    fs::create_dir(dir.path().join("gamma")).unwrap();
    fs::write(dir.path().join("gamma/delta.txt"), "d\n").unwrap();
    let name = there(dir.path());

    let mut h = Harness::new(120, 16);
    h.keys("C-x C-f").type_text(&format!("/{name}/"));
    h.wait_until(|h| h.screen.text().contains("beta.txt"));
    let listed = h.screen.text();
    assert!(
        listed.find("gamma/") < listed.find("alpha.txt"),
        "directories come first:\n{listed}"
    );
    h.type_text("be");
    h.wait_until(|h| !h.screen.text().contains("alpha.txt"));
    h.keys("RET");
    h.wait_until(|h| h.text() == "b\n");
    assert_eq!(file_name(&mut h), format!("{name}/beta.txt"));

    // From a buffer there, names are completed in its directory.
    h.keys("C-x C-f");
    assert!(h.prompt().ends_with(&format!("{name}/")), "{}", h.prompt());
    h.type_text("gamma/");
    h.wait_until(|h| h.screen.text().contains("delta.txt"));
    h.keys("RET");
    h.wait_until(|h| h.text() == "d\n");
}

#[test]
fn programs_are_not_run_for_a_remote_buffer_yet() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.txt");
    fs::write(&path, "there\n").unwrap();

    let mut h = Harness::new(120, 12);
    visit(&mut h, &there(&path));
    h.wait_until(|h| h.text() == "there\n");
    h.eval("(define ran #f) (run-later (lambda () (set! ran (run-process \"true\" '()))))");
    h.wait_until(|h| h.eval("ran") != "#f");
    assert!(
        h.eval("ran")
            .starts_with("(#f \"\" \"Programs are not run on other hosts yet"),
        "{}",
        h.eval("ran")
    );
}

#[test]
fn a_connection_that_was_ended_is_made_again() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.txt");
    fs::write(&path, "there\n").unwrap();
    // A host of its own, since the others share theirs with other tests.
    let name = on("alone", &path);

    let mut h = Harness::new(120, 12);
    visit(&mut h, &name);
    h.wait_until(|h| h.text() == "there\n");
    assert!(
        h.eval("(remote-hosts)")
            .contains("(\"/sftp-test:alone:\" \"connected\")"),
        "{}",
        h.eval("(remote-hosts)")
    );
    assert_eq!(h.eval("(remote-disconnect \"/sftp-test:alone:\")"), "#t");
    assert_eq!(h.eval("(remote-disconnect \"/sftp-test:alone:\")"), "#f");
    assert!(!h.eval("(remote-hosts)").contains("alone"));

    h.type_text("still ").keys("C-x C-s");
    h.wait_until(|h| h.echo().starts_with("Wrote"));
    assert_eq!(fs::read_to_string(&path).unwrap(), "still there\n");
    assert!(
        h.eval("(remote-hosts)")
            .contains("(\"/sftp-test:alone:\" \"connected\")")
    );
}

/// The rows under the prompt: what the name means, and what is offered.
fn offered(h: &Harness) -> String {
    h.screen.text()
}

#[test]
fn the_prompt_offers_the_next_step_of_a_remote_name() {
    let mut h = Harness::new(100, 20);
    // At the top: the ways to a place, each with what it is.
    h.keys("C-x C-f").type_text("/su");
    h.wait_until(|h| {
        offered(h).contains("Become a different user on this machine. Default: root.")
    });
    assert!(!offered(&h).contains("podman:"), "{}", offered(&h));
    // Taking one goes on from it, and a row says what is still missing.
    h.keys("RET");
    h.wait_until(|h| offered(h).contains("User root. For a different user, type sudo:NAME@:"));
    assert!(h.prompt().ends_with("//sudo:"), "{}", h.prompt());
    assert!(
        offered(&h).contains("Type sudo:: for root. Type sudo:USER@: for a different user."),
        "{}",
        offered(&h)
    );
    h.keys("RET");
    h.wait_until(|h| offered(h).contains("This machine, user root. Path: home directory"));
    assert!(h.prompt().ends_with("//sudo::"), "{}", h.prompt());
    assert!(offered(&h).contains("Home directory of that user."));
    h.keys("C-g");

    // After a host, what can follow: the chain needs no remembering.
    h.keys("C-x C-f").type_text("/ssh:evo:");
    h.wait_until(|h| offered(h).contains("Become root on that host."));
    let screen = offered(&h);
    for row in [
        "Host evo. Path: home directory",
        "ssh:evo:/",
        "Top directory of that host.",
        "ssh:evo:~/",
        "ssh:evo|sudo::",
        "ssh:evo|ssh:",
        "Connect from that host to a different host.",
    ] {
        assert!(screen.contains(row), "{row} is not in:\n{screen}");
    }
    // The shorter of two that match alike comes first.
    h.keys("C-n C-n C-n RET");
    h.wait_until(|h| offered(h).contains("Host evo. Then user root. Path: home directory"));
    assert!(h.prompt().ends_with("//ssh:evo|sudo::"), "{}", h.prompt());
    h.keys("C-g");
    h.keys("C-x C-f").type_text("/ssh:evo|");
    h.wait_until(|h| offered(h).contains("Become a different user on that host. Default: root."));
    assert!(offered(&h).contains("Type a method: ssh:, sudo:, podman: or docker:"));
    h.keys("C-g");

    // A model, or Scheme, asks the same in words.
    assert_eq!(
        h.eval("(file-remote-description \"/ssh:me@evo|sudo::/etc/nixos\")"),
        "\"Host evo, user me. Then user root. Path: /etc/nixos\""
    );
    assert_eq!(h.eval("(file-remote-description \"/etc/nixos\")"), "#f");
}

#[test]
fn places_used_before_are_offered_first() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.txt");
    fs::write(&path, "there\n").unwrap();
    // A host of its own, since the others share theirs with other tests.
    let name = on("again", &path);

    let mut h = Harness::new(100, 20);
    assert_eq!(h.eval("(remote-places)"), "()");
    visit(&mut h, &name);
    h.wait_until(|h| h.text() == "there\n");
    assert_eq!(h.eval("(remote-places)"), "(\"/sftp-test:again:\")");

    h.keys("C-x C-f").type_text("/");
    h.wait_until(|h| offered(h).contains("Used before. Connected."));
    let row = h.screen.row(10);
    assert!(row.starts_with("sftp-test:again:"), "{row}");
    // Taking it goes on from there.
    h.keys("RET");
    h.wait_until(|h| offered(h).contains("Top directory of that host."));
    assert!(h.prompt().ends_with("//sftp-test:again:"), "{}", h.prompt());
    h.keys("C-g");
}

#[test]
fn a_file_is_named_as_root_reads_it() {
    let mut h = Harness::new(100, 12);
    for (name, as_root) in [
        ("/etc/hosts", "/sudo::/etc/hosts"),
        ("/ssh:evo:/etc/hosts", "/ssh:evo|sudo::/etc/hosts"),
        (
            "/ssh:me@gate|ssh:evo:/etc",
            "/ssh:me@gate|ssh:evo|sudo::/etc",
        ),
        ("/ssh:evo:/", "/ssh:evo|sudo::/"),
        ("/sudo::/etc/hosts", "/sudo::/etc/hosts"),
        (
            "/ssh:evo|sudo:postgres@:/etc",
            "/ssh:evo|sudo:postgres@:/etc",
        ),
    ] {
        assert_eq!(
            h.eval(&format!("(file-as-root {name:?})")),
            format!("{as_root:?}"),
            "{name}"
        );
    }
    h.eval("(define said #f)
            (run-later (lambda () (set! said (guard (e (#t (error-object-message e))) (sudo-edit)))))");
    h.wait_until(|h| h.eval("said") != "#f");
    assert_eq!(h.eval("said"), "\"This buffer has no file\"");
}

#[test]
fn the_methods_are_a_table_that_scheme_fills() {
    let mut h = Harness::new(100, 20);
    // The table is the process's: other tests may have added to it.
    let names = h.eval("(remote-method-names)");
    assert!(
        names.starts_with("(\"sftp-test\" \"ssh\" \"sudo\" \"podman\" \"docker\""),
        "{names}"
    );
    // What the editor's own definitions make of a name.
    let command = h.eval("(remote-command \"/ssh:albin@evo#2222:/etc\")");
    assert!(
        command.starts_with("(\"ssh\" \"-o\" \"BatchMode=yes\""),
        "{command}"
    );
    assert!(
        command.ends_with("\"-l\" \"albin\" \"-p\" \"2222\" \"-s\" \"--\" \"evo\" \"sftp\")"),
        "{command}"
    );
    let jumped = h.eval("(remote-command \"/ssh:me@gate#2222|ssh:evo:/etc\")");
    assert!(
        jumped.ends_with("\"-J\" \"me@gate:2222\" \"-s\" \"--\" \"evo\" \"sftp\")"),
        "{jumped}"
    );
    let root = h.eval("(remote-command \"/sudo::/etc\")");
    assert!(
        root.starts_with("(\"sudo\" \"-n\" \"-u\" \"root\" \"--\" \""),
        "{root}"
    );
    assert!(root.ends_with("sftp-server\")"), "{root}");
    // Nothing is asked, and the rest is left to configuration files:
    // no control connection is forced on the command line.
    assert!(!command.contains("Control"), "{command}");
    let there = h.eval("(member \"--\" (remote-command \"/ssh:evo|sudo:postgres@:/etc\"))");
    assert!(
        there.starts_with("(\"--\" \"evo\" \"sudo -n -u postgres -- sh -c 'for s in "),
        "{there}"
    );
    let boxed = h.eval("(remote-command \"/podman:me@box:/app\")");
    assert!(
        boxed.starts_with(
            "(\"podman\" \"exec\" \"-i\" \"-u\" \"me\" \"box\" \"sh\" \"-c\" \"for s in "
        ),
        "{boxed}"
    );

    // A method of one's own is a few lines, and is offered and said in
    // words like the others.
    h.eval(
        "(define-remote-method \"doas\"
           'command '(\"doas\" \"-n\" \"-u\" user \"--\")
           'host 'optional 'user \"root\" 'stays #t
           'here \"Become a different user on this machine with doas.\"
           'there \"Become a different user on that host with doas.\"
           'usage \"Type doas:: for root.\")",
    );
    assert_eq!(
        h.eval("(list-tail (reverse (remote-command \"/doas:admin@:/etc\")) 1)"),
        "(\"--\" \"admin\" \"-u\" \"-n\" \"doas\")"
    );
    assert_eq!(
        h.eval("(file-remote-description \"/ssh:evo|doas::/etc\")"),
        "\"Host evo. Then user root. Path: /etc\""
    );
    assert_eq!(
        h.eval("(file-remote-description \"/doas:\")"),
        "\"Type doas:: for root.\""
    );
    h.keys("C-x C-f").type_text("/ssh:evo:");
    h.wait_until(|h| offered(h).contains("ssh:evo|doas::"));
    assert!(offered(&h).contains("Become root on that host."));
    h.keys("C-g");
    assert_eq!(
        h.eval("(guard (e (#t (error-object-message e))) (define-remote-method \"bad\" 'command '(\"x\" nothing)))"),
        "\"%define-remote-method: nothing is not a part of a command: host, user, port or jumps\""
    );
}

#[test]
fn a_prompt_says_what_scheme_tells_it_about_its_input() {
    let mut h = Harness::new(80, 12);
    h.eval(
        "(define (count-hint kind input)
           (when (equal? kind \"string\")
             (set-prompt-hint! input (and (not (string-null? input))
                                          (string-append (number->string (string-length input))
                                                         \" characters.\")))))
         (add-hook! 'prompt-change-hook count-hint)
         (run-later (lambda () (read-string \"Say: \")))",
    );
    h.wait_until(|h| h.prompt().contains("Say:"));
    h.type_text("abc");
    h.wait_until(|h| h.screen.text().contains("3 characters."));
    h.type_text("d");
    h.wait_until(|h| h.screen.text().contains("4 characters."));
    assert!(!h.screen.text().contains("3 characters."));
    h.keys("C-a C-k");
    h.wait_until(|h| !h.screen.text().contains("characters."));
    h.keys("C-g");
}

#[test]
fn the_configuration_of_the_user_wins_over_that_of_the_editor() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::new(100, 12);
    let config = |h: &mut Harness, user: &str| {
        let path = dir.path().join("generated");
        h.eval(&format!(
            "(write-file {:?} (%ssh-config {:?} {:?}))",
            path.display().to_string(),
            dir.path().join(user).display().to_string(),
            dir.path().display().to_string(),
        ));
        path
    };
    // What ssh would use for a host, by its own account. It connects to
    // nothing for this. Without ssh there is nothing to ask.
    let resolved = |file: &Path, host: &str| -> Option<String> {
        let output = std::process::Command::new("ssh")
            .arg("-F")
            .arg(file)
            .args(["-G", host])
            .output()
            .ok()?;
        // Where a build runs as nobody that the system knows, ssh
        // has no home directory to read from and says so.
        if String::from_utf8_lossy(&output.stderr).contains("No user exists") {
            return None;
        }
        assert!(output.status.success(), "{output:?}");
        Some(String::from_utf8_lossy(&output.stdout).into_owned())
    };

    // With nothing of the user's, the editor's values hold.
    let file = config(&mut h, "none");
    let Some(plain) = resolved(&file, "somewhere") else {
        return;
    };
    let ours = format!("controlpath {}/ssh-%C", dir.path().display());
    assert!(plain.contains("controlmaster auto"), "{plain}");
    assert!(plain.contains("controlpersist 600"), "{plain}");
    assert!(plain.contains("serveraliveinterval 10"), "{plain}");
    // ssh shows the path with the host worked in, or as it was given.
    let path_line = |text: &str| {
        let line = text.lines().find(|line| line.starts_with("controlpath "));
        line.unwrap_or_default().to_owned()
    };
    assert!(
        path_line(&plain).starts_with(&format!("controlpath {}/ssh-", dir.path().display())),
        "{plain} (wanted {ours})"
    );

    // What the user set holds, for the hosts that it was set for, and
    // what the user did not set is still the editor's.
    fs::write(
        dir.path().join("mine"),
        "Host special\n  ControlPath /somewhere/else-%C\n  ServerAliveInterval 30\n  Compression yes\n",
    )
    .unwrap();
    let file = config(&mut h, "mine");
    let special = resolved(&file, "special").unwrap();
    assert!(
        path_line(&special).starts_with("controlpath /somewhere/else-"),
        "{special}"
    );
    assert!(special.contains("serveraliveinterval 30"), "{special}");
    assert!(special.contains("compression yes"), "{special}");
    assert!(special.contains("controlmaster auto"), "{special}");
    let other = resolved(&file, "other").unwrap();
    assert!(
        path_line(&other).starts_with(&format!("controlpath {}/ssh-", dir.path().display())),
        "{other}"
    );
    assert!(other.contains("serveraliveinterval 10"), "{other}");
    assert!(other.contains("compression no"), "{other}");
}
