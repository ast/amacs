//! A client of SFTP, version 3: the protocol of `sftp-server`, which every
//! OpenSSH has.
//!
//! The client speaks through two pipes and does not care what is at their
//! other end: `ssh -s host sftp`, `sudo sftp-server`, or `sftp-server`
//! itself in the tests. Requests carry numbers and so do their answers,
//! so that threads share a connection without waiting for each other, and
//! the contents of a file are read and written a few pieces at a time: a
//! slow line costs one wait for several pieces.
//!
//! Nobody waits for a host that has stopped answering: a request gives up
//! after the patience of its thread, see [`with_patience`].

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::metadata::{DirEntry, FileType, Metadata};

const VERSION: u32 = 3;

const INIT: u8 = 1;
const VERSION_REPLY: u8 = 2;
const OPEN: u8 = 3;
const CLOSE: u8 = 4;
const READ: u8 = 5;
const WRITE: u8 = 6;
const LSTAT: u8 = 7;
const FSETSTAT: u8 = 10;
const OPENDIR: u8 = 11;
const READDIR: u8 = 12;
const REMOVE: u8 = 13;
const MKDIR: u8 = 14;
const RMDIR: u8 = 15;
const REALPATH: u8 = 16;
const STAT: u8 = 17;
const RENAME: u8 = 18;
const READLINK: u8 = 19;
const STATUS: u8 = 101;
const HANDLE: u8 = 102;
const DATA: u8 = 103;
const NAME: u8 = 104;
const ATTRS: u8 = 105;
const EXTENDED: u8 = 200;

const FX_OK: u32 = 0;
const FX_EOF: u32 = 1;
const FX_NO_SUCH_FILE: u32 = 2;
const FX_PERMISSION_DENIED: u32 = 3;
const FX_OP_UNSUPPORTED: u32 = 8;

const OPEN_READ: u32 = 0x01;
const OPEN_WRITE: u32 = 0x02;
const OPEN_CREATE: u32 = 0x08;
const OPEN_TRUNCATE: u32 = 0x10;
const OPEN_EXCLUSIVE: u32 = 0x20;

const ATTR_SIZE: u32 = 0x01;
const ATTR_OWNER: u32 = 0x02;
const ATTR_PERMISSIONS: u32 = 0x04;
const ATTR_TIMES: u32 = 0x08;
const ATTR_EXTENDED: u32 = 0x8000_0000;

/// Renaming onto a name that is taken, as `rename(2)` does it.
const POSIX_RENAME: &str = "posix-rename@openssh.com";
const FSYNC: &str = "fsync@openssh.com";

/// How much of a file one request reads or writes. Every server takes
/// this much.
const PIECE: usize = 32 * 1024;
/// How many pieces of a file are asked for or sent before the answers are
/// read.
const PIECES_AT_ONCE: usize = 8;
/// No answer is this long; a length above it is not SFTP.
const LONGEST_PACKET: usize = 1 << 20;

/// A request being put together: its length, kind and number, then what
/// the kind calls for.
struct Packet(Vec<u8>);

impl Packet {
    fn new(kind: u8, id: u32) -> Self {
        let mut bytes = vec![0; 4];
        bytes.push(kind);
        bytes.extend(id.to_be_bytes());
        Self(bytes)
    }

    fn u32(mut self, value: u32) -> Self {
        self.0.extend(value.to_be_bytes());
        self
    }

    fn u64(mut self, value: u64) -> Self {
        self.0.extend(value.to_be_bytes());
        self
    }

    fn bytes(mut self, bytes: &[u8]) -> Self {
        self.0.extend((bytes.len() as u32).to_be_bytes());
        self.0.extend(bytes);
        self
    }

    /// A path. The empty one is the directory that the server starts in,
    /// which is the user's home.
    fn path(self, path: &Path) -> Self {
        match path.as_os_str().as_bytes() {
            [] => self.bytes(b"."),
            bytes => self.bytes(bytes),
        }
    }

    /// Attributes that set nothing, or only the permissions.
    fn attrs(self, mode: Option<u32>) -> Self {
        match mode {
            Some(mode) => self.u32(ATTR_PERMISSIONS).u32(mode),
            None => self.u32(0),
        }
    }

