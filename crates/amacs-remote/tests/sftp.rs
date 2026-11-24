//! The SFTP client against the `sftp-server` of this machine, over pipes:
//! no network and no daemon.

use std::fs;
use std::io::{ErrorKind, Read};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::process::Command;

use amacs_remote::{FileType, Sftp, sftp_server};

fn connect() -> Sftp {
    let server = sftp_server().expect("OpenSSH's sftp-server is on this machine");
    Sftp::spawn(Command::new(server)).expect("sftp-server starts")
}

fn shell(script: &str) -> std::io::Result<Sftp> {
    let mut command = Command::new("sh");
    command.args(["-c", script]);
    Sftp::spawn(command)
}

/// Bytes that do not repeat at the size of a piece.
fn pattern(length: usize) -> Vec<u8> {
    (0..length).map(|at| (at % 251) as u8).collect()
}

#[test]
fn looks_at_files_directories_and_links() {
    let dir = tempfile::tempdir().unwrap();
    let sftp = connect();
    fs::write(dir.path().join("file"), "twelve bytes").unwrap();
    fs::create_dir(dir.path().join("sub")).unwrap();
    symlink("file", dir.path().join("link")).unwrap();
    symlink("nowhere", dir.path().join("dangling")).unwrap();

    let file = sftp.metadata(&dir.path().join("file")).unwrap();
    assert_eq!(file.file_type, FileType::File);
    assert_eq!(file.len, 12);
    assert_eq!(
        file.modified,
        // SFTP carries whole seconds.
        fs::metadata(dir.path().join("file"))
            .unwrap()
            .modified()
            .ok()
            .map(|time| {
                let since = time.duration_since(std::time::UNIX_EPOCH).unwrap();
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(since.as_secs())
            })
    );
    assert!(sftp.metadata(&dir.path().join("sub")).unwrap().is_dir());
    assert!(sftp.metadata(&dir.path().join("link")).unwrap().is_file());
    assert!(
        sftp.symlink_metadata(&dir.path().join("link"))
            .unwrap()
            .is_symlink()
    );
    assert_eq!(
        sftp.read_link(&dir.path().join("link")).unwrap(),
        Path::new("file")
    );
    assert_eq!(
        sftp.canonicalize(&dir.path().join("sub/../link")).unwrap(),
        fs::canonicalize(dir.path()).unwrap().join("file")
    );

    let missing = sftp.metadata(&dir.path().join("none")).unwrap_err();
    assert_eq!(missing.kind(), ErrorKind::NotFound);
    assert_eq!(missing.to_string(), "No such file or directory");
    let dangling = sftp.metadata(&dir.path().join("dangling")).unwrap_err();
    assert_eq!(dangling.kind(), ErrorKind::NotFound);
    assert!(
        sftp.symlink_metadata(&dir.path().join("dangling"))
            .unwrap()
            .is_symlink()
    );
}

#[test]
fn lists_a_directory() {
    let dir = tempfile::tempdir().unwrap();
    let sftp = connect();
    assert_eq!(sftp.read_dir(dir.path()).unwrap(), []);
    for number in 0..300 {
        fs::write(dir.path().join(format!("file-{number:03}")), "").unwrap();
    }
    fs::create_dir(dir.path().join("sub")).unwrap();
    symlink("sub", dir.path().join("link")).unwrap();

    let mut entries = sftp.read_dir(dir.path()).unwrap();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(entries.len(), 302);
    assert_eq!(entries[0].name, "file-000");
    assert!(entries[0].metadata.unwrap().is_file());
    assert_eq!(entries[300].name, "link");
    assert!(entries[300].metadata.unwrap().is_symlink());
    assert_eq!(entries[301].name, "sub");
    assert!(entries[301].metadata.unwrap().is_dir());

    let error = sftp.read_dir(&dir.path().join("none")).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::NotFound);
}

