//! Where the terminal binaries read from, and the Core they build.

use std::path::PathBuf;
use std::sync::Arc;

use seaquel_core::{CoreBuilder, CoreError, DuckdbHelper, LocalFiles};

/// The desktop app's identifier, which names its data dir (`.dev` in a
/// debug build, like the dev app).
pub const APP_IDENTIFIER: &str = if cfg!(debug_assertions) {
    "app.seaquel.desktop.dev"
} else {
    "app.seaquel.desktop"
};

/// The desktop app's data dir: `SEAQUEL_DATA_DIR` when set, else the
/// platform's data dir plus [`APP_IDENTIFIER`].
pub fn data_dir() -> Result<PathBuf, CoreError> {
    seaquel_core::storage::data_dir(APP_IDENTIFIER).map_err(CoreError::from)
}

/// What a terminal binary adds to [`core_builder`]'s Core.
#[derive(Debug, Clone, Default)]
pub struct CoreOptions {
    /// Lets Core read the user's files (the CLI, as the desktop: Decision
    /// 31 of phase 5e).
    pub local_files: Option<LocalFiles>,
    /// The known_hosts file in place of `~/.ssh/known_hosts` (the test
    /// hook).
    pub known_hosts: Option<PathBuf>,
    /// The DuckDB helper's `bin/duckdb` folder; `None` is
    /// [`duckdb_helper_dir`]. In-process tests pass one under their temp
    /// data dir.
    pub duckdb_helper_dir: Option<PathBuf>,
    /// `_DUCKDB_HELPER`: a built helper linked into that folder's layout
    /// when the Core is built. Ignored in a release build.
    pub test_duckdb_helper: Option<PathBuf>,
    /// `_DUCKDB_RELEASES`: the helper's download from this loopback server
    /// instead of GitHub. Only with the `duckdb-helper-install` feature,
    /// and ignored in a release build.
    pub duckdb_releases: Option<String>,
    /// `_SLOW_READ_MS`: a pause before each read of a file `install --from`
    /// copies. Only with the `duckdb-helper-install` feature, and ignored
    /// in a release build.
    pub slow_file_reads: Option<std::time::Duration>,
}

impl CoreOptions {
    /// These options plus what `hooks` set: the known_hosts file, the
    /// DuckDB helper and its release server. (`hooks` is empty in a release
    /// build.)
    #[must_use]
    pub fn with_hooks(mut self, hooks: &crate::TestHooks) -> Self {
        if let Some(path) = hooks.known_hosts() {
            self.known_hosts = Some(PathBuf::from(path));
        }
        if let Some(path) = hooks.duckdb_helper() {
            self.test_duckdb_helper = Some(PathBuf::from(path));
        }
        if let Some(base) = hooks.duckdb_releases() {
            self.duckdb_releases = Some(base.to_string());
        }
        if let Some(pause) = hooks.slow_read() {
            self.slow_file_reads = Some(pause);
        }
        self
    }
}

/// The DuckDB helper's folder, `<data_local_dir>/<identifier>/bin/duckdb`,
/// beside the command line tool the app installs; under
/// `SEAQUEL_DATA_DIR` when that is set. Each version's helper is
/// `<this>/<version>/seaquel-duckdb[.exe]`.
pub fn duckdb_helper_dir() -> Result<PathBuf, CoreError> {
    let root = seaquel_core::storage::data_local_dir(APP_IDENTIFIER).map_err(CoreError::from)?;
    Ok(root.join("bin").join("duckdb"))
}

