use std::path::PathBuf;

/// The wire code for [`StorageError::Legacy`].
pub const LEGACY_STORAGE: &str = "LEGACY_STORAGE";
/// The wire code for [`StorageError::Corrupt`].
pub const STORAGE_CORRUPT: &str = "STORAGE_CORRUPT";
/// The wire code for [`StorageError::NoDataDir`]. Like the two above,
/// retrying can't fix it, so the desktop keeps it and the UI shows it.
pub const NO_DATA_DIR: &str = "NO_DATA_DIR";
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
    /// [`NO_DATA_DIR`] or [`STORAGE_ERROR`].
    pub fn code(&self) -> &'static str {
        match self {
            StorageError::Legacy { .. } => LEGACY_STORAGE,
            StorageError::Corrupt { .. } => STORAGE_CORRUPT,
            StorageError::NoDataDir => NO_DATA_DIR,
            _ => STORAGE_ERROR,
        }
    }
}