    fn finish(mut self) -> Vec<u8> {
        let length = (self.0.len() - 4) as u32;
        self.0[..4].copy_from_slice(&length.to_be_bytes());
        self.0
    }
}

/// The attributes of a file as the protocol carries them: each is there
/// or not.
#[derive(Default)]
struct Attrs {
    size: Option<u64>,
    mode: Option<u32>,
    modified: Option<u32>,
}

impl Attrs {
    fn metadata(&self) -> Metadata {
        Metadata {
            file_type: self.mode.map_or(FileType::Other, FileType::from_mode),
            len: self.size.unwrap_or(0),
            modified: self
                .modified
                .map(|seconds| UNIX_EPOCH + Duration::from_secs(seconds.into())),
            mode: self.mode.unwrap_or(0) & 0o7777,
        }
    }
}

fn malformed(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("the host's answer is not SFTP: {what}"),
    )
}

/// An answer being taken apart.
struct Reply<'a>(&'a [u8]);

impl<'a> Reply<'a> {
    fn take(&mut self, count: usize) -> io::Result<&'a [u8]> {
        if self.0.len() < count {
            return Err(malformed("it ends early"));
        }
        let (taken, rest) = self.0.split_at(count);
        self.0 = rest;
        Ok(taken)
    }

    fn u32(&mut self) -> io::Result<u32> {
        let bytes = self.take(4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from(self.u32()?) << 32 | u64::from(self.u32()?))
    }

    fn bytes(&mut self) -> io::Result<&'a [u8]> {
        let length = self.u32()? as usize;
        self.take(length)
    }

    fn attrs(&mut self) -> io::Result<Attrs> {
        let flags = self.u32()?;
        let mut attrs = Attrs::default();
        if flags & ATTR_SIZE != 0 {
            attrs.size = Some(self.u64()?);
        }
        if flags & ATTR_OWNER != 0 {
            self.take(8)?;
        }
        if flags & ATTR_PERMISSIONS != 0 {
            attrs.mode = Some(self.u32()?);
        }
        if flags & ATTR_TIMES != 0 {
            self.u32()?;
            attrs.modified = Some(self.u32()?);
        }
        if flags & ATTR_EXTENDED != 0 {
            for _ in 0..self.u32()? {
                self.bytes()?;
                self.bytes()?;
            }
        }
        Ok(attrs)
    }

    /// The error that a status stands for, or `None` for success. The
    /// end of a file is an error here; those who expect it look for it.
    fn status(&mut self) -> io::Result<Option<io::Error>> {
        let code = self.u32()?;
        // Version 3 has a message; be content without one.
        let message = self
            .bytes()
            .map(|message| String::from_utf8_lossy(message).into_owned())
            .unwrap_or_default();
        let kind = match code {
            FX_OK => return Ok(None),
            FX_EOF => io::ErrorKind::UnexpectedEof,
            FX_NO_SUCH_FILE => io::ErrorKind::NotFound,
            FX_PERMISSION_DENIED => io::ErrorKind::PermissionDenied,
            FX_OP_UNSUPPORTED => io::ErrorKind::Unsupported,
            _ => io::ErrorKind::Other,
        };
        let message = match (code, message.is_empty()) {
            // What `std` says of the same, for local and remote files to
            // read alike.
            (FX_NO_SUCH_FILE, _) => "No such file or directory".to_owned(),
            (FX_PERMISSION_DENIED, _) => "Permission denied".to_owned(),
            (_, true) => format!("the host refused, with code {code}"),
            // OpenSSH has one word for most things that go wrong.
            (_, false) if message == "Failure" => "the host refused".to_owned(),
            (_, false) => message,
        };
        Ok(Some(io::Error::new(kind, message)))
    }
}

/// How long a request waits for its answer, unless [`with_patience`]
/// says otherwise: long enough for a slow line, short enough to notice a
/// dead one.
const PATIENCE: Duration = Duration::from_secs(30);

thread_local! {
    static PATIENCE_HERE: std::cell::Cell<Duration> = const { std::cell::Cell::new(PATIENCE) };
}

