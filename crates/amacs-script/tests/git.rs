//! The git views, which are written in Scheme: `runtime/lib/git.scm`.

mod common;

use std::path::Path;
use std::process::Command;

use common::Harness;

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn numbered(lines: std::ops::RangeInclusive<u32>) -> String {
    lines.map(|n| format!("line {n}\n")).collect()
}

/// A repository with one commit, a file changed in two places, and a new
/// file; and an editor showing its status.
fn setup() -> (tempfile::TempDir, Harness) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "--quiet", "--initial-branch=main"]);
    for (key, value) in [
        ("user.name", "Tester"),
        ("user.email", "tester@example.org"),
        ("commit.gpgsign", "false"),
    ] {
        git(root, &["config", key, value]);
    }
    std::fs::write(root.join("notes.txt"), numbered(1..=30)).unwrap();
    git(root, &["add", "notes.txt"]);
    git(root, &["commit", "--quiet", "-m", "Start the notes"]);
    let changed = numbered(1..=30)
        .replace("line 2\n", "line two\n")
        .replace("line 25\n", "line 25\nline 25 and a half\n");
    std::fs::write(root.join("notes.txt"), changed).unwrap();
    std::fs::write(root.join("new.txt"), "brand new\n").unwrap();

    let mut h = Harness::new(80, 30);
    h.eval(&format!(
        "(find-file {:?})",
        root.join("notes.txt").display().to_string()
    ));
    h.keys("C-x g");
    h.wait_until(|h| h.text().contains("Recent commits"));
    (dir, h)
}

/// Puts point at the start of the first line that contains a text.
fn goto(h: &mut Harness, text: &str) {
    h.eval(&format!(
        "(goto-char (string-search (buffer-string) {text:?})) (goto-char (line-beginning-position))"
    ));
}

/// Presses keys and waits until the buffer's text satisfies a condition.
fn keys_until(h: &mut Harness, keys: &str, done: impl Fn(&str) -> bool) {
    h.keys(keys);
    h.wait_until(|h| done(&h.text()));
}

#[test]
fn the_status_shows_what_has_changed() {
    let (_dir, mut h) = setup();
    assert_eq!(
        h.text(),
        "Head:     main  Start the notes\n\
         \n\
         ▾ Untracked files (1)\n\
         new.txt\n\
         \n\
         ▾ Unstaged changes (1)\n\
         ▸ modified  notes.txt\n\
         \n\
         ▾ Recent commits (1)\n"
            .to_owned()
            + &format!(
                "{} Start the notes\n",
                git(_dir.path(), &["log", "-1", "--format=%h"]).trim()
            )
    );
    assert!(
        h.mode_line().contains("*git: ") && h.mode_line().contains("(Git)"),
        "{}",
        h.mode_line()
    );
    assert_eq!(h.screen.face_at(2, 2).fg, h.face("git-heading").fg);

    // A file opens to show its hunks, in the colours of a diff.
    goto(&mut h, "modified");
    h.keys("TAB");
    let text = h.text();
    assert!(
        text.contains("▾ modified  notes.txt\n▾ @@ -1,5 +1,5 @@\n line 1\n-line 2\n+line two\n"),
        "{text}"
    );
    assert!(text.contains("+line 25 and a half\n"), "{text}");
    goto(&mut h, "-line 2");
    let row = h.screen.cursor().unwrap().0;
    assert_eq!(h.screen.face_at(row, 0).bg, h.face("git-removed").bg);
    assert_eq!(h.screen.face_at(row + 1, 0).bg, h.face("git-added").bg);
    // And closes again; a whole group folds too.
    goto(&mut h, "modified");
    h.keys("TAB");
    assert!(
        h.text().contains("▸ modified  notes.txt\n\n"),
        "{}",
        h.text()
    );
    goto(&mut h, "Untracked files");
    h.keys("TAB");
    assert!(
        h.text().contains("▸ Untracked files (1)\n\n"),
        "{}",
        h.text()
    );
}

