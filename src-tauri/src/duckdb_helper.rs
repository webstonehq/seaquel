//! The GUI's DuckDB helper install:
//! desktop-only Tauri commands beside `cli_info`/`install_cli`.
//!
//! - [`duckdb_helper_offer`]: this version's helper's status and download
//!   size (no request with the pinned asset);
//! - [`duckdb_helper_install`]: download and install it, with progress over
//!   a `Channel`. A second install while one runs (the prefetch and the dialog)
//!  joins it: its progress from where it is, and the
//!   same answer, from one download;
//! - [`duckdb_helper_cancel`]: aborts the running install, so Core's future
//!   drops and the partial file goes; the install answers `CANCELLED`;
//! - [`duckdb_helper_install_file`]: "Install from a file…", checked
//!   against the pinned digest (Core's `install_from_file(None)`).
//!
//! Everything goes through the app's one Core, so these, the
//! CLI flow and the prefetch share Core's install lock. Logs:
//! `activity=duckdb.helper` with `event=offer|install|cancel`, codes,
//! `downloaded`, `pruned`; never a path, a URL or the file the user picked.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use futures::future::{AbortHandle, Abortable};
use serde::Serialize;
use tauri::ipc::Channel;
use tauri::State;
use tokio::sync::watch;

use seaquel_core::{Core, CoreError, DuckdbHelperInstalled, DuckdbHelperStatus};
use seaquel_rpc::RpcError;

use crate::cli_download;

/// Compressed bytes received of the download's total.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HelperProgress {
    pub bytes: u64,
    pub total: u64,
}

/// A finished install, for the page (no path: it names the user's home).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HelperInstalled {
    /// `false` when this version was already there and intact.
    pub downloaded: bool,
    /// Old version folders removed.
    pub pruned: usize,
}

impl From<DuckdbHelperInstalled> for HelperInstalled {
    fn from(done: DuckdbHelperInstalled) -> Self {
        Self {
            downloaded: done.downloaded,
            pruned: done.pruned,
        }
    }
}

/// This version's helper, as `Core::duckdb_helper_status` says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OfferStatus {
    Installed,
    Missing,
    Outdated,
    Unsafe,
}

/// What the dialog asks with: the status, and for a helper that isn't
/// installed the download's size (or why it couldn't be had).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HelperOffer {
    pub status: OfferStatus,
    /// The app's version, which the helper must match.
    pub version: String,
    /// Compressed bytes; `None` when installed or when it couldn't be had.
    pub size: Option<u64>,
    /// Why the size couldn't be had (no network, not published, …).
    pub size_error: Option<RpcError>,
    /// The release asset's file name (`seaquel-duckdb-<triple>[.exe].gz`),
    /// Core's, so the dialog can name the file to copy for "Install from a
    /// file…"; `None` when installed or when the asset couldn't be had.
    pub asset_name: Option<String>,
    /// Whether "Install from a file…" can check a file: only with the
    /// digest built in (the dialog never asks for a hash).
    pub from_file: bool,
}

fn cancelled() -> RpcError {
    RpcError::new("CANCELLED", "The DuckDB helper's install was cancelled.")
}

/// The status and size the dialog asks with. The size comes from the pin
/// when there is one (no request), else from the release metadata (one
/// request), and isn't asked for an installed helper.
pub async fn offer(core: &Core) -> Result<HelperOffer, RpcError> {
    let status = status_word(&core.duckdb_helper_status()?);
    let (size, asset_name, size_error) = match status {
        OfferStatus::Installed => (None, None, None),
        _ => match core.duckdb_helper_asset().await {
            Ok(asset) => (Some(asset.size), Some(asset.name), None),
            Err(e) => (None, None, Some(RpcError::from(e))),
        },
    };
    let code = size_error.as_ref().map(|e| e.code.as_str()).unwrap_or("");
    log::info!(activity = "duckdb.helper", event = "offer", status = format!("{status:?}").as_str(), code = code; "DuckDB helper offer");
    Ok(HelperOffer {
        status,
        version: cli_download::VERSION.to_string(),
        size,
        size_error,
        asset_name,
        from_file: core.duckdb_helper_pin().is_some(),
    })
}

