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
/// `NO_DATA_DIR` and `STORAGE_ERROR`.
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
