//! Language servers, end to end, against the fake server of this crate.
//! The fake keeps its own copy of each document, so what it answers shows
//! whether the editor kept it told.

use std::path::{Path, PathBuf};
use std::time::Duration;

use amacs_core::{ServerConfig, Value};
use amacs_proto::FaceAttrs;
use amacs_testkit::{Harness, fake_lsp_path};

/// Longer than any delay, to make what waits for one happen.
const LATER: Duration = Duration::from_secs(3600);

struct Project {
    dir: tempfile::TempDir,
    h: Harness,
}

impl Project {
    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().canonicalize().unwrap().join(name)
    }

    /// Visits a file of the project and attaches it to the server.
    fn open(&mut self, name: &str) {
        let path = self.path(name);
        self.h.editor.find_file(&path).unwrap();
        self.h.editor.set_language(Some("rust")).unwrap();
        self.h.editor.lsp_ensure().unwrap();
        self.h.sync();
    }
}

/// A project of files with a server started with `args`. Delays are made
/// endless, so that nothing happens by itself while a test is slow.
fn project_with(files: &[(&str, &str)], args: &[&str]) -> Project {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    for (name, content) in files {
        std::fs::write(dir.path().join(name), content).unwrap();
    }
    let mut h = Harness::new(70, 16);
    h.editor.lsp_define_server(ServerConfig {
        languages: vec!["rust".to_owned()],
        program: fake_lsp_path().display().to_string(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        options: serde_json::Value::Null,
    });
    for option in ["lsp-idle-delay", "completion-delay"] {
        h.editor.set_option(option, Value::Int(1_000_000)).unwrap();
    }
    let mut project = Project { dir, h };
    if let Some((first, _)) = files.first() {
        project.open(first);
    }
    project
}

fn project(files: &[(&str, &str)]) -> Project {
    project_with(files, &[])
}

fn face(h: &Harness, name: &str) -> FaceAttrs {
    let faces = h.editor.faces();
    faces.table()[faces.face_id(name).unwrap().0 as usize]
}

fn mode_line(h: &Harness) -> String {
    let row = h.screen.size().1 - 2;
    h.screen.row(row)
}

#[test]
fn diagnostics_are_shown_and_follow_the_text() {
    for args in [&[][..], &["--utf8"], &["--full-sync"]] {
        let mut p = project_with(&[("main.rs", "let a = BUG;\nWARN here\n")], args);
        let h = &mut p.h;
        assert!(
            mode_line(h).ends_with("(Fundamental) [fake-lsp] E1 W1"),
            "{args:?}: {}",
            mode_line(h)
        );
        let error = h.screen.face_at(0, 8);
        assert!(error.underline);
        assert_eq!(
            error.underline_color,
            face(h, "diagnostic-error").underline_color
        );
        assert_eq!(
            h.screen.face_at(1, 0).underline_color,
            face(h, "diagnostic-warning").underline_color
        );
        assert!(!h.screen.face_at(0, 4).underline);

        // Text before the diagnostic, with chars that count differently in
        // every encoding, moves it along.
        h.type_text("ö😀 ");
        assert_eq!(h.screen.row(0), "ö😀 let a = BUG;");
        assert!(h.screen.face_at(0, 13).underline, "{args:?}");
        assert!(!h.screen.face_at(0, 11).underline, "{args:?}");

        // A second error, typed a char at a time, and its undoing.
        h.keys("C-e").type_text(" BUG");
        assert!(
            mode_line(h).ends_with("E2 W1"),
            "{args:?}: {}",
            mode_line(h)
        );
        h.keys("C-/");
        assert!(
            mode_line(h).ends_with("E1 W1"),
            "{args:?}: {}",
            mode_line(h)
        );
        // Fixing the first error leaves the warning.
        h.keys("C-a C-k");
        assert!(
            mode_line(h).ends_with("[fake-lsp] W1"),
            "{args:?}: {}",
            mode_line(h)
        );
        assert!(h.screen.face_at(1, 0).underline);
    }
}

#[test]
fn point_resting_on_something_says_what_it_is() {
    let mut p = project(&[("main.rs", "alpha BUG\n")]);
    let h = &mut p.h;
    h.keys("C-f");
    assert_eq!(h.echo(), "");
    h.wait(LATER);
    assert_eq!(h.echo(), "**alpha**", "what the server knows of the word");
    h.keys("M-f C-f C-f");
    assert_eq!(h.echo(), "");
    h.wait(LATER);
    assert_eq!(h.echo(), "fake: BUG found", "a diagnostic comes first");
    // Asked for outright, a longer answer gets a window of its own.
    h.keys("M-< C-c C-d");
    assert_eq!(h.screen.row(8), "**alpha**");
    assert_eq!(h.screen.row(10), "documentation of alpha");
    assert!(h.screen.row(14).contains("*lsp-help*"));
}

#[test]
fn diagnostics_can_be_stepped_through_and_listed() {
    let mut p = project(&[("main.rs", "a BUG\nb WARN\nc NOTE\n")]);
    let h = &mut p.h;
    h.keys("M-g n");
    assert_eq!(h.screen.cursor(), Some((0, 2)));
    assert_eq!(h.echo(), "BUG found");
    h.keys("M-g n M-g n");
    assert_eq!(h.screen.cursor(), Some((2, 2)));
    h.keys("M-g n");
    assert_eq!(h.echo(), "No more diagnostics after point");
    h.keys("M-g p");
    assert_eq!(h.screen.cursor(), Some((1, 2)));

    h.keys("C-c !");
    assert_eq!(h.prompt(), "2/3 Diagnostic:");
    assert_eq!(
        h.candidates(),
        [
            "1: error: BUG found     fake",
            "2: warning: WARN found  fake",
            "3: note: NOTE found     fake",
        ]
    );
    h.type_text("note").keys("RET");
    assert_eq!(h.screen.cursor(), Some((2, 2)));
}

#[test]
fn completions_pop_up_and_are_inserted() {
    let mut p = project(&[("main.rs", "alpha alphabet beta\n\n")]);
    let h = &mut p.h;
    h.keys("M->").type_text("al");
    assert_eq!(h.screen.row(3), "", "nothing until typing has paused");
    h.wait(LATER);
    assert_eq!(h.screen.row(3), " alpha     text");
    assert_eq!(h.screen.row(4), " alphabet  text");
    assert_eq!(h.screen.row(5), "", "only what matches is listed");
    assert_eq!(h.screen.face_at(3, 1), face(h, "popup-selected"));
    assert_eq!(h.screen.face_at(4, 1), face(h, "popup"));
    assert_eq!(h.screen.row(2), "al");
    assert_eq!(h.screen.cursor(), Some((2, 2)));

    // More typing narrows the list; C-n and C-p move in it, round the ends.
    h.type_text("p");
    assert_eq!(h.screen.row(3), " alpha     text");
    h.keys("C-n");
    assert_eq!(h.screen.face_at(4, 1), face(h, "popup-selected"));
    h.keys("C-n C-p");
    assert_eq!(h.screen.face_at(4, 1), face(h, "popup-selected"));
    h.keys("RET");
    assert_eq!(h.buffer_text(), "alpha alphabet beta\n\nalphabet");
    assert_eq!(h.screen.row(3), "");

    // Text that leaves no candidate closes the list, and so does moving.
    h.keys("RET").type_text("be");
    h.wait(LATER);
    assert_eq!(h.screen.row(4), " beta      text", "a word that starts so");
    assert_eq!(
        h.screen.row(5),
        " alphabet  text",
        "and then one that has it"
    );
    h.type_text("x");
    assert_eq!(h.screen.row(4), "");
    h.keys("DEL");
    h.wait(LATER);
    assert_eq!(h.screen.row(4), "", "it does not come back by itself");
    h.keys("C-M-i");
    assert_eq!(
        h.screen.row(4),
        " beta      text",
        "but it does when asked for"
    );
    h.keys("C-a");
    assert_eq!(h.screen.row(4), "");
    h.keys("C-e C-M-i C-g");
    assert_eq!(h.screen.row(4), "");
    assert_eq!(h.buffer_text(), "alpha alphabet beta\n\nalphabet\nbe");
}

#[test]
fn a_list_the_server_cut_short_is_asked_for_again() {
    let mut p = project(&[("main.rs", "PARTIAL alpha alphabet\n")]);
    let h = &mut p.h;
    h.keys("M->").type_text("al");
    h.wait(LATER);
    assert_eq!(h.screen.row(2), " alpha");
    assert_eq!(h.screen.row(3), "");
    // What is there cannot be narrowed to something that was held back.
    h.type_text("phab");
    assert_eq!(h.screen.row(2), " alphabet");
    h.keys("RET");
    assert_eq!(h.buffer_text(), "PARTIAL alpha alphabet\nalphabet");
}

#[test]
fn a_completion_can_be_a_snippet_or_bring_edits_with_it() {
    let mut p = project(&[("main.rs", "one\n")]);
    let h = &mut p.h;
    // A snippet: the text with point in its first field.
    h.keys("M->").type_text("pri");
    h.wait(LATER);
    assert_eq!(h.screen.row(2), " println  fn println(fmt)");
    h.keys("TAB");
    assert_eq!(h.buffer_text(), "one\nprintln(\"\")");
    assert_eq!(h.screen.cursor(), Some((1, 9)));
    h.type_text("hi").keys("TAB").type_text(";");
    assert_eq!(h.buffer_text(), "one\nprintln(\"hi\");");

    // An edit elsewhere comes with the candidate.
    h.keys("RET").type_text("imp");
    h.wait(LATER);
    h.keys("RET");
    assert_eq!(
        h.buffer_text(),
        "use import_me;\none\nprintln(\"hi\");\nimport_me"
    );
    assert_eq!(h.screen.cursor(), Some((3, 9)));

    // A candidate that says what it replaces.
    h.keys("RET").type_text("repl");
    h.wait(LATER);
    h.keys("RET");
    assert!(h.buffer_text().ends_with("\nreplaced_whole"));

    // After a char the server asked to be told of, with a placeholder
    // that the first char typed replaces.
    h.keys("RET").type_text("one.");
    h.wait(LATER);
    assert_eq!(h.screen.row(6), "    is_empty             method");
    assert_eq!(h.screen.row(7), "    len       fn len() -> usize");
    assert_eq!(h.screen.row(8), "    push                 method");
    h.type_text("pu").keys("RET");
    assert!(h.buffer_text().ends_with("\none.push(value)"));
    h.type_text("x");
    assert!(h.buffer_text().ends_with("\none.push(x)"));
    h.keys("TAB");
    assert_eq!(h.screen.cursor(), Some((5, 11)));
    // The server has followed all of it.
    h.type_text(" BUG");
    assert!(mode_line(h).ends_with("E1"), "{}", mode_line(h));
}

#[test]
fn definitions_and_references_are_found() {
    let mut p = project(&[
        ("main.rs", "fn target\nuse target\nextern_fn\n"),
        ("other.txt", "first\n   extern_fn here\n"),
    ]);
    let other = p.path("other.txt");
    let h = &mut p.h;
    h.keys("C-n M-f M-f M-.");
    assert_eq!(h.screen.cursor(), Some((0, 3)), "the first use of the word");
    h.keys("M-,");
    assert_eq!(h.screen.cursor(), Some((1, 10)));
    h.keys("M-,");
    assert_eq!(h.echo(), "No earlier place to go back to");

    // A definition in a file that is not open yet.
    h.keys("C-n M-.");
    assert_eq!(h.editor.buffer().file.as_deref(), Some(other.as_path()));
    assert_eq!(h.screen.cursor(), Some((1, 3)));
    h.keys("M-,");
    assert_eq!(h.editor.buffer().name, "main.rs");

    h.keys("M-< M-f M-f M-?");
    assert_eq!(h.prompt(), "1/2 2 references:");
    assert_eq!(
        h.candidates(),
        ["main.rs:1: fn target", "main.rs:2: use target"]
    );
    h.keys("C-n RET");
    assert_eq!(h.screen.cursor(), Some((1, 4)));

    h.keys("M-> M-.");
    assert_eq!(h.echo(), "No definition found");
}

#[test]
fn renaming_changes_every_open_file() {
    let mut p = project(&[
        ("main.rs", "fn old\nold + old\n"),
        ("lib.rs", "uses old here\n"),
    ]);
    p.open("lib.rs");
    let h = &mut p.h;
    h.keys("M-f M-f C-c C-r");
    assert_eq!(h.prompt(), "Rename old to: old");
    h.keys("C-a C-k").type_text("new_name").keys("RET");
    assert_eq!(h.echo(), "Renamed in 2 buffers; save them with C-x s");
    assert_eq!(h.buffer_text(), "uses new_name here\n");
    h.keys("C-x b").type_text("main.rs").keys("RET");
    assert_eq!(h.buffer_text(), "fn new_name\nnew_name + new_name\n");
    // One undo step for the whole rename in a buffer.
    h.keys("C-/");
    assert_eq!(h.buffer_text(), "fn old\nold + old\n");
    h.keys("C-?");

    h.keys("C-x s");
    assert_eq!(h.echo(), "Saved 2 files");
    assert_eq!(
        std::fs::read_to_string(p.path("lib.rs")).unwrap(),
        "uses new_name here\n"
    );
    assert_eq!(
        std::fs::read_to_string(p.path("main.rs")).unwrap(),
        "fn new_name\nnew_name + new_name\n"
    );
}

#[test]
fn code_actions_edit_or_run_a_command() {
    let mut p = project(&[("main.rs", "x BUG y\n")]);
    let h = &mut p.h;
    h.keys("C-c C-a");
    assert_eq!(h.prompt(), "1/1 Code action:");
    assert_eq!(
        h.candidates(),
        ["Leave a note"],
        "nothing to fix where point is"
    );
    h.keys("C-g M-f C-f C-c C-a");
    assert_eq!(
        h.candidates(),
        ["Replace BUG with FIXED  quickfix", "Leave a note"]
    );
    h.keys("RET");
    assert_eq!(h.buffer_text(), "x FIXED y\n");
    assert!(mode_line(h).ends_with("[fake-lsp]"), "{}", mode_line(h));

    // A command makes the server ask for an edit in turn.
    h.keys("C-c C-a RET");
    assert_eq!(h.buffer_text(), "// note\nx FIXED y\n");
    h.keys("C-/");
    assert_eq!(h.buffer_text(), "x FIXED y\n");
    h.keys("C-/");
    assert_eq!(h.buffer_text(), "x BUG y\n");
}

#[test]
fn symbols_are_listed_for_the_buffer_and_the_project() {
    let mut p = project(&[
        ("main.rs", "fn first\nbody\nfn second\n"),
        ("lib.rs", "fn third\nfn second_helper\n"),
    ]);
    p.open("lib.rs");
    let h = &mut p.h;
    h.keys("M-g i");
    assert_eq!(h.prompt(), "1/2 Go to symbol:");
    assert_eq!(
        h.candidates(),
        ["third          function", "second_helper  function"]
    );
    h.keys("C-n");
    assert_eq!(
        h.screen.cursor().map(|(row, _)| row),
        Some(5),
        "in the prompt"
    );
    h.keys("RET");
    assert_eq!(h.editor.point(), 12);

    h.keys("C-c C-s");
    assert_eq!(h.prompt(), "Find symbol: second_helper");
    h.keys("C-a C-k").type_text("second").keys("RET");
    assert_eq!(h.prompt(), "1/2 2 symbols:");
    assert_eq!(
        h.candidates(),
        ["lib.rs:2: fn second_helper", "main.rs:3: fn second"]
    );
    h.keys("C-n RET");
    assert_eq!(h.editor.buffer().name, "main.rs");
    assert_eq!(h.screen.cursor(), Some((2, 3)));
}

#[test]
fn a_server_that_dies_is_reported_and_can_be_restarted() {
    let mut p = project(&[("main.rs", "a BUG\n")]);
    let h = &mut p.h;
    h.keys("M->").type_text("CRASH");
    assert!(
        h.echo().starts_with("fake-lsp: the server exited"),
        "{}",
        h.echo()
    );
    assert!(mode_line(h).ends_with("(Fundamental)"), "{}", mode_line(h));
    assert!(!h.screen.face_at(0, 2).underline);
    h.keys("M-.");
    assert_eq!(h.echo(), "This buffer has no language server");

    h.keys("C-a C-k M-x").type_text("lsp-restart").keys("RET");
    assert_eq!(h.echo(), "Language server restarted");
    assert!(mode_line(h).ends_with("[fake-lsp] E1"), "{}", mode_line(h));

    h.keys("M-x").type_text("lsp-shutdown").keys("RET");
    assert!(mode_line(h).ends_with("(Fundamental)"));
    h.keys("M-x").type_text("lsp").keys("M-RET");
    assert!(h.echo().starts_with("fake-lsp in "), "{}", h.echo());
    assert!(mode_line(h).ends_with("[fake-lsp] E1"), "{}", mode_line(h));
}

#[test]
fn a_killed_buffer_is_closed_on_the_server() {
    let mut p = project(&[("main.rs", "shared\n"), ("lib.rs", "shared\n")]);
    p.open("lib.rs");
    let h = &mut p.h;
    h.keys("M-?");
    assert_eq!(h.prompt(), "1/2 2 references:");
    h.keys("C-g C-x k").type_text("main.rs").keys("RET M-?");
    assert_eq!(h.prompt(), "1/1 1 references:");
    h.keys("C-g");
}

#[test]
fn a_server_that_cannot_start_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("main.rs");
    std::fs::write(&file, "x\n").unwrap();
    let mut h = Harness::new(60, 8);
    h.editor.lsp_define_server(ServerConfig {
        languages: vec!["rust".to_owned()],
        program: "/nonexistent/language-server".to_owned(),
        args: Vec::new(),
        options: serde_json::Value::Null,
    });
    h.editor.find_file(&file).unwrap();
    h.editor.set_language(Some("rust")).unwrap();
    let error = h.editor.lsp_ensure().unwrap_err().to_string();
    assert!(
        error.starts_with("Cannot start the language server /nonexistent/language-server"),
        "{error}"
    );
    // A language without a server is not an error.
    h.editor.set_language(Some("python")).unwrap();
    h.editor.lsp_ensure().unwrap();
    h.keys("M-.");
    assert_eq!(h.echo(), "This buffer has no language server");
}

