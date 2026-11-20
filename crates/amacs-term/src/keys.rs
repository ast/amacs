//! Keys as the bytes a terminal sends for them.
//!
//! The encoding is xterm's: control characters for Control with a letter,
//! an escape before the key for Meta, `CSI` and `SS3` sequences for the
//! named keys with the modifier number of xterm, and the `CSI u` form for
//! what has no older sequence, such as Shift with Return, which programs
//! made for the terminals of today understand.

use alacritty_terminal::term::TermMode;
use amacs_proto::{Key, KeyCode, Mods, NamedKey};

/// The modifier number of xterm: 1 plus 1 for Shift, 2 for Alt, 4 for
/// Control and 8 for Super.
fn modifier(mods: Mods) -> u32 {
    1 + u32::from(mods.contains(Mods::SHIFT))
        + 2 * u32::from(mods.contains(Mods::META))
        + 4 * u32::from(mods.contains(Mods::CTRL))
        + 8 * u32::from(mods.contains(Mods::SUPER))
}

/// The control character that Control with a char gives, if there is one.
fn control(c: char) -> Option<u8> {
    Some(match c {
        'a'..='z' => c as u8 - b'a' + 1,
        'A'..='Z' => c as u8 - b'A' + 1,
        ' ' | '@' | '2' => 0,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '7' | '/' => 0x1f,
        '?' | '8' => 0x7f,
        _ => return None,
    })
}

/// `CSI code ; modifier u`, the form for keys without one of their own.
fn csi_u(code: u32, mods: Mods) -> Vec<u8> {
    if mods.is_empty() {
        format!("\x1b[{code}u").into_bytes()
    } else {
        format!("\x1b[{code};{}u", modifier(mods)).into_bytes()
    }
}

/// The bytes a key sends, or `None` for a key that sends nothing, such as
/// a button of the pointer.
pub fn encode_key(key: &Key, mode: &TermMode) -> Option<Vec<u8>> {
    let mods = key.mods;
    let meta = mods.contains(Mods::META);
    let others = mods.difference(Mods::META);
    let bytes = match key.code {
        KeyCode::Char(c) => {
            if others.contains(Mods::SUPER) {
                return Some(csi_u(c as u32, mods));
            }
            let mut bytes = Vec::new();
            if meta {
                bytes.push(0x1b);
            }
            if others.contains(Mods::CTRL) {
                match control(c) {
                    Some(byte) => bytes.push(byte),
                    None => return Some(csi_u(c as u32, mods)),
                }
            } else {
                let mut utf8 = [0; 4];
                bytes.extend_from_slice(c.encode_utf8(&mut utf8).as_bytes());
            }
            bytes
        }
        KeyCode::Named(named) => {
            let app_cursor = mode.contains(TermMode::APP_CURSOR);
            // Keys with a letter after `CSI` or `SS3`.
            let letter = match named {
                NamedKey::Up => Some('A'),
                NamedKey::Down => Some('B'),
                NamedKey::Right => Some('C'),
                NamedKey::Left => Some('D'),
                NamedKey::Home => Some('H'),
                NamedKey::End => Some('F'),
                NamedKey::F(1) => Some('P'),
                NamedKey::F(2) => Some('Q'),
                NamedKey::F(3) => Some('R'),
                NamedKey::F(4) => Some('S'),
                _ => None,
            };
            if let Some(letter) = letter {
                let ss3 = matches!(named, NamedKey::F(_))
                    || (app_cursor && !matches!(named, NamedKey::Home | NamedKey::End));
                return Some(
                    if mods.is_empty() {
                        if ss3 {
                            format!("\x1bO{letter}")
                        } else {
                            format!("\x1b[{letter}")
                        }
                    } else {
                        format!("\x1b[1;{}{letter}", modifier(mods))
                    }
                    .into_bytes(),
                );
            }
            // Keys with a number before a tilde.
            let number = match named {
                NamedKey::Insert => Some(2),
                NamedKey::Delete => Some(3),
                NamedKey::PageUp => Some(5),
                NamedKey::PageDown => Some(6),
                NamedKey::F(5) => Some(15),
                NamedKey::F(6) => Some(17),
                NamedKey::F(7) => Some(18),
                NamedKey::F(8) => Some(19),
                NamedKey::F(9) => Some(20),
                NamedKey::F(10) => Some(21),
                NamedKey::F(11) => Some(23),
                NamedKey::F(12) => Some(24),
                _ => None,
            };
            if let Some(number) = number {
                return Some(
                    if mods.is_empty() {
                        format!("\x1b[{number}~")
                    } else {
                        format!("\x1b[{number};{}~", modifier(mods))
                    }
                    .into_bytes(),
                );
            }
            match named {
                NamedKey::Return if others.is_empty() => with_meta(meta, b"\r"),
                NamedKey::Return => csi_u(13, mods),
                NamedKey::Tab if others.is_empty() => with_meta(meta, b"\t"),
                NamedKey::Tab if others == Mods::SHIFT => with_meta(meta, b"\x1b[Z"),
                NamedKey::Tab => csi_u(9, mods),
                NamedKey::Backspace if others.is_empty() => with_meta(meta, b"\x7f"),
                NamedKey::Backspace if others == Mods::CTRL => with_meta(meta, b"\x08"),
                NamedKey::Backspace => csi_u(127, mods),
                NamedKey::Escape if others.is_empty() => with_meta(meta, b"\x1b"),
                NamedKey::Escape => csi_u(27, mods),
                _ => return None,
            }
        }
    };
    Some(bytes)
}