/// Runs `action` with requests of this thread waiting at most `patience`
/// for an answer, and for a connection to be made. A request that waits
/// longer fails as timed out; the connection stays, and the answer is
/// thrown away when it comes.
pub fn with_patience<T>(patience: Duration, action: impl FnOnce() -> T) -> T {
    let before = PATIENCE_HERE.replace(patience);
    let result = action();
    PATIENCE_HERE.set(before);
    result
}

/// How long requests of this thread wait.
pub(crate) fn patience() -> Duration {
    PATIENCE_HERE.get()
}

/// The program at the other end, when this client started it.
struct Program {
    child: Mutex<Child>,
    /// The end of what it has written to its standard error: where `ssh`
    /// says why it could not connect.
    complaints: Mutex<Vec<u8>>,
}

impl Program {
    /// What the program said last, once it has ended, or shortly after
    /// if it does not end.
    fn last_words(&self) -> Option<String> {
        for _ in 0..20 {
            let mut child = self.child.lock().unwrap_or_else(PoisonError::into_inner);
            if matches!(child.try_wait(), Ok(Some(_)) | Err(_)) {
                break;
            }
            drop(child);
            std::thread::sleep(Duration::from_millis(10));
        }
        // The thread that reads may be a moment behind the exit.
        std::thread::sleep(Duration::from_millis(10));
        let complaints = self
            .complaints
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let text = String::from_utf8_lossy(&complaints);
        let last = text.lines().rev().find(|line| !line.trim().is_empty())?;
        Some(last.trim().to_owned())
    }

    fn end(&self) {
        let mut child = self.child.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Says why the pipes failed, in the words of the program if it left any.
fn explain(program: Option<&Program>, error: io::Error) -> io::Error {
    match program.and_then(Program::last_words) {
        Some(words) => io::Error::new(error.kind(), words),
        None if error.kind() == io::ErrorKind::UnexpectedEof => io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "the connection to the host closed",
        ),
        None => error,
    }
}

/// Reads one answer: its kind and the rest of it.
fn read_packet(input: &mut dyn Read) -> io::Result<(u8, Vec<u8>)> {
    let mut length = [0; 4];
    input.read_exact(&mut length)?;
    let size = u32::from_be_bytes(length) as usize;
    if size == 0 || size > LONGEST_PACKET {
        // A shell that prints a greeting, most likely.
        let start: String = length
            .iter()
            .filter(|byte| byte.is_ascii_graphic() || **byte == b' ')
            .map(|byte| char::from(*byte))
            .collect();
        return Err(malformed(&format!("it starts with {start:?}")));
    }
    let mut body = vec![0; size];
    input.read_exact(&mut body)?;
    let kind = body.remove(0);
    Ok((kind, body))
}

/// An answer on its way to whoever asked: the number of the request, the
/// kind, and the rest.
type Answer = (u32, u8, Vec<u8>);

/// What the thread that reads answers shares with those who ask.
#[derive(Default)]
struct Waiting {
    /// Who waits for the answer to which request.
    requests: HashMap<u32, mpsc::Sender<Answer>>,
    /// Why the connection is of no more use, once it is not.
    lost: Option<(io::ErrorKind, String)>,
}

/// The error for an answer that should have been something else.
fn refusal(kind: u8, body: &[u8]) -> io::Error {
    match status(kind, body) {
        Ok(Some(error)) | Err(error) => error,
        Ok(None) => malformed("it is success where something was asked for"),
    }
}

/// The error of a status answer, `None` for success; an answer of another
/// kind where a status belongs is an error too.
fn status(kind: u8, body: &[u8]) -> io::Result<Option<io::Error>> {
    if kind != STATUS {
        return Err(malformed("it is of the wrong kind"));
    }
    Reply(body).status()
}

/// A connection to the files of a host.
///
/// Paths are those of the host. One that is not absolute is taken from
/// the directory that the server started in, which is the home directory
/// of the user there.
///
/// Threads share a connection, and their requests are under way at the
/// same time. No call waits for the host longer than the patience of its
/// thread, see [`with_patience`].
pub struct Sftp {
    waiting: Arc<Mutex<Waiting>>,
    /// Bytes for the thread that writes them to the program, so that
    /// nobody who asks waits for a pipe that is full.
    outgoing: mpsc::Sender<Vec<u8>>,
    next_id: AtomicU32,
    program: Option<Arc<Program>>,
    posix_rename: bool,
    fsync: bool,
}

impl Sftp {
    /// Greets a server over a pair of pipes.
    pub fn new(
        input: impl Read + Send + 'static,
        output: impl Write + Send + 'static,
    ) -> io::Result<Self> {
        Self::greet(Box::new(input), Box::new(output), None)
    }

