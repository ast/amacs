//! Translation of keyboard symbols into editor keys.

use amacs_proto::{Key, KeyCode, Mods, NamedKey};
use smithay_client_toolkit::seat::keyboard::{Keysym, Modifiers};

/// The key for a pressed symbol, or `None` for symbols that are not keys of
/// their own, such as Shift.
///
/// The symbol is the one the layout produces with the modifiers held, so
/// `Shift+7` on a Swedish layout arrives as `/` and is reported as `/`
/// without Shift.
pub fn translate(keysym: Keysym, modifiers: Modifiers) -> Option<Key> {
    let mut mods = Mods::empty();
    for (held, modifier) in [
        (modifiers.ctrl, Mods::CTRL),
        (modifiers.alt, Mods::META),
        (modifiers.shift, Mods::SHIFT),
        (modifiers.logo, Mods::SUPER),
    ] {
        if held {
            mods |= modifier;
        }
    }

    let named = match keysym {
        Keysym::Return | Keysym::KP_Enter => Some(NamedKey::Return),
        Keysym::Tab | Keysym::KP_Tab => Some(NamedKey::Tab),
        // What Shift+Tab produces. Shift is already in the modifiers.
        Keysym::ISO_Left_Tab => Some(NamedKey::Tab),
        Keysym::BackSpace => Some(NamedKey::Backspace),
        Keysym::Escape => Some(NamedKey::Escape),
        Keysym::Delete | Keysym::KP_Delete => Some(NamedKey::Delete),
        Keysym::Insert | Keysym::KP_Insert => Some(NamedKey::Insert),
        Keysym::Left | Keysym::KP_Left => Some(NamedKey::Left),
        Keysym::Right | Keysym::KP_Right => Some(NamedKey::Right),
        Keysym::Up | Keysym::KP_Up => Some(NamedKey::Up),
        Keysym::Down | Keysym::KP_Down => Some(NamedKey::Down),
        Keysym::Home | Keysym::KP_Home => Some(NamedKey::Home),
        Keysym::End | Keysym::KP_End => Some(NamedKey::End),
        Keysym::Page_Up | Keysym::KP_Page_Up => Some(NamedKey::PageUp),
        Keysym::Page_Down | Keysym::KP_Page_Down => Some(NamedKey::PageDown),
        _ => {
            let (first, last) = (Keysym::F1.raw(), Keysym::F35.raw());
            let raw = keysym.raw();
            (first..=last)
                .contains(&raw)
                .then(|| NamedKey::F((raw - first + 1) as u8))
        }
    };
    if let Some(named) = named {
        return Some(Key::new(KeyCode::Named(named), mods));
    }

    let c = keysym.key_char().filter(|c| !c.is_control())?;
    let chord = mods.intersects(Mods::CTRL | Mods::META | Mods::SUPER);
    if chord && c.is_alphabetic() {
        // Chords are written with the lowercase letter: C-x, C-S-p. An
        // uppercase symbol without Shift held is Caps Lock's doing.
        let lower = c.to_lowercase().next().unwrap_or(c);
        Some(Key::new(KeyCode::Char(lower), mods))
    } else {
        // The symbol already shows what Shift did: `A`, `!`, `Ö`.
        Some(Key::new(KeyCode::Char(c), mods - Mods::SHIFT))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(ctrl: bool, alt: bool, shift: bool, logo: bool) -> Modifiers {
        Modifiers {
            ctrl,
            alt,
            shift,
            logo,
            ..Modifiers::default()
        }
    }

    fn key(keysym: Keysym, modifiers: Modifiers) -> String {
        translate(keysym, modifiers).unwrap().to_string()
    }

    #[test]
    fn plain_and_shifted_characters() {
        let none = Modifiers::default();
        let shift = mods(false, false, true, false);
        assert_eq!(key(Keysym::a, none), "a");
        assert_eq!(key(Keysym::A, shift), "A");
        assert_eq!(key(Keysym::slash, shift), "/");
        assert_eq!(key(Keysym::odiaeresis, none), "ö");
        assert_eq!(key(Keysym::space, none), "SPC");
    }

    #[test]
    fn chords_use_lowercase_letters_and_keep_shift() {
        let ctrl = mods(true, false, false, false);
        assert_eq!(key(Keysym::x, ctrl), "C-x");
        assert_eq!(key(Keysym::P, mods(true, false, true, false)), "C-S-p");
        assert_eq!(key(Keysym::odiaeresis, ctrl), "C-ö");
        assert_eq!(key(Keysym::period, ctrl), "C-.");
        assert_eq!(key(Keysym::plus, mods(false, true, false, false)), "M-+");
        assert_eq!(key(Keysym::less, mods(false, true, false, false)), "M-<");
        assert_eq!(key(Keysym::greater, mods(false, true, true, false)), "M->");
        assert_eq!(key(Keysym::space, ctrl), "C-SPC");

        let caps = Modifiers {
            ctrl: true,
            caps_lock: true,
            ..Modifiers::default()
        };
        assert_eq!(key(Keysym::X, caps), "C-x", "Caps Lock does not add Shift");
    }

    #[test]
    fn hyper_is_ctrl_meta_super() {
        assert_eq!(key(Keysym::c, mods(true, true, false, true)), "C-M-s-c");
    }

    #[test]
    fn named_keys() {
        let none = Modifiers::default();
        assert_eq!(key(Keysym::Return, none), "RET");
        assert_eq!(
            key(Keysym::ISO_Left_Tab, mods(false, false, true, false)),
            "S-TAB"
        );
        assert_eq!(key(Keysym::Tab, mods(true, false, false, false)), "C-TAB");
        assert_eq!(
            key(Keysym::BackSpace, mods(false, true, false, false)),
            "M-DEL"
        );
        assert_eq!(key(Keysym::Page_Up, none), "<prior>");
        assert_eq!(key(Keysym::F5, none), "<f5>");
        assert_eq!(key(Keysym::F12, mods(false, false, true, false)), "S-<f12>");
    }

    #[test]
    fn modifier_keys_alone_are_not_keys() {
        assert_eq!(translate(Keysym::Shift_L, Modifiers::default()), None);
        assert_eq!(translate(Keysym::Control_R, Modifiers::default()), None);
        assert_eq!(
            translate(Keysym::ISO_Level3_Shift, Modifiers::default()),
            None
        );
    }
}
