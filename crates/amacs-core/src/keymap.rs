use std::collections::HashMap;

use amacs_proto::{Key, parse_keys};

#[derive(Debug, Clone)]
enum Binding {
    Command(String),
    Prefix(Keymap),
}

/// What a key sequence means in a keymap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lookup<'a> {
    Command(&'a str),
    /// The sequence is the start of longer bindings.
    Prefix,
    Unbound,
}

/// Maps key sequences to command names. Commands are looked up by name when
/// the key is pressed, so redefining a command affects existing bindings.
#[derive(Debug, Clone, Default)]
pub struct Keymap {
    bindings: HashMap<Key, Binding>,
}

impl Keymap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Binds a key sequence, replacing whatever it or its prefixes were bound to.
    pub fn bind(&mut self, keys: &[Key], command: &str) {
        let Some((first, rest)) = keys.split_first() else {
            return;
        };
        if rest.is_empty() {
            self.bindings
                .insert(*first, Binding::Command(command.to_owned()));
            return;
        }
        let entry = self
            .bindings
            .entry(*first)
            .or_insert_with(|| Binding::Prefix(Keymap::new()));
        if let Binding::Command(_) = entry {
            *entry = Binding::Prefix(Keymap::new());
        }
        if let Binding::Prefix(map) = entry {
            map.bind(rest, command);
        }
    }

    /// Removes the binding of a key sequence, if it has one. Prefix maps left
    /// empty are removed too.
    pub fn unbind(&mut self, keys: &[Key]) {
        let Some((first, rest)) = keys.split_first() else {
            return;
        };
        if rest.is_empty() {
            self.bindings.remove(first);
        } else if let Some(Binding::Prefix(map)) = self.bindings.get_mut(first) {
            map.unbind(rest);
            if map.bindings.is_empty() {
                self.bindings.remove(first);
            }
        }
    }

    /// Binds a sequence written in Emacs notation, such as `"C-x C-f"`.
    ///
    /// # Panics
    /// If the description does not parse. It is meant for literal strings.
    pub fn bind_str(&mut self, keys: &str, command: &str) {
        let keys = parse_keys(keys).unwrap_or_else(|e| panic!("{e}"));
        self.bind(&keys, command);
    }

    pub fn lookup(&self, keys: &[Key]) -> Lookup<'_> {
        let Some((first, rest)) = keys.split_first() else {
            return Lookup::Prefix;
        };
        match self.bindings.get(first) {
            None => Lookup::Unbound,
            Some(Binding::Command(name)) if rest.is_empty() => Lookup::Command(name),
            Some(Binding::Command(_)) => Lookup::Unbound,
            Some(Binding::Prefix(map)) => map.lookup(rest),
        }
    }
}

/// Looks `keys` up in several keymaps, earlier ones taking precedence. The
/// sequence is a prefix if it is one in any map that does not bind it outright.
pub fn lookup_in<'a>(maps: &[&'a Keymap], keys: &[Key]) -> Lookup<'a> {
    let mut result = Lookup::Unbound;
    for map in maps {
        match map.lookup(keys) {
            Lookup::Command(name) => return Lookup::Command(name),
            Lookup::Prefix => result = Lookup::Prefix,
            Lookup::Unbound => {}
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(s: &str) -> Vec<Key> {
        parse_keys(s).unwrap()
    }

    #[test]
    fn prefix_keys_nest() {
        let mut map = Keymap::new();
        map.bind_str("C-x C-f", "find-file");
        map.bind_str("C-x 4 t", "transpose-frame");
        assert_eq!(map.lookup(&keys("C-x")), Lookup::Prefix);
        assert_eq!(map.lookup(&keys("C-x C-f")), Lookup::Command("find-file"));
        assert_eq!(map.lookup(&keys("C-x 4")), Lookup::Prefix);
        assert_eq!(map.lookup(&keys("C-x C-z")), Lookup::Unbound);
        assert_eq!(map.lookup(&keys("C-x C-f a")), Lookup::Unbound);
    }

    #[test]
    fn unbinding_removes_empty_prefixes() {
        let mut map = Keymap::new();
        map.bind_str("C-c a b", "one");
        map.bind_str("C-c x", "two");
        map.unbind(&keys("C-c a b"));
        assert_eq!(map.lookup(&keys("C-c a")), Lookup::Unbound);
        assert_eq!(map.lookup(&keys("C-c")), Lookup::Prefix);
        map.unbind(&keys("C-c x"));
        assert_eq!(map.lookup(&keys("C-c")), Lookup::Unbound);
    }

    #[test]
    fn rebinding_a_command_key_as_a_prefix_replaces_it() {
        let mut map = Keymap::new();
        map.bind_str("C-c", "old");
        map.bind_str("C-c v", "vterm");
        assert_eq!(map.lookup(&keys("C-c")), Lookup::Prefix);
        assert_eq!(map.lookup(&keys("C-c v")), Lookup::Command("vterm"));
    }

    #[test]
    fn earlier_maps_win_and_prefixes_merge() {
        let mut local = Keymap::new();
        local.bind_str("RET", "exit-minibuffer");
        local.bind_str("C-c x", "local-x");
        let mut global = Keymap::new();
        global.bind_str("RET", "newline");
        global.bind_str("C-c y", "global-y");
        let maps = [&local, &global];
        assert_eq!(
            lookup_in(&maps, &keys("RET")),
            Lookup::Command("exit-minibuffer")
        );
        assert_eq!(lookup_in(&maps, &keys("C-c")), Lookup::Prefix);
        assert_eq!(
            lookup_in(&maps, &keys("C-c y")),
            Lookup::Command("global-y")
        );
        assert_eq!(lookup_in(&maps, &keys("C-c z")), Lookup::Unbound);
    }
}