    /// Starts a program that speaks SFTP on its standard input and
    /// output, and greets it. The program is ended when the client is
    /// dropped. If it ends by itself, the last line that it wrote to its
    /// standard error is the error that calls give.
    pub fn spawn(mut command: Command) -> io::Result<Self> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let (Some(output), Some(input), Some(mut errors)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(io::Error::other("the program has no pipes"));
        };
        let program = Arc::new(Program {
            child: Mutex::new(child),
            complaints: Mutex::new(Vec::new()),
        });
        let kept = Arc::downgrade(&program);
        // Left to itself: it ends when the last holder of the pipe does,
        // which may be a program that outlives this one.
        std::thread::Builder::new()
            .name("sftp-stderr".to_owned())
            .spawn(move || {
                let mut piece = [0; 1024];
                while let Ok(count @ 1..) = errors.read(&mut piece) {
                    let Some(program) = kept.upgrade() else {
                        break;
                    };
                    let mut kept = program
                        .complaints
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner);
                    kept.extend(&piece[..count]);
                    let excess = kept.len().saturating_sub(4096);
                    kept.drain(..excess);
                }
            })?;
        let greeted = Self::greet(Box::new(input), Box::new(output), Some(program.clone()));
        if greeted.is_err() {
            program.end();
        }
        greeted
    }

    fn greet(
        mut input: Box<dyn Read + Send>,
        mut output: Box<dyn Write + Send>,
        program: Option<Arc<Program>>,
    ) -> io::Result<Self> {
        let why = |error| explain(program.as_deref(), error);
        // The greeting has a version where requests have a number.
        output
            .write_all(&Packet::new(INIT, VERSION).finish())
            .and_then(|()| output.flush())
            .map_err(why)?;
        let (kind, body) = read_packet(&mut input).map_err(why)?;
        if kind != VERSION_REPLY {
            return Err(malformed("it is no greeting"));
        }
        let mut reply = Reply(&body);
        let version = reply.u32()?;
        if version < VERSION {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("the host speaks version {version} of SFTP, which is too old"),
            ));
        }
        let (mut posix_rename, mut fsync) = (false, false);
        while let (Ok(name), Ok(_)) = (reply.bytes(), reply.bytes()) {
            posix_rename |= name == POSIX_RENAME.as_bytes();
            fsync |= name == FSYNC.as_bytes();
        }

        let waiting = Arc::new(Mutex::new(Waiting::default()));
        let (outgoing, to_write) = mpsc::channel::<Vec<u8>>();
        // Ends when the client is dropped, and the program's input with
        // it, which ends a server.
        std::thread::Builder::new()
            .name("sftp-write".to_owned())
            .spawn(move || {
                for bytes in to_write {
                    if output
                        .write_all(&bytes)
                        .and_then(|()| output.flush())
                        .is_err()
                    {
                        break;
                    }
                }
            })?;
        let (shared, ended) = (Arc::clone(&waiting), program.clone());
        // Ends when the program closes its output.
        std::thread::Builder::new()
            .name("sftp-read".to_owned())
            .spawn(move || {
                let error = loop {
                    match read_packet(&mut input) {
                        Ok((kind, mut body)) if body.len() >= 4 => {
                            let id = u32::from_be_bytes([body[0], body[1], body[2], body[3]]);
                            body.drain(..4);
                            let mut waiting = shared.lock().unwrap_or_else(PoisonError::into_inner);
                            // An answer that nobody waits for any more
                            // came too late.
                            if let Some(asker) = waiting.requests.remove(&id) {
                                let _ = asker.send((id, kind, body));
                            }
                        }
                        Ok(_) => break malformed("it ends early"),
                        Err(error) => break error,
                    }
                };
                let error = explain(ended.as_deref(), error);
                let mut waiting = shared.lock().unwrap_or_else(PoisonError::into_inner);
                waiting.lost = Some((error.kind(), error.to_string()));
                // Those who wait see that nobody will answer.
                waiting.requests.clear();
            })?;
        Ok(Self {
            waiting,
            outgoing,
            next_id: AtomicU32::new(0),
            program,
            posix_rename,
            fsync,
        })
    }

    /// Whether the connection is of no more use: the program ended, or
    /// what it wrote was not SFTP.
    pub fn is_lost(&self) -> bool {
        let waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
        waiting.lost.is_some()
    }

    fn next_id(&self) -> u32 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Sends requests that are already put together, with these numbers,
    /// and returns their answers in the order of the numbers.
    fn exchange(&self, ids: &[u32], requests: Vec<u8>) -> io::Result<Vec<(u8, Vec<u8>)>> {
        let (asker, answers) = mpsc::channel();
        {
            let mut waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
            if waiting.lost.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::NotConnected,
                    "the connection to the host is lost",
                ));
            }
            for id in ids {
                waiting.requests.insert(*id, asker.clone());
            }
        }
        drop(asker);
        // If the writer is gone, so is the reader soon, and the wait
        // below ends.
        let _ = self.outgoing.send(requests);
        let patience = patience();
        let mut ordered: Vec<Option<(u8, Vec<u8>)>> = vec![None; ids.len()];
        for _ in ids {
            match answers.recv_timeout(patience) {
                Ok((id, kind, body)) => {
                    let place = ids.iter().position(|asked| *asked == id);
                    ordered[place.expect("only what was asked is answered")] = Some((kind, body));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let mut waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
                    for id in ids {
                        waiting.requests.remove(id);
                    }
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "the host is not answering",
                    ));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
                    return Err(match &waiting.lost {
                        Some((kind, why)) => io::Error::new(*kind, why.clone()),
                        None => io::Error::new(
                            io::ErrorKind::ConnectionAborted,
                            "the connection to the host closed",
                        ),
                    });
                }
            }
        }
        Ok(ordered.into_iter().flatten().collect())
    }

    /// Sends one request and returns its answer.
    fn request(&self, build: impl FnOnce(u32) -> Packet) -> io::Result<(u8, Vec<u8>)> {
        let id = self.next_id();
        let mut answers = self.exchange(&[id], build(id).finish())?;
        answers.pop().ok_or_else(|| malformed("it is missing"))
    }

    /// A request that is answered with success or an error and no more.
    fn command(&self, build: impl FnOnce(u32) -> Packet) -> io::Result<()> {
        let (kind, body) = self.request(build)?;
        match status(kind, &body)? {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn attrs(&self, kind: u8, path: &Path) -> io::Result<Attrs> {
        let (kind, body) = self.request(|id| Packet::new(kind, id).path(path))?;
        match kind {
            ATTRS => Reply(&body).attrs(),
            _ => Err(refusal(kind, &body)),
        }
    }

    /// A request that is answered with one name.
    fn name(&self, kind: u8, path: &Path) -> io::Result<PathBuf> {
        let (kind, body) = self.request(|id| Packet::new(kind, id).path(path))?;
        if kind != NAME {
            return Err(refusal(kind, &body));
        }
        let mut reply = Reply(&body);
        if reply.u32()? != 1 {
            return Err(malformed("one name was asked for"));
        }
        Ok(PathBuf::from(OsString::from_vec(reply.bytes()?.to_owned())))
    }

    fn extended(&self, name: &str, build: impl FnOnce(Packet) -> Packet) -> io::Result<()> {
        self.command(|id| build(Packet::new(EXTENDED, id).bytes(name.as_bytes())))
    }

    /// What a file is, following links.
    pub fn metadata(&self, path: &Path) -> io::Result<Metadata> {
        Ok(self.attrs(STAT, path)?.metadata())
    }

    /// What a name is: of a link, the link itself.
    pub fn symlink_metadata(&self, path: &Path) -> io::Result<Metadata> {
        Ok(self.attrs(LSTAT, path)?.metadata())
    }

    /// Where a link points.
    pub fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        self.name(READLINK, path)
    }

    /// The absolute path of a file, with links followed. OpenSSH gives a
    /// path also when the last part of it is not there.
    pub fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        self.name(REALPATH, path)
    }

    /// What a directory holds, without `.` and `..`, in no order.
    pub fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        let handle = self.open_with(|id| Packet::new(OPENDIR, id).path(path))?;
        let mut entries = Vec::new();
        loop {
            let (kind, body) = self.request(|id| Packet::new(READDIR, id).bytes(&handle.id))?;
            if kind != NAME {
                match refusal(kind, &body) {
                    error if error.kind() == io::ErrorKind::UnexpectedEof => break,
                    error => return Err(error),
                }
            }
            let mut reply = Reply(&body);
            for _ in 0..reply.u32()? {
                let name = reply.bytes()?;
                // The line that `ls -l` would print.
                reply.bytes()?;
                let attrs = reply.attrs()?;
                if name == b"." || name == b".." {
                    continue;
                }
                entries.push(DirEntry {
                    name: OsString::from_vec(name.to_owned()),
                    metadata: attrs.mode.is_some().then(|| attrs.metadata()),
                });
            }
        }
        handle.close()?;
        Ok(entries)
    }

    /// Opens a file for reading from its start.
    pub fn open(&self, path: &Path) -> io::Result<FileReader<'_>> {
        let handle =
            self.open_with(|id| Packet::new(OPEN, id).path(path).u32(OPEN_READ).attrs(None))?;
        Ok(FileReader {
            handle,
            offset: 0,
            buffer: Vec::new(),
            taken: 0,
            pieces: 1,
            ended: false,
        })
    }

    /// The contents of a file.
    pub fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        let mut contents = Vec::new();
        self.open(path)?.read_to_end(&mut contents)?;
        Ok(contents)
    }

    /// Makes a file hold `contents` and nothing else, writing over what it
    /// held. A link is followed.
    pub fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        let flags = OPEN_WRITE | OPEN_CREATE | OPEN_TRUNCATE;
        let handle =
            self.open_with(|id| Packet::new(OPEN, id).path(path).u32(flags).attrs(None))?;
        let mut writer = FileWriter::new(handle);
        writer.write_all(contents)?;
        writer.finish(false)
    }

    /// Replaces a file by what `content` writes, so that the file is the
    /// old one or the new one whatever happens in between: the bytes go
    /// to a file beside it that is then renamed over it. The new file has
    /// the permissions of the old. A link is followed, not replaced.
    pub fn write_atomically(
        &self,
        path: &Path,
        content: &mut dyn FnMut(&mut dyn Write) -> io::Result<()>,
    ) -> io::Result<()> {
        let target = self.canonicalize(path).unwrap_or_else(|_| path.to_owned());
        let name = target
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
        // Others may be writing the same file on that host, from
        // processes whose numbers are not known here.
        let moment = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.subsec_nanos());
        let mut temp_name = OsString::from(".#");
        temp_name.push(name);
        temp_name.push(format!(".{}-{moment:08x}.tmp", std::process::id()));
        let temp = target.with_file_name(temp_name);
        let mode = self.metadata(&target).ok().map(|meta| meta.mode);

        let result = (|| {
            let flags = OPEN_WRITE | OPEN_CREATE | OPEN_EXCLUSIVE;
            let handle =
                self.open_with(|id| Packet::new(OPEN, id).path(&temp).u32(flags).attrs(mode))?;
            // The mode given on opening is cut down by the umask there.
            if mode.is_some() {
                self.command(|id| Packet::new(FSETSTAT, id).bytes(&handle.id).attrs(mode))?;
            }
            let mut writer = FileWriter::new(handle);
            content(&mut writer)?;
            writer.finish(true)?;
            self.rename(&temp, &target)
        })();
        if result.is_err() {
            // Best effort: the write error is what the user needs to see.
            let _ = self.remove_file(&temp);
        }
        result
    }

    /// Gives a file or directory another name, in place of whatever file
    /// has that name already.
    pub fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        if self.posix_rename {
            return self.extended(POSIX_RENAME, |packet| packet.path(from).path(to));
        }
        // The rename of version 3 does not replace. Without the extension
        // there is a moment with no file of the new name.
        let rename = || self.command(|id| Packet::new(RENAME, id).path(from).path(to));
        match rename() {
            Err(error) if self.symlink_metadata(to).is_ok_and(|meta| !meta.is_dir()) => {
                self.remove_file(to).map_err(|_| error)?;
                rename()
            }
            result => result,
        }
    }

    /// Deletes a file or a link.
    pub fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.command(|id| Packet::new(REMOVE, id).path(path))
    }

    /// Deletes a directory that holds nothing.
    pub fn remove_dir(&self, path: &Path) -> io::Result<()> {
        self.command(|id| Packet::new(RMDIR, id).path(path))
    }

    /// Deletes a directory and everything in it. Links in it are deleted,
    /// not followed.
    pub fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
        for entry in self.read_dir(path)? {
            let inner = path.join(&entry.name);
            // Asked of the name itself, whatever the listing said: what a
            // link to a directory leads to is not to be emptied.
            if self.symlink_metadata(&inner)?.is_dir() {
                self.remove_dir_all(&inner)?;
            } else {
                self.remove_file(&inner)?;
            }
        }
        self.remove_dir(path)
    }

    /// Creates a directory, and the directories above it that are
    /// missing. One that is there already is no error.
    pub fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        if self.metadata(path).is_ok_and(|meta| meta.is_dir()) {
            return Ok(());
        }
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            self.create_dir_all(parent)?;
        }
        let made = self.command(|id| Packet::new(MKDIR, id).path(path).attrs(None));
        match made {
            // Someone else may have made it meanwhile.
            Err(_) if self.metadata(path).is_ok_and(|meta| meta.is_dir()) => Ok(()),
            result => result,
        }
    }

    fn open_with(&self, build: impl FnOnce(u32) -> Packet) -> io::Result<Handle<'_>> {
        let (kind, body) = self.request(build)?;
        match kind {
            HANDLE => Ok(Handle {
                sftp: self,
                id: Reply(&body).bytes()?.to_owned(),
            }),
            _ => Err(refusal(kind, &body)),
        }
    }
}