/// A `seaquel-duckdb` built beside a debug app, used instead of a download
/// only where [`cli_download::may_use_debug_helper`] allows it.
/// `beside` finds it (`cli_download::debug_helper`). Release builds
/// always download.
pub fn local_helper(
    identifier: &str,
    data_dir_env: Option<&OsStr>,
    beside: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    if !cfg!(debug_assertions) || !cli_download::may_use_debug_helper(identifier, data_dir_env) {
        return None;
    }
    beside()
}

/// The running download, which a second install joins.
struct Running {
    id: u64,
    progress: watch::Receiver<Option<HelperProgress>>,
    result: watch::Receiver<Option<Result<HelperInstalled, RpcError>>>,
    abort: AbortHandle,
}

/// The app's helper installs in flight (managed state): at most one
/// download, which later installs join, and the file install, so
/// [`HelperInstalls::cancel`] can stop either.
#[derive(Default)]
pub struct HelperInstalls {
    running: Mutex<Option<Running>>,
    file: Mutex<Option<(u64, AbortHandle)>>,
    next: AtomicU64,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl HelperInstalls {
    /// Installs this version's helper: from `local` (a debug build's own,
    /// checked against its own hash) or downloaded, `on_progress` getting
    /// the download's progress. Joins an install already running instead
    /// of starting another.
    pub async fn install(
        &self,
        core: &Core,
        local: Option<PathBuf>,
        mut on_progress: impl FnMut(HelperProgress) + Send,
    ) -> Result<HelperInstalled, RpcError> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (progress_tx, progress_rx) = watch::channel(None);
        let (result_tx, result_rx) = watch::channel(None);
        let (abort, registration) = AbortHandle::new_pair();
        let joined = {
            let mut running = lock(&self.running);
            match &*running {
                Some(r) => Some((r.progress.clone(), r.result.clone())),
                None => {
                    *running = Some(Running {
                        id,
                        progress: progress_rx,
                        result: result_rx,
                        abort,
                    });
                    None
                }
            }
        };
        if let Some((progress, result)) = joined {
            return join(progress, result, on_progress).await;
        }

        // This call drives the install; the slot goes whenever it ends
        // (also when this future is dropped, which drops Core's install).
        let _slot = Slot {
            running: &self.running,
            id,
        };
        let install = async {
            match local {
                Some(file) => {
                    let sha256 = hash_file(&file).await?;
                    core.duckdb_helper_install_from_file(&file, Some(&sha256))
                        .await
                }
                None => {
                    core.duckdb_helper_install(&mut |p| {
                        let p = HelperProgress {
                            bytes: p.bytes,
                            total: p.total,
                        };
                        progress_tx.send_replace(Some(p));
                        on_progress(p);
                    })
                    .await
                }
            }
        };
        let result = match Abortable::new(install, registration).await {
            Ok(done) => done.map(HelperInstalled::from).map_err(RpcError::from),
            Err(_aborted) => Err(cancelled()),
        };
        match &result {
            Ok(done) => {
                log::info!(activity = "duckdb.helper", event = "install", downloaded = done.downloaded, pruned = done.pruned; "DuckDB helper install");
            }
            Err(e) => {
                log::warn!(activity = "duckdb.helper", event = "install", code = e.code.as_str(); "DuckDB helper install failed");
            }
        }
        result_tx.send_replace(Some(result.clone()));
        result
    }

