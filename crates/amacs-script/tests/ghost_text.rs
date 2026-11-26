//! Text fields of a web browser in buffers, which is written in Scheme:
//! `runtime/lib/ghost-text.scm`. The tests play the GhostText extension.

#[allow(dead_code)]
#[path = "common/browser.rs"]
mod browser;
mod common;

use browser::Socket;
use common::Harness;

/// An editor that listens, on a port that is free, and the port.
fn setup() -> (Harness, u16) {
    let mut h = Harness::new(70, 10);
    h.eval("(set-option! 'ghost-text-port 0)");
    h.eval("(ghost-text-start)");
    let port = h
        .eval("(websocket-server-port %ghost-server)")
        .parse()
        .unwrap();
    (h, port)
}

/// What the extension sends for a field.
fn field(host: &str, text: &str, start: usize, end: usize) -> String {
    serde_json::json!({
        "title": "A page",
        "url": host,
        "syntax": "",
        "text": text,
        "selections": [{ "start": start, "end": end }],
    })
    .to_string()
}

/// Turns the extension on for a field and waits for its buffer.
fn open(h: &mut Harness, port: u16, host: &str, text: &str, at: usize) -> Socket {
    let mut socket = browser::connect(port, Some(browser::EXTENSION)).unwrap();
    socket.send(&field(host, text, at, at));
    let name = format!("\"*ghost: {host}*\"");
    h.wait_until(|h| h.eval("(buffer-name)") == name);
    socket
}

fn message(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap()
}

#[test]
fn the_extension_is_told_where_the_websocket_is() {
    let (mut h, port) = setup();
    let answer = browser::get(&mut h, port, Some(browser::EXTENSION));
    let body = answer.split("\r\n\r\n").nth(1).unwrap();
    assert_eq!(
        message(body),
        serde_json::json!({ "ProtocolVersion": 1, "WebSocketPort": port })
    );
    assert_eq!(h.eval("(ghost-text-running?)"), "#t");
}

#[test]
fn a_web_page_is_refused() {
    let (mut h, port) = setup();
    assert_eq!(
        browser::connect(port, Some("https://example.com")).err(),
        Some(403)
    );
    assert!(browser::get(&mut h, port, Some("https://example.com")).starts_with("HTTP/1.1 403 "));
    assert_eq!(h.eval("(ghost-text-fields)"), "()");
}

#[test]
fn a_field_comes_up_in_a_buffer_with_its_text_and_its_cursor() {
    let (mut h, port) = setup();
    // The cursor is after the emoji, which is two code units of UTF-16.
    let _socket = open(&mut h, port, "example.com", "a\u{1F600}b\nsecond", 3);
    assert_eq!(h.text().lines().next(), Some("a\u{1F600}b"));
    assert_eq!(h.eval("(point)"), "2");
    assert_eq!(h.eval("(major-mode)"), "text-mode");
    assert_eq!(h.eval("(minor-mode-enabled? 'ghost-text-mode)"), "#t");
    assert_eq!(h.eval("(ghost-text-fields)"), "(\"*ghost: example.com*\")");
    assert!(h.echo().contains("Type C-c C-c to finish"), "{}", h.echo());
}

#[test]
fn the_site_chooses_the_mode_and_a_selection_becomes_the_region() {
    let (mut h, port) = setup();
    let mut socket = browser::connect(port, Some(browser::EXTENSION)).unwrap();
    socket.send(&field("gist.github.com", "# Title", 2, 7));
    h.wait_until(|h| h.eval("(buffer-name)") == "\"*ghost: gist.github.com*\"");
    assert_eq!(h.eval("(major-mode)"), "markdown-mode");
    assert_eq!(h.eval("(list (mark) (point) (region-active?))"), "(2 7 #t)");
}

#[test]
fn what_is_typed_goes_to_the_page() {
    let (mut h, port) = setup();
    let mut socket = open(&mut h, port, "example.com", "one", 3);
    h.type_text(" \u{1F600}two");
    let sent = message(&socket.next(&mut h).unwrap());
    assert_eq!(sent["text"], "one \u{1F600}two");
    // In the code units of UTF-16: the emoji is two.
    assert_eq!(
        sent["selections"],
        serde_json::json!([{ "start": 9, "end": 9 }])
    );
    // Moving about changes nothing, and nothing is sent.
    h.keys("C-a C-f");
    assert!(socket.quiet(&mut h));
    h.keys("C-k");
    let sent = message(&socket.next(&mut h).unwrap());
    assert_eq!(sent["text"], "o");
    assert_eq!(
        sent["selections"],
        serde_json::json!([{ "start": 1, "end": 1 }])
    );
}

#[test]
fn what_is_typed_in_the_page_comes_to_the_buffer_around_point() {
    let (mut h, port) = setup();
    let mut socket = open(&mut h, port, "example.com", "one two three", 7);
    assert_eq!(h.eval("(point)"), "7");
    socket.send(&field("example.com", "zero one two three", 0, 0));
    h.wait_until(|h| h.text().starts_with("zero one two three"));
    // Point is still after "two".
    assert_eq!(h.eval("(point)"), "12");
    // The change is the page's own: it is not sent back.
    assert!(socket.quiet(&mut h));
    h.type_text("!");
    assert_eq!(
        message(&socket.next(&mut h).unwrap())["text"],
        "zero one two! three"
    );
    // What the page did is undone here like any other change.
    h.keys("C-/ C-/");
    assert_eq!(
        message(&socket.next(&mut h).unwrap())["text"],
        "one two three"
    );
}

