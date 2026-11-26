//! Several frames on one editor, clients waiting for buffers, and auto-save.

use std::fs;

use amacs_core::{Editor, EditorEvent, FrameId};
use amacs_proto::{InputEvent, parse_keys};
use amacs_testkit::{Harness, Screen};

/// An editor with a headless base frame and two attached frames, as a
/// server with two clients has.
struct TwoFrames {
    editor: Editor,
    frames: [FrameId; 2],
    screens: [Screen; 2],
}

impl TwoFrames {
    fn new() -> Self {
        let mut editor = Editor::new(80, 24);
        editor.set_daemon(true);
        editor.set_default_directory("/nonexistent/amacs-test");
        let frames = [editor.create_frame(40, 6), editor.create_frame(30, 5)];
        let mut this = Self {
            editor,
            frames,
            screens: [Screen::default(), Screen::default()],
        };
        this.redisplay();
        this
    }

    fn redisplay(&mut self) {
        for (frame, screen) in self.frames.iter().zip(&mut self.screens) {
            let ops = self.editor.redisplay_frame(*frame);
            screen.apply(&ops);
        }
    }

    fn keys(&mut self, frame: usize, keys: &str) -> &mut Self {
        for key in parse_keys(keys).unwrap() {
            self.editor
                .handle_frame_event(self.frames[frame], InputEvent::Key(key));
        }
        self.redisplay();
        self
    }

    fn type_text(&mut self, frame: usize, text: &str) -> &mut Self {
        for c in text.chars() {
            let event = InputEvent::Key(amacs_proto::Key::char(c));
            self.editor.handle_frame_event(self.frames[frame], event);
        }
        self.redisplay();
        self
    }
}

#[test]
fn frames_show_the_same_buffer_with_their_own_windows_and_sizes() {
    let mut t = TwoFrames::new();
    t.type_text(0, "shared text");
    assert_eq!(t.screens[0].row(0), "shared text");
    assert_eq!(t.screens[1].row(0), "shared text");
    assert_eq!(t.screens[0].size(), (40, 6));
    assert_eq!(t.screens[1].size(), (30, 5));

    t.keys(1, "C-x 2");
    assert_eq!(
        t.screens[1].row(2),
        "shared text",
        "the second frame is split"
    );
    assert_eq!(t.screens[0].row(2), "", "the first frame is not");

    t.keys(1, "M-<").type_text(1, "> ");
    assert_eq!(t.screens[0].row(0), "> shared text");
    assert_eq!(
        t.screens[0].cursor(),
        Some((0, 13)),
        "each frame keeps its own point"
    );
    assert_eq!(t.screens[1].cursor(), Some((0, 2)));
}

#[test]
fn messages_and_prompts_belong_to_one_frame() {
    let mut t = TwoFrames::new();
    t.keys(0, "C-x");
    assert_eq!(t.screens[0].row(5), "C-x-");
    assert_eq!(t.screens[1].row(4), "");

    t.keys(0, "C-g M-g g").type_text(0, "12");
    assert_eq!(t.screens[0].row(5), "Goto line: 12");
    assert_eq!(
        t.screens[1].row(4),
        "",
        "the other frame does not show the prompt"
    );

    // Typing in the other frame leaves the first frame's prompt as it is:
    // each frame has its own input in progress.
    t.type_text(1, "x");
    assert_eq!(t.screens[0].row(5), "Goto line: 12");
    assert_eq!(t.screens[0].row(0), "x");
    assert_eq!(t.screens[1].row(4), "");
    // A key sequence begun in one frame is not continued by the other.
    t.keys(1, "C-x");
    assert_eq!(t.screens[1].row(4), "C-x-");
    t.type_text(0, "3");
    assert_eq!(t.screens[0].row(5), "Goto line: 123");
    assert_eq!(t.screens[1].row(4), "C-x-");
    t.keys(1, "C-g");
    // The prompt is answered, or given up, where it was asked.
    t.keys(0, "C-g");
    assert_eq!(t.screens[0].row(5), "Quit");
    t.type_text(0, "y");
    // Its point was at the start, before the x typed in the other frame.
    assert_eq!(
        t.screens[1].row(0),
        "yx",
        "and the first frame is back in its buffer"
    );
}