fn write_script(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn programs_get_the_environment_of_the_projects_envrc() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::create_dir(root.join(".git")).unwrap();
    std::fs::write(root.join(".envrc"), "use flake\n").unwrap();
    std::fs::write(root.join("main.rs"), "a BUG\n").unwrap();
    // A stand-in for direnv that says where the language server is.
    let direnv = root.join("fake-direnv");
    write_script(
        &direnv,
        &format!(
            r#"echo '{{"FAKE_LSP": "{}", "AMACS_TEST_VALUE": "from direnv"}}'"#,
            fake_lsp_path().display()
        ),
    );

    let mut h = Harness::new(70, 8);
    let direnv = Value::str(direnv.display().to_string());
    h.editor.set_option("direnv-program", direnv).unwrap();
    // Only the environment says what `$FAKE_LSP` is.
    h.editor.lsp_define_server(ServerConfig {
        languages: vec!["rust".to_owned()],
        program: "sh".to_owned(),
        args: vec!["-c".to_owned(), "exec \"$FAKE_LSP\"".to_owned()],
        options: serde_json::Value::Null,
    });
    h.editor.find_file(&root.join("main.rs")).unwrap();
    h.editor.set_language(Some("rust")).unwrap();
    h.editor.lsp_ensure().unwrap();
    h.sync();
    assert!(mode_line(&h).ends_with("[sh] E1"), "{}", mode_line(&h));

    // Formatters get it too.
    h.editor.define_formatter(
        "show",
        amacs_core::Formatter {
            program: "sh".to_owned(),
            args: vec!["-c".to_owned(), "echo \"$AMACS_TEST_VALUE\"".to_owned()],
        },
    );
    h.editor
        .set_local_option("formatter", Value::sym("show"))
        .unwrap();
    h.keys("C-c C-f");
    assert_eq!(h.buffer_text(), "from direnv\n");
}