impl Drop for Sftp {
    fn drop(&mut self) {
        // A server ends when its input does, which it does when the
        // thread that writes sees this client gone; a program that does
        // not end by itself is ended.
        if let Some(program) = &self.program {
            program.end();
        }
    }
}

/// An open file or directory on the host, closed when dropped.
struct Handle<'a> {
    sftp: &'a Sftp,
    /// The name that the server gave it. Empty once closed.
    id: Vec<u8>,
}

impl Handle<'_> {
    fn close(mut self) -> io::Result<()> {
        let id = std::mem::take(&mut self.id);
        self.sftp
            .command(|request| Packet::new(CLOSE, request).bytes(&id))
    }
}

impl Drop for Handle<'_> {
    fn drop(&mut self) {
        if !self.id.is_empty() {
            let id = std::mem::take(&mut self.id);
            let _ = self
                .sftp
                .command(|request| Packet::new(CLOSE, request).bytes(&id));
        }
    }
}

/// A file on the host being read from start to end.
pub struct FileReader<'a> {
    handle: Handle<'a>,
    /// Where in the file the next piece asked for starts.
    offset: u64,
    buffer: Vec<u8>,
    /// How much of the buffer has been handed out.
    taken: usize,
    /// How many pieces to ask for next. It grows as reading goes on, so
    /// that a look at the start of a file costs one request.
    pieces: usize,
    ended: bool,
}

