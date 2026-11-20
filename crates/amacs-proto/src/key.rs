use std::fmt;
use std::str::FromStr;

use bitflags::bitflags;
use serde::{Deserialize, Serialize};

bitflags! {
    /// Keyboard modifiers, written `C-`, `M-`, `S-` and `s-` as in Emacs.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct Mods: u8 {
        const CTRL = 1;
        const META = 1 << 1;
        const SHIFT = 1 << 2;
        const SUPER = 1 << 3;
    }
}

impl Mods {
    /// The Hyper key on the user's keyboard sends Ctrl+Meta+Super, so `H-` is
    /// an alias for that chord rather than a modifier of its own.
    pub const HYPER: Mods = Mods::CTRL.union(Mods::META).union(Mods::SUPER);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NamedKey {
    Return,
    Tab,
    Backspace,
    Escape,
    Delete,
    Insert,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    F(u8),
    /// A button of the pointer was pressed: 1 is the main one, 2 the
    /// middle one, 3 the one for menus.
    Mouse(u8),
    /// It was pressed a second time in quick succession, and a third.
    DoubleMouse(u8),
    TripleMouse(u8),
    /// The pointer came to another place with the button held.
    MotionMouse(u8),
    /// The button was let go after the pointer had moved.
    DragMouse(u8),
    /// Text was pasted. It is not a key that is pressed, but a mode binds
    /// it as one to take the text itself, with `last-paste`.
    Paste,
}

/// Makes the key of a kind that has a given number.
type Numbered = fn(u8) -> NamedKey;

impl NamedKey {
    /// The keys that carry a number, by what stands before it in their
    /// names: `<f5>`, `<mouse-1>`.
    const NUMBERED: &[(Numbered, &str)] = &[
        (NamedKey::F, "f"),
        (NamedKey::Mouse, "mouse-"),
        (NamedKey::DoubleMouse, "double-mouse-"),
        (NamedKey::TripleMouse, "triple-mouse-"),
        (NamedKey::MotionMouse, "motion-mouse-"),
        (NamedKey::DragMouse, "drag-mouse-"),
    ];

    /// Whether the key is something done with the pointer.
    pub fn is_mouse(self) -> bool {
        matches!(
            self,
            NamedKey::Mouse(_)
                | NamedKey::DoubleMouse(_)
                | NamedKey::TripleMouse(_)
                | NamedKey::MotionMouse(_)
                | NamedKey::DragMouse(_)
        )
    }

    const NAMES: &[(NamedKey, &str)] = &[
        (NamedKey::Return, "RET"),
        (NamedKey::Tab, "TAB"),
        (NamedKey::Backspace, "DEL"),
        (NamedKey::Escape, "ESC"),
        (NamedKey::Delete, "<delete>"),
        (NamedKey::Insert, "<insert>"),
        (NamedKey::Left, "<left>"),
        (NamedKey::Right, "<right>"),
        (NamedKey::Up, "<up>"),
        (NamedKey::Down, "<down>"),
        (NamedKey::Home, "<home>"),
        (NamedKey::End, "<end>"),
        (NamedKey::PageUp, "<prior>"),
        (NamedKey::PageDown, "<next>"),
        (NamedKey::Paste, "<paste>"),
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KeyCode {
    /// A key that produces a character. The char already reflects Shift where
    /// that changes it (`A`, `!`), in which case `Mods::SHIFT` is not set.
    Char(char),
    Named(NamedKey),
}

/// One key press with its modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Key {
    pub code: KeyCode,
    pub mods: Mods,
}

impl Key {
    pub const fn new(code: KeyCode, mods: Mods) -> Self {
        Self { code, mods }
    }

    pub const fn char(c: char) -> Self {
        Self::new(KeyCode::Char(c), Mods::empty())
    }

    pub const fn ctrl(c: char) -> Self {
        Self::new(KeyCode::Char(c), Mods::CTRL)
    }

    pub const fn meta(c: char) -> Self {
        Self::new(KeyCode::Char(c), Mods::META)
    }

    pub const fn named(key: NamedKey) -> Self {
        Self::new(KeyCode::Named(key), Mods::empty())
    }

