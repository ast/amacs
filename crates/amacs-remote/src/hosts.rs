//! The hosts that the user's `ssh` knows by name: those of its
//! configuration, with what it says of each, and those met before. The
//! prompt for file names offers them after `/ssh:`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// A host that `ssh` knows, with what its configuration says of it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SshHost {
    /// The name that `ssh` is given.
    pub name: String,
    /// The address behind the name, if it has another.
    pub host_name: Option<String>,
    pub user: Option<String>,
    pub port: Option<String>,
    /// The host or hosts that it is reached through.
    pub jump: Option<String>,
    /// Not named in the configuration, only met before.
    pub only_known: bool,
}

/// Whether a file name matches a pattern of `*` and `?`, as the patterns
/// of `Include` in the configuration of `ssh` are.
fn wildcard(pattern: &str, name: &str) -> bool {
    let (pattern, name): (Vec<char>, Vec<char>) =
        (pattern.chars().collect(), name.chars().collect());
    // Where the last star was, and how much it has taken so far.
    let (mut p, mut n, mut star) = (0, 0, None);
    while n < name.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == name[n]) {
            p += 1;
            n += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some((p, n));
            p += 1;
        } else if let Some((at, taken)) = star {
            p = at + 1;
            n = taken + 1;
            star = Some((at, taken + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|c| *c == '*')
}

/// The files that an `Include` names: relative to the directory of the
/// user's configuration, with `~` as the home directory and patterns in
/// the last part.
fn included(pattern: &str, ssh_directory: &Path) -> Vec<PathBuf> {
    let path = match pattern.strip_prefix("~/") {
        Some(rest) => match ssh_directory.parent() {
            Some(home) => home.join(rest),
            None => return Vec::new(),
        },
        None => ssh_directory.join(pattern),
    };
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return Vec::new();
    };
    if !name.contains(['*', '?']) {
        return vec![path];
    }
    let Some(directory) = path.parent() else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| wildcard(name, &entry.file_name().to_string_lossy()))
        .map(|entry| entry.path())
        .collect();
    files.sort();
    files
}

/// Reads a configuration of `ssh` into `hosts`: the hosts that it names
/// outright, without patterns, in the order of the file, each with what
/// is said under its name. Files that it includes are read where they
/// are named.
fn read_config(file: &Path, ssh_directory: &Path, depth: usize, hosts: &mut Vec<SshHost>) {
    let Ok(config) = std::fs::read_to_string(file) else {
        return;
    };
    // The hosts that the lines being read are about.
    let mut current: Vec<usize> = Vec::new();
    for line in config.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // `Key value`, or `Key=value`, in any case.
        let (key, value) = match line.split_once(|c: char| c.is_whitespace() || c == '=') {
            Some((key, value)) => (key, value.trim_start_matches([' ', '\t', '='])),
            None => (line, ""),
        };
        let words = || value.split_whitespace().map(|word| word.trim_matches('"'));
        match key.to_ascii_lowercase().as_str() {
            "host" => {
                current.clear();
                for name in words() {
                    if name.is_empty() || name.contains(['*', '?', '!', ',']) {
                        continue;
                    }
                    let at = hosts.iter().position(|host| host.name == name);
                    current.push(at.unwrap_or_else(|| {
                        hosts.push(SshHost {
                            name: name.to_owned(),
                            ..SshHost::default()
                        });
                        hosts.len() - 1
                    }));
                }
            }
            // What follows is about hosts by other things than a name.
            "match" => current.clear(),
            "include" if depth < 8 => {
                for pattern in words() {
                    for file in included(pattern, ssh_directory) {
                        read_config(&file, ssh_directory, depth + 1, hosts);
                    }
                }
            }
            key => {
                let Some(value) = words().next() else {
                    continue;
                };
                for at in &current {
                    let host = &mut hosts[*at];
                    // The first that is said holds, as for `ssh`.
                    let slot = match key {
                        "hostname" => &mut host.host_name,
                        "user" => &mut host.user,
                        "port" => &mut host.port,
                        "proxyjump" => &mut host.jump,
                        _ => continue,
                    };
                    slot.get_or_insert_with(|| value.to_owned());
                }
            }
        }
    }
}

