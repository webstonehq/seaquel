//! A workspace: one user's metadata storage and secret store.
//!
//! The desktop app opens one at startup; the web server opens one per user.
//! Interfaces reach both only through the [`Workspace`] Core hands them, and
//! `seaquel-rpc`'s `dispatch_workspace` serves them to the GUIs.

use std::fmt;
use std::path::{Path, PathBuf};
#[cfg(feature = "secrets")]
use std::sync::Arc;

#[cfg(feature = "secrets")]
use seaquel_secrets::SecretStore;
#[cfg(feature = "storage")]
use seaquel_storage::{Storage, StorageOptions};

/// The metadata file's name in a desktop data dir.
pub const DESKTOP_STORAGE_FILE: &str = "seaquel.db";

/// What [`crate::Core::open_workspace`] opens. Build it with
/// [`WorkspaceSpec::new`] and the `with_*` methods, since which fields exist
/// depends on Core's features.
#[non_exhaustive]
pub struct WorkspaceSpec {
    /// The desktop app's data dir, or a web user's `DATA_DIR/users/<id>`.
    pub data_dir: PathBuf,
    /// The metadata file's name inside `data_dir`: [`DESKTOP_STORAGE_FILE`]
    /// by default; the web server uses `meta.db`.
    #[cfg(feature = "storage")]
    pub storage_file: String,
    #[cfg(feature = "storage")]
    pub storage_options: StorageOptions,
    /// The desktop app's keychain. The web server has none, and secret calls
    /// on its workspaces fail with `NOT_SUPPORTED`.
    #[cfg(feature = "secrets")]
    pub secrets: Option<Arc<dyn SecretStore>>,
}

impl WorkspaceSpec {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            #[cfg(feature = "storage")]
            storage_file: DESKTOP_STORAGE_FILE.to_string(),
            #[cfg(feature = "storage")]
            storage_options: StorageOptions::default(),
            #[cfg(feature = "secrets")]
            secrets: None,
        }
    }

    #[cfg(feature = "storage")]
    #[must_use]
    pub fn with_storage_file(mut self, name: impl Into<String>) -> Self {
        self.storage_file = name.into();
        self
    }

    /// How the storage opens. `StorageOptions { read_only: true, .. }`
    /// opens it without writing (the CLI).
    #[cfg(feature = "storage")]
    #[must_use]
    pub fn with_storage_options(mut self, options: StorageOptions) -> Self {
        self.storage_options = options;
        self
    }

    #[cfg(feature = "secrets")]
    #[must_use]
    pub fn with_secrets(mut self, store: Arc<dyn SecretStore>) -> Self {
        self.secrets = Some(store);
        self
    }
}

impl fmt::Debug for WorkspaceSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = f.debug_struct("WorkspaceSpec");
        s.field("data_dir", &self.data_dir);
        #[cfg(feature = "storage")]
        s.field("storage_file", &self.storage_file)
            .field("storage_options", &self.storage_options);
        #[cfg(feature = "secrets")]
        s.field("secrets", &self.secrets.as_ref().map(|_| "<store>"));
        s.finish()
    }
}

/// One user's open storage and secret store.
pub struct Workspace {
    data_dir: PathBuf,
    #[cfg(feature = "storage")]
    storage: Storage,
    #[cfg(feature = "secrets")]
    secrets: Option<Arc<dyn SecretStore>>,
}

impl Workspace {
    pub(crate) async fn open(spec: WorkspaceSpec) -> Result<Self, CoreError> {
        #[cfg(feature = "storage")]
        let storage = Storage::open(spec.data_dir.join(&spec.storage_file), spec.storage_options)
            .await
            .map_err(CoreError::from)?;
        Ok(Self {
            data_dir: spec.data_dir,
            #[cfg(feature = "storage")]
            storage,
            #[cfg(feature = "secrets")]
            secrets: spec.secrets,
        })
    }

    /// The dir this workspace was opened on.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// The metadata storage. Pass it to the query modules in
    /// [`crate::storage`] (`storage::connections::load_all(ws.storage())`).
    #[cfg(feature = "storage")]
    pub fn storage(&self) -> &Storage {
        &self.storage
    }

    /// The secret store, or `None` on a workspace without one (the web
    /// server's).
    #[cfg(feature = "secrets")]
    pub fn secrets(&self) -> Option<&dyn SecretStore> {
        self.secrets.as_deref()
    }

    /// Close the storage's connections. Calls made after this fail. The web
    /// server calls it when it evicts a workspace.
    pub async fn close(&self) {
        #[cfg(feature = "storage")]
        self.storage.close().await;
    }
}

