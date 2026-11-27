//! Files: those of messages, fetched to disk, and those that the user sends.
//!
//! What is fetched is kept under `$XDG_CACHE_HOME/amacs/matrix/<user>/`, in
//! directories that only the user may look into:
//!
//! - `files/<media>/<name>`: the file of a message, decrypted, under the
//!   name that the sender gave it, made safe. `<media>` is the address of
//!   the media on the server, made safe as the user's address is, so that
//!   two files of one name do not meet and a file is fetched once.
//! - `thumbnails/<media>.<ext>`: the picture to show for a message that is
//!   a picture. Only PNG, JPEG, WebP and GIF are kept, which the editor can
//!   draw; which of them a file is, is read from its first bytes and not
//!   believed from what the message says.
//!
//! A file is written beside its place and moved there when it is whole, so
//! that a file that is there is a whole file.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use matrix_sdk::Client;
use matrix_sdk::media::{MediaFormat, MediaRequestParameters};
use matrix_sdk::ruma::UserId;
use matrix_sdk::ruma::events::room::message::MessageType;
use matrix_sdk::ruma::events::room::{ImageInfo, MediaSource};
use tokio::sync::{broadcast, mpsc};

use crate::Task;
use crate::protocol::{explain, log};
use crate::session::safe_name;

/// The largest picture that is fetched whole to be shown, when the message
/// has no smaller one.
const SHOW_WHOLE_BELOW: u64 = 5 * 1024 * 1024;

/// The kinds of picture that the editor can draw, by the extension of their
/// files.
const PICTURE_EXTENSIONS: [&str; 4] = ["png", "jpg", "webp", "gif"];

/// Where the cache of all users is: `amacs/matrix` under the directory for
/// caches of the XDG rules.
pub fn cache_root(xdg_cache_home: Option<&str>, home: Option<&str>) -> Result<PathBuf, String> {
    // The rules say that a path that is not absolute is to be ignored.
    let base = match xdg_cache_home
        .map(Path::new)
        .filter(|path| path.is_absolute())
    {
        Some(path) => path.to_owned(),
        None => match home.map(Path::new).filter(|path| path.is_absolute()) {
            Some(home) => home.join(".cache"),
            None => {
                return Err(
                    "There is no place for the files of Matrix: set XDG_CACHE_HOME or HOME."
                        .to_owned(),
                );
            }
        },
    };
    Ok(base.join("amacs").join("matrix"))
}