#[test]
fn reads_files_of_every_size() {
    let dir = tempfile::tempdir().unwrap();
    let sftp = connect();
    let piece = 32 * 1024;
    for length in [
        0,
        1,
        piece - 1,
        piece,
        piece + 1,
        3 * piece,
        40 * piece + 17,
    ] {
        let path = dir.path().join(format!("file-{length}"));
        fs::write(&path, pattern(length)).unwrap();
        assert!(
            sftp.read(&path).unwrap() == pattern(length),
            "{length} bytes"
        );
    }

    // The start of a file, in small bites.
    let path = dir.path().join(format!("file-{}", 3 * piece));
    let mut start: Vec<u8> = Vec::new();
    let mut reader = sftp.open(&path).unwrap();
    let mut bite = [0; 1000];
    for _ in 0..40 {
        let count = reader.read(&mut bite).unwrap();
        start.extend(&bite[..count]);
    }
    assert!(start == pattern(3 * piece)[..start.len()]);
    assert!(start.len() > piece);
    // A file can be left before its end, and others read meanwhile.
    assert_eq!(sftp.read(&dir.path().join("file-1")).unwrap(), [0]);
    drop(reader);

    let error = sftp.read(&dir.path().join("none")).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::NotFound);
    assert!(sftp.read(dir.path()).is_err());
}

#[test]
fn writes_over_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let sftp = connect();
    let path = dir.path().join("file");
    for length in [5, 40 * 32 * 1024 + 17, 0, 3] {
        sftp.write(&path, &pattern(length)).unwrap();
        assert!(
            fs::read(&path).unwrap() == pattern(length),
            "{length} bytes"
        );
    }
    let error = sftp.write(&dir.path().join("none/file"), b"").unwrap_err();
    assert_eq!(error.kind(), ErrorKind::NotFound);
}

#[test]
fn replaces_a_file_whole_or_not_at_all() {
    let dir = tempfile::tempdir().unwrap();
    let sftp = connect();
    let path = dir.path().join("file");
    let names = || {
        let mut names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        names
    };

    // A new file.
    sftp.write_atomically(&path, &mut |out| out.write_all(b"first"))
        .unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"first");

    // The permissions of the old file are those of the new, also the
    // ones that a umask would take away.
    fs::set_permissions(&path, fs::Permissions::from_mode(0o757)).unwrap();
    let long = pattern(20 * 32 * 1024 + 5);
    sftp.write_atomically(&path, &mut |out| {
        // In several goes, as a rope writes itself.
        long.chunks(1000).try_for_each(|chunk| out.write_all(chunk))
    })
    .unwrap();
    assert!(fs::read(&path).unwrap() == long);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
        0o757
    );

    // A failure half way leaves the old file and nothing beside it.
    let error = sftp
        .write_atomically(&path, &mut |out| {
            out.write_all(b"half")?;
            Err(std::io::Error::other("stop"))
        })
        .unwrap_err();
    assert_eq!(error.to_string(), "stop");
    assert!(fs::read(&path).unwrap() == long);
    assert_eq!(names(), ["file"]);

    // A link is written through.
    let link = dir.path().join("link");
    symlink("file", &link).unwrap();
    sftp.write_atomically(&link, &mut |out| out.write_all(b"through"))
        .unwrap();
    assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
    assert_eq!(fs::read(&path).unwrap(), b"through");
    assert_eq!(names(), ["file", "link"]);

    let error = sftp
        .write_atomically(&dir.path().join("none/file"), &mut |out| out.write_all(b""))
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::NotFound);
}

#[test]
fn renames_makes_and_deletes() {
    let dir = tempfile::tempdir().unwrap();
    let sftp = connect();
    let at = |name: &str| dir.path().join(name);

    fs::write(at("a"), "a").unwrap();
    fs::write(at("b"), "b").unwrap();
    sftp.rename(&at("a"), &at("c")).unwrap();
    assert_eq!(fs::read(at("c")).unwrap(), b"a");
    // Onto a name that is taken.
    sftp.rename(&at("c"), &at("b")).unwrap();
    assert_eq!(fs::read(at("b")).unwrap(), b"a");
    assert!(!at("c").exists());
    assert_eq!(
        sftp.rename(&at("none"), &at("d")).unwrap_err().kind(),
        ErrorKind::NotFound
    );

    sftp.create_dir_all(&at("one/two/three")).unwrap();
    assert!(at("one/two/three").is_dir());
    sftp.create_dir_all(&at("one/two")).unwrap();
    assert!(sftp.create_dir_all(&at("b/under-a-file")).is_err());

    sftp.remove_file(&at("b")).unwrap();
    assert!(!at("b").exists());
    assert_eq!(
        sftp.remove_file(&at("b")).unwrap_err().kind(),
        ErrorKind::NotFound
    );

    // What a link in a deleted directory leads to stays.
    fs::create_dir(at("kept")).unwrap();
    fs::write(at("kept/file"), "kept").unwrap();
    fs::write(at("one/two/file"), "").unwrap();
    symlink(at("kept"), at("one/two/link")).unwrap();
    symlink(at("kept/file"), at("one/file-link")).unwrap();
    assert!(sftp.remove_dir(&at("one")).is_err());
    sftp.remove_dir_all(&at("one")).unwrap();
    assert!(!at("one").exists());
    assert_eq!(fs::read(at("kept/file")).unwrap(), b"kept");
}