#[test]
fn files_and_hunks_are_staged_and_unstaged() {
    let (dir, mut h) = setup();
    let root = dir.path();
    goto(&mut h, "new.txt");
    keys_until(&mut h, "s", |text| text.contains("Staged changes (1)"));
    assert!(h.text().contains("▸ new file  new.txt"), "{}", h.text());
    assert!(!h.text().contains("Untracked"), "{}", h.text());
    assert_eq!(git(root, &["diff", "--cached", "--name-only"]), "new.txt\n");

    // One hunk of two.
    goto(&mut h, "modified");
    h.keys("TAB");
    goto(&mut h, "+line 25 and a half");
    keys_until(&mut h, "s", |text| text.contains("Staged changes (2)"));
    let staged = git(root, &["diff", "--cached", "--", "notes.txt"]);
    assert!(
        staged.contains("+line 25 and a half") && !staged.contains("line two"),
        "{staged}"
    );
    let unstaged = git(root, &["diff", "--", "notes.txt"]);
    assert!(
        unstaged.contains("line two") && !unstaged.contains("and a half"),
        "{unstaged}"
    );

    // Back out of the index: the hunk, then everything.
    goto(&mut h, "Staged changes");
    h.keys("n n TAB");
    goto(&mut h, "+line 25 and a half");
    keys_until(&mut h, "u", |text| text.contains("Staged changes (1)"));
    assert_eq!(git(root, &["diff", "--cached", "--name-only"]), "new.txt\n");
    goto(&mut h, "Staged changes");
    keys_until(&mut h, "u", |text| !text.contains("Staged changes"));
    assert_eq!(git(root, &["diff", "--cached", "--name-only"]), "");
    // All at once.
    goto(&mut h, "Unstaged changes");
    keys_until(&mut h, "s", |text| !text.contains("Unstaged changes"));
    assert_eq!(
        git(root, &["diff", "--cached", "--name-only"]),
        "notes.txt\n"
    );
    h.keys("s");
    assert_eq!(h.echo(), "That cannot be staged");
}

#[test]
fn changes_are_discarded_after_asking() {
    let (dir, mut h) = setup();
    let root = dir.path();
    goto(&mut h, "modified");
    h.keys("TAB");
    goto(&mut h, "+line two");
    h.keys("k");
    assert_eq!(h.echo(), "Discard this hunk? (y or n)");
    h.keys("n");
    assert!(
        std::fs::read_to_string(root.join("notes.txt"))
            .unwrap()
            .contains("line two")
    );
    h.keys("k y");
    h.wait_until(|h| !h.text().contains("line two"));
    let file = std::fs::read_to_string(root.join("notes.txt")).unwrap();
    assert!(
        file.contains("line 2\n") && file.contains("and a half"),
        "{file}"
    );

    goto(&mut h, "new.txt");
    h.keys("k");
    assert_eq!(h.echo(), "Delete new.txt? (y or n)");
    h.keys("y");
    h.wait_until(|h| !h.text().contains("Untracked"));
    assert!(!root.join("new.txt").exists());
}