/// How [`Workspace::connect_saved`] treats an SSH server's host key.
#[cfg(all(
    feature = "storage",
    feature = "secrets",
    feature = "ssh",
    feature = "workspace"
))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyPolicy {
    /// Accept only a host already in known_hosts. An unknown one fails with
    /// `UNKNOWN_HOST_KEY`, and known_hosts isn't written. The MCP server.
    KnownOnly,
    /// Also accept, and record in known_hosts, an unknown host whose key has
    /// this fingerprint (`SHA256:…`), the one the user approved in the trust
    /// prompt. For the GUI once it connects through Core (phase 5).
    Trust(String),
}

/// How [`Workspace::connect_saved`] connects: the host key policy, and
/// whether the engine is locked down. A [`HostKeyPolicy`] converts into
/// options with `restricted` off, so `connect_saved(core, id, policy)` still
/// reads as before.
#[cfg(all(
    feature = "storage",
    feature = "secrets",
    feature = "ssh",
    feature = "workspace"
))]
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ConnectSavedOptions {
    pub host_key: HostKeyPolicy,
    /// Sets [`seaquel_types::ConnectConfig::restricted`]: a DuckDB
    /// connection opens its instance with no access to files but its own
    /// database, no extension installs or loads, and its configuration
    /// locked. Other engines ignore it. For the MCP server, whose DuckDB
    /// instances are its own; the GUI leaves it off, since the lock can't be
    /// undone on a running instance and would break the editor's file
    /// functions.
    pub restricted: bool,
}

#[cfg(all(
    feature = "storage",
    feature = "secrets",
    feature = "ssh",
    feature = "workspace"
))]
impl ConnectSavedOptions {
    pub fn new(host_key: HostKeyPolicy) -> Self {
        Self {
            host_key,
            restricted: false,
        }
    }

    /// See [`ConnectSavedOptions::restricted`].
    pub fn restricted(mut self, restricted: bool) -> Self {
        self.restricted = restricted;
        self
    }
}

#[cfg(all(
    feature = "storage",
    feature = "secrets",
    feature = "ssh",
    feature = "workspace"
))]
impl From<HostKeyPolicy> for ConnectSavedOptions {
    fn from(host_key: HostKeyPolicy) -> Self {
        Self::new(host_key)
    }
}

/// `Workspace::connect_saved` for an id with no saved connection.
#[cfg(all(
    feature = "storage",
    feature = "secrets",
    feature = "ssh",
    feature = "workspace"
))]
pub const SAVED_CONNECTION_NOT_FOUND: &str = "CONNECTION_NOT_FOUND";

/// `Workspace::connect_saved` when the secret store refused a read the
/// connection needs (a denied keychain prompt, a locked keychain).
#[cfg(all(
    feature = "storage",
    feature = "secrets",
    feature = "ssh",
    feature = "workspace"
))]
pub const SECRET_UNREADABLE: &str = "SECRET_UNREADABLE";

/// `Workspace::connect_saved` on a workspace without a secret store (the web
/// server's) for a connection that needs a saved secret.
#[cfg(all(
    feature = "storage",
    feature = "secrets",
    feature = "ssh",
    feature = "workspace"
))]
pub const NO_SECRET_STORE: &str = "NO_SECRET_STORE";

#[cfg(all(
    feature = "storage",
    feature = "secrets",
    feature = "ssh",
    feature = "workspace"
))]
use seaquel_workspace::connections::UnreadableSecret;

