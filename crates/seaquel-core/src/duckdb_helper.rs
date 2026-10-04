//! The DuckDB helper's status and install (the DuckDB helper plan, Task 4,
//! Decisions 9 and 10).
//!
//! - [`Core::duckdb_helper_status`] (`engine-duckdb-remote`, no HTTP): is
//!   this version's helper there and does it pass the check every start
//!   runs ([`DuckdbHelper::check`])?
//! - [`Core::duckdb_helper_asset`], [`Core::duckdb_helper_install`] and
//!   [`Core::duckdb_helper_install_from_file`] (`duckdb-helper-install`):
//!   the release asset `seaquel-duckdb-<triple>[.exe].gz`, downloaded and
//!   installed by `seaquel-http`'s `release_asset` into
//!   `<dir>/<version>/seaquel-duckdb[.exe]`, the path the check looks at.
//!   The install makes `bin`, `bin/duckdb` and the version folder 0700 and
//!   takes group and world write off `<identifier>` (the folder above
//!   `bin`), so the check passes on what it made. An install of a version
//!   that is already there and intact downloads nothing. A successful one
//!   removes version folders older than the two newest, never the running
//!   version's.
//!
//! The interface installs before it connects again: `RemoteEngine::open`
//! fails at once with `ENGINE_NOT_INSTALLED`, so Core's connect timeout
//! never covers a download. The first start of a newly installed file gets
//! the 20 s retry (`remote/process.rs`, Task 3's M5): the file is new by
//! inode and mtime, so nothing needs doing here.
//!
//! Logs: `activity=duckdb.helper`, `event=install`, whether it downloaded,
//! byte counts, the number pruned, codes. Never a path or a URL.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::{Core, CoreError, DuckdbHelper};

#[cfg(feature = "duckdb-helper-install")]
pub use seaquel_http::release_asset::{
    Progress as DuckdbHelperProgress, ReleaseSource as DuckdbHelperReleases,
};

/// Whether this version's helper can be started.
#[derive(Clone, PartialEq, Eq)]
pub enum DuckdbHelperStatus {
    /// There, and it passes the start's check.
    Installed { path: PathBuf },
    /// Not there, and no other version's either.
    Missing,
    /// Not there, but another version's is: the terminal binary was
    /// updated since the last install.
    Outdated,
    /// There, but it or a folder above it is a symlink or writable by
    /// someone else (an install tightens folders this user owns).
    Unsafe,
}

impl fmt::Debug for DuckdbHelperStatus {
    /// Without the path (under the user's home).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Installed { .. } => "Installed",
            Self::Missing => "Missing",
            Self::Outdated => "Outdated",
            Self::Unsafe => "Unsafe",
        })
    }
}

/// This platform's helper asset in this version's release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DuckdbHelperAsset {
    /// `seaquel-duckdb-<triple>[.exe].gz`.
    pub name: String,
    /// Compressed, in bytes: what the download will be.
    pub size: u64,
}

/// A finished install.
#[derive(Clone, PartialEq, Eq)]
pub struct DuckdbHelperInstalled {
    pub path: PathBuf,
    /// `false` when this version was already installed and intact.
    pub downloaded: bool,
    /// Old version folders removed.
    pub pruned: usize,
}

impl fmt::Debug for DuckdbHelperInstalled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DuckdbHelperInstalled")
            .field("downloaded", &self.downloaded)
            .field("pruned", &self.pruned)
            .finish_non_exhaustive()
    }
}

/// Version folders kept besides the running version's.
#[cfg(feature = "duckdb-helper-install")]
const KEEP_VERSIONS: usize = 2;

fn no_helper() -> CoreError {
    CoreError::new("NOT_SUPPORTED", "this build doesn't run DuckDB in a helper")
}

/// `2026.10.1` as numbers; `None` for a name that isn't a version.
#[cfg(feature = "duckdb-helper-install")]
fn version_key(name: &str) -> Option<Vec<u64>> {
    let parts: Option<Vec<u64>> = name.split('.').map(|p| p.parse().ok()).collect();
    parts.filter(|p| !p.is_empty())
}