#[test]
fn closing_the_terminal_closes_only_that_clients_frame() {
    let mut t = TwoFrames::new();
    t.type_text(0, "text").keys(0, "C-x C-c");
    assert!(!t.editor.should_exit());
    assert_eq!(
        t.editor.take_events(),
        [EditorEvent::FrameClosed(t.frames[0])]
    );
    assert_eq!(t.editor.frame_ids().count(), 2);
    assert_eq!(t.editor.current_frame(), t.frames[1]);
    assert!(
        t.editor.redisplay_frame(t.frames[0]).is_empty(),
        "a closed frame draws nothing"
    );

    t.keys(1, "C-x 5 0");
    assert_eq!(t.editor.current_frame(), t.editor.base_frame());
    assert_eq!(t.editor.text().to_string(), "text");

    // Only an explicit command stops the server.
    t.editor.execute("kill-editor");
    assert!(t.editor.should_exit());
}

#[test]
fn the_base_frame_cannot_be_deleted() {
    let mut h = Harness::new(40, 6);
    h.keys("C-x 5 0");
    assert_eq!(h.echo(), "Attempt to delete the sole frame");
    h.keys("C-x C-c");
    assert!(h.editor.should_exit(), "outside a server, C-x C-c exits");
}

#[test]
fn finishing_a_buffer_releases_its_waiting_clients() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("COMMIT_EDITMSG");
    fs::write(&path, "").unwrap();

    let mut t = TwoFrames::new();
    t.keys(0, "C-x #");
    assert_eq!(t.screens[0].row(5), "No client is waiting for this buffer");

    t.editor.select_frame(t.frames[0]);
    t.editor.find_file(&path).unwrap();
    let buffer = t.editor.find_buffer("COMMIT_EDITMSG").unwrap();
    t.editor.add_waiter(buffer, 7);
    t.type_text(0, "Fix it").keys(0, "C-x #");
    // The frame is too narrow for the whole question.
    assert!(
        t.screens[0]
            .row(5)
            .starts_with("Save COMMIT_EDITMSG before finishing?")
    );
    assert!(t.editor.take_events().is_empty());
    t.type_text(0, "y");
    assert_eq!(fs::read_to_string(&path).unwrap(), "Fix it");
    assert_eq!(t.editor.take_events(), [EditorEvent::WaiterReleased(7)]);
    assert!(
        t.editor.find_buffer("COMMIT_EDITMSG").is_none(),
        "the buffer is killed"
    );
}

#[test]
fn killing_a_buffer_releases_waiters_and_gone_clients_are_forgotten() {
    let mut h = Harness::new(60, 6);
    h.keys("C-x b").type_text("one").keys("RET");
    let one = h.editor.find_buffer("one").unwrap();
    h.editor.add_waiter(one, 1);
    h.editor.add_waiter(one, 2);
    h.editor.remove_waiters(1);
    h.keys("C-x k RET");
    assert_eq!(h.editor.take_events(), [EditorEvent::WaiterReleased(2)]);
}

/// Turns off the undo history on disk, which is what brings unsaved
/// changes back when it is there: auto-save data is what is left.
fn without_undo_journal(h: &mut Harness) {
    h.editor
        .set_option("persistent-undo", amacs_core::Value::Bool(false))
        .unwrap();
}