impl FileReader<'_> {
    /// Asks for the next pieces of the file, all at once, and keeps what
    /// comes back up to the first gap.
    fn fill(&mut self) -> io::Result<()> {
        self.buffer.clear();
        self.taken = 0;
        let sftp = self.handle.sftp;
        let mut requests = Vec::new();
        let mut ids = Vec::new();
        for piece in 0..self.pieces {
            let id = sftp.next_id();
            ids.push(id);
            requests.extend(
                Packet::new(READ, id)
                    .bytes(&self.handle.id)
                    .u64(self.offset + (piece * PIECE) as u64)
                    .u32(PIECE as u32)
                    .finish(),
            );
        }
        for (kind, body) in sftp.exchange(&ids, requests)? {
            if kind != DATA {
                match refusal(kind, &body) {
                    error if error.kind() == io::ErrorKind::UnexpectedEof => self.ended = true,
                    error => return Err(error),
                }
                break;
            }
            let data = Reply(&body).bytes()?;
            self.buffer.extend(data);
            self.offset += data.len() as u64;
            // After a piece that came short, the pieces that follow are
            // not what follows in the file.
            if data.len() < PIECE {
                break;
            }
        }
        self.pieces = (self.pieces * 2).min(PIECES_AT_ONCE);
        Ok(())
    }
}