/// Where the cache of all users is, in the place that the environment
/// gives.
pub fn cache_root_of_environment() -> Result<PathBuf, String> {
    cache_root(
        std::env::var("XDG_CACHE_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )
}

/// The name of a file as its sender gave it, made safe to be the name of a
/// file in a directory of the cache: no part of a path, nothing that hides
/// the file or a program reads as an option, no character that controls a
/// terminal, and not too long. A name with nothing left is "file".
pub fn safe_file_name(name: &str) -> String {
    // Only what follows the last separator of either kind is a name.
    let name = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let mut safe: String = name
        .chars()
        .map(|c| if c.is_control() { '_' } else { c })
        .collect::<String>()
        .trim()
        .to_owned();
    while safe.starts_with(['.', '-', '~']) {
        safe.remove(0);
    }
    // Most file systems take 255 bytes for a name. The end of the name is
    // kept, where the extension is.
    while safe.len() > 200 {
        safe.remove(0);
    }
    if safe.is_empty() {
        "file".to_owned()
    } else {
        safe
    }
}

/// The name under which a media of the server is kept: its address without
/// the scheme, made safe.
pub fn media_key(source: &MediaSource) -> String {
    let uri = match source {
        MediaSource::Plain(uri) => uri.as_str(),
        MediaSource::Encrypted(file) => file.url.as_str(),
    };
    safe_name(uri.strip_prefix("mxc://").unwrap_or(uri))
}

/// The extension for the kind of picture that data is, by its first bytes,
/// if it is of a kind that the editor can draw.
pub fn picture_extension(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if data.starts_with(b"\xff\xd8\xff") {
        Some("jpg")
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some("gif")
    } else if data.len() >= 12 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

/// Whether a media type is of a picture that the editor can draw.
fn is_picture_type(mimetype: Option<&str>) -> bool {
    matches!(
        mimetype,
        Some("image/png" | "image/jpeg" | "image/webp" | "image/gif")
    )
}

/// The media type of a file to send, by the extension of its name. A file
/// of a kind that is not known here is sent as bytes without a meaning,
/// which the SDK sends as a file to download.
pub fn media_type(path: &Path) -> mime::Mime {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let name = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "avif" => "image/avif",
        "heic" => "image/heic",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "flac" => "audio/flac",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "json" => "application/json",
        "txt" | "org" | "log" | "scm" | "rs" | "el" | "c" | "h" | "py" | "sh" | "toml" | "diff"
        | "patch" => "text/plain",
        "md" => "text/markdown",
        "html" | "htm" => "text/html",
        "csv" => "text/csv",
        _ => "application/octet-stream",
    };
    name.parse().unwrap_or(mime::APPLICATION_OCTET_STREAM)
}

/// The file of a message: where it is on the server, and its name.
pub struct Attachment {
    pub source: MediaSource,
    pub name: String,
}

/// The file of a message, if it is one with a file.
pub fn attachment(message: &MessageType) -> Option<Attachment> {
    let (source, name) = match message {
        MessageType::Image(content) => (&content.source, content.filename()),
        MessageType::File(content) => (&content.source, content.filename()),
        MessageType::Audio(content) => (&content.source, content.filename()),
        MessageType::Video(content) => (&content.source, content.filename()),
        _ => return None,
    };
    Some(Attachment {
        source: source.clone(),
        name: name.to_owned(),
    })
}

/// What to fetch to show a picture: the smaller picture that the message
/// brings, if it is of a kind that the editor can draw, or else the picture
/// itself, if it is of such a kind and not large.
pub fn picture_to_show(source: &MediaSource, info: Option<&ImageInfo>) -> Option<MediaSource> {
    let info = info?;
    if let Some(thumbnail) = &info.thumbnail_source {
        let mimetype = info
            .thumbnail_info
            .as_ref()
            .and_then(|info| info.mimetype.as_deref());
        // A thumbnail that does not say what it is, is tried: its bytes
        // will tell.
        if mimetype.is_none() || is_picture_type(mimetype) {
            return Some(thumbnail.clone());
        }
    }
    let small = info
        .size
        .is_some_and(|size| u64::from(size) <= SHOW_WHOLE_BELOW);
    (small && is_picture_type(info.mimetype.as_deref())).then(|| source.clone())
}

/// The directories of one user in the cache.
#[derive(Clone, Debug)]
pub struct Cache {
    dir: PathBuf,
}

impl Cache {
    pub fn new(root: &Path, user: &UserId) -> Self {
        Self {
            dir: root.join(safe_name(user.as_str())),
        }
    }

    /// Where the file of a message is kept.
    pub fn file_path(&self, attachment: &Attachment) -> PathBuf {
        self.dir
            .join("files")
            .join(media_key(&attachment.source))
            .join(safe_file_name(&attachment.name))
    }

    fn thumbnail_path(&self, key: &str, extension: &str) -> PathBuf {
        self.dir
            .join("thumbnails")
            .join(format!("{key}.{extension}"))
    }

    /// The picture that is kept under a key, if one is.
    fn thumbnail_on_disk(&self, key: &str) -> Option<PathBuf> {
        PICTURE_EXTENSIONS
            .iter()
            .map(|extension| self.thumbnail_path(key, extension))
            .find(|path| path.is_file())
    }
}

/// Writes a file of the cache, with the directories above it. Only the user
/// may read what is made here.
fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().ok_or(std::io::ErrorKind::InvalidInput)?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let mut name = path.file_name().unwrap_or_default().to_owned();
    name.push(".part");
    let temporary = dir.join(name);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(data)?;
    drop(file);
    fs::rename(&temporary, path)
}

async fn fetch(client: &Client, source: &MediaSource) -> Result<Vec<u8>, String> {
    let request = MediaRequestParameters {
        source: source.clone(),
        format: MediaFormat::File,
    };
    // The SDK's own cache is not used: the file is kept here, once.
    client
        .media()
        .get_media_content(&request, false)
        .await
        .map_err(|error| explain("The file could not be fetched", error))
}

/// Fetches the file of a message to the cache, decrypted, unless it is
/// there, and gives its place.
pub async fn download(
    client: &Client,
    cache: &Cache,
    attachment: &Attachment,
) -> Result<PathBuf, String> {
    let path = cache.file_path(attachment);
    if path.is_file() {
        return Ok(path);
    }
    let data = fetch(client, &attachment.source).await?;
    let place = path.clone();
    tokio::task::spawn_blocking(move || write_private(&place, &data))
        .await
        .map_err(|error| explain("The file could not be written", error))?
        .map_err(|error| explain("The file could not be written", error))?;
    Ok(path)
}

/// What is known of the pictures to show, by their keys.
#[derive(Default)]
struct ThumbnailState {
    /// The pictures that are on disk.
    ready: HashMap<String, PathBuf>,
    /// The keys that are being fetched, or were and gave no picture, so
    /// that they are not asked for again and again.
    asked: HashSet<String>,
}

/// A picture to get for a message.
struct ThumbnailRequest {
    key: String,
    source: MediaSource,
}

/// Gets the pictures to show for messages, beside everything else: the
/// lists of the open rooms ask it for a picture and go on without, and
/// hear from it when the picture is on disk. All lists share one.
#[derive(Clone)]
pub struct Thumbnailer {
    state: Arc<Mutex<ThumbnailState>>,
    requests: mpsc::UnboundedSender<ThumbnailRequest>,
    done: broadcast::Sender<String>,
}

impl Thumbnailer {
    /// A thumbnailer and the work that it has to do, which the caller
    /// runs: [`Thumbnailer::run`].
    fn new() -> (Self, mpsc::UnboundedReceiver<ThumbnailRequest>) {
        let (requests, receiver) = mpsc::unbounded_channel();
        let (done, _) = broadcast::channel(64);
        (
            Self {
                state: Arc::default(),
                requests,
                done,
            },
            receiver,
        )
    }

    /// A thumbnailer that gets pictures with a client into a cache, and
    /// the task that does so, which stops when it is dropped.
    pub fn start(client: Client, cache: Cache) -> (Self, Task) {
        let (thumbnailer, requests) = Self::new();
        let task = Task::spawn(thumbnailer.clone().run(client, cache, requests));
        (thumbnailer, task)
    }

    /// A thumbnailer that gets nothing: for when there is no place to
    /// keep pictures, and for tests.
    pub fn idle() -> Self {
        Self::new().0
    }

    fn state(&self) -> std::sync::MutexGuard<'_, ThumbnailState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The picture of a key, if it is on disk and known to be.
    pub fn get(&self, key: &str) -> Option<PathBuf> {
        self.state().ready.get(key).cloned()
    }

    /// The picture of a key if it is on disk and known to be. If not, it is
    /// asked for, once, and those who listen hear when it is there.
    pub fn get_or_ask(&self, key: &str, source: &MediaSource) -> Option<PathBuf> {
        let mut state = self.state();
        if let Some(path) = state.ready.get(key) {
            return Some(path.clone());
        }
        if state.asked.insert(key.to_owned()) {
            // Without a task that gets pictures, there are none.
            let _ = self.requests.send(ThumbnailRequest {
                key: key.to_owned(),
                source: source.clone(),
            });
        }
        None
    }

    /// The keys of the pictures that come to be on disk, from now on.
    pub fn listen(&self) -> broadcast::Receiver<String> {
        self.done.subscribe()
    }

    async fn run(
        self,
        client: Client,
        cache: Cache,
        mut requests: mpsc::UnboundedReceiver<ThumbnailRequest>,
    ) {
        while let Some(request) = requests.recv().await {
            match get_thumbnail(&client, &cache, &request).await {
                Ok(Some(path)) => {
                    self.state().ready.insert(request.key.clone(), path);
                    // Nobody listens when no room is open.
                    let _ = self.done.send(request.key);
                }
                // Not a picture that the editor can draw: the message stays
                // without one.
                Ok(None) => {}
                Err(error) => log(format!("no picture to show for a message: {error}")),
            }
        }
    }
}

async fn get_thumbnail(
    client: &Client,
    cache: &Cache,
    request: &ThumbnailRequest,
) -> Result<Option<PathBuf>, String> {
    if let Some(path) = cache.thumbnail_on_disk(&request.key) {
        return Ok(Some(path));
    }
    let data = fetch(client, &request.source).await?;
    let Some(extension) = picture_extension(&data) else {
        return Ok(None);
    };
    let path = cache.thumbnail_path(&request.key, extension);
    let place = path.clone();
    tokio::task::spawn_blocking(move || write_private(&place, &data))
        .await
        .map_err(|error| explain("The picture could not be written", error))?
        .map_err(|error| explain("The picture could not be written", error))?;
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use matrix_sdk::ruma::events::room::ThumbnailInfo;
    use matrix_sdk::ruma::events::room::message::{
        FileMessageEventContent, ImageMessageEventContent,
    };
    use matrix_sdk::ruma::{mxc_uri, uint, user_id};

    use super::*;

    fn plain(uri: &str) -> MediaSource {
        MediaSource::Plain(uri.into())
    }

    #[test]
    fn the_cache_is_where_xdg_says() {
        assert_eq!(
            cache_root(Some("/x/cache"), Some("/home/u")).unwrap(),
            Path::new("/x/cache/amacs/matrix")
        );
        assert_eq!(
            cache_root(None, Some("/home/u")).unwrap(),
            Path::new("/home/u/.cache/amacs/matrix")
        );
        assert_eq!(
            cache_root(Some("relative"), Some("/home/u")).unwrap(),
            Path::new("/home/u/.cache/amacs/matrix")
        );
        assert!(cache_root(None, None).is_err());
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(safe_file_name("kettle.jpg"), "kettle.jpg");
        assert_eq!(safe_file_name("my holiday (1).JPG"), "my holiday (1).JPG");
        assert_eq!(safe_file_name("tekanna åäö.png"), "tekanna åäö.png");
        assert_eq!(safe_file_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_file_name("C:\\Users\\a\\b.txt"), "b.txt");
        assert_eq!(safe_file_name(".bashrc"), "bashrc");
        assert_eq!(safe_file_name("..."), "file");
        assert_eq!(safe_file_name(".."), "file");
        assert_eq!(safe_file_name(""), "file");
        assert_eq!(safe_file_name("a/"), "file");
        assert_eq!(safe_file_name("-rf"), "rf");
        assert_eq!(safe_file_name("a\nb\u{1b}[31m.txt"), "a_b_[31m.txt");
        let long = format!("{}.png", "x".repeat(400));
        let safe = safe_file_name(&long);
        assert!(safe.len() <= 200 && safe.ends_with(".png"));
        // A name of letters of more than one byte is cut between letters.
        assert!(safe_file_name(&"å".repeat(300)).len() <= 200);
        for name in ["../x", "/", "a/b", "\0", ".", "~root", "x\0y"] {
            let safe = safe_file_name(name);
            assert!(
                !safe.contains('/') && !safe.contains('\0') && !safe.starts_with('.'),
                "{safe:?}"
            );
            assert!(!safe.is_empty());
        }
    }

    #[test]
    fn files_have_their_place_in_the_cache() {
        let cache = Cache::new(
            Path::new("/c/amacs/matrix"),
            user_id!("@nobody:example.org"),
        );
        let attachment = Attachment {
            source: plain("mxc://example.org/AbC123"),
            name: "../kettle.jpg".to_owned(),
        };
        assert_eq!(
            cache.file_path(&attachment),
            Path::new(
                "/c/amacs/matrix/%40nobody%3Aexample.org/files/example.org%2FAbC123/kettle.jpg"
            )
        );
        assert_eq!(
            cache.thumbnail_path("example.org%2FAbC123", "png"),
            Path::new(
                "/c/amacs/matrix/%40nobody%3Aexample.org/thumbnails/example.org%2FAbC123.png"
            )
        );
        // A media address that is none is no way out of the cache either.
        assert_eq!(media_key(&plain("mxc://../../x")), "%2E.%2F..%2Fx");
    }

    #[test]
    fn files_are_written_for_the_user_only() {
        use std::os::unix::fs::PermissionsExt as _;
        let root =
            std::env::temp_dir().join(format!("amacs-matrix-test-{}-media", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let cache = Cache::new(&root, user_id!("@nobody:example.org"));
        let path = cache.thumbnail_path("example.org%2Fa", "png");
        assert_eq!(cache.thumbnail_on_disk("example.org%2Fa"), None);
        write_private(&path, b"\x89PNG\r\n\x1a\n").unwrap();
        assert_eq!(
            cache.thumbnail_on_disk("example.org%2Fa"),
            Some(path.clone())
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        for dir in [
            path.parent().unwrap(),
            path.parent().unwrap().parent().unwrap(),
        ] {
            assert_eq!(
                fs::metadata(dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        // Nothing is left beside the file.
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn pictures_are_known_by_their_first_bytes() {
        assert_eq!(picture_extension(b"\x89PNG\r\n\x1a\n...."), Some("png"));
        assert_eq!(picture_extension(b"\xff\xd8\xff\xe0...."), Some("jpg"));
        assert_eq!(picture_extension(b"GIF89a...."), Some("gif"));
        assert_eq!(picture_extension(b"RIFF\x10\0\0\0WEBPVP8 "), Some("webp"));
        assert_eq!(picture_extension(b"RIFF\x10\0\0\0WAVEfmt "), None);
        assert_eq!(picture_extension(b"<svg xmlns="), None);
        assert_eq!(picture_extension(b"RIFF"), None);
        assert_eq!(picture_extension(b""), None);
    }

    #[test]
    fn files_to_send_have_the_type_of_their_extension() {
        assert_eq!(
            media_type(Path::new("/a/kettle.JPG")).essence_str(),
            "image/jpeg"
        );
        assert_eq!(media_type(Path::new("a.png")).type_(), mime::IMAGE);
        assert_eq!(
            media_type(Path::new("notes.txt")).essence_str(),
            "text/plain"
        );
        assert_eq!(
            media_type(Path::new("paper.pdf")).essence_str(),
            "application/pdf"
        );
        assert_eq!(
            media_type(Path::new("Makefile")),
            mime::APPLICATION_OCTET_STREAM
        );
        assert_eq!(
            media_type(Path::new("a.unknown")),
            mime::APPLICATION_OCTET_STREAM
        );
    }

    #[test]
    fn messages_with_files_say_which() {
        let image = MessageType::Image(ImageMessageEventContent::plain(
            "kettle.jpg".to_owned(),
            mxc_uri!("mxc://example.org/abc").to_owned(),
        ));
        let found = attachment(&image).unwrap();
        assert_eq!(found.name, "kettle.jpg");
        assert_eq!(media_key(&found.source), "example.org%2Fabc");

        // With a caption, the name is the name and not the caption.
        let mut file = FileMessageEventContent::plain(
            "The minutes of the meeting".to_owned(),
            mxc_uri!("mxc://example.org/def").to_owned(),
        );
        file.filename = Some("minutes.pdf".to_owned());
        assert_eq!(
            attachment(&MessageType::File(file)).unwrap().name,
            "minutes.pdf"
        );
        assert!(attachment(&MessageType::text_plain("hello")).is_none());
    }

    #[test]
    fn the_picture_to_show_is_the_small_one_or_a_small_whole() {
        let whole = plain("mxc://example.org/whole");
        let small = plain("mxc://example.org/small");
        let key = |source: Option<MediaSource>| source.as_ref().map(media_key);

        // Nothing is known of the picture: it is not fetched.
        assert_eq!(key(picture_to_show(&whole, None)), None);
        let mut info = ImageInfo::new();
        assert_eq!(key(picture_to_show(&whole, Some(&info))), None);

        // A small picture of a kind that can be drawn is fetched whole.
        info.mimetype = Some("image/png".to_owned());
        info.size = Some(uint!(40_000));
        assert_eq!(
            key(picture_to_show(&whole, Some(&info))),
            Some("example.org%2Fwhole".to_owned())
        );
        info.size = Some(uint!(6_000_000));
        assert_eq!(key(picture_to_show(&whole, Some(&info))), None);
        info.size = Some(uint!(40_000));
        info.mimetype = Some("image/svg+xml".to_owned());
        assert_eq!(key(picture_to_show(&whole, Some(&info))), None);

        // The thumbnail of the message comes first, also for a large
        // picture of a kind that can not be drawn.
        info.size = Some(uint!(60_000_000));
        info.mimetype = Some("image/heic".to_owned());
        info.thumbnail_source = Some(small.clone());
        let mut thumbnail = ThumbnailInfo::new();
        thumbnail.mimetype = Some("image/jpeg".to_owned());
        info.thumbnail_info = Some(Box::new(thumbnail.clone()));
        assert_eq!(
            key(picture_to_show(&whole, Some(&info))),
            Some("example.org%2Fsmall".to_owned())
        );
        // But not a thumbnail of a kind that can not be drawn.
        thumbnail.mimetype = Some("image/avif".to_owned());
        info.thumbnail_info = Some(Box::new(thumbnail));
        assert_eq!(key(picture_to_show(&whole, Some(&info))), None);
    }

    #[test]
    fn a_picture_is_asked_for_once() {
        let (thumbnailer, mut requests) = Thumbnailer::new();
        let source = plain("mxc://example.org/abc");
        assert_eq!(thumbnailer.get_or_ask("k", &source), None);
        assert_eq!(thumbnailer.get_or_ask("k", &source), None);
        assert_eq!(
            requests.try_recv().map(|request| request.key),
            Ok("k".to_owned())
        );
        assert!(requests.try_recv().is_err());
        thumbnailer
            .state()
            .ready
            .insert("k".to_owned(), PathBuf::from("/c/k.png"));
        assert_eq!(
            thumbnailer.get_or_ask("k", &source),
            Some(PathBuf::from("/c/k.png"))
        );
        assert_eq!(thumbnailer.get("k"), Some(PathBuf::from("/c/k.png")));
        assert_eq!(Thumbnailer::idle().get_or_ask("k", &source), None);
    }
}