/// Whether `entry` (a folder in `dir`) holds a helper.
fn holds_helper(entry: &Path) -> bool {
    std::fs::symlink_metadata(entry.join(DuckdbHelper::file_name()))
        .map(|m| m.is_file())
        .unwrap_or(false)
}

impl Core {
    fn helper(&self) -> Result<&DuckdbHelper, CoreError> {
        self.duckdb_helper.as_ref().ok_or_else(no_helper)
    }

    /// Whether this version's helper is installed and passes the check
    /// every start runs. Reads only metadata: no hash, no network.
    pub fn duckdb_helper_status(&self) -> Result<DuckdbHelperStatus, CoreError> {
        let helper = self.helper()?;
        if let Ok(path) = helper.check() {
            return Ok(DuckdbHelperStatus::Installed { path });
        }
        if std::fs::symlink_metadata(helper.path()).is_ok() {
            return Ok(DuckdbHelperStatus::Unsafe);
        }
        let other = std::fs::read_dir(&helper.dir)
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| e.file_name() != helper.version.as_str() && holds_helper(&e.path()));
        Ok(if other {
            DuckdbHelperStatus::Outdated
        } else {
            DuckdbHelperStatus::Missing
        })
    }
}

#[cfg(feature = "duckdb-helper-install")]
mod install {
    use super::*;
    use log::{info, warn};
    use seaquel_http::release_asset::{
        install_file_cancellable, installed, prepare_target, target_triple, Fetcher, InstallError,
        InstallTarget,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// Sets its flag when dropped: a dropped `install_from_file` future
    /// stops the blocking copy between reads, so its partial file goes
    /// (review I2).
    struct CancelOnDrop(Arc<AtomicBool>);

    impl Drop for CancelOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
        }
    }

    /// `f` on tokio's blocking pool (review M2: hashing a 35 MB file and
    /// copying one off the async threads), as `imports.rs` reads files.
    async fn blocking<T: Send + 'static>(
        f: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, CoreError> {
        tokio::task::spawn_blocking(f)
            .await
            .map_err(|_| CoreError::new("FILE_ERROR", "the DuckDB helper's install stopped"))
    }

    fn from_install(e: InstallError) -> CoreError {
        CoreError::new(e.code(), e.message)
    }

    /// `seaquel-duckdb-<triple>[.exe].gz`, as `release.yml` names it.
    pub(super) fn asset_name() -> Result<String, CoreError> {
        let triple =
            target_triple(std::env::consts::OS, std::env::consts::ARCH).ok_or_else(|| {
                CoreError::new(
                    "NOT_SUPPORTED",
                    "no DuckDB helper is published for this platform",
                )
            })?;
        Ok(format!(
            "seaquel-duckdb-{triple}{}.gz",
            std::env::consts::EXE_SUFFIX
        ))
    }

    /// `<identifier>` / `bin` / `duckdb` / `<version>` / the file, from the
    /// locator's `dir` (`<identifier>/bin/duckdb`).
    fn target(helper: &DuckdbHelper) -> Result<InstallTarget, CoreError> {
        let invalid = || {
            CoreError::new(
                "INVALID_ARGUMENT",
                "the DuckDB helper's folder isn't <app folder>/bin/duckdb",
            )
        };
        let name = |p: &Path| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(str::to_string)
                .ok_or_else(invalid)
        };
        let duckdb = name(&helper.dir)?;
        let bin_dir = helper.dir.parent().ok_or_else(invalid)?;
        let bin = name(bin_dir)?;
        let root = bin_dir.parent().ok_or_else(invalid)?.to_path_buf();
        Ok(InstallTarget {
            root,
            folders: vec![bin, duckdb, helper.version.clone()],
            file_name: DuckdbHelper::file_name(),
        })
    }

    /// Removes version folders in `dir` older than the [`KEEP_VERSIONS`]
    /// newest, never `running`'s. Names that aren't versions and symlinks
    /// are left alone. Best effort (a running helper on Windows can't be
    /// removed); returns how many went.
    fn prune(dir: &Path, running: &str) -> usize {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        // Every version folder but the running one's, newest first, with
        // the running version ranked among them (it is never removed).
        let mut ranked: Vec<(Vec<u64>, Option<PathBuf>)> = entries
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                (name != running).then_some(())?;
                Some((version_key(&name)?, Some(e.path())))
            })
            .collect();
        if let Some(key) = version_key(running) {
            ranked.push((key, None));
        }
        ranked.sort_by(|a, b| b.0.cmp(&a.0));
        let mut removed = 0;
        for path in ranked
            .into_iter()
            .skip(KEEP_VERSIONS)
            .filter_map(|(_, p)| p)
        {
            match std::fs::remove_dir_all(&path) {
                Ok(()) => removed += 1,
                Err(e) => {
                    warn!(activity = "duckdb.helper", event = "prune", kind = format!("{:?}", e.kind()).as_str(); "an old DuckDB helper couldn't be removed");
                }
            }
        }
        removed
    }

    impl Core {
        fn fetcher(&self, helper: &DuckdbHelper) -> Fetcher {
            let source = self
                .duckdb_helper_releases
                .clone()
                .unwrap_or_else(DuckdbHelperReleases::github);
            Fetcher::new(source, &helper.version)
        }

        /// This platform's helper in this version's release: its name and
        /// download size, from the release metadata (one request).
        pub async fn duckdb_helper_asset(&self) -> Result<DuckdbHelperAsset, CoreError> {
            let helper = self.helper()?;
            let name = asset_name()?;
            let asset = self
                .fetcher(helper)
                .asset(&helper.version, &name)
                .await
                .map_err(from_install)?;
            Ok(DuckdbHelperAsset {
                name: asset.name,
                size: asset.size,
            })
        }

        /// The helper already there, intact (its file's SHA-256 is the one
        /// its install recorded) and passing the start's check.
        ///
        /// A folder that went loose since (review M5) is tightened first, as
        /// an install would, so this works offline too.
        async fn already_installed(&self, target: &InstallTarget, helper: &DuckdbHelper) -> bool {
            let (target, helper) = (target.clone(), helper.clone());
            blocking(move || {
                installed(&target).is_some()
                    && prepare_target(&target).is_ok()
                    && helper.check().is_ok()
            })
            .await
            .unwrap_or(false)
        }

        fn finish(
            &self,
            helper: &DuckdbHelper,
            downloaded: bool,
        ) -> Result<DuckdbHelperInstalled, CoreError> {
            // What the install made must pass what every start checks.
            let path = helper
                .check()
                .map_err(|e| Self::failed(CoreError::new(e.code, e.message)))?;
            let pruned = prune(&helper.dir, &helper.version);
            info!(activity = "duckdb.helper", event = "install", downloaded = downloaded, pruned = pruned; "the DuckDB helper is installed");
            Ok(DuckdbHelperInstalled {
                path,
                downloaded,
                pruned,
            })
        }

        fn failed(e: CoreError) -> CoreError {
            warn!(activity = "duckdb.helper", event = "install", code = e.code.as_str(); "the DuckDB helper's install failed");
            e
        }

        /// Downloads and installs this version's helper, unless it is
        /// already there and intact (then nothing is fetched). `progress`
        /// gets the compressed bytes received, about every 64 KiB and at
        /// the end. One install at a time per Core; a second waits and then
        /// finds the first's file. Dropping the future stops the download
        /// and deletes the partial file.
        pub async fn duckdb_helper_install(
            &self,
            progress: &mut (dyn FnMut(DuckdbHelperProgress) + Send),
        ) -> Result<DuckdbHelperInstalled, CoreError> {
            let helper = self.helper()?;
            let target = target(helper)?;
            let _turn = self.duckdb_helper_installing.lock().await;
            if self.already_installed(&target, helper).await {
                return self.finish(helper, false);
            }
            let name = asset_name()?;
            let fetcher = self.fetcher(helper);
            let result = async {
                let asset = fetcher.asset(&helper.version, &name).await?;
                fetcher.install(&asset, &target, progress).await
            }
            .await;
            match result {
                Ok(done) => {
                    info!(activity = "duckdb.helper", event = "install", bytes = done.size; "the DuckDB helper was downloaded");
                    self.finish(helper, true)
                }
                Err(e) => Err(Self::failed(from_install(e))),
            }
        }

        /// Installs a copy of the release asset the user brought over
        /// (`seaquel-cli duckdb install --from FILE --sha256 HEX`), checked
        /// against `sha256`, the asset's digest as the release page shows
        /// it. A file already gunzipped is checked against its own hash.
        ///
        /// The copy runs on the blocking pool. Dropping the future tells it
        /// to stop at its next read (`CANCELLED`, the partial file deleted);
        /// a caller about to exit should give the pool a moment
        /// (`Runtime::shutdown_timeout`) so that happens.
        pub async fn duckdb_helper_install_from_file(
            &self,
            file: &Path,
            sha256: &str,
        ) -> Result<DuckdbHelperInstalled, CoreError> {
            let helper = self.helper()?;
            let target = target(helper)?;
            let _turn = self.duckdb_helper_installing.lock().await;
            let (file, sha256) = (file.to_path_buf(), sha256.to_string());
            let cancel = CancelOnDrop(Arc::new(AtomicBool::new(false)));
            let flag = cancel.0.clone();
            let done =
                blocking(move || install_file_cancellable(&file, &sha256, &target, &flag)).await?;
            drop(cancel);
            match done {
                Ok(_) => self.finish(helper, true),
                Err(e) => Err(Self::failed(from_install(e))),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn prune_in(running: &str, present: &[&str]) -> Vec<String> {
            let tmp = tempfile::tempdir().unwrap();
            for v in present {
                std::fs::create_dir(tmp.path().join(v)).unwrap();
            }
            prune(tmp.path(), running);
            let mut left: Vec<String> = std::fs::read_dir(tmp.path())
                .unwrap()
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .collect();
            left.sort();
            left
        }

        #[test]
        fn prune_keeps_the_two_newest_and_the_running_version() {
            // The running version is the newest: one older one stays.
            assert_eq!(
                prune_in(
                    "2026.10.0",
                    &["2026.10.0", "2026.9.0", "2026.8.0", "2026.2.0"]
                ),
                ["2026.10.0", "2026.9.0"]
            );
            // The running version is older than two others: they stay too.
            assert_eq!(
                prune_in(
                    "2026.1.0",
                    &["2026.1.0", "2026.3.0", "2026.9.0", "2026.10.0"]
                ),
                ["2026.1.0", "2026.10.0", "2026.9.0"]
            );
            // The running version is the second newest.
            assert_eq!(
                prune_in("2026.9.0", &["2026.10.0", "2026.9.0", "2026.8.0"]),
                ["2026.10.0", "2026.9.0"]
            );
            // Not yet installed (a failed install never prunes, but still).
            assert_eq!(
                prune_in("2026.11.0", &["2026.10.0", "2026.9.0"]),
                ["2026.10.0"]
            );
            // Names that aren't versions stay; a running name that isn't a
            // version keeps the two newest.
            assert_eq!(
                prune_in("dev", &["dev", "x.y", "2026.1.0", "2026.2.0", "2026.3.0"]),
                ["2026.2.0", "2026.3.0", "dev", "x.y"]
            );
        }

        #[test]
        fn versions_compare_as_numbers() {
            assert!(version_key("2026.10.0") > version_key("2026.9.9"));
            assert_eq!(version_key("2026.x"), None);
            assert_eq!(version_key(""), None);
        }
    }
}