#[test]
fn a_text_that_the_browser_says_back_does_not_undo_what_was_typed_since() {
    let (mut h, port) = setup();
    let mut socket = open(&mut h, port, "example.com", "", 0);
    h.type_text("a");
    assert_eq!(message(&socket.next(&mut h).unwrap())["text"], "a");
    h.type_text("b");
    assert_eq!(message(&socket.next(&mut h).unwrap())["text"], "ab");
    h.type_text("c");
    // The page is behind: it has only now taken the first text in.
    socket.send(&field("example.com", "a", 1, 1));
    assert_eq!(message(&socket.next(&mut h).unwrap())["text"], "abc");
    assert!(socket.quiet(&mut h));
    assert!(h.text().starts_with("abc"));
}

#[test]
fn finishing_sends_the_last_of_the_text_and_closes_the_field() {
    let (mut h, port) = setup();
    h.eval("(switch-to-buffer \"before\")");
    let mut socket = open(&mut h, port, "example.com", "draft", 5);
    h.type_text("!");
    h.keys("C-c C-c");
    assert_eq!(message(&socket.next(&mut h).unwrap())["text"], "draft!");
    assert_eq!(socket.next(&mut h), None);
    assert_eq!(h.eval("(buffer-name)"), "\"before\"");
    assert_eq!(h.eval("(ghost-text-fields)"), "()");
    assert_eq!(
        h.eval("(member \"*ghost: example.com*\" (buffer-list))"),
        "#f"
    );
    assert!(
        h.echo().contains("The text is in the browser"),
        "{}",
        h.echo()
    );
}

#[test]
fn cancelling_gives_the_field_the_text_it_had() {
    let (mut h, port) = setup();
    let mut socket = open(&mut h, port, "example.com", "as it was", 0);
    h.type_text("changed ");
    assert_eq!(
        message(&socket.next(&mut h).unwrap())["text"],
        "changed as it was"
    );
    h.keys("C-c C-k");
    assert_eq!(message(&socket.next(&mut h).unwrap())["text"], "as it was");
    assert_eq!(socket.next(&mut h), None);
    assert_eq!(h.eval("(ghost-text-fields)"), "()");
}

#[test]
fn a_field_that_the_browser_closes_takes_its_buffer_along() {
    let (mut h, port) = setup();
    h.eval("(switch-to-buffer \"before\")");
    let socket = open(&mut h, port, "example.com", "text", 0);
    socket.close();
    h.wait_until(|h| h.eval("(buffer-name)") == "\"before\"");
    assert_eq!(h.eval("(ghost-text-fields)"), "()");
}

#[test]
fn killing_the_buffer_hands_the_field_back() {
    let (mut h, port) = setup();
    let mut socket = open(&mut h, port, "example.com", "text", 0);
    h.eval("(kill-buffer)");
    assert_eq!(socket.next(&mut h), None);
    assert_eq!(h.eval("(ghost-text-fields)"), "()");
}

#[test]
fn two_fields_of_one_site_have_a_buffer_each() {
    let (mut h, port) = setup();
    let mut first = open(&mut h, port, "example.com", "first", 5);
    let mut second = browser::connect(port, Some(browser::EXTENSION)).unwrap();
    second.send(&field("example.com", "second", 6, 6));
    h.wait_until(|h| h.eval("(buffer-name)") == "\"*ghost: example.com*<2>\"");
    h.type_text("!");
    assert_eq!(message(&second.next(&mut h).unwrap())["text"], "second!");
    assert!(first.quiet(&mut h));
}

#[test]
fn stopping_hands_every_field_back_and_lets_go_of_the_port() {
    let (mut h, port) = setup();
    let mut socket = open(&mut h, port, "example.com", "text", 4);
    h.type_text("!");
    h.eval("(ghost-text-stop)");
    assert_eq!(message(&socket.next(&mut h).unwrap())["text"], "text!");
    assert_eq!(socket.next(&mut h), None);
    assert_eq!(h.eval("(ghost-text-running?)"), "#f");
    assert!(!h.eval("(buffer-list)").contains("ghost"));
}

#[test]
fn a_window_opened_for_a_field_shows_it() {
    let (mut h, port) = setup();
    h.eval("(set-option! 'ghost-text-new-window #t)");
    // No program opens a window here: the test is the window.
    h.eval("(define (start-process . ignored) #t)");
    h.eval("(switch-to-buffer \"before\")");
    let mut socket = browser::connect(port, Some(browser::EXTENSION)).unwrap();
    socket.send(&field("example.com", "text", 0, 0));
    h.wait_until(|h| h.eval("(ghost-text-fields)") != "()");
    // The frame that was last used is left alone.
    assert_eq!(h.eval("(buffer-name)"), "\"before\"");
    h.eval("(run-hooks 'after-make-frame-hook)");
    assert_eq!(h.eval("(buffer-name)"), "\"*ghost: example.com*\"");
}