/// The names in a file of the keys of hosts met before: those that are
/// written out, and are names and not addresses.
fn read_known(file: &Path, hosts: &mut Vec<SshHost>) {
    let Ok(known) = std::fs::read_to_string(file) else {
        return;
    };
    let mut seen: HashSet<String> = hosts.iter().map(|host| host.name.clone()).collect();
    for line in known.lines() {
        let mut fields = line.split_whitespace();
        let names = match fields.next() {
            // A marker, such as `@cert-authority`, comes before the names.
            Some(marker) if marker.starts_with('@') => fields.next(),
            first => first,
        };
        for name in names.unwrap_or("").split(',') {
            // `[host]:port` for a port that is not the usual one.
            let name = match name.strip_prefix('[') {
                Some(rest) => rest.split(']').next().unwrap_or(""),
                None => name,
            };
            let plain = !name.is_empty()
                && !name.starts_with(['|', '#'])
                && !name.contains(['*', '?', '!'])
                && name.parse::<std::net::IpAddr>().is_err();
            if plain && seen.insert(name.to_owned()) {
                hosts.push(SshHost {
                    name: name.to_owned(),
                    only_known: true,
                    ..SshHost::default()
                });
            }
        }
    }
}

/// The hosts that the `ssh` of a user knows by name, given the directory
/// that holds its configuration: those of `config` and the files it
/// includes, then those of `known_hosts`.
pub fn ssh_hosts_in(ssh_directory: &Path) -> Vec<SshHost> {
    let mut hosts = Vec::new();
    read_config(&ssh_directory.join("config"), ssh_directory, 0, &mut hosts);
    read_known(&ssh_directory.join("known_hosts"), &mut hosts);
    hosts
}

/// The hosts that the user's `ssh` knows by name: see [`ssh_hosts_in`].
pub fn ssh_hosts() -> Vec<SshHost> {
    std::env::var_os("HOME").map_or_else(Vec::new, |home| {
        ssh_hosts_in(&Path::new(&home).join(".ssh"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_configuration_of_ssh_is_read_with_what_it_includes() {
        let home = tempfile::tempdir().unwrap();
        let ssh = home.path().join(".ssh");
        std::fs::create_dir_all(ssh.join("conf.d")).unwrap();
        std::fs::write(
            ssh.join("config"),
            "# mine\nInclude conf.d/*.conf ~/elsewhere\n\
             Host evo\n  HostName evo.example.org\n  User albin\n  Port=2222\n  ProxyJump gate\n\
             \n  user ignored\n\
             host  pi  nas \"quoted\"\n  User pi\n\
             Host *.example.org !bad *\n  User everyone\n\
             Host=equals\nMatch host evo\n  User matched\nHost evo\n  Port 1\n",
        )
        .unwrap();
        std::fs::write(ssh.join("conf.d/b.conf"), "Host second\n").unwrap();
        std::fs::write(
            ssh.join("conf.d/a.conf"),
            "Host first\n  HostName 10.0.0.1\n",
        )
        .unwrap();
        std::fs::write(ssh.join("conf.d/skipped"), "Host never\n").unwrap();
        std::fs::write(home.path().join("elsewhere"), "Host far\n").unwrap();
        std::fs::write(
            ssh.join("known_hosts"),
            "evo ssh-ed25519 AAAA\n|1|hashed|more ssh-ed25519 AAAA\n\
             met.example.org,192.168.1.2 ssh-ed25519 AAAA\n[odd.example.org]:2222 ssh-rsa AAAA\n\
             @cert-authority *.example.org,ca.example.org ssh-rsa AAAA\n# note\n::1 ssh-rsa AAAA\n",
        )
        .unwrap();

        let hosts = ssh_hosts_in(&ssh);
        let names: Vec<&str> = hosts.iter().map(|host| host.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "first",
                "second",
                "far",
                "evo",
                "pi",
                "nas",
                "quoted",
                "equals",
                "met.example.org",
                "odd.example.org",
                "ca.example.org"
            ]
        );
        let evo = &hosts[3];
        assert_eq!(evo.host_name.as_deref(), Some("evo.example.org"));
        assert_eq!(evo.user.as_deref(), Some("albin"));
        assert_eq!(evo.port.as_deref(), Some("2222"));
        assert_eq!(evo.jump.as_deref(), Some("gate"));
        assert_eq!(hosts[0].host_name.as_deref(), Some("10.0.0.1"));
        assert_eq!(hosts[4].user.as_deref(), Some("pi"));
        assert_eq!(
            hosts[7],
            SshHost {
                name: "equals".to_owned(),
                ..SshHost::default()
            }
        );
        assert!(hosts[8].only_known && !hosts[7].only_known);
        assert_eq!(ssh_hosts_in(&home.path().join("none")), []);

        assert!(wildcard("*.conf", "a.conf") && !wildcard("*.conf", "a.conf.bak"));
        assert!(wildcard("a?c*", "abcde") && wildcard("*", "") && !wildcard("a?", "a"));
        assert!(wildcard("*a*b", "xxaxxbxxab") && !wildcard("*a*b", "xxaxxbx"));
    }
}
