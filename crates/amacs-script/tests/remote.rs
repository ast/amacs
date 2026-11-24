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
    visit(&mut h, "/sudo::/etc/hosts");
    h.wait_until(|h| h.echo().contains("only /ssh:host: is reached so far"));
    assert!(
        h.echo().contains("Cannot open /sudo::/etc/hosts"),
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