#[cfg(all(
    feature = "storage",
    feature = "secrets",
    feature = "ssh",
    feature = "workspace"
))]
impl Workspace {
    /// Connect a saved connection on `core`: load its row, read its secrets
    /// from this workspace's secret store, open its SSH tunnel if it has one,
    /// and open it on Core. Returns Core's connection id.
    ///
    /// The config is `seaquel_workspace::connections`' port of how the
    /// desktop app connects the row (its docs and the connect-config
    /// fixtures say which path each row takes). Secrets are read the way the
    /// app's autoReconnect reads them, and the same rows give up, with
    /// `CREDENTIALS_REQUIRED` and a message naming the connection. There is
    /// one connect attempt: no retry with another config after a failure.
    ///
    /// `options` is a [`HostKeyPolicy`] or a [`ConnectSavedOptions`], whose
    /// `restricted` locks a DuckDB instance down (the MCP server).
    ///
    /// The tunnel lives as long as the connection: [`crate::Core::disconnect`]
    /// closes it, a failed connect closes it, and so does dropping Core. It
    /// is also closed if this future is dropped before it finishes.
    ///
    /// Errors: `CONNECTION_NOT_FOUND` (no such saved connection),
    /// `SECRET_UNREADABLE` (the secret store refused a read the row needs;
    /// checked before anything is opened), `NO_SECRET_STORE` (the row needs a
    /// secret and this workspace has no store), `CREDENTIALS_REQUIRED`,
    /// `INVALID_CONNECTION`, the storage codes, the
    /// SSH codes (`UNKNOWN_HOST_KEY` says to connect once in the app), and
    /// Core's connect errors. No message contains a secret: any secret the
    /// driver or SSH layer echoes is replaced by `<redacted>`.
    pub async fn connect_saved(
        &self,
        core: &crate::Core,
        id: &str,
        options: impl Into<ConnectSavedOptions>,
    ) -> Result<String, CoreError> {
        use seaquel_workspace::connections::{build_config, read_secrets, tunnel_config};

        let ConnectSavedOptions {
            host_key: policy,
            restricted,
        } = options.into();

        log::info!(activity = "workspace.connect_saved", saved_connection_id = id; "Connecting a saved connection");
        let row = seaquel_storage::connections::load_all(&self.storage)
            .await?
            .into_iter()
            .find(|c| c.id == id)
            .ok_or_else(|| {
                CoreError::new(
                    SAVED_CONNECTION_NOT_FOUND,
                    format!("Saved connection not found: {id}"),
                )
            })?;

        // Every failed read, including those `read_secrets` gives up after
        // (it returns no `Secrets` then).
        let failed: std::sync::Mutex<Vec<UnreadableSecret>> = std::sync::Mutex::default();
        let store = self.secrets.clone();
        let read = read_secrets(&row, |key: String| {
            let store = store.clone();
            let failed = &failed;
            async move {
                let code = match store {
                    Some(store) => match store.get(&key).await {
                        Ok(value) => return Ok(value),
                        Err(e) => e.code().to_string(),
                    },
                    None => NO_SECRET_STORE.to_string(),
                };
                failed
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(UnreadableSecret {
                        key,
                        code: code.clone(),
                    });
                Err(code)
            }
        })
        .await;
        // The app would connect without an unreadable secret (and the fixtures
        // keep that); here a denied keychain prompt must not turn into a
        // password-less attempt or a misleading auth error.
        let failed = failed
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(first) = failed.first() {
            return Err(unreadable_error(&row.name, first));
        }
        let secrets = read.map_err(config_error)?;

        let trust = match &policy {
            HostKeyPolicy::KnownOnly => None,
            HostKeyPolicy::Trust(fingerprint) => Some(fingerprint.clone()),
        };
        let tunnel = match tunnel_config(&row, &secrets, trust).map_err(config_error)? {
            Some(config) => Some(core.ssh_open(&config).await.map_err(|e| {
                let e = redact(e, &secrets);
                if e.code == "UNKNOWN_HOST_KEY" && policy == HostKeyPolicy::KnownOnly {
                    CoreError::new(
                        e.code,
                        format!(
                            "The SSH server of connection {:?} isn't a known host yet. Connect \
                             to it once in the Seaquel app and trust its host key. ({})",
                            row.name, e.message
                        ),
                    )
                } else {
                    e
                }
            })?),
            None => None,
        };
        // Closes the tunnel if this future is dropped from here on.
        let guard = tunnel.as_ref().map(|t| core.tunnel_guard(&t.tunnel_id));

        let config = build_config(&row, &secrets, tunnel.as_ref().map(|t| t.local_port)).map(
            |mut config| {
                if restricted {
                    config.restricted = Some(true);
                }
                config
            },
        );
        let connected = match config {
            Ok(config) => core
                .connect(&config)
                .await
                .map_err(|e| redact(CoreError::new(e.code, e.message), &secrets)),
            Err(e) => Err(config_error(e)),
        };
        let opened = tunnel.zip(guard);
        match connected {
            Ok(result) => {
                if let Some((tunnel, guard)) = opened {
                    // Now `disconnect` closes it.
                    core.own_tunnel(&result.connection_id, &tunnel.tunnel_id);
                    guard.keep();
                }
                Ok(result.connection_id)
            }
            Err(e) => {
                if let Some((_, guard)) = opened {
                    guard.close().await;
                }
                Err(e)
            }
        }
    }
}

#[cfg(all(
    feature = "storage",
    feature = "secrets",
    feature = "ssh",
    feature = "workspace"
))]
fn unreadable_error(name: &str, failed: &UnreadableSecret) -> CoreError {
    let what = if failed.key.starts_with("ssh-key:") {
        "SSH key passphrase"
    } else if failed.key.starts_with("ssh:") {
        "SSH password"
    } else {
        "password"
    };
    if failed.code == NO_SECRET_STORE {
        return CoreError::new(
            NO_SECRET_STORE,
            format!(
                "Connection {name:?} needs its saved {what}, but this workspace has no secret \
                 store to read it from."
            ),
        );
    }
    CoreError::new(
        SECRET_UNREADABLE,
        format!(
            "Seaquel couldn't read the saved {what} of connection {name:?} from the keychain \
             ({}). Allow Seaquel to access the keychain when the system asks, or open the \
             connection in the Seaquel app and save the {what} again.",
            failed.code
        ),
    )
}