    /// The char this key inserts when it is not bound to anything else.
    pub fn self_insert_char(&self) -> Option<char> {
        match self.code {
            KeyCode::Char(c) if self.mods.is_empty() => Some(c),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid key description `{0}`")]
pub struct ParseKeyError(String);

impl FromStr for Key {
    type Err = ParseKeyError;

    /// Parses Emacs key notation: `C-x`, `M-<left>`, `C-M-s-c`, `H-c`, `RET`, `SPC`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseKeyError(s.to_owned());
        let mut mods = Mods::empty();
        let mut rest = s;
        // A modifier prefix is a letter and a dash followed by more text, so
        // that `-` and `C--` still parse as the dash key.
        loop {
            let mut chars = rest.chars();
            let (Some(letter), Some('-')) = (chars.next(), chars.next()) else {
                break;
            };
            if chars.as_str().is_empty() {
                break;
            }
            mods |= match letter {
                'C' => Mods::CTRL,
                'M' => Mods::META,
                'S' => Mods::SHIFT,
                's' => Mods::SUPER,
                'H' => Mods::HYPER,
                _ => break,
            };
            rest = chars.as_str();
        }

        let code = if let Some(&(named, _)) = NamedKey::NAMES.iter().find(|(_, n)| *n == rest) {
            KeyCode::Named(named)
        } else if rest == "SPC" {
            KeyCode::Char(' ')
        } else if let Some(inner) = rest.strip_prefix('<').and_then(|r| r.strip_suffix('>')) {
            let digits = inner.trim_start_matches(|c: char| !c.is_ascii_digit());
            let prefix = &inner[..inner.len() - digits.len()];
            let (make, _) = NamedKey::NUMBERED
                .iter()
                .find(|(_, name)| *name == prefix)
                .ok_or_else(err)?;
            KeyCode::Named(make(digits.parse().map_err(|_| err())?))
        } else {
            let mut chars = rest.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if !c.is_control() => KeyCode::Char(c),
                _ => return Err(err()),
            }
        };
        Ok(Key { code, mods })
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (flag, prefix) in [
            (Mods::CTRL, "C-"),
            (Mods::META, "M-"),
            (Mods::SHIFT, "S-"),
            (Mods::SUPER, "s-"),
        ] {
            if self.mods.contains(flag) {
                f.write_str(prefix)?;
            }
        }
        match self.code {
            KeyCode::Char(' ') => f.write_str("SPC"),
            KeyCode::Char(c) => write!(f, "{c}"),
            KeyCode::Named(NamedKey::F(n)) => write!(f, "<f{n}>"),
            KeyCode::Named(NamedKey::Mouse(n)) => write!(f, "<mouse-{n}>"),
            KeyCode::Named(NamedKey::DoubleMouse(n)) => write!(f, "<double-mouse-{n}>"),
            KeyCode::Named(NamedKey::TripleMouse(n)) => write!(f, "<triple-mouse-{n}>"),
            KeyCode::Named(NamedKey::MotionMouse(n)) => write!(f, "<motion-mouse-{n}>"),
            KeyCode::Named(NamedKey::DragMouse(n)) => write!(f, "<drag-mouse-{n}>"),
            KeyCode::Named(named) => {
                let name = NamedKey::NAMES.iter().find(|(k, _)| *k == named);
                f.write_str(name.map_or("<unknown>", |(_, n)| n))
            }
        }
    }
}

/// Parses a whitespace-separated key sequence such as `"C-x C-f"`.
pub fn parse_keys(s: &str) -> Result<Vec<Key>, ParseKeyError> {
    s.split_whitespace().map(str::parse).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(s: &str) -> Key {
        s.parse().unwrap()
    }

    #[test]
    fn parses_modifiers_and_names() {
        assert_eq!(key("C-x"), Key::ctrl('x'));
        assert_eq!(
            key("M-<left>"),
            Key::new(KeyCode::Named(NamedKey::Left), Mods::META)
        );
        assert_eq!(key("C-SPC"), Key::ctrl(' '));
        assert_eq!(key("RET"), Key::named(NamedKey::Return));
        assert_eq!(key("<f5>"), Key::named(NamedKey::F(5)));
        assert_eq!(key("C-ö"), Key::ctrl('ö'));
        assert_eq!(
            key("C-S-p"),
            Key::new(KeyCode::Char('p'), Mods::CTRL | Mods::SHIFT)
        );
    }

    #[test]
    fn hyper_is_ctrl_meta_super() {
        assert_eq!(key("H-c"), key("C-M-s-c"));
        assert_eq!(key("H-c").to_string(), "C-M-s-c");
    }

    #[test]
    fn dash_is_a_key() {
        assert_eq!(key("-"), Key::char('-'));
        assert_eq!(key("C--"), Key::ctrl('-'));
        assert_eq!(key("C-c"), Key::ctrl('c'));
        assert_eq!(key("M--"), Key::meta('-'));
    }

    #[test]
    fn display_round_trips() {
        for s in [
            "C-x",
            "M-x",
            "C-M-s-c",
            "RET",
            "TAB",
            "DEL",
            "SPC",
            "C-SPC",
            "<f12>",
            "M-<prior>",
            "C-S-p",
            "å",
            "<mouse-1>",
            "C-<double-mouse-1>",
            "<triple-mouse-1>",
            "<motion-mouse-1>",
            "S-<drag-mouse-3>",
        ] {
            assert_eq!(key(s).to_string(), s);
        }
    }

    #[test]
    fn rejects_garbage() {
        assert!("".parse::<Key>().is_err());
        assert!("C-xy".parse::<Key>().is_err());
        assert!("<nope>".parse::<Key>().is_err());
        assert!(parse_keys("C-x C-f").is_ok());
    }
}