#[test]
fn a_commit_is_made_with_a_message_written_in_a_buffer() {
    let (dir, mut h) = setup();
    let root = dir.path();
    goto(&mut h, "Unstaged changes");
    keys_until(&mut h, "s", |text| text.contains("Staged changes"));
    h.keys("c");
    assert_eq!(h.prompt(), "Commit:");
    let menu = h.screen.text();
    assert!(
        menu.contains(" c   Commit") && menu.contains(" -a  [ ] Stage all changes"),
        "{menu}"
    );
    h.keys("c");
    h.wait_until(|h| h.text().contains("# Write the message above"));
    assert!(
        h.mode_line().contains("COMMIT_EDITMSG") && h.mode_line().contains("Commit"),
        "{}",
        h.mode_line()
    );
    assert!(h.text().contains("#   M  notes.txt"), "{}", h.text());
    h.type_text("Rewrite line two").keys("C-c C-c");
    h.wait_until(|h| h.text().contains("Recent commits (2)"));
    assert!(h.echo().contains("Rewrite line two"), "{}", h.echo());
    assert_eq!(
        git(root, &["log", "-1", "--format=%s"]),
        "Rewrite line two\n"
    );
    assert!(
        h.text().starts_with("Head:     main  Rewrite line two\n"),
        "{}",
        h.text()
    );
    assert!(!h.text().contains("Staged changes"), "{}", h.text());

    // Amending starts from the old message; cancelling leaves all as it was.
    h.keys("c a");
    h.wait_until(|h| h.text().starts_with("Rewrite line two\n"));
    h.keys("C-c C-k");
    assert_eq!(h.echo(), "Commit cancelled");
    assert_eq!(
        git(root, &["log", "--format=%s"]),
        "Rewrite line two\nStart the notes\n"
    );
    // With the switch, everything tracked goes in without staging.
    std::fs::write(root.join("notes.txt"), "all new\n").unwrap();
    h.keys("c - a");
    assert!(
        h.screen.text().contains(" -a  [x] Stage all changes"),
        "{}",
        h.screen.text()
    );
    h.keys("a");
    h.wait_until(|h| h.text().starts_with("Rewrite line two\n"));
    h.keys("M-<").type_text("Really ").keys("C-c C-c");
    h.wait_until(|h| {
        h.text()
            .starts_with("Head:     main  Really Rewrite line two")
    });
    assert_eq!(
        git(root, &["log", "--format=%s"]),
        "Really Rewrite line two\nStart the notes\n"
    );
    assert_eq!(git(root, &["show", "HEAD:notes.txt"]), "all new\n");
}

#[test]
fn a_change_leads_to_its_place_in_the_file() {
    let (dir, mut h) = setup();
    goto(&mut h, "modified");
    h.keys("TAB");
    goto(&mut h, "+line 25 and a half");
    h.keys("RET");
    assert_eq!(h.eval("(buffer-name)"), "\"notes.txt\"");
    assert_eq!(h.eval("(line-number-at-pos)"), "26");
    assert_eq!(
        h.eval("(buffer-substring (line-beginning-position) (line-end-position))"),
        "\"line 25 and a half\""
    );
    // Who wrote what, from the file.
    h.keys("M-x").type_text("git-blame").keys("RET");
    h.wait_until(|h| h.text().contains("Tester"));
    assert_eq!(h.eval("(line-number-at-pos)"), "26");
    let hash = git(dir.path(), &["log", "-1", "--format=%h"]);
    assert!(
        h.text().lines().next().unwrap().contains(&hash.trim()[..7]),
        "{}",
        h.text()
    );
}

#[test]
fn the_log_shows_commits_and_what_they_changed() {
    let (dir, mut h) = setup();
    let root = dir.path();
    git(root, &["commit", "--quiet", "-am", "Change two lines"]);
    h.keys("g");
    h.wait_until(|h| h.text().contains("Recent commits (2)"));
    h.keys("l l");
    h.wait_until(|h| h.text().contains("Start the notes") && !h.text().contains("Head:"));
    let first = h.text().lines().next().unwrap().to_owned();
    assert!(
        first.contains("Tester") && first.ends_with("Change two lines (HEAD -> main)"),
        "{first}"
    );
    h.keys("RET");
    h.wait_until(|h| h.text().contains("@@"));
    let text = h.text();
    assert!(
        text.contains("Author: Tester <tester@example.org>"),
        "{text}"
    );
    assert!(
        text.contains("▾ modified  notes.txt\n▾ @@ -1,5 +1,5 @@"),
        "{text}"
    );
    assert!(text.contains("-line 2\n+line two\n"), "{text}");
    // A commit cannot be staged, but its lines lead to the file.
    goto(&mut h, "+line two");
    h.keys("s");
    assert_eq!(h.echo(), "That is staged already");
    h.keys("RET");
    assert_eq!(
        h.eval("(list (buffer-name) (line-number-at-pos))"),
        "(\"notes.txt\" 2)"
    );
}