fn with_meta(meta: bool, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 1);
    if meta {
        out.push(0x1b);
    }
    out.extend_from_slice(bytes);
    out
}

/// The bytes that pasting text sends: wrapped in the bracketed paste
/// sequences when the program asked for them, and otherwise with the line
/// breaks as the Return key sends them.
pub fn encode_paste(text: &str, mode: &TermMode) -> Vec<u8> {
    if mode.contains(TermMode::BRACKETED_PASTE) {
        let mut out = b"\x1b[200~".to_vec();
        out.extend_from_slice(text.as_bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        text.replace('\n', "\r").into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(keys: &str, mode: TermMode) -> Vec<u8> {
        let key: Key = keys.parse().unwrap();
        encode_key(&key, &mode).unwrap()
    }

    #[test]
    fn letters_controls_and_meta() {
        let none = TermMode::NONE;
        assert_eq!(bytes("a", none), b"a");
        assert_eq!(bytes("C-a", none), b"\x01");
        assert_eq!(bytes("C-SPC", none), b"\0");
        assert_eq!(bytes("M-x", none), b"\x1bx");
        assert_eq!(bytes("C-M-a", none), b"\x1b\x01");
        assert_eq!(bytes("é", none), "é".as_bytes());
        assert_eq!(bytes("C-é", none), b"\x1b[233;5u");
        assert_eq!(bytes("s-a", none), b"\x1b[97;9u");
    }

    #[test]
    fn named_keys_follow_xterm() {
        let none = TermMode::NONE;
        assert_eq!(bytes("RET", none), b"\r");
        assert_eq!(bytes("S-RET", none), b"\x1b[13;2u");
        assert_eq!(bytes("M-RET", none), b"\x1b\r");
        assert_eq!(bytes("TAB", none), b"\t");
        assert_eq!(bytes("S-TAB", none), b"\x1b[Z");
        assert_eq!(bytes("DEL", none), b"\x7f");
        assert_eq!(bytes("C-DEL", none), b"\x08");
        assert_eq!(bytes("ESC", none), b"\x1b");
        assert_eq!(bytes("<up>", none), b"\x1b[A");
        assert_eq!(bytes("<up>", TermMode::APP_CURSOR), b"\x1bOA");
        assert_eq!(bytes("C-<left>", none), b"\x1b[1;5D");
        assert_eq!(bytes("<home>", TermMode::APP_CURSOR), b"\x1b[H");
        assert_eq!(bytes("<f1>", none), b"\x1bOP");
        assert_eq!(bytes("<f5>", none), b"\x1b[15~");
        assert_eq!(bytes("S-<f5>", none), b"\x1b[15;2~");
        assert_eq!(bytes("<delete>", none), b"\x1b[3~");
        assert_eq!(bytes("<next>", none), b"\x1b[6~");
        let mouse: Key = "<mouse-1>".parse().unwrap();
        assert_eq!(encode_key(&mouse, &none), None);
    }

    #[test]
    fn pasting() {
        assert_eq!(encode_paste("a\nb", &TermMode::NONE), b"a\rb");
        assert_eq!(
            encode_paste("a\nb", &TermMode::BRACKETED_PASTE),
            b"\x1b[200~a\nb\x1b[201~"
        );
    }
}
