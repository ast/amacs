//! A stand-in for a web browser and its extension: a program that asks a
//! port of this machine over HTTP and talks to it over a WebSocket.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use tungstenite::client::IntoClientRequest;
use tungstenite::{Message, WebSocket};

use super::Harness;

/// The origin that an extension of Firefox gives.
pub const EXTENSION: &str = "moz-extension://0d7c1f6e-test";

pub struct Socket(WebSocket<TcpStream>);

/// Asks the port with a plain GET and returns the whole answer.
pub fn get(h: &mut Harness, port: u16, origin: Option<&str>) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let origin = origin.map_or(String::new(), |origin| format!("Origin: {origin}\r\n"));
    write!(
        stream,
        "GET / HTTP/1.1\r\nHost: localhost:{port}\r\n{origin}\r\n"
    )
    .unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    h.redisplay();
    answer
}

/// Asks for a WebSocket. An `Err` is the status the server refused with.
pub fn connect(port: u16, origin: Option<&str>) -> Result<Socket, u16> {
    let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let mut request = format!("ws://localhost:{port}/")
        .into_client_request()
        .unwrap();
    if let Some(origin) = origin {
        request
            .headers_mut()
            .insert("Origin", origin.parse().unwrap());
    }
    match tungstenite::client(request, stream) {
        Ok((socket, _)) => {
            socket
                .get_ref()
                .set_read_timeout(Some(Duration::from_millis(5)))
                .unwrap();
            Ok(Socket(socket))
        }
        Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Http(response))) => {
            Err(response.status().as_u16())
        }
        Err(other) => panic!("the handshake failed: {other:?}"),
    }
}

impl Socket {
    pub fn send(&mut self, text: &str) {
        self.0.send(Message::text(text)).unwrap();
    }

    /// The next text message, or `None` when the connection has ended.
    /// The editor goes on working while this waits.
    pub fn next(&mut self, h: &mut Harness) -> Option<String> {
        let start = Instant::now();
        loop {
            h.redisplay();
            match self.0.read() {
                Ok(Message::Text(text)) => return Some(text.as_str().to_owned()),
                Ok(Message::Close(_)) => {}
                Ok(other) => panic!("unexpected message: {other:?}"),
                Err(tungstenite::Error::Io(error))
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => return None,
            }
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "the editor sent nothing"
            );
        }
    }

    /// Whether nothing arrives for a while that is long for the editor.
    pub fn quiet(&mut self, h: &mut Harness) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(300) {
            h.redisplay();
            match self.0.read() {
                Err(tungstenite::Error::Io(_)) => {}
                _ => return false,
            }
        }
        true
    }

    pub fn close(mut self) {
        let _ = self.0.close(None);
        let _ = self.0.flush();
    }
}
