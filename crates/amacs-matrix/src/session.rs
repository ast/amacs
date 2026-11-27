//! Who is logged in, and where that is kept between runs.
//!
//! Each user has a directory under `$XDG_STATE_HOME/amacs/matrix`, named by
//! the user's Matrix address. In it are the stores of the SDK (`store`: the
//! rooms, the events and the keys of encryption, in SQLite, encrypted with
//! the passphrase) and two files of this program: `session`, and `device`,
//! which is empty and only says that the store holds the keys of a device.
//!
//! The session is the OAuth client id, the ids of the user and of the
//! device, and the access and refresh tokens. It can not go into the SDK's
//! store: the store has a place for values of the application, but writes
//! them as they are, not encrypted. So the session has a file of its own,
//! sealed with the cipher that the SDK's stores use themselves
//! (`matrix-sdk-store-encryption`): a random key encrypts the session, and
//! the passphrase, stretched with PBKDF2, encrypts the key. The tokens
//! change whenever the access token is refreshed, and the file is written
//! again each time. Only the key needs the slow stretching, and it is done
//! once in a run.
//!
//! The file also says which server the client talks to and which kind of
//! sliding sync the server has, so that a start with a session needs no
//! answer from the network before it shows the rooms that it knows.

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use matrix_sdk::authentication::oauth::registration::{
    ApplicationType, ClientMetadata, Localized, OAuthGrantType,
};
use matrix_sdk::authentication::oauth::{ClientRegistrationData, OAuthSession};
use matrix_sdk::reqwest::Url;
use matrix_sdk::ruma::UserId;
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::sliding_sync::{Version, VersionBuilder};
use matrix_sdk::store::RoomLoadSettings;
use matrix_sdk::utils::UrlOrQuery;
use matrix_sdk::utils::local_server::{
    LocalServerBuilder, LocalServerIpAddress, LocalServerResponse,
};
use matrix_sdk::{Client, SessionChange};
use matrix_sdk_store_encryption::StoreCipher;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;

use crate::protocol::{Output, explain, log};

/// The page of the project, which the server shows to the user who is asked
/// to let this program in. The server wants one.
const CLIENT_URI: &str = "https://github.com/ast/amacs";

/// What the browser shows when the server sends the user back to this
/// program.
const REDIRECT_PAGE: &str = "The login continues in amacs. You can close this page.";

/// The kind of sliding sync that the server has. The SDK has a type for
/// this, but not one that can be written to a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SlidingSync {
    Stable,
    Unstable,
}

impl SlidingSync {
    fn builder(self) -> VersionBuilder {
        match self {
            Self::Stable => VersionBuilder::Stable,
            Self::Unstable => VersionBuilder::Unstable,
        }
    }

    fn version(self) -> Version {
        match self {
            Self::Stable => Version::Stable,
            Self::Unstable => Version::Unstable,
        }
    }

    fn of(version: &Version) -> Option<Self> {
        match version {
            Version::Stable => Some(Self::Stable),
            Version::Unstable => Some(Self::Unstable),
            Version::None => None,
        }
    }
}

/// What the session file holds, before it is sealed.
#[derive(Clone, Serialize, Deserialize)]
pub struct Stored {
    pub homeserver: String,
    pub sliding_sync: SlidingSync,
    pub session: OAuthSession,
}

/// The session file as it is on disk. Both parts are ciphertext.
#[derive(Serialize, Deserialize)]
struct Sealed {
    version: u8,
    /// The key, encrypted with the passphrase.
    key: Vec<u8>,
    /// What `Stored` holds, encrypted with the key.
    data: Vec<u8>,
}

const SEALED_VERSION: u8 = 1;

/// Reads and writes the session file of one user.
pub struct Keeper {
    path: PathBuf,
    cipher: StoreCipher,
    /// The key as the file has it, kept so that a write does not stretch
    /// the passphrase again.
    sealed_key: Vec<u8>,
}

impl Keeper {
    /// A keeper for a file that is not there yet. This stretches the
    /// passphrase, which takes a moment: call it where waiting is allowed.
    pub fn create(path: &Path, passphrase: &str) -> Result<Self, String> {
        let cipher = StoreCipher::new().map_err(|error| explain("No key could be made", error))?;
        let sealed_key = cipher
            .export(passphrase)
            .map_err(|error| explain("The key of the session could not be locked", error))?;
        Ok(Self {
            path: path.to_owned(),
            cipher,
            sealed_key,
        })
    }