impl Read for FileReader<'_> {
    fn read(&mut self, into: &mut [u8]) -> io::Result<usize> {
        while self.taken == self.buffer.len() {
            if self.ended || into.is_empty() {
                return Ok(0);
            }
            self.fill()?;
        }
        let count = into.len().min(self.buffer.len() - self.taken);
        into[..count].copy_from_slice(&self.buffer[self.taken..self.taken + count]);
        self.taken += count;
        Ok(count)
    }
}

/// A file on the host being written from its start.
struct FileWriter<'a> {
    handle: Handle<'a>,
    offset: u64,
    /// Bytes not yet sent.
    pending: Vec<u8>,
}

impl<'a> FileWriter<'a> {
    fn new(handle: Handle<'a>) -> Self {
        Self {
            handle,
            offset: 0,
            pending: Vec::with_capacity(PIECE * PIECES_AT_ONCE),
        }
    }

    /// Sends what is pending, as pieces that are all under way before
    /// the first is answered.
    fn send(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let sftp = self.handle.sftp;
        let mut requests = Vec::with_capacity(self.pending.len() + 64 * PIECES_AT_ONCE);
        let mut ids = Vec::new();
        for piece in self.pending.chunks(PIECE) {
            let id = sftp.next_id();
            ids.push(id);
            requests.extend(
                Packet::new(WRITE, id)
                    .bytes(&self.handle.id)
                    .u64(self.offset)
                    .bytes(piece)
                    .finish(),
            );
            self.offset += piece.len() as u64;
        }
        self.pending.clear();
        for (kind, body) in sftp.exchange(&ids, requests)? {
            if let Some(error) = status(kind, &body)? {
                return Err(error);
            }
        }
        Ok(())
    }