#[test]
fn the_undo_history_of_a_file_outlives_the_editor() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let path = dir.path().join("h.txt");
    fs::write(&path, "x").unwrap();
    let editor = |path: &std::path::Path| {
        let mut h = Harness::new(80, 6);
        h.editor.set_state_directory(Some(state.clone()));
        h.editor.find_file(path).unwrap();
        h.resize(80, 6);
        h
    };

    let mut h = editor(&path);
    assert_eq!(h.echo(), "");
    h.type_text("ab").keys("C-x C-s");
    h.keys("C-e RET").type_text("unsaved");
    h.editor.write_undo_journals();
    assert_eq!(fs::read_dir(state.join("undo")).unwrap().count(), 1);
    assert!(!state.join("auto-save").exists());

    // Another editor, as after a power failure: the file as it was saved,
    // with the history of how it got there.
    let mut h = editor(&path);
    assert_eq!(
        h.echo(),
        "h.txt has changes that were not saved; use M-x recover-this-file"
    );
    assert_eq!(h.buffer_text(), "abx");
    h.keys("C-/");
    assert_eq!(h.buffer_text(), "x", "what an earlier editor did is undone");
    h.keys("C-?");
    assert_eq!(h.buffer_text(), "abx");
    h.keys("M-x").type_text("recover-this-file").keys("RET");
    assert_eq!(h.buffer_text(), "abx\nunsaved");
    assert_eq!(
        h.echo(),
        "Recovered the unsaved changes; save the buffer to keep them"
    );

    // Saved under another name, the history goes with the text.
    let copy = dir.path().join("copy.txt");
    h.keys("C-x C-w")
        .keys("C-a C-k")
        .type_text(copy.to_str().unwrap())
        .keys("RET");
    assert_eq!(fs::read_to_string(&copy).unwrap(), "abx\nunsaved");
    h.editor.write_undo_journals();
    let mut h = editor(&copy);
    assert_eq!(h.echo(), "", "it was saved: nothing to recover");
    h.keys("C-/");
    assert_eq!(h.buffer_text(), "abx\n", "one step, as it was made");

    // Read again from a file that something else changed, the buffer has
    // no history, and neither has the journal.
    fs::write(&copy, "other").unwrap();
    h.keys("M-x").type_text("revert-buffer").keys("RET");
    if !h.prompt().is_empty() {
        h.keys("y");
    }
    assert_eq!(h.buffer_text(), "other");
    h.editor.write_undo_journals();
    let mut h = editor(&copy);
    h.keys("C-/");
    assert_eq!(h.echo(), "No further undo information");

    // Changes that the user gave up with the buffer are not offered.
    let mut h = editor(&copy);
    h.type_text("given up").keys("C-x k RET y");
    assert_ne!(h.editor.buffer().name, "copy.txt");
    let h = editor(&copy);
    assert_eq!(h.echo(), "");
    assert_eq!(h.buffer_text(), "other");

    // Without the option, nothing is kept.
    let plain = dir.path().join("plain.txt");
    fs::write(&plain, "x").unwrap();
    let mut h = Harness::new(80, 6);
    h.editor.set_state_directory(Some(state.clone()));
    without_undo_journal(&mut h);
    h.editor.find_file(&plain).unwrap();
    h.type_text("y");
    h.editor.write_undo_journals();
    assert_eq!(fs::read_dir(state.join("undo")).unwrap().count(), 2);
}

#[test]
fn a_secret_buffer_goes_to_no_file_of_the_editors() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let path = dir.path().join("secret.txt");
    fs::write(&path, "x").unwrap();
    let secret = |h: &mut Harness| {
        h.editor
            .set_local_option("buffer-secret", amacs_core::Value::Bool(true))
            .unwrap();
    };

    let mut h = Harness::new(80, 6);
    h.editor.set_state_directory(Some(state.clone()));
    h.editor.find_file(&path).unwrap();
    // It had a journal from being read; made secret, that ends.
    h.editor.write_undo_journals();
    let journal = fs::read_dir(state.join("undo")).unwrap().next().unwrap();
    let journal = journal.unwrap().path();
    let before = fs::read(&journal).unwrap();
    secret(&mut h);
    h.type_text("hunter2");
    assert_eq!(h.editor.auto_save(true), 0);
    assert!(!state.join("auto-save").exists());
    assert_eq!(fs::read(&journal).unwrap(), before);
    // Saving does not start one again.
    h.keys("C-x C-s");
    h.type_text("more");
    h.editor.auto_save(true);
    assert_eq!(fs::read(&journal).unwrap(), before);
    assert!(!state.join("auto-save").exists());
    // Its text is not given to a program.
    h.keys("C-x h M-|");
    assert_eq!(
        h.echo(),
        "This buffer is secret: its text is not given to programs"
    );

    // Another buffer is as before.
    let plain = dir.path().join("plain.txt");
    fs::write(&plain, "x").unwrap();
    h.editor.find_file(&plain).unwrap();
    h.type_text("y");
    assert_eq!(h.editor.auto_save(true), 1);
}