    /// The keeper and the session of a file, or nothing if there is no
    /// file. This stretches the passphrase too.
    pub fn open(path: &Path, passphrase: &str) -> Result<Option<(Self, Stored)>, String> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(explain("The session could not be read", error)),
        };
        let sealed: Sealed = serde_json::from_slice(&bytes)
            .map_err(|error| explain("The file of the session is damaged", error))?;
        if sealed.version != SEALED_VERSION {
            return Err(
                "The file of the session is of a version that this program does not know."
                    .to_owned(),
            );
        }
        // The cipher checks what it decrypts, so a wrong passphrase is
        // found here and not as nonsense later.
        let cipher = StoreCipher::import(passphrase, &sealed.key)
            .map_err(|_| "The passphrase does not open the Matrix store.".to_owned())?;
        let stored = cipher
            .decrypt_value(&sealed.data)
            .map_err(|error| explain("The file of the session is damaged", error))?;
        Ok(Some((
            Self {
                path: path.to_owned(),
                cipher,
                sealed_key: sealed.key,
            },
            stored,
        )))
    }

    /// Writes the session. The file is written beside the old one and then
    /// takes its place, so that there is always a whole one.
    pub fn save(&self, stored: &Stored) -> Result<(), String> {
        let data = self
            .cipher
            .encrypt_value(stored)
            .map_err(|error| explain("The session could not be encrypted", error))?;
        let sealed = Sealed {
            version: SEALED_VERSION,
            key: self.sealed_key.clone(),
            data,
        };
        let bytes = serde_json::to_vec(&sealed)
            .map_err(|error| explain("The session could not be written", error))?;
        let temporary = self.path.with_extension("new");
        let write = || -> std::io::Result<()> {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, &self.path)
        };
        write().map_err(|error| explain("The session could not be written", error))
    }
}

/// The name of a user's directory: the Matrix address with every byte that
/// is not a letter, a digit, a dot, a hyphen or an underscore written as
/// `%XX`, and a dot at the start too. Two addresses never get one name, and
/// no name leads out of the directory that it is made in.
pub fn safe_name(user: &str) -> String {
    let mut name = String::with_capacity(user.len() + 8);
    for (index, byte) in user.bytes().enumerate() {
        let plain = byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_')
            || (byte == b'.' && index > 0);
        if plain {
            name.push(char::from(byte));
        } else {
            name.push_str(&format!("%{byte:02X}"));
        }
    }
    if name.is_empty() {
        name.push('%');
    }
    name
}

