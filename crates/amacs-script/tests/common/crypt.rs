//! Keys for the tests of what is encrypted: a home for gpg and keys for
//! age that are the test's own, never the user's.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::Harness;

/// A home for gpg with a key in it, away from the user's own. Its path is
/// short: the agent's socket is in it, and a socket's name has a limit.
pub struct GpgHome {
    dir: tempfile::TempDir,
}

impl GpgHome {
    pub fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("g")
            .tempdir_in("/tmp")
            .unwrap();
        std::fs::set_permissions(
            dir.path(),
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .unwrap();
        Self { dir }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    fn gpg(&self) -> Command {
        let mut command = Command::new("gpg");
        command
            .env("GNUPGHOME", self.path())
            .args(["--batch", "--quiet"]);
        command
    }

    /// Makes a key for a name, with a passphrase or with none.
    pub fn key(&self, name: &str, passphrase: &str) {
        let made = self
            .gpg()
            .args(["--pinentry-mode", "loopback", "--passphrase", passphrase])
            .args(["--quick-generate-key", name, "default", "default", "never"])
            .output()
            .unwrap();
        assert!(
            made.status.success(),
            "{}",
            String::from_utf8_lossy(&made.stderr)
        );
    }

    /// Makes the agent forget the passphrases it has.
    pub fn forget(&self) {
        let _ = Command::new("gpgconf")
            .env("GNUPGHOME", self.path())
            .args(["--reload", "gpg-agent"])
            .output();
    }

    /// Tells an editor to use this home.
    pub fn used_by(&self, h: &mut Harness) {
        h.eval(&format!(
            "(set-option! 'crypt-environment '((\"GNUPGHOME\" {:?})))",
            self.path().display().to_string()
        ));
    }
}

impl Drop for GpgHome {
    fn drop(&mut self) {
        let _ = Command::new("gpgconf")
            .env("GNUPGHOME", self.path())
            .args(["--kill", "all"])
            .output();
    }
}

/// Evaluates an expression that waits for a program, and returns its
/// value as written, or `(error "message")` if it failed.
pub fn wait_for(h: &mut Harness, expression: &str) -> String {
    h.eval("(define %test-out '%pending)");
    h.eval(&format!(
        "(run-later (lambda ()
           (set! %test-out
                 (guard (e (#t (list 'error (if (error-object? e) (error-object-message e) e))))
                   {expression}))))"
    ));
    h.wait_until(|h| h.eval("%test-out") != "%pending");
    h.eval("%test-out")
}

/// An age key in a file, and its public half.
pub fn age_key(dir: &Path, name: &str) -> (PathBuf, String) {
    let file = dir.join(name);
    let made = Command::new("age-keygen")
        .arg("-o")
        .arg(&file)
        .output()
        .unwrap();
    assert!(made.status.success());
    let public = std::fs::read_to_string(&file)
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("# public key: ").map(str::to_owned))
        .unwrap();
    (file, public)
}

/// A file name as Scheme reads a string.
pub fn quoted(path: &Path) -> String {
    format!("{:?}", path.display().to_string())
}