    /// "Install from a file…": `path` checked against the pinned digest.
    pub async fn install_file(
        &self,
        core: &Core,
        path: &Path,
    ) -> Result<HelperInstalled, RpcError> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (abort, registration) = AbortHandle::new_pair();
        *lock(&self.file) = Some((id, abort));
        let _slot = FileSlot {
            file: &self.file,
            id,
        };
        // `None`: checked against the pin (Core's `WRONG_FILE` for another
        // file, `INVALID_ARGUMENT` with no pin).
        let install = core.duckdb_helper_install_from_file(path, None);
        let result = match Abortable::new(install, registration).await {
            Ok(done) => done.map(HelperInstalled::from).map_err(RpcError::from),
            Err(_aborted) => Err(cancelled()),
        };
        match &result {
            Ok(done) => {
                log::info!(activity = "duckdb.helper", event = "install", from_file = true, pruned = done.pruned; "DuckDB helper install from a file");
            }
            Err(e) => {
                log::warn!(activity = "duckdb.helper", event = "install", from_file = true, code = e.code.as_str(); "DuckDB helper install from a file failed");
            }
        }
        result
    }

    /// Stops the running download and the file install, if any; each
    /// answers `CANCELLED` and leaves no partial file. The download is
    /// shared, so this also cancels it for every install that joined it:
    /// the prefetch, the CLI flow. Returns whether an install
    /// was registered when it was called, not whether it then
    /// stopped: one that was finishing may still answer with its result.
    pub fn cancel(&self) -> bool {
        let mut any = false;
        if let Some(running) = &*lock(&self.running) {
            running.abort.abort();
            any = true;
        }
        if let Some((_, abort)) = &*lock(&self.file) {
            abort.abort();
            any = true;
        }
        log::info!(activity = "duckdb.helper", event = "cancel", running = any; "DuckDB helper install cancel");
        any
    }
}

/// Clears the running download's slot when its driver ends, however.
struct Slot<'a> {
    running: &'a Mutex<Option<Running>>,
    id: u64,
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        let mut running = lock(self.running);
        if running.as_ref().is_some_and(|r| r.id == self.id) {
            *running = None;
        }
    }
}

/// Clears the file install's slot when it ends, however.
struct FileSlot<'a> {
    file: &'a Mutex<Option<(u64, AbortHandle)>>,
    id: u64,
}

impl Drop for FileSlot<'_> {
    fn drop(&mut self) {
        let mut file = lock(self.file);
        if file.as_ref().is_some_and(|(id, _)| *id == self.id) {
            *file = None;
        }
    }
}

/// A second install: the running one's progress from where it is, then
/// its answer. A driver gone without one (dropped) is `CANCELLED`.
async fn join(
    mut progress: watch::Receiver<Option<HelperProgress>>,
    mut result: watch::Receiver<Option<Result<HelperInstalled, RpcError>>>,
    mut on_progress: impl FnMut(HelperProgress) + Send,
) -> Result<HelperInstalled, RpcError> {
    if let Some(p) = *progress.borrow_and_update() {
        on_progress(p);
    }
    loop {
        if result.borrow().is_some() {
            break;
        }
        tokio::select! {
            biased;
            changed = result.changed() => {
                if changed.is_err() {
                    break;
                }
            }
            changed = progress.changed() => match changed {
                Ok(()) => {
                    if let Some(p) = *progress.borrow_and_update() {
                        on_progress(p);
                    }
                }
                // The download is over; its answer follows (or never comes).
                Err(_) => {
                    let _ = result.wait_for(Option::is_some).await;
                    break;
                }
            },
        }
    }
    let answer = result.borrow().clone();
    answer.unwrap_or_else(|| Err(cancelled()))
}

/// The SHA-256 of a debug build's own helper, on the blocking pool.
async fn hash_file(path: &Path) -> Result<String, CoreError> {
    let path = path.to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        use sha2::{Digest, Sha256};
        let unreadable =
            |e: std::io::Error| CoreError::new("FILE_ERROR", format!("{:?}", e.kind()));
        let mut file = std::fs::File::open(&path).map_err(unreadable)?;
        let mut hasher = Sha256::new();
        std::io::copy(&mut file, &mut hasher).map_err(unreadable)?;
        Ok(format!("{:x}", hasher.finalize()))
    })
    .await
    .map_err(|_| CoreError::new("FILE_ERROR", "hashing the helper stopped"))?
}

/// The install command's body: the debug rule ([`local_helper`]) applied
/// to `identifier` and `data_dir_env` at this call site.
pub async fn install_for(
    core: &Core,
    installs: &HelperInstalls,
    identifier: &str,
    data_dir_env: Option<&OsStr>,
    beside: impl FnOnce() -> Option<PathBuf>,
    on_progress: impl FnMut(HelperProgress) + Send,
) -> Result<HelperInstalled, RpcError> {
    let local = local_helper(identifier, data_dir_env, beside);
    installs.install(core, local, on_progress).await
}

/// The data dir override the debug rule reads.
pub fn data_dir_env() -> Option<std::ffi::OsString> {
    std::env::var_os(seaquel_core::storage::DATA_DIR_ENV)
}

