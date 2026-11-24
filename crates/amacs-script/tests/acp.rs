//! The agent workflow: the compose buffer, prompt history, code references,
//! the window layout, and the conversation as sections. For most of it the
//! connection is stubbed and what an agent would send is fed in; the rest
//! talk to `fake-acp`, an agent that speaks the protocol, and those are the
//! tests of several sessions, of conversations taken up again, of prompts
//! that wait and of images. The real adapter is tried by hand.

mod common;

use common::Harness;
use serde_json::{Value, json};

/// A harness with Claude's connection and sending stubbed out: it looks
/// connected, and sent prompts are recorded in `sent` instead of going to a
/// process.
fn connected() -> Harness {
    let mut h = Harness::new(80, 24);
    h.eval(
        r#"(set! %acp-connected? (lambda () #t))
           (set! %acp-session "stub")
           (set! %acp-root (or (project-root) (working-directory)))
           (define sent '())
           (set! %acp-send-prompt (lambda (text) (set! sent (cons text sent))))"#,
    );
    h
}

#[test]
fn starting_lays_out_the_conversation_and_a_compose_buffer() {
    let mut h = connected();
    h.keys("C-c c c");
    // Two windows: the conversation above, the compose buffer below.
    assert_eq!(h.eval("(length (window-list))"), "2");
    assert_eq!(h.eval("(buffer-name)"), "\"*claude prompt*\"");
    // The compose buffer is the smaller one.
    let compose_h: usize = h.eval("(window-height)").parse().unwrap();
    assert!(compose_h <= 10, "compose window is {compose_h} rows");
    // Its minor mode is on.
    assert!(
        h.eval("(minor-mode-enabled? 'claude-compose-mode)")
            .starts_with("#t")
    );
}

#[test]
fn a_prompt_is_written_and_sent_with_c_c_c_c() {
    let mut h = connected();
    h.keys("C-c c c");
    h.type_text("first line\nsecond line");
    h.keys("C-c C-c");
    // It was sent and the buffer cleared for the next.
    assert_eq!(h.eval("sent"), "(\"first line\\nsecond line\")");
    assert_eq!(h.eval("(buffer-string)"), "\"\"");
    // Another prompt.
    h.type_text("again").keys("C-c C-c");
    assert_eq!(h.eval("(car sent)"), "\"again\"");
    assert_eq!(h.eval("(length sent)"), "2");
}

#[test]
fn an_empty_prompt_is_refused() {
    let mut h = connected();
    h.keys("C-c c c");
    h.keys("C-c C-c");
    assert!(h.echo().contains("empty"), "{}", h.echo());
    assert_eq!(h.eval("sent"), "()");
}

#[test]
fn earlier_prompts_come_back_with_m_p_and_m_n() {
    let mut h = connected();
    h.keys("C-c c c");
    h.type_text("one").keys("C-c C-c");
    h.type_text("two").keys("C-c C-c");
    // M-p walks back from the newest.
    h.keys("M-p");
    assert_eq!(h.eval("(buffer-string)"), "\"two\"");
    h.keys("M-p");
    assert_eq!(h.eval("(buffer-string)"), "\"one\"");
    // M-n walks forward, then empties past the end.
    h.keys("M-n");
    assert_eq!(h.eval("(buffer-string)"), "\"two\"");
    h.keys("M-n");
    assert_eq!(h.eval("(buffer-string)"), "\"\"");
}

#[test]
fn a_reference_to_the_region_is_added_from_a_file_buffer() {
    let mut h = Harness::new(80, 24);
    h.eval(
        r#"(set! %acp-connected? (lambda () #t))
           (set! %acp-session "stub")
           (set! %acp-root (or (project-root) (working-directory)))
           (set! %acp-send-prompt (lambda (text) #f))
           (find-file "/nonexistent/amacs-test/sub/code.rs")
           (insert "line1\nline2\nline3\nline4\n")"#,
    );
    // In the code buffer, select lines 2 to 3, then add the reference.
    h.keys("M-< C-n C-SPC C-n C-e");
    h.keys("C-c c r");
    assert!(h.echo().contains("@code.rs#L2-3"), "{}", h.echo());
    assert_eq!(
        h.eval(r#"(buffer-string "*claude prompt*")"#),
        r#""@code.rs#L2-3 ""#
    );
}

// The conversation

/// A harness where a prompt has been sent to an agent that is not there, so
/// that what it would answer can be fed in with [`feed`].
fn talking() -> Harness {
    let mut h = Harness::new(80, 40);
    h.eval(
        r#"(set! %acp-connected? (lambda () #t))
           (set! %acp-session "stub")
           (set! %acp-root (or (project-root) (working-directory)))
           (set! %acp-request (lambda (method params callback) #f))"#,
    );
    h.keys("C-c c c");
    h.type_text("Change the notes").keys("C-c C-c");
    h
}

/// Hands the editor an update of the session, as a line from the agent.
fn feed(h: &mut Harness, update: &Value) {
    let line = json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {"sessionId": "stub", "update": update},
    });
    h.eval(&format!("(%acp-filter #f {:?})", format!("{line}\n")));
}

fn say(h: &mut Harness, kind: &str, text: &str) {
    feed(
        h,
        &json!({"sessionUpdate": kind, "content": {"type": "text", "text": text}}),
    );
}

/// Waits until the screen shows a text.
fn see(h: &mut Harness, text: &str) {
    h.wait_until(|h| h.screen.text().contains(text));
}

/// The first row of the screen that holds a text.
fn row_of(h: &Harness, text: &str) -> usize {
    (0..h.screen.size().1)
        .find(|&row| h.screen.row(row).contains(text))
        .unwrap_or_else(|| panic!("no row with {text:?} in:\n{}", h.screen.text()))
}

fn rows_with(h: &Harness, text: &str) -> usize {
    (0..h.screen.size().1)
        .filter(|&row| h.screen.row(row).contains(text))
        .count()
}

/// Selects the window of the conversation and puts point on the row that
/// holds a text.
fn go_to(h: &mut Harness, text: &str) {
    if h.eval("(buffer-name)") != "\"*claude*\"" {
        h.keys("C-x o");
    }
    let row = row_of(h, text);
    h.eval(&format!(
        "(goto-line (+ (line-number-at-pos (window-start)) {row}))"
    ));
}

fn a_change() -> Value {
    json!({
        "sessionUpdate": "tool_call",
        "toolCallId": "edit",
        "title": "Edit notes.txt",
        "status": "completed",
        "locations": [{"path": "notes.txt", "line": 2}],
        "content": [{
            "type": "diff",
            "path": "notes.txt",
            "oldText": "one\ntwo\nthree\n",
            "newText": "one\n2\nthree\n",
        }],
    })
}

#[test]
fn the_pieces_of_an_answer_join_under_their_prompt() {
    let mut h = talking();
    say(&mut h, "agent_message_chunk", "Hello ");
    say(&mut h, "agent_message_chunk", "there.\nA second **line**.");
    see(&mut h, "Hello there.");
    assert_eq!(h.screen.row(0), "▾ Change the notes");
    assert_eq!(h.screen.row(1), "Hello there.");
    assert_eq!(h.screen.row(2), "A second **line**.");
    // The prompt has its face, and the answer is coloured as Markdown.
    assert_eq!(h.screen.face_at(0, 2).fg, h.face("agent-prompt").fg);
    assert!(h.screen.face_at(2, 12).bold);
}

#[test]
fn a_thought_is_folded_until_it_is_opened() {
    let mut h = talking();
    say(&mut h, "agent_thought_chunk", "Let me look\nat the ");
    say(&mut h, "agent_thought_chunk", "notes.");
    see(&mut h, "▸ Thinking: Let me look");
    assert!(!h.screen.text().contains("at the notes."));
    go_to(&mut h, "Thinking");
    h.keys("TAB");
    assert!(h.screen.text().contains("▾ Thinking: Let me look"));
    assert!(h.screen.text().contains("  at the notes."));
    h.keys("TAB");
    assert!(!h.screen.text().contains("at the notes."));
}

#[test]
fn a_tool_call_changes_its_state_where_it_stands() {
    let mut h = talking();
    feed(
        &mut h,
        &json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "read",
            "title": "Read notes.txt",
            "status": "pending",
            "locations": [{"path": "notes.txt", "line": 7}],
        }),
    );
    see(&mut h, "Read notes.txt  pending");
    say(&mut h, "agent_message_chunk", "Reading.");
    feed(
        &mut h,
        &json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "read",
            "status": "completed",
            "content": [{"type": "content", "content": {"type": "text", "text": "one\ntwo"}}],
        }),
    );
    see(&mut h, "Read notes.txt  completed");
    assert!(!h.screen.text().contains("pending"));
    assert_eq!(rows_with(&h, "Read notes.txt"), 1);
    // It stands above what came after it, and its title was not lost.
    assert!(row_of(&h, "Read notes.txt") < row_of(&h, "Reading."));
    let row = row_of(&h, "completed");
    let col = h.screen.row(row).chars().position(|c| c == 'c').unwrap();
    assert_eq!(h.screen.face_at(row, col).fg, h.face("agent-done").fg);
    // What it read is under it, folded, with where it read.
    assert!(!h.screen.text().contains("  two"));
    go_to(&mut h, "Read notes.txt");
    h.keys("TAB");
    assert!(h.screen.text().contains("  two"));
    assert!(h.screen.text().contains("  notes.txt:7"));
}