#[cfg(all(
    feature = "storage",
    feature = "secrets",
    feature = "ssh",
    feature = "workspace"
))]
fn config_error(e: seaquel_workspace::connections::ConfigError) -> CoreError {
    CoreError::new(e.code, e.message)
}

/// Secrets shorter than this aren't redacted (see [`redact`]).
#[cfg(all(
    feature = "storage",
    feature = "secrets",
    feature = "ssh",
    feature = "workspace"
))]
const MIN_REDACTED_LEN: usize = 4;

/// `e` with every secret in `secrets`, raw or percent-encoded as it goes
/// into a URL, replaced by `<redacted>`.
///
/// Secrets shorter than [`MIN_REDACTED_LEN`] characters are left alone: a
/// one- to three-character string matches ordinary text ("pw", "sa", "1"),
/// so replacing it would garble the message, and where the `<redacted>`
/// markers landed would itself give the secret away. Drivers don't echo
/// passwords; this is a second line of defence for real ones.
#[cfg(all(
    feature = "storage",
    feature = "secrets",
    feature = "ssh",
    feature = "workspace"
))]
fn redact(mut e: CoreError, secrets: &seaquel_workspace::connections::Secrets) -> CoreError {
    use seaquel_workspace::connection_string::encode_uri_component;
    for secret in [&secrets.db, &secrets.ssh, &secrets.ssh_key]
        .into_iter()
        .flatten()
        .filter(|s| s.chars().count() >= MIN_REDACTED_LEN)
    {
        for form in [secret.clone(), encode_uri_component(secret)] {
            if e.message.contains(&form) {
                e.message = e.message.replace(&form, "<redacted>");
            }
        }
    }
    e
}

impl fmt::Debug for Workspace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = f.debug_struct("Workspace");
        s.field("data_dir", &self.data_dir);
        #[cfg(feature = "storage")]
        s.field("storage", &self.storage.path());
        #[cfg(feature = "secrets")]
        s.field("secrets", &self.secrets.as_ref().map(|_| "<store>"));
        s.finish()
    }
}

/// A Core failure outside a database connection, with the same shape as
/// `DbError`. Storage keeps its codes: `LEGACY_STORAGE`, `STORAGE_CORRUPT`,
/// `NO_DATA_DIR`, `STORAGE_ERROR`, and from a read-only open
/// `STORAGE_NEEDS_UPGRADE` and `STORAGE_NOT_FOUND`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreError {
    pub code: String,
    pub message: String,
}

impl CoreError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CoreError {}

#[cfg(feature = "storage")]
impl From<seaquel_storage::StorageError> for CoreError {
    fn from(e: seaquel_storage::StorageError) -> Self {
        Self::new(e.code(), e.to_string())
    }
}

#[cfg(feature = "secrets")]
impl From<seaquel_secrets::SecretError> for CoreError {
    fn from(e: seaquel_secrets::SecretError) -> Self {
        Self::new(e.code(), e.to_string())
    }
}

#[cfg(all(
    test,
    feature = "storage",
    feature = "secrets",
    feature = "ssh",
    feature = "workspace"
))]
mod redact_tests {
    use super::*;
    use seaquel_workspace::connections::Secrets;

    fn secrets() -> Secrets {
        Secrets {
            db: Some("db p@ss%1".into()),
            ssh: Some("ssh/pw+x".into()),
            ssh_key: Some("key:phrase&".into()),
            ..Secrets::default()
        }
    }

    fn redacted(message: &str, secrets: &Secrets) -> String {
        redact(CoreError::new("X", message), secrets).message
    }

    #[test]
    fn raw_and_url_encoded_secrets_are_redacted() {
        let s = secrets();
        for (raw, encoded) in [
            ("db p@ss%1", "db%20p%40ss%251"),
            ("ssh/pw+x", "ssh%2Fpw%2Bx"),
            ("key:phrase&", "key%3Aphrase%26"),
        ] {
            assert_eq!(
                redacted(&format!("failed near {raw} here"), &s),
                "failed near <redacted> here"
            );
            assert_eq!(
                redacted(&format!("postgres://u:{encoded}@h/db refused"), &s),
                "postgres://u:<redacted>@h/db refused"
            );
        }
        assert_eq!(redacted("nothing secret", &s), "nothing secret");
    }

    #[test]
    fn short_secrets_are_left_alone() {
        let s = Secrets {
            db: Some("sa".into()),
            ssh: Some("abc".into()),
            ..Secrets::default()
        };
        assert_eq!(
            redacted("login failed for user sa (abc)", &s),
            "login failed for user sa (abc)"
        );
        // Four characters is enough.
        let s = Secrets {
            db: Some("abcd".into()),
            ..Secrets::default()
        };
        assert_eq!(redacted("x abcd y", &s), "x <redacted> y");
    }
}