#[test]
fn a_relative_path_is_from_where_the_server_started() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("file"), "here").unwrap();
    let mut command = Command::new(sftp_server().unwrap());
    command.current_dir(dir.path());
    let sftp = Sftp::spawn(command).unwrap();
    assert_eq!(sftp.read(Path::new("file")).unwrap(), b"here");
    assert_eq!(
        sftp.canonicalize(Path::new("")).unwrap(),
        fs::canonicalize(dir.path()).unwrap()
    );
    assert_eq!(sftp.read_dir(Path::new("")).unwrap().len(), 1);
}

#[test]
fn a_program_that_fails_says_why() {
    let error = shell("echo 'ssh: Could not resolve hostname nowhere' >&2; exit 255")
        .err()
        .expect("nothing greets");
    assert_eq!(error.to_string(), "ssh: Could not resolve hostname nowhere");

    let error = shell("exit 1").err().expect("nothing greets");
    assert_eq!(error.to_string(), "the connection to the host closed");

    // A shell that says hello before the server starts.
    let error = shell("echo Welcome to this host; cat >/dev/null")
        .err()
        .expect("a greeting of the wrong kind");
    assert_eq!(
        error.to_string(),
        "the host's answer is not SFTP: it starts with \"Welc\""
    );
}

#[test]
fn a_lost_connection_fails_every_call() {
    let dir = tempfile::tempdir().unwrap();
    let server = sftp_server().unwrap();
    // A server that ends after a second.
    let sftp = shell(&format!("timeout 1 {}", server.display())).unwrap();
    assert!(sftp.metadata(dir.path()).unwrap().is_dir());
    std::thread::sleep(std::time::Duration::from_millis(1500));
    assert!(sftp.metadata(dir.path()).is_err());
    let error = sftp.metadata(dir.path()).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::NotConnected);
}

#[test]
fn threads_share_a_connection() {
    let dir = tempfile::tempdir().unwrap();
    let sftp = connect();
    for number in 0..4 {
        fs::write(
            dir.path().join(number.to_string()),
            pattern(300_000 + number),
        )
        .unwrap();
    }
    std::thread::scope(|scope| {
        for number in 0..4 {
            let (sftp, dir) = (&sftp, dir.path());
            scope.spawn(move || {
                for _ in 0..5 {
                    let read = sftp.read(&dir.join(number.to_string())).unwrap();
                    assert!(read == pattern(300_000 + number));
                }
            });
        }
    });
}

#[test]
fn a_host_that_does_not_answer_is_not_waited_for() {
    use std::time::{Duration, Instant};

    let dir = tempfile::tempdir().unwrap();
    let server = sftp_server().unwrap();
    let pid_file = dir.path().join("pid");
    // A server that can be stopped and let go on.
    let sftp = shell(&format!(
        "{} <&0 & echo $! > {}; wait",
        server.display(),
        pid_file.display()
    ))
    .unwrap();
    assert!(sftp.metadata(dir.path()).unwrap().is_dir());
    // The shell writes the number a moment after the server is there.
    let pid = loop {
        match fs::read_to_string(&pid_file) {
            Ok(pid) if pid.ends_with('\n') => break pid,
            _ => std::thread::sleep(Duration::from_millis(2)),
        }
    };
    let signal = |name: &str| {
        let status = Command::new("kill")
            .args([name, pid.trim()])
            .status()
            .unwrap();
        assert!(status.success());
    };

    signal("-STOP");
    let asked = Instant::now();
    let error =
        amacs_remote::with_patience(Duration::from_millis(200), || sftp.metadata(dir.path()))
            .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::TimedOut);
    assert_eq!(error.to_string(), "the host is not answering");
    assert!(asked.elapsed() < Duration::from_secs(2));

    // The connection is as good as before, and the late answer is not
    // taken for another.
    signal("-CONT");
    fs::write(dir.path().join("file"), "later").unwrap();
    assert_eq!(sftp.read(&dir.path().join("file")).unwrap(), b"later");
    assert!(!sftp.is_lost());
}