#[test]
fn a_refused_envrc_is_reported_and_the_server_starts_without_it() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::create_dir(root.join(".git")).unwrap();
    std::fs::write(root.join(".envrc"), "").unwrap();
    std::fs::write(root.join("main.rs"), "a BUG\n").unwrap();
    let direnv = root.join("fake-direnv");
    write_script(
        &direnv,
        "echo 'direnv: error .envrc is blocked. Run `direnv allow` to approve its content' >&2; exit 1",
    );
    let mut h = Harness::new(100, 8);
    let direnv = Value::str(direnv.display().to_string());
    h.editor.set_option("direnv-program", direnv).unwrap();
    h.editor.lsp_define_server(ServerConfig {
        languages: vec!["rust".to_owned()],
        program: fake_lsp_path().display().to_string(),
        args: Vec::new(),
        options: serde_json::Value::Null,
    });
    h.editor.find_file(&root.join("main.rs")).unwrap();
    h.editor.set_language(Some("rust")).unwrap();
    h.editor.lsp_ensure().unwrap();
    h.sync();
    assert!(
        h.echo()
            .starts_with("direnv: error .envrc is blocked. Run `direnv allow`"),
        "{}",
        h.echo()
    );
    assert!(
        mode_line(&h).ends_with("[fake-lsp] E1"),
        "{}",
        mode_line(&h)
    );
}