    /// Sends the rest and closes the file. With `sync`, has the host put
    /// it on its disk first, if the host can be asked to.
    fn finish(mut self, sync: bool) -> io::Result<()> {
        self.send()?;
        if sync && self.handle.sftp.fsync {
            self.handle
                .sftp
                .extended(FSYNC, |packet| packet.bytes(&self.handle.id))?;
        }
        self.handle.close()
    }
}

impl Write for FileWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.pending.extend(bytes);
        if self.pending.len() >= PIECE * PIECES_AT_ONCE {
            self.send()?;
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.send()
    }
}

/// The `sftp-server` program of this machine, if it has OpenSSH: the one
/// that `AMACS_SFTP_SERVER` names, one on `PATH`, the one that belongs to
/// the `ssh` on `PATH`, or one where distributions put it.
pub fn sftp_server() -> Option<PathBuf> {
    if let Some(named) = std::env::var_os("AMACS_SFTP_SERVER") {
        return Some(PathBuf::from(named));
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    let beside_ssh = std::env::split_paths(&path)
        .filter_map(|directory| std::fs::canonicalize(directory.join("ssh")).ok())
        .filter_map(|ssh| Some(ssh.parent()?.parent()?.join("libexec/sftp-server")));
    let usual = [
        "/usr/lib/openssh/sftp-server",
        "/usr/libexec/openssh/sftp-server",
        "/usr/lib/ssh/sftp-server",
        "/usr/libexec/sftp-server",
    ];
    std::env::split_paths(&path)
        .map(|directory| directory.join("sftp-server"))
        .chain(beside_ssh)
        .chain(usual.into_iter().map(PathBuf::from))
        .find(|program| program.is_file())
}