#[test]
fn a_change_is_shown_as_a_diff_and_ret_goes_to_its_line() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("notes.txt"), "one\n2\nthree\n").unwrap();
    let mut h = talking();
    h.eval(&format!(
        "(set! %acp-root {:?})",
        dir.path().display().to_string()
    ));
    feed(&mut h, &a_change());
    see(&mut h, "+2");
    assert!(h.screen.text().contains("▾ Edit notes.txt  completed"));
    assert!(h.screen.text().contains("  @@ -1,3 +1,3 @@"));
    let removed = row_of(&h, "-two");
    assert_eq!(h.screen.face_at(removed, 2).bg, h.face("git-removed").bg);
    assert_eq!(h.screen.face_at(removed + 1, 2).bg, h.face("git-added").bg);
    // Three lines down in the file, to see that the line is gone to.
    go_to(&mut h, " three");
    h.keys("RET");
    assert!(h.eval("(buffer-file-name)").ends_with("notes.txt\""));
    assert_eq!(h.eval("(line-number-at-pos)"), "3");
}

#[test]
fn the_plan_is_a_list_that_a_later_plan_replaces() {
    let mut h = talking();
    feed(
        &mut h,
        &json!({"sessionUpdate": "plan", "entries": [
            {"content": "Read the notes", "status": "completed"},
            {"content": "Change them", "status": "in_progress"},
            {"content": "Tell about it", "status": "pending"},
        ]}),
    );
    see(&mut h, "Plan (1/3)");
    assert!(h.screen.text().contains("  [x] Read the notes"));
    assert!(h.screen.text().contains("  [~] Change them"));
    assert!(h.screen.text().contains("  [ ] Tell about it"));
    feed(
        &mut h,
        &json!({"sessionUpdate": "plan", "entries": [
            {"content": "Read the notes", "status": "completed"},
            {"content": "Change them", "status": "completed"},
            {"content": "Tell about it", "status": "in_progress"},
        ]}),
    );
    see(&mut h, "Plan (2/3)");
    assert_eq!(rows_with(&h, "Plan ("), 1);
    assert!(h.screen.text().contains("  [x] Change them"));
}

