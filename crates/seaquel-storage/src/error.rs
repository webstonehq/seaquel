use std::path::PathBuf;

/// The wire code for [`StorageError::Legacy`].
pub const LEGACY_STORAGE: &str = "LEGACY_STORAGE";
/// The wire code for [`StorageError::Corrupt`].
pub const STORAGE_CORRUPT: &str = "STORAGE_CORRUPT";
/// The wire code for [`StorageError::NoDataDir`]. Like the two above,
/// retrying can't fix it, so the desktop keeps it and the UI shows it.
pub const NO_DATA_DIR: &str = "NO_DATA_DIR";
/// The wire code for [`StorageError::NeedsUpgrade`] and
/// [`StorageError::DataStepPending`]: a read-only open found a file the app
/// hasn't brought up to date yet.
pub const STORAGE_NEEDS_UPGRADE: &str = "STORAGE_NEEDS_UPGRADE";
/// The wire code for [`StorageError::NotFound`]: a read-only open found no
/// file, and never creates one.
pub const STORAGE_NOT_FOUND: &str = "STORAGE_NOT_FOUND";
/// The wire code for [`StorageError::ReadOnly`]: a write on storage opened
/// with `StorageOptions::read_only`.
pub const STORAGE_READ_ONLY: &str = "STORAGE_READ_ONLY";
/// The wire code for every other storage failure.
pub const STORAGE_ERROR: &str = "STORAGE_ERROR";

/// Legacy `tauri-plugin-store` files. If one of them sits in the data dir and
/// `seaquel.db` doesn't, the data was never imported into SQLite.
pub const LEGACY_JSON_FILES: [&str; 3] = [
    "database_connections.json",
    "projects.json",
    "app_state.json",
];

/// A storage failure. [`StorageError::code`] gives its wire code.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// The data dir holds only the JSON files releases before 2026.4.5 wrote.
    /// This release can't import them; one from 2026.4.5 through 2026.9.x
    /// can.
    #[error(
        "{} holds data from a Seaquel release older than 2026.4.5 ({}), which this release \
         can't read. Install a release from 2026.4.5 through 2026.9.x, launch it once so it \
         imports that data, then upgrade to this release.",
        dir.display(),
        files.join(", ")
    )]
    Legacy { dir: PathBuf, files: Vec<String> },

    /// The file isn't a SQLite database, or SQLite reports it corrupt.
    /// `untouched` is true when that was found before anything could write
    /// to the file (a bad header, or the read-only probe), so it is byte for
    /// byte as it was. When the baseline or a migration hit the corruption,
    /// the file had already been switched to WAL mode.
    #[error(
        "{} isn't a readable Seaquel database ({reason}).{}",
        path.display(),
        if *untouched { " The file wasn't changed." } else { "" }
    )]
    Corrupt {
        path: PathBuf,
        reason: String,
        untouched: bool,
    },

    /// A read-only open ([`crate::StorageOptions::read_only`]) found a file
    /// that `Storage::open` would change: the baseline, a numbered migration
    /// or a data step is pending. The file was only read.
    #[error(
        "{} needs an update this program can't make ({reason}). Open the Seaquel app once to \
         update your data.",
        path.display()
    )]
    NeedsUpgrade { path: PathBuf, reason: String },

    /// A read-only open found the schema and migrations current but a data
    /// step (`data_steps.rs`) not recorded. Its code is
    /// [`STORAGE_NEEDS_UPGRADE`] too, since a caller treats it the same way,
    /// but the message differs: the app runs a pending step on every open
    /// and only logs a failure, so a step that keeps failing leaves the file
    /// here even after the app has opened it, and opening the app again
    /// won't help.
    #[error(
        "{} has a data cleanup ({step}) that hasn't run. If you haven't opened the Seaquel app \
         since updating it, open it once to update your data. If you have, the app couldn't \
         finish that cleanup: check the app's log for \"data step {step} failed\".",
        path.display()
    )]
    DataStepPending { path: PathBuf, step: String },

    /// A read-only open found no file at `path`. Nothing was created.
    #[error(
        "{} doesn't exist. Open the Seaquel app once to create it, or check the data directory \
         (SEAQUEL_DATA_DIR).",
        path.display()
    )]
    NotFound { path: PathBuf },

    /// A write transaction was asked of storage opened read-only (the CLI).
    /// Nothing was sent to SQLite.
    #[error("{} was opened read-only, so it can't be written", path.display())]
    ReadOnly { path: PathBuf },

    /// [`crate::Storage::write`] waited [`crate::WRITE_WAIT`] for this
    /// process's earlier writers and gave up: a write that never finishes,
    /// or a write begun while the caller holds another (a deadlock). Its
    /// code is [`STORAGE_ERROR`].
    #[error("{} is busy: an earlier write didn't finish within {} s", path.display(), waited.as_secs())]
    WriteLockTimeout {
        path: PathBuf,
        waited: std::time::Duration,
    },

    /// No data dir: `SEAQUEL_DATA_DIR` is unset and the platform has none
    /// (no home directory).
    #[error("no data directory: this platform reports none, and SEAQUEL_DATA_DIR isn't set")]
    NoDataDir,

    /// A file system call on `path` failed.
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// SQLite failed.
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),

    /// A numbered migration failed, or the file has one this build doesn't
    /// know (it was opened by a newer release).
    #[error(transparent)]
    Migrate(#[from] sqlx::migrate::MigrateError),
}

impl StorageError {
    /// The wire code: [`LEGACY_STORAGE`], [`STORAGE_CORRUPT`],
    /// [`STORAGE_NEEDS_UPGRADE`], [`STORAGE_NOT_FOUND`], [`STORAGE_READ_ONLY`],
    /// [`NO_DATA_DIR`] or [`STORAGE_ERROR`].
    pub fn code(&self) -> &'static str {
        match self {
            StorageError::Legacy { .. } => LEGACY_STORAGE,
            StorageError::Corrupt { .. } => STORAGE_CORRUPT,
            StorageError::NeedsUpgrade { .. } | StorageError::DataStepPending { .. } => {
                STORAGE_NEEDS_UPGRADE
            }
            StorageError::NotFound { .. } => STORAGE_NOT_FOUND,
            StorageError::ReadOnly { .. } => STORAGE_READ_ONLY,
            StorageError::NoDataDir => NO_DATA_DIR,
            _ => STORAGE_ERROR,
        }
    }
}
