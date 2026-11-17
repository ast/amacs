//! File names as `file://` URIs.

use std::path::{Path, PathBuf};

fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/')
}

/// The URI of an absolute path.
pub fn path_to_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for byte in path.as_os_str().as_encoded_bytes() {
        if is_unreserved(*byte) {
            uri.push(char::from(*byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

/// The path that a `file://` URI names, or `None` for any other URI.
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // An authority, such as `localhost`, comes before the path.
    let path = &rest[rest.find('/')?..];
    let mut bytes = Vec::with_capacity(path.len());
    let mut input = path.bytes();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let hex = [input.next()?, input.next()?];
            let hex = std::str::from_utf8(&hex).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
        } else {
            bytes.push(byte);
        }
    }
    Some(PathBuf::from(String::from_utf8(bytes).ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_round_trip() {
        for path in ["/home/a/src/main.rs", "/tmp/a b/ö#1?.c", "/"] {
            let uri = path_to_uri(Path::new(path));
            assert!(uri.is_ascii(), "{uri}");
            assert_eq!(uri_to_path(&uri), Some(PathBuf::from(path)), "{uri}");
        }
        assert_eq!(path_to_uri(Path::new("/a b/c")), "file:///a%20b/c");
    }

    #[test]
    fn foreign_spellings_are_understood() {
        // Servers escape more than they must, in either case.
        assert_eq!(
            uri_to_path("file:///a%3a/b%2Bc"),
            Some(PathBuf::from("/a:/b+c"))
        );
        assert_eq!(
            uri_to_path("file://localhost/etc/hosts"),
            Some(PathBuf::from("/etc/hosts"))
        );
        assert_eq!(uri_to_path("untitled:Untitled-1"), None);
        assert_eq!(uri_to_path("file:///a%2"), None);
    }
}
