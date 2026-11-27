//! Translation of terminal events into editor input.

use amacs_proto::{InputEvent, Key, KeyCode, Mods, NamedKey};
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::event::{Event, KeyCode as TermKey, KeyEvent, KeyEventKind, KeyModifiers};

/// Whether the terminal reports keys with the kitty keyboard protocol. Set
/// when the terminal is taken over.
pub(crate) static ENHANCED_KEYS: AtomicBool = AtomicBool::new(false);

pub fn translate(event: Event) -> Option<InputEvent> {
    match event {
        Event::Key(key) if key.kind != KeyEventKind::Release => {
            let legacy = !ENHANCED_KEYS.load(Ordering::Relaxed);
            translate_key(key)
                .map(|key| if legacy { legacy_key(key) } else { key })
                .map(InputEvent::Key)
        }
        Event::Resize(cols, rows) => Some(InputEvent::Resize {
            cols,
            rows,
            cell: None,
        }),
        Event::Paste(text) => Some(InputEvent::Paste(
            text.replace("\r\n", "\n").replace('\r', "\n"),
        )),
        _ => None,
    }
}

/// Without the kitty protocol, `C-/` and `C-_` both arrive as the control
/// char 0x1F, which reads as `C-7`. Emacs users mean undo by it.
fn legacy_key(key: Key) -> Key {
    if key == Key::ctrl('7') {
        Key::ctrl('_')
    } else {
        key
    }
}

fn translate_key(event: KeyEvent) -> Option<Key> {
    let mut mods = Mods::empty();
    for (terminal, ours) in [
        (KeyModifiers::CONTROL, Mods::CTRL),
        (KeyModifiers::ALT, Mods::META),
        (KeyModifiers::SHIFT, Mods::SHIFT),
        (KeyModifiers::SUPER, Mods::SUPER),
    ] {
        if event.modifiers.contains(terminal) {
            mods |= ours;
        }
    }

    let code = match event.code {
        TermKey::Char(c) => {
            let chord = mods.intersects(Mods::CTRL | Mods::META | Mods::SUPER);
            if chord && c.is_alphabetic() {
                // Chords are written with the lowercase letter: C-x, C-S-p.
                // Terminals differ in which case they report.
                let lower = c.to_lowercase().next().unwrap_or(c);
                if c.is_uppercase() && lower != c {
                    mods |= Mods::SHIFT;
                }
                KeyCode::Char(lower)
            } else {
                // The char already shows what Shift did: `A`, `!`, `Ö`.
                mods -= Mods::SHIFT;
                KeyCode::Char(c)
            }
        }
        TermKey::Enter => KeyCode::Named(NamedKey::Return),
        TermKey::Tab => KeyCode::Named(NamedKey::Tab),
        TermKey::BackTab => {
            mods |= Mods::SHIFT;
            KeyCode::Named(NamedKey::Tab)
        }
        TermKey::Backspace => KeyCode::Named(NamedKey::Backspace),
        TermKey::Esc => KeyCode::Named(NamedKey::Escape),
        TermKey::Delete => KeyCode::Named(NamedKey::Delete),
        TermKey::Insert => KeyCode::Named(NamedKey::Insert),
        TermKey::Left => KeyCode::Named(NamedKey::Left),
        TermKey::Right => KeyCode::Named(NamedKey::Right),
        TermKey::Up => KeyCode::Named(NamedKey::Up),
        TermKey::Down => KeyCode::Named(NamedKey::Down),
        TermKey::Home => KeyCode::Named(NamedKey::Home),
        TermKey::End => KeyCode::Named(NamedKey::End),
        TermKey::PageUp => KeyCode::Named(NamedKey::PageUp),
        TermKey::PageDown => KeyCode::Named(NamedKey::PageDown),
        TermKey::F(n) => KeyCode::Named(NamedKey::F(n)),
        _ => return None,
    };
    Some(Key::new(code, mods))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: TermKey, modifiers: KeyModifiers) -> String {
        translate_key(KeyEvent::new(code, modifiers))
            .unwrap()
            .to_string()
    }

    #[test]
    fn control_chords_use_lowercase_letters() {
        assert_eq!(key(TermKey::Char('x'), KeyModifiers::CONTROL), "C-x");
        assert_eq!(key(TermKey::Char('X'), KeyModifiers::CONTROL), "C-S-x");
        assert_eq!(
            key(
                TermKey::Char('p'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            ),
            "C-S-p"
        );
        assert_eq!(
            key(
                TermKey::Char('P'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            ),
            "C-S-p"
        );
        assert_eq!(key(TermKey::Char('ö'), KeyModifiers::CONTROL), "C-ö");
        assert_eq!(key(TermKey::Char('x'), KeyModifiers::ALT), "M-x");
    }

    #[test]
    fn shift_is_folded_into_plain_characters() {
        assert_eq!(key(TermKey::Char('A'), KeyModifiers::SHIFT), "A");
        assert_eq!(key(TermKey::Char('!'), KeyModifiers::SHIFT), "!");
        assert_eq!(
            key(TermKey::Char('<'), KeyModifiers::ALT | KeyModifiers::SHIFT),
            "M-<"
        );
    }

    #[test]
    fn hyper_chord_and_named_keys() {
        let hyper = KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER;
        assert_eq!(key(TermKey::Char('c'), hyper), "C-M-s-c");
        assert_eq!(key(TermKey::Char(' '), KeyModifiers::CONTROL), "C-SPC");
        assert_eq!(key(TermKey::BackTab, KeyModifiers::SHIFT), "S-TAB");
        assert_eq!(key(TermKey::Backspace, KeyModifiers::ALT), "M-DEL");
        assert_eq!(key(TermKey::Left, KeyModifiers::CONTROL), "C-<left>");
    }

    #[test]
    fn key_releases_and_pastes() {
        let release = KeyEvent::new_with_kind(
            TermKey::Char('a'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        assert_eq!(translate(Event::Key(release)), None);
        assert_eq!(
            translate(Event::Paste("a\r\nb\rc".to_owned())),
            Some(InputEvent::Paste("a\nb\nc".to_owned()))
        );
    }
}
