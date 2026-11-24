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
    /// The command for any single key that no active keymap binds, as the
    /// `[t]` binding of Emacs: a terminal sends every other key to its
    /// program.
    default: Option<String>,
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

    /// Sets, or with `None` removes, the command for the keys that no
    /// active keymap binds.
    pub fn set_default(&mut self, command: Option<&str>) {
        self.default = command.map(str::to_owned);
    }

    pub fn default_binding(&self) -> Option<&str> {
        self.default.as_deref()
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

    /// What can follow a prefix: each next key with the command it runs, or
    /// `+prefix` if it leads further.
    pub fn entries(&self, prefix: &[Key]) -> Vec<(Key, String)> {
        let mut map = self;
        for key in prefix {
            match map.bindings.get(key) {
                Some(Binding::Prefix(next)) => map = next,
                _ => return Vec::new(),
            }
        }
        map.bindings
            .iter()
            .map(|(key, binding)| {
                let shown = match binding {
                    Binding::Command(name) => name.clone(),
                    Binding::Prefix(_) => "+prefix".to_owned(),
                };
                (*key, shown)
            })
            .collect()
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

/// Looks `keys` up in several keymaps, earlier ones taking precedence: the
/// sequence is what the first map that knows it says, a command or a
/// prefix, so that a mode can make a prefix of a key that is a command
/// elsewhere. A single key that no map binds goes to the first default
/// binding, if a map has one.
pub fn lookup_in<'a>(maps: &[&'a Keymap], keys: &[Key]) -> Lookup<'a> {
    let mut result = Lookup::Unbound;
    for map in maps {
        match map.lookup(keys) {
            Lookup::Command(name) if result == Lookup::Unbound => {
                return Lookup::Command(name);
            }
            Lookup::Command(_) | Lookup::Unbound => {}
            Lookup::Prefix => result = Lookup::Prefix,
        }
    }
    if result == Lookup::Unbound
        && keys.len() == 1
        && let Some(name) = maps.iter().find_map(|map| map.default_binding())
    {
        return Lookup::Command(name);
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
    fn a_default_binding_takes_the_keys_that_nothing_binds() {
        let mut mode = Keymap::new();
        mode.set_default(Some("send-key"));
        mode.bind_str("C-c C-c", "send-interrupt");
        let mut global = Keymap::new();
        global.bind_str("C-x C-f", "find-file");
        global.bind_str("M-x", "execute-extended-command");
        let maps = [&mode, &global];
        assert_eq!(lookup_in(&maps, &keys("a")), Lookup::Command("send-key"));
        assert_eq!(lookup_in(&maps, &keys("C-d")), Lookup::Command("send-key"));
        assert_eq!(
            lookup_in(&maps, &keys("M-x")),
            Lookup::Command("execute-extended-command")
        );
        // Keys that lead further are still prefixes, and what follows them
        // is not caught by the default.
        assert_eq!(lookup_in(&maps, &keys("C-x")), Lookup::Prefix);
        assert_eq!(
            lookup_in(&maps, &keys("C-x C-f")),
            Lookup::Command("find-file")
        );
        assert_eq!(lookup_in(&maps, &keys("C-x C-z")), Lookup::Unbound);
        assert_eq!(
            lookup_in(&maps, &keys("C-c C-c")),
            Lookup::Command("send-interrupt")
        );
        mode.set_default(None);
        assert_eq!(lookup_in(&[&mode, &global], &keys("a")), Lookup::Unbound);
    }

    #[test]
    fn a_prefix_in_an_earlier_map_wins_over_a_command_in_a_later_one() {
        let mut mode = Keymap::new();
        mode.bind_str("C-q C-k", "term-char-mode");
        let mut global = Keymap::new();
        global.bind_str("C-q", "quoted-insert");
        let maps = [&mode, &global];
        assert_eq!(lookup_in(&maps, &keys("C-q")), Lookup::Prefix);
        assert_eq!(
            lookup_in(&maps, &keys("C-q C-k")),
            Lookup::Command("term-char-mode")
        );
        assert_eq!(
            lookup_in(&[&global], &keys("C-q")),
            Lookup::Command("quoted-insert")
        );
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