#[test]
fn branches_and_stashes() {
    let (dir, mut h) = setup();
    let root = dir.path();
    h.keys("b c").type_text("feature").keys("RET");
    h.wait_until(|h| h.text().starts_with("Head:     feature"));
    assert_eq!(git(root, &["branch", "--show-current"]), "feature\n");

    h.keys("z z");
    h.wait_until(|h| h.text().contains("Stashes (1)"));
    assert!(!h.text().contains("Unstaged"), "{}", h.text());
    assert_eq!(
        std::fs::read_to_string(root.join("notes.txt")).unwrap(),
        numbered(1..=30)
    );
    // Git is asked for the branches first, so the prompt takes a moment.
    h.keys("b b");
    h.wait_until(|h| h.prompt().contains("Check out:"));
    h.type_text("main").keys("RET");
    h.wait_until(|h| h.text().starts_with("Head:     main"));
    h.keys("z p");
    h.wait_until(|h| h.text().contains("Unstaged changes (1)"));
    assert!(!h.text().contains("Stashes"), "{}", h.text());

    // What git says when it refuses is what the user is told.
    h.keys("b k");
    h.wait_until(|h| h.prompt().contains("Delete branch:"));
    h.type_text("main").keys("RET");
    h.wait_until(|h| h.echo().contains("main"));
    assert!(
        h.echo().to_lowercase().contains("cannot delete"),
        "{}",
        h.echo()
    );
    h.keys("P p");
    h.wait_until(|h| h.echo().contains("origin"));
    assert!(h.echo().starts_with("fatal:"), "{}", h.echo());
    h.keys("$");
    assert!(
        h.text().contains("$ git push --set-upstream origin main"),
        "{}",
        h.text()
    );
}

#[test]
fn lines_that_differ_from_git_are_marked_beside_their_numbers() {
    let (dir, mut h) = setup();
    let root = dir.path();
    h.eval("(enable-mode! 'git-gutter-mode) (set-option! 'line-numbers #t)");
    h.keys("q");
    assert_eq!(h.eval("(buffer-name)"), "\"notes.txt\"");
    assert!(
        h.screen.row(1).starts_with("  2 line two"),
        "{}",
        h.screen.row(1)
    );
    // Saving works them out: a changed line, and further down an added one.
    h.keys("M-<").type_text("x").keys("DEL C-x C-s");
    h.wait_until(|h| h.screen.row(1).starts_with("  2▎line two"));
    assert_eq!(h.screen.face_at(1, 3).fg, h.face("git-gutter-changed").fg);
    assert!(
        h.screen.row(0).starts_with("  1 line 1"),
        "{}",
        h.screen.row(0)
    );
    h.eval("(goto-line 26)");
    let row = h.screen.cursor().unwrap().0;
    assert!(
        h.screen.row(row).starts_with(" 26▎line 25 and a half"),
        "{}",
        h.screen.row(row)
    );
    assert_eq!(h.screen.face_at(row, 3).fg, h.face("git-gutter-added").fg);
    // A mark stays with its line while the text above it changes.
    h.keys("M-< RET");
    assert!(
        h.screen.row(2).starts_with("  3▎line two"),
        "{}",
        h.screen.row(2)
    );
    assert!(
        h.screen.row(1).starts_with("  2 line 1"),
        "{}",
        h.screen.row(1)
    );
    h.keys("C-/");
    // Once the change is committed there is nothing to mark.
    git(root, &["commit", "--quiet", "-am", "Change"]);
    h.keys("M-<").type_text("x").keys("DEL C-x C-s");
    h.wait_until(|h| h.screen.row(1).starts_with("  2 line two"));
    // Removed lines show on the line they came after. Code given as text
    // can ask for the marks as well as a hook can.
    let committed = git(root, &["show", "HEAD:notes.txt"]);
    std::fs::write(root.join("notes.txt"), committed.replace("line 3\n", "")).unwrap();
    h.keys("M-x").type_text("revert-buffer").keys("RET");
    h.eval("(git-gutter-update)");
    h.wait_until(|h| h.screen.row(1).starts_with("  2▎line two"));
    assert_eq!(h.screen.face_at(1, 3).fg, h.face("git-gutter-removed").fg);
}
