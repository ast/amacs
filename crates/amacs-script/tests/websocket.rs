//! WebSocket servers that Scheme code runs: `make-websocket-server`, with
//! real connections.

#[allow(dead_code)]
#[path = "common/browser.rs"]
mod browser;
mod common;

use common::Harness;

fn setup(properties: &str) -> (Harness, u16) {
    let mut h = Harness::new(70, 10);
    h.eval(&format!(
        r#"(define opened '())
           (define closed '())
           (define server
             (make-websocket-server 0
               'open (lambda (connection origin)
                       (set! opened (cons (list connection origin) opened)))
               'message (lambda (connection text)
                          (if (equal? text "bye")
                              (websocket-close connection)
                              (websocket-send connection (string-append "got " text))))
               'close (lambda (connection) (set! closed (cons connection closed)))
               {properties}))"#
    ));
    let port = h.eval("(websocket-server-port server)").parse().unwrap();
    (h, port)
}

#[test]
fn messages_go_both_ways_and_either_side_can_close() {
    let (mut h, port) = setup("");
    let mut first = browser::connect(port, None).unwrap();
    h.wait_until(|h| h.eval("opened") == "((1 #f))");
    assert_eq!(h.eval("(websocket-connections server)"), "(1)");
    first.send("one");
    assert_eq!(first.next(&mut h).as_deref(), Some("got one"));
    // Text outside the basic plane, and more than a frame's small form.
    let long = "\u{1F600}é".repeat(40_000);
    first.send(&long);
    assert_eq!(first.next(&mut h), Some(format!("got {long}")));

    let mut second = browser::connect(port, None).unwrap();
    h.wait_until(|h| h.eval("(websocket-connections server)") == "(1 2)");
    second.send("bye");
    assert_eq!(second.next(&mut h), None);
    h.wait_until(|h| h.eval("closed") == "(2)");
    assert_eq!(h.eval("(websocket-live? 2)"), "#f");
    assert!(h.session.eval("(websocket-send 2 \"x\")").is_err());

    first.close();
    h.wait_until(|h| h.eval("closed") == "(1 2)");
    assert_eq!(h.eval("(websocket-connections server)"), "()");
}

#[test]
fn only_the_origins_named_are_served() {
    let (mut h, port) = setup(r#"'origins '("moz-extension://") 'reply "{\"a\":1}""#);
    assert_eq!(
        browser::connect(port, Some("https://example.com")).err(),
        Some(403)
    );
    assert!(browser::get(&mut h, port, Some("https://example.com")).starts_with("HTTP/1.1 403 "));
    assert_eq!(h.eval("opened"), "()");

    let answer = browser::get(&mut h, port, Some(browser::EXTENSION));
    assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
    assert!(answer.contains("Content-Type: application/json\r\n"));
    assert!(answer.contains(&format!(
        "Access-Control-Allow-Origin: {}\r\n",
        browser::EXTENSION
    )));
    assert!(answer.ends_with("\r\n\r\n{\"a\":1}"));
    // A program that is not a browser gives no origin.
    assert!(browser::get(&mut h, port, None).ends_with("{\"a\":1}"));

    let _socket = browser::connect(port, Some(browser::EXTENSION)).unwrap();
    h.wait_until(|h| h.eval("opened") == format!("((1 \"{}\"))", browser::EXTENSION));
}

#[test]
fn a_plain_request_is_not_answered_unless_the_server_has_an_answer() {
    let (mut h, port) = setup("");
    assert!(browser::get(&mut h, port, None).starts_with("HTTP/1.1 404 "));
    h.eval(r#"(set-websocket-server-reply! server "[]")"#);
    assert!(browser::get(&mut h, port, None).ends_with("\r\n\r\n[]"));
}

#[test]
fn a_deleted_server_lets_go_of_its_port_and_its_connections() {
    let (mut h, port) = setup("");
    let mut socket = browser::connect(port, None).unwrap();
    h.wait_until(|h| h.eval("(websocket-connections server)") == "(1)");
    h.eval("(delete-websocket-server server)");
    assert_eq!(socket.next(&mut h), None);
    assert_eq!(h.eval("(websocket-server-list)"), "()");
    // Without word to the server's procedures.
    assert_eq!(h.eval("closed"), "()");
    // The port is free again.
    let start = std::time::Instant::now();
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(h.session.eval("(websocket-server-port server)").is_err());
}

#[test]
fn a_port_that_is_taken_is_an_error() {
    let (mut h, port) = setup("");
    let error = h
        .session
        .eval(&format!("(make-websocket-server {port})"))
        .unwrap_err();
    assert!(
        error
            .message
            .contains(&format!("cannot listen on port {port}")),
        "{}",
        error.message
    );
}
