//! `amacs-mcp`: a Model Context Protocol server on standard input and
//! output, for a language model to work with a running amacs.
//!
//! It talks to the amacs server at `AMACS_SOCKET`, or at the default
//! socket, and starts one if none is running.

use std::io::{BufRead, Write};
use std::process::ExitCode;

use amacs_mcp::Mcp;
use amacs_proto::socket_path;

fn main() -> ExitCode {
    let mut mcp = Mcp::new(socket_path());
    let (stdin, stdout) = (std::io::stdin(), std::io::stdout());
    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            return ExitCode::FAILURE;
        };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str(&line) {
            Ok(request) => mcp.handle(&request),
            Err(error) => Some(serde_json::json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": { "code": -32700, "message": format!("this is not JSON: {error}") },
            })),
        };
        if let Some(reply) = reply {
            let mut out = stdout.lock();
            if writeln!(out, "{reply}").and_then(|()| out.flush()).is_err() {
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
}