#[test]
fn what_was_folded_stays_folded_as_more_comes() {
    let mut h = talking();
    feed(&mut h, &a_change());
    see(&mut h, "+2");
    go_to(&mut h, "Edit notes.txt");
    h.keys("TAB");
    assert!(!h.screen.text().contains("+2"));
    say(&mut h, "agent_message_chunk", "That is done.");
    see(&mut h, "That is done.");
    assert!(h.screen.text().contains("▸ Edit notes.txt"));
    assert!(!h.screen.text().contains("+2"));
    // And point is still on the section it was on.
    assert_eq!(
        h.eval("(line-number-at-pos)"),
        (row_of(&h, "Edit notes.txt") + 1).to_string()
    );
}

#[test]
fn the_end_is_followed_unless_the_reader_is_further_up() {
    let mut h = talking();
    let lines: Vec<String> = (0..60).map(|n| format!("line {n}")).collect();
    say(&mut h, "agent_message_chunk", &lines.join("\n"));
    // Point was at the end, so the end is shown.
    see(&mut h, "line 59");
    // Someone who goes up to read is left there.
    h.keys("C-x o M-<");
    assert_eq!(h.screen.row(0), "▾ Change the notes");
    say(&mut h, "agent_message_chunk", "\nthe last line");
    h.wait_until(|h| {
        h.eval(r#"(buffer-string "*claude*")"#)
            .contains("the last line")
    });
    assert_eq!(h.screen.row(0), "▾ Change the notes");
    assert!(!h.screen.text().contains("the last line"));
}

#[test]
fn m_n_and_m_p_move_by_turn() {
    let mut h = talking();
    say(&mut h, "agent_message_chunk", "First answer.");
    // The turn is over, as the answer to the prompt would say.
    h.eval("(set! %acp-busy #f)");
    h.keys("C-c c c");
    h.type_text("And again").keys("C-c C-c");
    say(&mut h, "agent_message_chunk", "Second answer.");
    see(&mut h, "Second answer.");
    h.keys("C-x o M-<");
    h.keys("M-n");
    assert_eq!(
        h.eval("(line-number-at-pos)"),
        (row_of(&h, "And again") + 1).to_string()
    );
    h.keys("M-p");
    assert_eq!(h.eval("(line-number-at-pos)"), "1");
}

#[test]
fn a_diff_is_made_from_the_text_before_and_after() {
    let mut h = Harness::new(80, 24);
    assert_eq!(h.eval(r#"(%acp-line-diff "a\nb\n" "a\nb\n")"#), "()");
    // A new file is all added.
    assert_eq!(
        h.eval(r#"(map car (%acp-line-diff #f "x\ny\n"))"#),
        r#"("@@ -0,0 +1,2 @@" "+x" "+y")"#
    );
    // Three lines around a change, and each hunk on its own.
    let old = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm\nn\no\np\n";
    let new = "a\nB\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm\nn\nO\np\n";
    assert_eq!(
        h.eval(&format!("(map car (%acp-line-diff {old:?} {new:?}))")),
        r#"("@@ -1,5 +1,5 @@" " a" "-b" "+B" " c" " d" " e" "@@ -12,5 +12,5 @@" " l" " m" " n" "-o" "+O" " p")"#
    );
    // Each line knows which line of the new text it is at.
    assert_eq!(
        h.eval(r#"(map cdr (%acp-line-diff "one\ntwo\nthree\n" "one\n2\nthree\n"))"#),
        "(1 1 2 2 3)"
    );
    // Texts too long to compare are shown as old lines, then new.
    assert_eq!(
        h.eval(
            r#"(set! %acp-diff-bound 0)
               (map car (%acp-line-diff "a\nb\nc\n" "a\nc\nd\n"))"#
        ),
        r#"("@@ -1,3 +1,3 @@" " a" "-b" "-c" "+c" "+d")"#
    );
}

// Leave

/// Hands the editor a request for leave to do the tool call `rm`, as a line
/// from the agent, and keeps what the editor sends in `replies`, newest
/// first.
fn ask_leave(h: &mut Harness) {
    h.eval(
        r"(define replies '())
          (set! %acp-send (lambda (message) (set! replies (cons message replies))))",
    );
    feed(
        h,
        &json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "rm",
            "title": "Remove the build",
            "status": "pending",
        }),
    );
    let line = json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "session/request_permission",
        "params": {
            "sessionId": "stub",
            "toolCall": {"toolCallId": "rm"},
            "options": [
                {"optionId": "allow-once", "name": "Yes", "kind": "allow_once"},
                {"optionId": "allow-always", "name": "Yes, always", "kind": "allow_always"},
                {"optionId": "allow-here", "name": "Yes, here", "kind": "allow_always"},
                {"optionId": "reject", "name": "No", "kind": "reject_once"},
            ],
        },
    });
    h.eval(&format!("(%acp-filter #f {:?})", format!("{line}\n")));
    see(h, "asks leave");
}

/// What the editor sent last, which is its answer to the request for
/// leave once it has given one.
fn answer(h: &mut Harness) -> Value {
    let line: String = serde_json::from_str(&h.eval("(json-encode (car replies))")).unwrap();
    serde_json::from_str(&line).unwrap()
}

fn chose(option: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 7,
        "result": {"outcome": {"outcome": "selected", "optionId": option}},
    })
}