#[tauri::command]
pub async fn duckdb_helper_offer(core: State<'_, Core>) -> Result<HelperOffer, RpcError> {
    offer(&core).await
}

#[tauri::command]
pub async fn duckdb_helper_install(
    app: tauri::AppHandle,
    channel: Channel<HelperProgress>,
    core: State<'_, Core>,
    installs: State<'_, HelperInstalls>,
) -> Result<HelperInstalled, RpcError> {
    let env = data_dir_env();
    install_for(
        &core,
        &installs,
        &app.config().identifier,
        env.as_deref(),
        cli_download::debug_helper,
        move |p| {
            let _ = channel.send(p);
        },
    )
    .await
}

#[tauri::command]
pub fn duckdb_helper_cancel(installs: State<'_, HelperInstalls>) -> bool {
    installs.cancel()
}

#[tauri::command]
pub async fn duckdb_helper_install_file(
    path: String,
    core: State<'_, Core>,
    installs: State<'_, HelperInstalls>,
) -> Result<HelperInstalled, RpcError> {
    installs.install_file(&core, Path::new(&path)).await
}

fn status_word(status: &DuckdbHelperStatus) -> OfferStatus {
    match status {
        DuckdbHelperStatus::Installed { .. } => OfferStatus::Installed,
        DuckdbHelperStatus::Missing => OfferStatus::Missing,
        DuckdbHelperStatus::Outdated => OfferStatus::Outdated,
        DuckdbHelperStatus::Unsafe => OfferStatus::Unsafe,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use seaquel_core::DuckdbHelperReleases;
    use seaquel_http::release_asset::testing::{
        asset_path, gzip, release_path, MockReleases, Route,
    };
    use std::fs;
    use std::sync::Arc;
    use std::time::Duration;

    const LIMIT: Duration = Duration::from_secs(30);
    const HELPER: &[u8] = b"#!/bin/sh\nexit 0\n";

    fn asset_name() -> String {
        cli_download::helper_asset_name()
    }

    /// `<tmp>/app/bin/duckdb`, as the app's folder under `SEAQUEL_DATA_DIR`.
    fn dir_in(tmp: &Path) -> PathBuf {
        tmp.join("app").join("bin").join("duckdb")
    }

    /// A Core with only the helper's locator, downloading from `releases`
    /// and pinned to `pin` when given. Never the compiled pin:
    /// these tests serve their own files.
    fn core(dir: PathBuf, releases: Option<DuckdbHelperReleases>, pin: Option<&[u8]>) -> Core {
        let mut builder =
            seaquel_core::with_plugins(|_| false).duckdb_helper(seaquel_core::DuckdbHelper {
                dir,
                version: cli_download::VERSION.to_string(),
            });
        if let Some(source) = releases {
            builder = builder.duckdb_helper_releases(source);
        }
        if let Some(asset) = pin {
            let hex = seaquel_http::release_asset::testing::digest(asset);
            builder = builder
                .duckdb_helper_pinned(asset.len() as u64, hex.strip_prefix("sha256:").unwrap());
        }
        builder.build()
    }

    /// A release server that has nothing: any request is a 404.
    fn nowhere() -> DuckdbHelperReleases {
        DuckdbHelperReleases::new("http://127.0.0.1:9/api", "http://127.0.0.1:9/download").unwrap()
    }

    fn installed_file(dir: &Path) -> PathBuf {
        dir.join(cli_download::VERSION)
            .join(format!("seaquel-duckdb{}", std::env::consts::EXE_SUFFIX))
    }

    /// Partial downloads left in the version folder.
    fn parts(dir: &Path) -> Vec<String> {
        fs::read_dir(dir.join(cli_download::VERSION))
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n.ends_with(".part"))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A download served in small pieces with gaps, so it is still running
    /// a moment after it starts.
    fn slow(mock: &MockReleases, asset: Vec<u8>, stall_after: Option<usize>) {
        mock.publish(cli_download::VERSION, &asset_name(), asset.clone());
        mock.route(
            &asset_path(cli_download::VERSION, &asset_name()),
            Route::Body {
                status: 200,
                length: Some(asset.len() as u64),
                body: asset,
                stall_after,
                close_after: None,
                piece: 4 * 1024,
                gap: Duration::from_millis(20),
            },
        );
    }

    /// Random bytes, so gzip leaves them about their size.
    fn noise(len: usize) -> Vec<u8> {
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_offer_names_a_missing_helper_and_its_size() {
        let tmp = tempfile::tempdir().unwrap();
        let mock = MockReleases::start().await;
        let asset = gzip(HELPER);
        mock.publish(cli_download::VERSION, &asset_name(), asset.clone());
        let core = core(dir_in(tmp.path()), Some(mock.source()), None);
        let offer = offer(&core).await.unwrap();
        assert_eq!(offer.status, OfferStatus::Missing);
        assert_eq!(offer.size, Some(asset.len() as u64));
        assert!(offer.size_error.is_none());
        assert_eq!(offer.version, cli_download::VERSION);
        assert!(!offer.from_file, "no pin, so no file install");
        // The file a copy must be: Core's asset name.
        assert_eq!(offer.asset_name.as_deref(), Some(asset_name().as_str()));
        let json = serde_json::to_value(&offer).unwrap();
        assert_eq!(json["assetName"], asset_name());
        assert_eq!(json["status"], "missing");
        assert_eq!(json["sizeError"], serde_json::Value::Null);
        assert_eq!(json["fromFile"], false);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_offer_of_an_installed_helper_asks_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = dir_in(tmp.path());
        let mock = MockReleases::start().await;
        mock.publish(cli_download::VERSION, &asset_name(), gzip(HELPER));
        let core = core(dir.clone(), Some(mock.source()), None);
        let installs = HelperInstalls::default();
        installs.install(&core, None, |_| {}).await.unwrap();
        let asked = mock.requests().len();
        let offer = offer(&core).await.unwrap();
        assert_eq!(offer.status, OfferStatus::Installed);
        assert_eq!(offer.size, None);
        assert_eq!(offer.asset_name, None);
        assert_eq!(
            mock.requests().len(),
            asked,
            "no request for an installed helper"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_offer_names_an_outdated_helper() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = dir_in(tmp.path());
        let old = dir.join("2000.1.1");
        fs::create_dir_all(&old).unwrap();
        fs::write(
            old.join(format!("seaquel-duckdb{}", std::env::consts::EXE_SUFFIX)),
            HELPER,
        )
        .unwrap();
        let mock = MockReleases::start().await;
        mock.publish(cli_download::VERSION, &asset_name(), gzip(HELPER));
        let offer = offer(&core(dir, Some(mock.source()), None)).await.unwrap();
        assert_eq!(offer.status, OfferStatus::Outdated);
        assert!(offer.size.is_some());
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn the_offer_names_an_unsafe_helper() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = dir_in(tmp.path());
        let mock = MockReleases::start().await;
        mock.publish(cli_download::VERSION, &asset_name(), gzip(HELPER));
        let core = core(dir.clone(), Some(mock.source()), None);
        HelperInstalls::default()
            .install(&core, None, |_| {})
            .await
            .unwrap();
        fs::set_permissions(
            dir.join(cli_download::VERSION),
            fs::Permissions::from_mode(0o777),
        )
        .unwrap();
        let offer = offer(&core).await.unwrap();
        assert_eq!(offer.status, OfferStatus::Unsafe);
        assert!(
            offer.size.is_some(),
            "an install repairs it, so it is offered"
        );
    }

    /// No network: the status still comes, with the reason the size
    /// didn't, so the dialog can offer "Install from a file…".
    #[tokio::test(flavor = "multi_thread")]
    async fn the_offer_without_a_network_says_why_there_is_no_size() {
        let tmp = tempfile::tempdir().unwrap();
        let offer = offer(&core(dir_in(tmp.path()), Some(nowhere()), None))
            .await
            .unwrap();
        assert_eq!(offer.status, OfferStatus::Missing);
        assert_eq!(offer.size, None);
        assert_eq!(offer.size_error.unwrap().code, "NETWORK_ERROR");
    }

    /// With the pin, the size is known offline and a file can be checked.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_pinned_offer_needs_no_network() {
        let tmp = tempfile::tempdir().unwrap();
        let asset = gzip(HELPER);
        let offer = offer(&core(dir_in(tmp.path()), Some(nowhere()), Some(&asset)))
            .await
            .unwrap();
        assert_eq!(offer.size, Some(asset.len() as u64));
        assert!(offer.size_error.is_none());
        assert!(offer.from_file);
        assert_eq!(offer.asset_name.as_deref(), Some(asset_name().as_str()));
    }

    /// The install over a channel: progress only grows and ends at the
    /// total, the answer says it downloaded, and the file is where the
    /// terminal binaries look.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_install_reports_progress_and_installs() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = dir_in(tmp.path());
        let mock = MockReleases::start().await;
        let body = noise(300 * 1024);
        let asset = gzip(&body);
        slow(&mock, asset.clone(), None);
        let core = core(dir.clone(), Some(mock.source()), None);
        let installs = HelperInstalls::default();
        let mut seen = Vec::new();
        let done = tokio::time::timeout(LIMIT, installs.install(&core, None, |p| seen.push(p)))
            .await
            .unwrap()
            .unwrap();
        assert!(done.downloaded);
        assert_eq!(fs::read(installed_file(&dir)).unwrap(), body);
        assert!(seen.len() > 2, "{seen:?}");
        assert!(
            seen.windows(2).all(|w| w[0].bytes <= w[1].bytes),
            "{seen:?}"
        );
        let last = seen.last().unwrap();
        assert_eq!(
            (last.bytes, last.total),
            (asset.len() as u64, asset.len() as u64)
        );
        let json = serde_json::to_value(done).unwrap();
        assert_eq!(json, serde_json::json!({"downloaded": true, "pruned": 0}));
    }

    /// Cancel mid-download: the install answers `CANCELLED`, no `.part`
    /// is left, nothing is installed, and a later install works.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_cancel_mid_download_leaves_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = dir_in(tmp.path());
        let mock = MockReleases::start().await;
        let asset = gzip(&noise(256 * 1024));
        slow(&mock, asset.clone(), Some(64 * 1024));
        let core = Arc::new(core(dir.clone(), Some(mock.source()), None));
        let installs = Arc::new(HelperInstalls::default());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let task = {
            let (core, installs) = (core.clone(), installs.clone());
            tokio::spawn(async move {
                installs
                    .install(&core, None, move |p| {
                        let _ = tx.send(p);
                    })
                    .await
            })
        };
        // Wait until bytes have arrived (the server then stalls).
        loop {
            let p = tokio::time::timeout(LIMIT, rx.recv())
                .await
                .unwrap()
                .unwrap();
            if p.bytes > 0 {
                break;
            }
        }
        assert!(installs.cancel(), "an install was running");
        let err = tokio::time::timeout(LIMIT, task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(err.code, "CANCELLED", "{err:?}");
        assert!(parts(&dir).is_empty(), "{:?}", parts(&dir));
        assert!(!installed_file(&dir).exists());
        assert!(!installs.cancel(), "nothing runs any more");

        // The next install starts afresh.
        mock.route(
            &asset_path(cli_download::VERSION, &asset_name()),
            Route::ok(asset),
        );
        let done = installs.install(&core, None, |_| {}).await.unwrap();
        assert!(done.downloaded);
    }

    /// The prefetch and the dialog: the second install joins the first,
    /// gets progress from where it is and the same answer, and the asset
    /// is downloaded once.
    #[tokio::test(flavor = "multi_thread")]
    async fn two_installs_at_once_share_one_download() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = dir_in(tmp.path());
        let mock = MockReleases::start().await;
        slow(&mock, gzip(&noise(300 * 1024)), None);
        let core = Arc::new(core(dir.clone(), Some(mock.source()), None));
        let installs = Arc::new(HelperInstalls::default());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let first = {
            let (core, installs) = (core.clone(), installs.clone());
            tokio::spawn(async move {
                installs
                    .install(&core, None, move |p| {
                        let _ = tx.send(p);
                    })
                    .await
            })
        };
        tokio::time::timeout(LIMIT, rx.recv())
            .await
            .unwrap()
            .unwrap();
        let mut joined = Vec::new();
        let second = tokio::time::timeout(LIMIT, installs.install(&core, None, |p| joined.push(p)))
            .await
            .unwrap()
            .unwrap();
        let first = first.await.unwrap().unwrap();
        assert_eq!(first, second);
        assert!(second.downloaded, "the joiner gets the download's answer");
        assert!(!joined.is_empty(), "the joiner saw progress");
        assert!(joined.windows(2).all(|w| w[0].bytes <= w[1].bytes));
        assert_eq!(
            mock.hits(&asset_path(cli_download::VERSION, &asset_name())),
            1
        );
        assert_eq!(mock.hits(&release_path(cli_download::VERSION)), 1);
    }

    /// "Install from a file…" under the pin: the release's file installs,
    /// another is `WRONG_FILE` with nothing made.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_file_installs_against_the_pin() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = dir_in(tmp.path());
        let asset = gzip(HELPER);
        let core = core(dir.clone(), Some(nowhere()), Some(&asset));
        let installs = HelperInstalls::default();

        let wrong = tmp.path().join("wrong.gz");
        fs::write(&wrong, b"not the helper").unwrap();
        let err = installs.install_file(&core, &wrong).await.unwrap_err();
        assert_eq!(err.code, "WRONG_FILE", "{err:?}");
        assert!(!err.message.contains(&tmp.path().display().to_string()));
        assert!(!installed_file(&dir).exists());

        let right = tmp.path().join("seaquel-duckdb.gz");
        fs::write(&right, &asset).unwrap();
        let done = installs.install_file(&core, &right).await.unwrap();
        assert!(done.downloaded);
        assert_eq!(fs::read(installed_file(&dir)).unwrap(), HELPER);
    }

    /// Without the pin there is nothing to check a file against.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_file_needs_the_pin() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("seaquel-duckdb.gz");
        fs::write(&file, gzip(HELPER)).unwrap();
        let core = core(dir_in(tmp.path()), Some(nowhere()), None);
        let err = HelperInstalls::default()
            .install_file(&core, &file)
            .await
            .unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT", "{err:?}");
    }

    /// Task 1 review M3, at the command's call site: a debug build installs
    /// the helper found beside it only into a `.dev` folder or under
    /// `SEAQUEL_DATA_DIR`; otherwise it downloads like a release build.
    #[cfg(debug_assertions)]
    #[tokio::test(flavor = "multi_thread")]
    async fn the_command_s_install_follows_the_debug_rule() {
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join("seaquel-duckdb-local");
        fs::write(&local, b"local helper").unwrap();
        let mock = MockReleases::start().await;
        mock.publish(cli_download::VERSION, &asset_name(), gzip(HELPER));
        let asset = asset_path(cli_download::VERSION, &asset_name());

        for (case, identifier, env, expect_local) in [
            ("real", "app.seaquel.desktop", None, false),
            ("empty", "app.seaquel.desktop", Some(OsStr::new("")), false),
            ("dev", "app.seaquel.desktop.dev", None, true),
            (
                "scratch",
                "app.seaquel.desktop",
                Some(OsStr::new("/scratch")),
                true,
            ),
        ] {
            let dir = tmp.path().join(case).join("bin").join("duckdb");
            let core = core(dir.clone(), Some(mock.source()), None);
            let hits = mock.hits(&asset);
            let found = local.clone();
            install_for(
                &core,
                &HelperInstalls::default(),
                identifier,
                env,
                move || Some(found),
                |_| {},
            )
            .await
            .unwrap();
            let got = fs::read(installed_file(&dir)).unwrap();
            if expect_local {
                assert_eq!(got, b"local helper", "{case}");
                assert_eq!(mock.hits(&asset), hits, "{case}: nothing downloaded");
            } else {
                assert_eq!(got, HELPER, "{case}");
                assert_eq!(mock.hits(&asset), hits + 1, "{case}: downloaded");
            }
        }
    }

    /// Release builds never use a local helper.
    #[test]
    fn local_helper_is_found_only_where_allowed() {
        let found = || Some(PathBuf::from("/x/seaquel-duckdb"));
        assert_eq!(local_helper("app.seaquel.desktop", None, found), None);
        let got = local_helper("app.seaquel.desktop.dev", None, found);
        if cfg!(debug_assertions) {
            assert_eq!(got, found());
        } else {
            assert_eq!(got, None);
        }
    }
}