#[test]
fn auto_save_writes_beside_not_over_the_file_and_recovers() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let path = dir.path().join("work.txt");
    fs::write(&path, "saved\n").unwrap();

    let mut h = Harness::new(80, 6);
    h.editor.set_state_directory(Some(state.clone()));
    without_undo_journal(&mut h);
    h.editor.find_file(&path).unwrap();
    assert_eq!(
        h.editor.auto_save(false),
        0,
        "unmodified buffers are not written"
    );

    h.keys("M->").type_text("unsaved work");
    assert_eq!(h.editor.auto_save(false), 1);
    assert_eq!(h.editor.auto_save(false), 0, "nothing changed since");
    assert_eq!(h.editor.auto_save(true), 1, "forcing writes anyway");
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "saved\n",
        "the file itself is untouched"
    );
    let saved: Vec<_> = fs::read_dir(state.join("auto-save")).unwrap().collect();
    assert_eq!(saved.len(), 1);

    // A new editor, as after a crash, notices the data and can recover it.
    let mut h = Harness::new(80, 6);
    h.editor.set_state_directory(Some(state.clone()));
    without_undo_journal(&mut h);
    h.editor.find_file(&path).unwrap();
    h.keys("C-l");
    assert_eq!(h.buffer_text(), "saved\n");
    h.editor.find_file(&path).unwrap();
    h.keys("M-x").type_text("recover-this-file").keys("RET");
    assert_eq!(h.buffer_text(), "saved\nunsaved work");
    assert_eq!(
        h.echo(),
        "Recovered from auto-save data; save the buffer to keep it"
    );
    h.keys("C-/");
    assert_eq!(h.buffer_text(), "saved\n", "recovery can be undone");
    h.keys("C-?");

    // Saving makes the auto-save data obsolete.
    h.keys("C-x C-s");
    assert_eq!(fs::read_dir(state.join("auto-save")).unwrap().count(), 0);
    h.keys("M-x").type_text("recover-this-file").keys("RET");
    assert_eq!(h.echo(), "No auto-save data for this buffer");
}

#[test]
fn visiting_a_file_with_auto_save_data_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let path = dir.path().join("a!b.txt");
    fs::write(&path, "x").unwrap();

    let mut h = Harness::new(80, 6);
    h.editor.set_state_directory(Some(state.clone()));
    without_undo_journal(&mut h);
    h.editor.find_file(&path).unwrap();
    h.type_text("y");
    h.editor.auto_save(false);

    // A new editor, as after a crash.
    let mut fresh = Harness::new(80, 6);
    fresh.editor.set_state_directory(Some(state));
    without_undo_journal(&mut fresh);
    fresh.editor.find_file(&path).unwrap();
    fresh.resize(80, 6);
    assert_eq!(
        fresh.echo(),
        "a!b.txt has auto-save data; use M-x recover-this-file"
    );
}

#[test]
fn undoing_all_changes_removes_the_auto_save_file() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let path = dir.path().join("f.txt");
    fs::write(&path, "x").unwrap();

    let mut h = Harness::new(80, 6);
    h.editor.set_state_directory(Some(state.clone()));
    h.editor.find_file(&path).unwrap();
    h.type_text("y");
    h.editor.auto_save(false);
    h.keys("C-/");
    assert_eq!(h.editor.auto_save(false), 0);
    assert_eq!(fs::read_dir(state.join("auto-save")).unwrap().count(), 0);
}

#[test]
fn goto_line_moves_to_a_line_and_column() {
    let mut h = Harness::new(40, 8);
    h.paste("one\ntwo\nthree");
    h.keys("M-g g").type_text("2").keys("RET");
    assert_eq!(h.screen.cursor(), Some((1, 0)));
    h.editor.goto_line(3, 99);
    h.keys("C-l");
    assert_eq!(
        h.editor.point(),
        13,
        "a column past the end stops at the end of the line"
    );
    h.editor.goto_line(0, 0);
    assert_eq!(h.editor.point(), 0);
    h.keys("M-g g").type_text("x").keys("RET");
    assert_eq!(h.echo(), "Please enter a line number");
}