#[test]
fn leave_is_asked_with_the_tool_call_and_not_in_the_minibuffer() {
    let mut h = talking();
    ask_leave(&mut h);
    // The tool call says so, with a key for each answer, and is open.
    let row = row_of(&h, "Remove the build");
    assert_eq!(h.screen.row(row), "▾ Remove the build  asks leave");
    assert_eq!(h.screen.row(row + 1), "  y  Yes");
    assert_eq!(h.screen.row(row + 2), "  a  Yes, always");
    assert_eq!(h.screen.row(row + 3), "  1  Yes, here");
    assert_eq!(h.screen.row(row + 4), "  n  No");
    assert_eq!(h.screen.face_at(row, 20).fg, h.face("agent-asks").fg);
    // The echo area and the mode line say so too, and nothing is answered:
    // what is typed goes where it was going.
    assert!(
        h.echo().contains("Claude asks leave: Remove the build"),
        "{}",
        h.echo()
    );
    assert!(h.screen.text().contains("asks leave, working]"));
    h.type_text("still typing");
    assert_eq!(
        h.eval(r#"(buffer-string "*claude prompt*")"#),
        "\"still typing\""
    );
    assert_eq!(h.eval("replies"), "()");
}

#[test]
fn c_c_c_y_allows_once_from_anywhere() {
    let mut h = talking();
    ask_leave(&mut h);
    h.keys("C-c c y");
    assert_eq!(answer(&mut h), chose("allow-once"));
    assert!(
        h.echo().contains("Allowed: Remove the build"),
        "{}",
        h.echo()
    );
    // The question is gone from the tool call and from the mode line.
    h.wait_until(|h| !h.screen.text().contains("asks leave"));
    assert!(h.screen.text().contains("Remove the build  pending"));
    // And there is nothing more to answer.
    h.keys("C-c c y");
    assert!(h.echo().contains("not asking leave"), "{}", h.echo());
}

#[test]
fn c_c_c_n_refuses() {
    let mut h = talking();
    ask_leave(&mut h);
    h.keys("C-c c n");
    assert_eq!(answer(&mut h), chose("reject"));
}

#[test]
fn every_answer_is_in_a_menu_of_keys() {
    let mut h = talking();
    ask_leave(&mut h);
    h.keys("C-c c o");
    let screen = h.screen.text();
    assert!(
        screen.contains("Claude asks leave: Remove the build"),
        "{screen}"
    );
    assert!(screen.contains(" a   Yes, always"), "{screen}");
    h.keys("1");
    assert_eq!(answer(&mut h), chose("allow-here"));
}

#[test]
fn ret_on_an_answer_gives_it() {
    let mut h = talking();
    ask_leave(&mut h);
    go_to(&mut h, "  a  Yes, always");
    h.keys("RET");
    assert_eq!(answer(&mut h), chose("allow-always"));
}

#[test]
fn a_cancelled_turn_leaves_the_question_unanswered() {
    let mut h = talking();
    ask_leave(&mut h);
    h.keys("C-c c k");
    // The cancel went first, then the word that nothing was chosen.
    assert_eq!(
        answer(&mut h),
        json!({"jsonrpc": "2.0", "id": 7, "result": {"outcome": {"outcome": "cancelled"}}})
    );
    assert!(
        h.eval("(json-encode (cadr replies))")
            .contains("session/cancel")
    );
    h.wait_until(|h| !h.screen.text().contains("asks leave"));
}

#[test]
fn each_answer_gets_a_key_by_its_kind() {
    let mut h = Harness::new(80, 24);
    assert_eq!(
        h.eval(
            r#"(map car (%acp-option-keys
                         (map (lambda (kind) (json-object 'optionId kind 'name kind 'kind kind))
                              '("reject_always" "allow_once" "allow_once" "odd" "reject_once"
                                "allow_always"))))"#
        ),
        r#"("r" "y" "1" "2" "n" "a")"#
    );
}

