//! Messages on the wire: a `Content-Length` header, a blank line, and that
//! many bytes of JSON.

use std::io::{self, BufRead, Write};

use serde_json::Value;

/// Messages larger than this are refused rather than read into memory.
const MAX_MESSAGE: usize = 256 << 20;

pub fn write_message(writer: &mut impl Write, message: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(message)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

/// Reads one message. Returns `None` at a clean end of the stream.
pub fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let invalid = |what: &str| io::Error::new(io::ErrorKind::InvalidData, what.to_owned());
    let mut length = None;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return if length.is_none() {
                Ok(None)
            } else {
                Err(io::ErrorKind::UnexpectedEof.into())
            };
        }
        let header = line.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| invalid("bad Content-Length"))?,
            );
        }
    }
    let length = length.ok_or_else(|| invalid("message without Content-Length"))?;
    if length > MAX_MESSAGE {
        return Err(invalid("message too large"));
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|error| invalid(&format!("message is not JSON: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn messages_round_trip() {
        let mut wire = Vec::new();
        let first = json!({"jsonrpc": "2.0", "id": 1, "method": "x", "params": {"å": [1, 2]}});
        let second = json!({"jsonrpc": "2.0", "method": "y"});
        write_message(&mut wire, &first).unwrap();
        write_message(&mut wire, &second).unwrap();
        let mut reader = io::BufReader::new(wire.as_slice());
        assert_eq!(read_message(&mut reader).unwrap(), Some(first));
        assert_eq!(read_message(&mut reader).unwrap(), Some(second));
        assert_eq!(read_message(&mut reader).unwrap(), None);
    }

    #[test]
    fn other_headers_are_skipped_and_damage_is_an_error() {
        let wire = b"Content-Type: x\r\ncontent-length: 2\r\n\r\n{}";
        let mut reader = io::BufReader::new(wire.as_slice());
        assert_eq!(read_message(&mut reader).unwrap(), Some(json!({})));

        for bad in [
            &b"Content-Length: 10\r\n\r\n{}"[..],
            b"Content-Length: 2\r\n\r\n{]",
            b"Foo: 1\r\n\r\n{}",
            b"Content-Length: x\r\n\r\n",
            b"Content-Length: 2\r\n",
        ] {
            let mut reader = io::BufReader::new(bad);
            assert!(read_message(&mut reader).is_err(), "{bad:?}");
        }
    }
}