/// Where the state of all users is, in the place that the environment
/// gives.
pub fn state_root_of_environment() -> Result<PathBuf, String> {
    state_root(
        std::env::var("XDG_STATE_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )
}

/// Where the state of all users is: `amacs/matrix` under the directory for
/// state of the XDG rules.
fn state_root(xdg_state_home: Option<&str>, home: Option<&str>) -> Result<PathBuf, String> {
    // The rules say that a path that is not absolute is to be ignored.
    let base = match xdg_state_home
        .map(Path::new)
        .filter(|path| path.is_absolute())
    {
        Some(path) => path.to_owned(),
        None => match home.map(Path::new).filter(|path| path.is_absolute()) {
            Some(home) => home.join(".local/state"),
            None => {
                return Err(
                    "There is no place for the Matrix store: set XDG_STATE_HOME or HOME."
                        .to_owned(),
                );
            }
        },
    };
    Ok(base.join("amacs").join("matrix"))
}

/// The directories and files of one user.
#[derive(Clone, Debug)]
pub struct Paths {
    dir: PathBuf,
}

impl Paths {
    /// The paths of a user under a root, with the directory made if it is
    /// not there. Only the user may look into what is made here.
    pub fn make(root: &Path, user: &UserId) -> Result<Self, String> {
        let dir = root.join(safe_name(user.as_str()));
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .map_err(|error| {
                explain("The directory of the Matrix store could not be made", error)
            })?;
        Ok(Self { dir })
    }

    pub fn session(&self) -> PathBuf {
        self.dir.join("session")
    }

    pub fn store(&self) -> PathBuf {
        self.dir.join("store")
    }

    /// Moves a thing of this directory out of the way, under a name that
    /// says when. Nothing is deleted: a person can look at it later.
    fn set_aside(&self, path: &Path, why: &str) {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return;
        };
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |time| time.as_secs());
        let aside = self.dir.join(format!("{name}.{why}-{seconds}"));
        match fs::rename(path, &aside) {
            Ok(()) => log(format!("moved {} to {}", path.display(), aside.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => log(format!("could not move {}: {error}", path.display())),
        }
    }

    /// The file that says that the store holds the keys of a device.
    fn device_marker(&self) -> PathBuf {
        self.dir.join("device")
    }

    /// Notes that the store now holds the keys of a device. This is
    /// written when a login has succeeded, before the session is.
    fn mark_device(&self) {
        if let Err(error) = fs::write(self.device_marker(), "") {
            log(format!("the store could not be marked as in use: {error}"));
        }
    }

    fn unmark_device(&self) {
        let _ = fs::remove_file(self.device_marker());
    }

    /// Makes the directory ready for a login. A store that holds the keys
    /// of a device, without a session, is of a device whose tokens are
    /// lost. A login makes a new device, and the SDK refuses to put a new
    /// device's keys into the store of another, so such a store is moved
    /// aside. A store from a login that was not finished holds no device
    /// and is used again.
    pub fn clear_for_login(&self) {
        if self.device_marker().exists() {
            self.set_aside(&self.store(), "stale");
            self.unmark_device();
        }
    }

    /// Puts away the session and the store of a device that the server no
    /// longer knows, so that the next login starts clean.
    pub fn retire(&self) {
        self.set_aside(&self.session(), "ended");
        self.set_aside(&self.store(), "ended");
        self.unmark_device();
    }
}

/// Which server a client is for.
pub enum Server<'a> {
    /// What the user gave as the address of the server: a name or a URL.
    /// The SDK asks the server which it is.
    Given(&'a str),
    /// The server of the user's address, found through its well-known file.
    OfUser(&'a UserId),
    /// The URL that an earlier run found, which is used as it is, with no
    /// question to the network.
    Known(&'a str),
}

/// A client with its stores in the user's directory, encrypted with the
/// passphrase, which refreshes its access token by itself.
pub async fn build_client(
    paths: &Paths,
    passphrase: &str,
    server: Server<'_>,
    sliding_sync: Option<SlidingSync>,
) -> Result<Client, String> {
    let builder = Client::builder()
        .sqlite_store(paths.store(), Some(passphrase))
        .handle_refresh_tokens()
        .sliding_sync_version_builder(
            sliding_sync.map_or(VersionBuilder::None, SlidingSync::builder),
        );
    let builder = match server {
        Server::Given(address) => builder.server_name_or_homeserver_url(address),
        Server::OfUser(user) => builder.server_name(user.server_name()),
        Server::Known(url) => builder.homeserver_url(url),
    };
    builder
        .build()
        .await
        .map_err(|error| explain("The Matrix client could not be made", error))
}

/// A client that is logged in again with the session of an earlier run.
/// This asks the server nothing.
pub async fn restore(
    paths: &Paths,
    passphrase: &str,
    stored: &Stored,
    user: &UserId,
) -> Result<Client, String> {
    if stored.session.user.meta.user_id != user {
        return Err(format!(
            "The session in the store is of {}, not of {user}.",
            stored.session.user.meta.user_id
        ));
    }
    let client = build_client(
        paths,
        passphrase,
        Server::Known(&stored.homeserver),
        Some(stored.sliding_sync),
    )
    .await?;
    client
        .oauth()
        .restore_session(stored.session.clone(), RoomLoadSettings::default())
        .await
        .map_err(|error| explain("The session could not be taken up again", error))?;
    Ok(client)
}

/// What this program says of itself when it registers with the server: its
/// name, that it is a program on the user's machine, and where the browser
/// is to send the user back to.
fn client_metadata(redirect_uri: &Url) -> Result<Raw<ClientMetadata>, String> {
    let client_uri = Url::parse(CLIENT_URI).map_err(|error| error.to_string())?;
    let mut metadata = ClientMetadata::new(
        ApplicationType::Native,
        vec![OAuthGrantType::AuthorizationCode {
            redirect_uris: vec![redirect_uri.clone()],
        }],
        Localized::new(client_uri, []),
    );
    metadata.client_name = Some(Localized::new("amacs".to_owned(), []));
    Raw::new(&metadata).map_err(|error| explain("The registration could not be written", error))
}

/// Logs a user in through the browser.
///
/// A small server on this machine waits for the browser to come back. The
/// URL to open goes to the editor as a "login-url" event. This returns when
/// the user has finished in the browser, which can take as long as the user
/// likes; a caller that gives up drops the future, and the small server
/// stops with it.
pub async fn login(
    paths: &Paths,
    passphrase: &str,
    user: &UserId,
    homeserver: Option<&str>,
    out: &Output,
) -> Result<(Client, Keeper, Stored), String> {
    let server = homeserver.map_or(Server::OfUser(user), Server::Given);
    let client = build_client(paths, passphrase, server, None).await?;
    let oauth = client.oauth();

    let (redirect_uri, redirect) = LocalServerBuilder::new()
        // One address, so that the URL that is registered is the URL that
        // the server listens on.
        .ip_address(LocalServerIpAddress::Localhostv4)
        .response(LocalServerResponse::PlainText(REDIRECT_PAGE.to_owned()))
        .spawn()
        .await
        .map_err(|error| {
            explain(
                "No port could be opened for the browser to come back to",
                error,
            )
        })?;

    let registration = ClientRegistrationData::new(client_metadata(&redirect_uri)?);
    let authorization = oauth
        .login(redirect_uri, None, Some(registration), None)
        .user_id_hint(user)
        .build()
        .await
        .map_err(|error| explain("The server did not let the login start", error))?;
    out.login_url(authorization.url.as_str());

    let Some(query) = redirect.await else {
        oauth.abort_login(&authorization.state).await;
        return Err("The browser came back without an answer of the server.".to_owned());
    };
    oauth
        .finish_login(UrlOrQuery::Query(query.0))
        .await
        .map_err(|error| explain("The login failed", error))?;

    // From here the server has a device for this program, and the store
    // has the device's keys. If the rest fails, the device is given back,
    // so that none is left that nobody uses, and the store is put away,
    // so that the next login can make another.
    paths.mark_device();
    match after_login(&client, paths, passphrase, user).await {
        Ok((keeper, stored)) => Ok((client, keeper, stored)),
        Err(error) => {
            if let Err(logout) = oauth.logout().await {
                log(format!(
                    "the logout after a failed login failed too: {logout}"
                ));
            }
            paths.clear_for_login();
            Err(error)
        }
    }
}

async fn after_login(
    client: &Client,
    paths: &Paths,
    passphrase: &str,
    user: &UserId,
) -> Result<(Keeper, Stored), String> {
    match client.user_id() {
        Some(logged_in) if logged_in == user => {}
        Some(other) => {
            return Err(format!(
                "You logged in as {other}, but matrix-user-id is {user}."
            ));
        }
        None => return Err("The login did not say who you are.".to_owned()),
    }

    // The server was just reached, so this is the time to ask which sliding
    // sync it has. The stable one comes first in the answer.
    let sliding_sync = client
        .available_sliding_sync_versions()
        .await
        .iter()
        .find_map(SlidingSync::of)
        .ok_or("The server does not have sliding sync, which this program needs.")?;
    client.set_sliding_sync_version(sliding_sync.version());

    let session = client
        .oauth()
        .full_session()
        .ok_or("The login gave no session.")?;
    let stored = Stored {
        homeserver: client.homeserver().to_string(),
        sliding_sync,
        session,
    };

    let path = paths.session();
    let passphrase = passphrase.to_owned();
    let keeper = tokio::task::spawn_blocking(move || Keeper::create(&path, &passphrase))
        .await
        .map_err(|error| explain("The key of the session could not be locked", error))??;
    keeper.save(&stored)?;
    Ok((keeper, stored))
}

/// Why the watch of a session ended.
pub enum SessionEnd {
    /// The server no longer knows the session: the user has to log in again.
    Revoked,
    /// The client is gone.
    Closed,
}

/// Writes the session again whenever its tokens change, until the session
/// ends. The receiver is made by the caller, before the session that it has
/// is written for the first time, so that no change falls between.
pub async fn keep_session(
    client: Client,
    keeper: Keeper,
    mut stored: Stored,
    mut changes: tokio::sync::broadcast::Receiver<SessionChange>,
) -> SessionEnd {
    loop {
        match changes.recv().await {
            // When changes were missed, the tokens may be among them.
            Ok(SessionChange::TokensRefreshed) | Err(RecvError::Lagged(_)) => {
                let Some(session) = client.oauth().full_session() else {
                    continue;
                };
                stored.session = session;
                if let Err(error) = keeper.save(&stored) {
                    // The session still works in this run. The next start
                    // may need a login.
                    log(error);
                }
            }
            Ok(SessionChange::UnknownToken(_)) => return SessionEnd::Revoked,
            Err(RecvError::Closed) => return SessionEnd::Closed,
        }
    }
}

#[cfg(test)]
mod tests {
    use matrix_sdk::authentication::oauth::{ClientId, UserSession};
    use matrix_sdk::ruma::{device_id, user_id};
    use matrix_sdk::{SessionMeta, SessionTokens};

    use super::*;

    /// A directory for one test, under the directory for temporary files,
    /// which is removed when the test ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("amacs-matrix-test-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn stored() -> Stored {
        Stored {
            homeserver: "https://matrix.example.org/".to_owned(),
            sliding_sync: SlidingSync::Unstable,
            session: OAuthSession {
                client_id: ClientId::new("client-0123".to_owned()),
                user: UserSession {
                    meta: SessionMeta {
                        user_id: user_id!("@nobody:example.org").to_owned(),
                        device_id: device_id!("DEVICEID").to_owned(),
                    },
                    tokens: SessionTokens {
                        access_token: "access-token-in-the-clear".to_owned(),
                        refresh_token: Some("refresh-token-in-the-clear".to_owned()),
                    },
                },
            },
        }
    }

    #[test]
    fn names_are_safe_for_a_path() {
        assert_eq!(safe_name("@nobody:matrix.org"), "%40nobody%3Amatrix.org");
        assert_eq!(
            safe_name("@a_b-c.d=e/f:example.org"),
            "%40a_b-c.d%3De%2Ff%3Aexample.org"
        );
        // Nothing leads out of the directory or hides in it.
        assert_eq!(safe_name(".."), "%2E.");
        assert_eq!(safe_name("../../etc/passwd"), "%2E.%2F..%2Fetc%2Fpasswd");
        assert_eq!(safe_name(""), "%");
        assert_eq!(safe_name("a%40b"), "a%2540b");
        assert_eq!(safe_name("@ö:b"), "%40%C3%B6%3Ab");
        for name in ["@nobody:matrix.org", "..", "/", "a/b", "\0", "."] {
            let safe = safe_name(name);
            assert!(
                !safe.contains('/') && !safe.contains('\0') && !safe.starts_with('.'),
                "{safe}"
            );
        }
        // Two addresses do not share a name.
        assert_ne!(safe_name("@a:b"), safe_name("%40a%3Ab"));
    }

    #[test]
    fn the_state_is_where_xdg_says() {
        let root = |xdg, home| state_root(xdg, home);
        assert_eq!(
            root(Some("/x/state"), Some("/home/u")).unwrap(),
            Path::new("/x/state/amacs/matrix")
        );
        assert_eq!(
            root(None, Some("/home/u")).unwrap(),
            Path::new("/home/u/.local/state/amacs/matrix")
        );
        assert_eq!(
            root(Some("relative"), Some("/home/u")).unwrap(),
            Path::new("/home/u/.local/state/amacs/matrix")
        );
        assert!(root(None, None).is_err());
    }

    #[test]
    fn directories_are_for_the_user_only() {
        use std::os::unix::fs::PermissionsExt as _;
        let scratch = Scratch::new("dirs");
        let root = scratch.0.join("amacs").join("matrix");
        let paths = Paths::make(&root, user_id!("@nobody:example.org")).unwrap();
        for dir in [&root, &scratch.0.join("amacs"), &paths.dir] {
            let mode = fs::metadata(dir).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "{}", dir.display());
        }
        assert_eq!(paths.dir, root.join("%40nobody%3Aexample.org"));
        // Making them again is no error.
        Paths::make(&root, user_id!("@nobody:example.org")).unwrap();
    }

    #[test]
    fn a_session_comes_back_and_is_not_in_the_clear() {
        use std::os::unix::fs::PermissionsExt as _;
        let scratch = Scratch::new("session");
        let path = scratch.0.join("session");
        assert!(Keeper::open(&path, "passphrase").unwrap().is_none());

        let keeper = Keeper::create(&path, "passphrase").unwrap();
        keeper.save(&stored()).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let on_disk = String::from_utf8_lossy(&fs::read(&path).unwrap()).into_owned();
        for secret in [
            "access-token",
            "refresh-token",
            "client-0123",
            "DEVICEID",
            "nobody",
            "passphrase",
        ] {
            assert!(!on_disk.contains(secret), "{secret} is in the clear");
        }

        let (keeper, back) = Keeper::open(&path, "passphrase").unwrap().unwrap();
        assert_eq!(back.homeserver, "https://matrix.example.org/");
        assert_eq!(back.sliding_sync, SlidingSync::Unstable);
        assert_eq!(back.session.client_id.as_str(), "client-0123");
        assert_eq!(back.session.user.meta.device_id, "DEVICEID");
        assert_eq!(
            back.session.user.tokens.access_token,
            "access-token-in-the-clear"
        );

        // New tokens take the place of the old, with the same key.
        let mut changed = stored();
        changed.session.user.tokens.access_token = "the-next-token".to_owned();
        keeper.save(&changed).unwrap();
        let (_, back) = Keeper::open(&path, "passphrase").unwrap().unwrap();
        assert_eq!(back.session.user.tokens.access_token, "the-next-token");

        let wrong = Keeper::open(&path, "another").err().unwrap();
        assert_eq!(wrong, "The passphrase does not open the Matrix store.");

        fs::write(&path, "not a session").unwrap();
        assert!(Keeper::open(&path, "passphrase").is_err());
    }

    #[test]
    fn a_store_without_a_session_is_moved_aside() {
        let scratch = Scratch::new("aside");
        let paths = Paths::make(&scratch.0, user_id!("@nobody:example.org")).unwrap();
        // No store, or the store of a login that was not finished, is
        // left alone.
        paths.clear_for_login();
        fs::create_dir(paths.store()).unwrap();
        fs::write(paths.store().join("state.sqlite3"), "x").unwrap();
        paths.clear_for_login();
        assert!(paths.store().exists());

        // The store of a device goes aside, once.
        paths.mark_device();
        paths.clear_for_login();
        assert!(!paths.store().exists());
        let names = || -> Vec<String> {
            fs::read_dir(&paths.dir)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect()
        };
        assert_eq!(names().len(), 1);
        assert!(names()[0].starts_with("store.stale-"), "{:?}", names());
        paths.clear_for_login();
        assert_eq!(names().len(), 1);

        // A session that ended goes aside with its store.
        fs::create_dir(paths.store()).unwrap();
        fs::write(paths.session(), "x").unwrap();
        paths.mark_device();
        paths.retire();
        let mut names = names();
        names.sort();
        assert_eq!(names.len(), 3, "{names:?}");
        assert!(names[0].starts_with("session.ended-"), "{names:?}");
        assert!(names[1].starts_with("store.ended-"), "{names:?}");
    }

    #[test]
    fn the_registration_is_of_a_native_program_named_amacs() {
        let redirect = Url::parse("http://127.0.0.1:20123/").unwrap();
        let metadata = client_metadata(&redirect).unwrap();
        let value: serde_json::Value = serde_json::from_str(metadata.json().get()).unwrap();
        assert_eq!(value["client_name"], "amacs");
        assert_eq!(value["application_type"], "native");
        assert_eq!(
            value["redirect_uris"],
            serde_json::json!(["http://127.0.0.1:20123/"])
        );
        assert_eq!(value["client_uri"], CLIENT_URI);
    }
}