#[test]
fn a_part_of_a_file_is_read_by_line_and_limit() {
    let mut h = Harness::new(80, 24);
    assert_eq!(
        h.eval(r#"(%acp-slice "a\nb\nc\nd\n" #f #f)"#),
        r#""a\nb\nc\nd\n""#
    );
    assert_eq!(h.eval(r#"(%acp-slice "a\nb\nc\nd\n" 2 2)"#), r#""b\nc""#);
    assert_eq!(h.eval(r#"(%acp-slice "a\nb\nc\nd" 3 #f)"#), r#""c\nd""#);
    assert_eq!(h.eval(r#"(%acp-slice "a\nb" 9 1)"#), r#""""#);
}

// With an agent that speaks the protocol

/// A harness in which `fake-acp` has been started in a directory and is
/// ready for a prompt, with point in the compose buffer.
fn with_fake_agent(dir: &std::path::Path) -> Harness {
    let mut h = Harness::new(80, 40);
    h.session.editor_mut().set_default_directory(dir);
    let agent = amacs_testkit::example_path("fake-acp");
    h.eval(&format!(
        "(set-option! 'acp-command (list {:?}))",
        agent.display().to_string()
    ));
    h.keys("C-c c c");
    see(&mut h, "Ready.");
    h.wait_until(|h| h.eval("(length %acp-commands)") == "2");
    h
}

#[test]
fn leave_asked_by_an_agent_is_given_and_the_turn_goes_on() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = with_fake_agent(dir.path());
    h.type_text("ask me").keys("C-c C-c");
    see(&mut h, "Remove the build  asks leave");
    h.keys("C-c c o a");
    see(&mut h, "You chose: allow-always");
    see(&mut h, "— end_turn —");
    assert!(h.screen.text().contains("Remove the build  completed"));
    h.eval("(claude-quit)");
}

#[test]
fn a_program_is_run_for_an_agent_that_asks() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = with_fake_agent(dir.path());
    h.type_text("run it").keys("C-c C-c");
    see(&mut h, "— end_turn —");
    let screen = h.screen.text();
    // The agent got the exit status and the output, of both streams, and
    // could not ask again once it had let the terminal go.
    assert!(
        screen.contains("It ended with 3 and wrote 2 lines. Truncated: false. Released: true."),
        "{screen}"
    );
    // The tool call shows what the program wrote, where the agent works
    // and with the variable it set, and how it ended.
    let name = dir.path().file_name().unwrap().to_str().unwrap();
    assert!(screen.contains("▾ Run the greeting  failed"), "{screen}");
    assert!(screen.contains(&format!("  hello from {name}")), "{screen}");
    assert!(screen.contains("  oops"), "{screen}");
    assert!(screen.contains("  [exit 3]"), "{screen}");
    h.eval("(claude-quit)");
}

#[test]
fn only_the_end_of_much_output_is_kept() {
    let mut h = Harness::new(80, 24);
    assert_eq!(
        h.eval(
            r#"(define t (vector #f "" #f #f '() 10 0 #f #f))
               (%acp-terminal-output! "t" t "abcdefgh")
               (%acp-terminal-output! "t" t "ijkl")
               (list (vector-ref t 1) (vector-ref t 2) (vector-ref t 6))"#
        ),
        r#"("cdefghijkl" #t 10)"#
    );
    // The limit is in bytes: chars of two bytes each fill it twice as fast.
    assert_eq!(
        h.eval(
            r#"(define t (vector #f "" #f #f '() 10 0 #f #f))
               (%acp-terminal-output! "t" t "ååååååå")
               (list (vector-ref t 1) (vector-ref t 6))"#
        ),
        r#"("ååå" 6)"#
    );
}

#[test]
fn the_mode_and_the_model_are_chosen_among_what_the_agent_offers() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = with_fake_agent(dir.path());
    assert!(h.screen.text().contains("[fake-model, default]"));
    h.keys("C-c c m");
    // Each option is offered with its value now.
    let screen = h.screen.text();
    assert!(
        screen.contains("Mode") && screen.contains("Default"),
        "{screen}"
    );
    h.type_text("Mode").keys("RET");
    assert!(h.screen.text().contains("Plans, and changes nothing"));
    h.type_text("Plan").keys("RET");
    see(&mut h, "[fake-model, plan]");
    assert!(h.echo().contains("Mode: Plan"), "{}", h.echo());
    // Values that come in groups are offered alike.
    h.keys("C-c c m");
    h.type_text("Model").keys("RET");
    h.type_text("Faker").keys("RET");
    see(&mut h, "[faker-model, plan]");
    h.eval("(claude-quit)");
}

#[test]
fn an_agent_with_modes_and_no_options_has_its_mode_set() {
    let mut h = talking();
    h.eval(
        r#"(define asked '())
           (set! %acp-request
             (lambda (method params callback)
               (set! asked (list method (json-encode params)))
               (callback '() #f)))
           (set! %acp-modes
             (json-decode "{\"currentModeId\":\"code\",\"availableModes\":[{\"id\":\"code\",\"name\":\"Code\"},{\"id\":\"ask\",\"name\":\"Ask\"}]}"))"#,
    );
    // The mode is all there is to set, so its values are offered at once.
    h.keys("C-c c m");
    h.type_text("Ask").keys("RET");
    assert_eq!(
        h.eval("asked"),
        r#"("session/set_mode" "{\"sessionId\":\"stub\",\"modeId\":\"ask\"}")"#,
        "the editor writes the fields in the order it was given them"
    );
    assert_eq!(h.eval("%acp-mode"), "\"ask\"");
}

#[test]
fn a_slash_command_is_chosen_and_sent() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = with_fake_agent(dir.path());
    h.keys("C-c c /");
    assert!(h.screen.text().contains("Make the conversation shorter"));
    h.type_text("/compact").keys("RET");
    see(&mut h, "You said: /compact");
    see(&mut h, "— end_turn —");
    // One that takes something asks for it, with the agent's hint.
    h.keys("C-c c /");
    h.type_text("/review").keys("RET");
    assert!(
        h.prompt().contains("/review (which change):"),
        "{}",
        h.prompt()
    );
    h.type_text("the last").keys("RET");
    see(&mut h, "You said: /review the last");
    // It is among the prompts that M-p brings back.
    assert_eq!(h.eval("(last %acp-history)"), "\"/review the last\"");
    h.eval("(claude-quit)");
}

#[test]
fn a_whole_turn_with_an_agent_that_speaks_the_protocol() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::new(80, 40);
    h.session.editor_mut().set_default_directory(dir.path());
    let agent = amacs_testkit::example_path("fake-acp");
    h.eval(&format!(
        "(set-option! 'acp-command (list {:?}))",
        agent.display().to_string()
    ));
    h.keys("C-c c c");
    see(&mut h, "Ready.");
    h.type_text("hello").keys("C-c C-c");
    see(&mut h, "— end_turn —");
    let screen = h.screen.text();
    assert!(screen.contains("▾ hello"), "{screen}");
    assert!(
        screen.contains("▸ Thinking: Let me look at the notes."),
        "{screen}"
    );
    assert!(screen.contains("You said: hello"), "{screen}");
    assert!(screen.contains("▾ Edit notes.txt  completed"), "{screen}");
    assert!(screen.contains("  +2"), "{screen}");
    assert!(screen.contains("Plan (1/2)"), "{screen}");
    assert!(screen.contains("Done."), "{screen}");
    // The model, the mode and how full the context is are on the mode
    // line, and the work is over.
    assert!(
        screen.contains("(Agent) [fake-model, default, 28k/1M]"),
        "{screen}"
    );
    // The agent was started where the command was given.
    assert_eq!(
        h.eval("%acp-root"),
        format!("{:?}", dir.path().display().to_string())
    );
    h.eval("(claude-quit)");
}

// Several sessions, and those that were

/// A harness with `fake-acp` as the agent, in a directory, and nothing
/// started yet.
fn fake_agent(dir: &std::path::Path) -> Harness {
    let mut h = Harness::new(80, 40);
    h.session.editor_mut().set_default_directory(dir);
    let agent = amacs_testkit::example_path("fake-acp");
    h.eval(&format!(
        "(set-option! 'acp-command (list {:?}))",
        agent.display().to_string()
    ));
    h
}

#[test]
fn another_session_has_a_conversation_of_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = with_fake_agent(dir.path());
    h.type_text("first").keys("C-c C-c");
    see(&mut h, "— end_turn —");
    // C-c c c shows the session that runs; with a prefix argument it
    // starts another, with buffers of its own.
    h.keys("C-c c c");
    assert_eq!(h.eval("(length %acp-sessions)"), "1");
    h.keys("C-u C-c c c");
    see(&mut h, "Ready.");
    assert_eq!(h.eval("(length %acp-sessions)"), "2");
    assert_eq!(h.eval("(buffer-name)"), "\"*claude prompt*<2>\"");
    h.type_text("second").keys("C-c C-c");
    see(&mut h, "You said: second");
    see(&mut h, "— end_turn —");
    // Each conversation holds what was said in it.
    let first = h.eval(r#"(buffer-string "*claude*")"#);
    let second = h.eval(r#"(buffer-string "*claude*<2>")"#);
    assert!(
        first.contains("You said: first") && !first.contains("second"),
        "{first}"
    );
    assert!(
        second.contains("You said: second") && !second.contains("first"),
        "{second}"
    );
    // A command given in a conversation is about its session.
    h.eval(r#"(switch-to-buffer "*claude*")"#);
    h.keys("m");
    h.type_text("Mode").keys("RET");
    h.type_text("Plan").keys("RET");
    h.wait_until(|h| {
        h.eval(r#"(with-current-buffer "*claude*" (mode-line-text))"#)
            .contains("plan")
    });
    assert!(
        h.eval(r#"(with-current-buffer "*claude*<2>" (mode-line-text))"#)
            .contains("default")
    );
    // C-c c b goes to another, chosen by name.
    h.keys("C-c c b");
    assert!(h.screen.text().contains("ready"), "{}", h.screen.text());
    h.type_text("*claude*<2>").keys("RET");
    assert_eq!(h.eval("(buffer-name)"), "\"*claude prompt*<2>\"");
    // Stopping one leaves the other.
    h.eval("(claude-quit)");
    assert_eq!(
        h.eval(r#"(map (lambda (session) (%acp-in session %acp-connected?)) %acp-sessions)"#),
        "(#t #f)"
    );
    h.eval(r#"(switch-to-buffer "*claude*")"#);
    h.eval("(claude-quit)");
}

#[test]
fn a_prompt_written_during_a_turn_waits_for_its_end() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = with_fake_agent(dir.path());
    h.type_text("slow one").keys("C-c C-c");
    h.type_text("the next").keys("C-c C-c");
    // It is not refused: it is shown as waiting, and the mode line counts it.
    assert!(h.echo().contains("waits its turn"), "{}", h.echo());
    see(&mut h, "Waits its turn: the next");
    assert!(h.screen.text().contains("1 waiting"), "{}", h.screen.text());
    // When the turn is over it is sent, and has a turn of its own.
    see(&mut h, "At last: slow one");
    see(&mut h, "You said: the next");
    assert_eq!(rows_with(&h, "Waits its turn"), 0);
    let text = h.eval(r#"(buffer-string "*claude*")"#);
    assert!(
        text.find("At last: slow one").unwrap() < text.find("▾ the next").unwrap(),
        "{text}"
    );
    h.eval("(claude-quit)");
}

#[test]
fn cancelling_drops_what_waited() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = with_fake_agent(dir.path());
    h.type_text("slow one").keys("C-c C-c");
    h.type_text("the next").keys("C-c C-c");
    h.keys("C-c c k");
    assert!(h.echo().contains("1 prompt that waited"), "{}", h.echo());
    see(&mut h, "At last: slow one");
    h.wait_until(|h| h.eval("%acp-busy") == "#f");
    assert_eq!(h.eval("%acp-queue"), "()");
    assert!(!h.screen.text().contains("You said: the next"));
    // It is still among the prompts that M-p brings back.
    h.keys("M-p");
    assert_eq!(h.eval("(buffer-string)"), "\"the next\"");
    h.eval("(claude-quit)");
}

#[test]
fn a_prompt_goes_into_the_turn_that_is_going_on() {
    let mut h = talking();
    h.eval(
        r#"(define steered '())
           (set! %acp-busy #t)
           (set! %acp-agent (json-decode "{\"_meta\":{\"steering\":{\"supported\":true}}}"))
           (set! %acp-request
             (lambda (method params callback)
               (set! steered (cons (list method (json-ref (vector-ref (json-ref params 'prompt) 0) 'text))
                                   steered))))"#,
    );
    say(
        &mut h,
        "agent_message_chunk",
        "I will start with the first.",
    );
    let turns = h.eval("(length %acp-turns)");
    h.type_text("No, the second").keys("C-c C-s");
    assert_eq!(
        h.eval("steered"),
        r#"(("_session/steering" "No, the second"))"#
    );
    // It is shown in the turn, where it was said, and no new turn starts.
    see(&mut h, "› No, the second");
    assert_eq!(h.eval("(length %acp-turns)"), turns);
    assert_eq!(h.eval("(buffer-string)"), "\"\"");
    // An agent that takes nothing into a turn is not sent it.
    h.eval("(set! %acp-agent #f)");
    h.type_text("again").keys("C-c C-s");
    assert!(
        h.echo().contains("takes nothing into a turn"),
        "{}",
        h.echo()
    );
    assert_eq!(h.eval("(length steered)"), "1");
}

#[test]
fn an_image_named_in_a_prompt_is_sent_with_it() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("shot.png"), b"\x89PNG not really").unwrap();
    let mut h = with_fake_agent(dir.path());
    // The command adds the reference; one that is typed does as well.
    h.keys("C-c C-a");
    h.type_text("shot.png").keys("RET");
    assert_eq!(h.eval("(buffer-string)"), "\"@shot.png \"");
    h.keys("C-c C-k");
    h.type_text("look at @shot.png and @missing.png")
        .keys("C-c C-c");
    see(&mut h, "Saw 1 images.");
    assert!(
        h.screen.text().contains("▾ look at @shot.png"),
        "{}",
        h.screen.text()
    );
    // A file that is no image is not taken.
    std::fs::write(dir.path().join("notes.txt"), "notes").unwrap();
    h.keys("C-c C-a");
    h.type_text("notes.txt").keys("RET");
    assert!(h.echo().contains("Not an image"), "{}", h.echo());
    h.eval("(claude-quit)");
}

#[test]
fn a_conversation_that_was_is_taken_up_again() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = fake_agent(dir.path());
    h.keys("C-c c R");
    // The conversations of the directory, with when they were last written.
    h.wait_until(|h| h.prompt().contains("Take up"));
    let screen = h.screen.text();
    assert!(
        screen.contains("About the notes") && screen.contains("2026-11-23 14:02"),
        "{screen}"
    );
    assert!(screen.contains("About the tests"), "{screen}");
    h.type_text("About the notes").keys("RET");
    see(&mut h, "Ready, with the conversation as it was.");
    // It is shown from its start: each prompt heads a turn, its pieces joined.
    let text = h.eval(r#"(buffer-string "*claude*")"#);
    assert!(text.contains("▾ What is in the notes?"), "{text}");
    assert!(text.contains("This was about-notes."), "{text}");
    assert!(text.contains("▾ And then?"), "{text}");
    assert!(!text.contains("Write a prompt below"), "{text}");
    assert_eq!(h.eval("%acp-session"), "\"about-notes\"");
    assert_eq!(h.eval("%acp-title"), "\"About the notes\"");
    // And it goes on.
    h.type_text("more").keys("C-c C-c");
    see(&mut h, "You said: more");
    h.eval("(claude-quit)");
}

#[test]
fn an_agent_that_ended_takes_its_conversation_up_where_it_was() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = with_fake_agent(dir.path());
    h.type_text("hello").keys("C-c C-c");
    see(&mut h, "— end_turn —");
    h.eval("(claude-quit)");
    see(&mut h, "Claude stopped");
    assert_eq!(h.eval("%acp-resumable"), "\"fake\"");
    // Started again in its conversation, it goes on with the session.
    h.keys("C-c c c");
    see(&mut h, "Going on with fake.");
    assert_eq!(h.eval("(length %acp-sessions)"), "1");
    assert!(
        h.eval(r#"(buffer-string "*claude*")"#)
            .contains("You said: hello")
    );
    h.eval("(claude-quit)");
    // A session that cannot be gone on with gives way to a new one.
    h.eval(r#"(set! %acp-resumable "gone")"#);
    h.keys("C-c c c");
    see(&mut h, "[session/resume failed: no such session]");
    h.wait_until(|h| h.eval("%acp-session") == "\"fake\"");
    assert_eq!(h.eval("%acp-resumable"), "#f");
    h.eval("(claude-quit)");
}

#[test]
fn a_session_is_started_from_another_as_it_is() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = with_fake_agent(dir.path());
    h.type_text("hello").keys("C-c C-c");
    see(&mut h, "— end_turn —");
    h.keys("C-c c f");
    see(&mut h, "Ready, in a session of its own");
    assert_eq!(h.eval("%acp-session"), "\"fork-of-fake\"");
    assert_eq!(h.eval("%acp-buffer"), "\"*claude*<2>\"");
    // It starts with what was said, and what is said next is its own.
    h.type_text("onwards").keys("C-c C-c");
    see(&mut h, "You said: onwards");
    let first = h.eval(r#"(buffer-string "*claude*")"#);
    let second = h.eval(r#"(buffer-string "*claude*<2>")"#);
    assert!(
        second.contains("You said: hello") && second.contains("You said: onwards"),
        "{second}"
    );
    assert!(
        first.contains("You said: hello") && !first.contains("onwards"),
        "{first}"
    );
    h.eval("(claude-quit)");
    h.eval(r#"(switch-to-buffer "*claude*") (claude-quit)"#);
}

#[test]
fn servers_and_directories_are_given_with_a_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = fake_agent(dir.path());
    h.eval(
        r#"(set-option! 'acp-mcp-servers
             '(("amacs" "amacs-mcp" "--socket" "/run/amacs.sock")
               ("docs" "https://example.org/mcp")))
           (set-option! 'acp-additional-directories '("/tmp"))"#,
    );
    assert_eq!(
        h.eval("(json-encode (%acp-mcp-servers))"),
        format!(
            "{:?}",
            concat!(
                r#"[{"name":"amacs","command":"amacs-mcp","args":["--socket","/run/amacs.sock"],"env":[]},"#,
                r#"{"type":"http","name":"docs","url":"https://example.org/mcp","headers":[]}]"#
            )
        )
    );
    h.keys("C-c c c");
    see(&mut h, "Given 2 servers and 1 directories.");
    h.eval("(claude-quit)");
}

#[test]
fn choosing_none_of_those_that_were_leaves_a_new_conversation() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = fake_agent(dir.path());
    h.keys("C-c c R");
    h.wait_until(|h| h.prompt().contains("Take up"));
    h.keys("C-g");
    see(&mut h, "Ready. Write a prompt below");
    h.wait_until(|h| h.eval("%acp-session") == "\"fake\"");
    h.type_text("hello").keys("C-c C-c");
    see(&mut h, "You said: hello");
    h.eval("(claude-quit)");
}