/// A terminal binary's Core, as the desktop app builds its own: every
/// engine, `ConnectPolicy::Unrestricted` (it connects wherever its user
/// asks) and `TokioExecutor`, plus `options`. Each binary adds what only it
/// has (the CLI's import paths, the TUI's AI client) on the builder this
/// returns.
///
/// DuckDB is the remote engine: each
/// connection runs in a `seaquel-duckdb` helper of this app version, found
/// under `options.duckdb_helper_dir` (else [`duckdb_helper_dir`]). The
/// other engines come through `with_plugins(|id| id != "duckdb")`, so a
/// build where Cargo unified the native driver in (a workspace test build)
/// still registers DuckDB once, and remotely. With no data-local dir (no
/// home, no `SEAQUEL_DATA_DIR`) DuckDB isn't registered: a connect is
/// `ENGINE_NOT_AVAILABLE`. Both binaries stop before that anyway, on the
/// data dir.
pub fn core_builder(options: CoreOptions) -> CoreBuilder {
    let mut builder = seaquel_core::with_plugins(|id| id != "duckdb")
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .executor(Arc::new(seaquel_runtime::TokioExecutor));
    #[cfg(debug_assertions)]
    let explicit_dir = options.duckdb_helper_dir.is_some();
    let dir = match options.duckdb_helper_dir {
        Some(dir) => Some(dir),
        None => match duckdb_helper_dir() {
            Ok(dir) => Some(dir),
            Err(e) => {
                log::warn!(activity = "duckdb.helper", code = e.code.as_str(); "No folder for the DuckDB helper; DuckDB is unavailable");
                None
            }
        },
    };
    if let Some(dir) = dir {
        let helper = DuckdbHelper {
            dir,
            version: crate::VERSION.to_string(),
        };
        #[cfg(debug_assertions)]
        if let Some(file) = &options.test_duckdb_helper {
            let env = std::env::var_os(seaquel_core::storage::DATA_DIR_ENV);
            if !may_link_test_helper(explicit_dir, env.as_deref()) {
                log::warn!(activity = "duckdb.helper"; "The DuckDB helper test hook is set without SEAQUEL_DATA_DIR; it isn't linked into the real data dir");
            } else if let Err(e) = link_test_helper(file, &helper) {
                log::warn!(activity = "duckdb.helper", kind:? = e.kind(); "The DuckDB helper test hook's file couldn't be linked");
            }
        }
        builder = builder.duckdb_helper(helper);
    }
    #[cfg(all(feature = "duckdb-helper-install", debug_assertions))]
    if let Some(pause) = options.slow_file_reads {
        builder = builder.duckdb_helper_slow_file_reads(pause);
    }
    #[cfg(all(feature = "duckdb-helper-install", debug_assertions))]
    if let Some(base) = &options.duckdb_releases {
        let base = base.trim_end_matches('/');
        match seaquel_core::DuckdbHelperReleases::new(
            &format!("{base}/api"),
            &format!("{base}/download"),
        ) {
            Ok(source) => builder = builder.duckdb_helper_releases(source),
            Err(e) => {
                log::warn!(activity = "duckdb.helper", code = e.code(); "The DuckDB release test hook was refused");
            }
        }
    }
    if let Some(local_files) = options.local_files {
        builder = builder.local_files(local_files);
    }
    if let Some(path) = options.known_hosts {
        builder = builder.ssh_known_hosts(path);
    }
    builder
}

/// Whether the `_DUCKDB_HELPER` hook may write its link: only
/// into a folder the caller chose (`CoreOptions::duckdb_helper_dir`) or
/// under a non-empty `SEAQUEL_DATA_DIR`, so a debug binary run with the
/// hook but without the data-dir override never writes into the real
/// data-local dir.
#[cfg(any(debug_assertions, test))]
fn may_link_test_helper(explicit_dir: bool, data_dir_env: Option<&std::ffi::OsStr>) -> bool {
    explicit_dir || data_dir_env.is_some_and(|v| !v.is_empty())
}

/// The `_DUCKDB_HELPER` hook (debug builds): `file` hard-linked (else
/// copied) to `helper.path()`, the folders from `<identifier>` down made
/// as the install makes them (0700 on Unix), and whatever was there before
/// replaced. The hook names one built file; Core's start checks the
/// install's layout and permissions, so the hook builds that layout rather
/// than the locator gaining a second, unchecked form (Task 6's choice).
#[cfg(debug_assertions)]
fn link_test_helper(file: &std::path::Path, helper: &DuckdbHelper) -> std::io::Result<()> {
    let target = helper.path();
    let folder = target
        .parent()
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(folder)?;
    match std::fs::remove_file(&target) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    if std::fs::hard_link(file, &target).is_err() {
        std::fs::copy(file, &target)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_identifier_follows_the_build() {
        if cfg!(debug_assertions) {
            assert_eq!(APP_IDENTIFIER, "app.seaquel.desktop.dev");
        } else {
            assert_eq!(APP_IDENTIFIER, "app.seaquel.desktop");
        }
    }

    #[test]
    fn local_files_only_when_asked() {
        let core = core_builder(CoreOptions::default()).build();
        assert_eq!(core.local_files(), None);
        let core = core_builder(CoreOptions {
            local_files: Some(LocalFiles::Allowed),
            ..CoreOptions::default()
        })
        .build();
        assert_eq!(core.local_files(), Some(LocalFiles::Allowed));
    }

    /// The `_DUCKDB_HELPER` hook writes only into a folder the
    /// caller chose (`duckdb_helper_dir`) or under `SEAQUEL_DATA_DIR`, never
    /// into the real data-local dir.
    #[test]
    fn the_helper_hook_links_only_into_a_chosen_folder() {
        use std::ffi::OsStr;
        assert!(may_link_test_helper(true, None));
        assert!(may_link_test_helper(true, Some(OsStr::new(""))));
        assert!(may_link_test_helper(false, Some(OsStr::new("/tmp/data"))));
        assert!(!may_link_test_helper(false, None));
        assert!(!may_link_test_helper(false, Some(OsStr::new(""))));
    }

    #[test]
    fn every_engine_is_registered() {
        let core = core_builder(CoreOptions::default()).build();
        let mut ids = core.engine_ids();
        ids.sort_unstable();
        assert_eq!(ids, ["duckdb", "mssql", "mysql", "postgres", "sqlite"]);
    }
}
