//! Downloading and installing a release asset (the DuckDB helper plan,
//! Task 4, Decisions 9 and 10): what `src-tauri/src/cli_download.rs` does
//! for the app's CLI copy, made async and streaming.
//!
//! - **The metadata** ([`Fetcher::asset`]): the release `v<version>` from
//!   the [`ReleaseSource`] (GitHub's API), the asset by exact name, its size
//!   (at most [`MAX_ASSET_BYTES`]) and its `sha256:` digest.
//! - **The install** ([`Fetcher::install`]): the install's folders are made
//!   private first ([`InstallTarget`]); then the asset is streamed through a
//!   SHA-256 hasher and a gzip decoder into a temporary file in the target's
//!   folder (0600, so a partial file is never executable), its size and
//!   digest checked, gzip checked to its trailer (CRC and length), the file
//!   made 0700, synced and renamed over the target. A record beside it
//!   (`<file>.installed`) holds the installed file's SHA-256, which
//!   [`installed`] checks. Dropping the future deletes the temporary file;
//!   a process killed mid-download can leave one (0600, never run).
//! - **Offline** ([`install_file`]): a file the user copied over, checked
//!   against the SHA-256 they give, gzipped or not.
//!
//! The client is `seaquel-http`'s: the OS and webpki roots, extra roots
//! from `NODE_EXTRA_CA_CERTS`, proxies from the environment (and the OS
//! with `system-proxy`). The only identifying header is the User-Agent,
//! `Seaquel/<version>`. Redirects (GitHub serves assets from another host)
//! are followed by hand, at most [`MAX_REDIRECTS`], and only to `https:`
//! (the metadata's only on the API's own origin), or to a loopback address
//! over `http:` when the source itself is loopback (a test's server).
//! Errors ([`InstallError`]) name no URL and no path, and nothing here
//! logs.

use std::fmt;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use flate2::write::GzDecoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::client::{load_extra_roots_with, ClientOptions, LazyClient, ShowPath};

#[cfg(feature = "testing")]
pub mod testing;

/// The largest asset accepted, compressed (the helper is about 12 MB).
pub const MAX_ASSET_BYTES: u64 = 64 * 1024 * 1024;
/// The largest file an asset may decompress to (the helper is about 35 MB
/// in a release build, 113 MB in a debug one).
pub const MAX_INSTALLED_BYTES: u64 = 256 * 1024 * 1024;
/// The most redirects a request follows.
pub const MAX_REDIRECTS: usize = 5;
/// Progress is reported about this often, in compressed bytes.
pub const PROGRESS_STEP: u64 = 64 * 1024;

const MAX_METADATA_BYTES: usize = 4 * 1024 * 1024;
const MAX_RECORD_BYTES: u64 = 4096;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Between two reads: a server or proxy that stops sending.
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const TEMP_PREFIX: &str = ".seaquel-download-";
const TEMP_SUFFIX: &str = ".part";
const RECORD_SUFFIX: &str = ".installed";
/// A `.seaquel-download-*.part` file older than this is a dead install's
/// (a process killed mid-copy) and the next install into its folder
/// removes it (review I2). A live install writes its file at least every
/// [`IDLE_TIMEOUT`], so it is never this old.
pub const STALE_PART_AGE: Duration = Duration::from_secs(10 * 60);
const ACTIVITY: &str = "release.asset";
/// The metadata request as a whole (review M1).
pub const METADATA_TIMEOUT: Duration = Duration::from_secs(60);

/// What went wrong, as a code an interface can word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallErrorKind {
    /// The release `v<version>` doesn't exist (404).
    ReleaseNotFound,
    /// The release has no asset of that name.
    AssetNotFound,
    /// The release metadata isn't what GitHub sends.
    MetadataInvalid,
    /// The asset has no `sha256:` digest.
    DigestMissing,
    /// The digest isn't 64 hex digits.
    DigestInvalid,
    /// The asset's size is 0 or past [`MAX_ASSET_BYTES`].
    SizeInvalid,
    /// The download is shorter or longer than the metadata says.
    SizeMismatch,
    /// The download's SHA-256 isn't the metadata's (or the given one).
    DigestMismatch,
    /// The asset isn't valid gzip (cut short, corrupt, a bad CRC).
    NotGzip,
    /// The asset decompresses past the cap.
    TooLarge,
    /// No connection, a timeout, a connection lost, a proxy that refused.
    Network,
    /// The server answered with another status.
    Http,
    /// A redirect to anything but `https:` (or loopback), or too many.
    RedirectRefused,
    /// An install folder is a symlink, isn't a folder or belongs to
    /// another user.
    UnsafeFolder,
    /// The disk: a folder or file couldn't be made, written or renamed.
    File,
    /// A version, name or folder that isn't one safe path component, or a
    /// source that isn't `https:`.
    InvalidArgument,
    /// The install was stopped ([`install_file_cancellable`]'s flag).
    Cancelled,
}

impl InstallErrorKind {
    pub fn code(self) -> &'static str {
        match self {
            Self::ReleaseNotFound => "RELEASE_NOT_FOUND",
            Self::AssetNotFound => "ASSET_NOT_FOUND",
            Self::MetadataInvalid => "RELEASE_METADATA_INVALID",
            Self::DigestMissing => "DIGEST_MISSING",
            Self::DigestInvalid => "DIGEST_INVALID",
            Self::SizeInvalid => "SIZE_INVALID",
            Self::SizeMismatch => "SIZE_MISMATCH",
            Self::DigestMismatch => "DIGEST_MISMATCH",
            Self::NotGzip => "GZIP_ERROR",
            Self::TooLarge => "ASSET_TOO_LARGE",
            Self::Network => "NETWORK_ERROR",
            Self::Http => "HTTP_ERROR",
            Self::RedirectRefused => "REDIRECT_REFUSED",
            Self::UnsafeFolder => "UNSAFE_FOLDER",
            Self::File => "FILE_ERROR",
            Self::InvalidArgument => "INVALID_ARGUMENT",
            Self::Cancelled => "CANCELLED",
        }
    }
}

/// A failed fetch or install. The message names no URL and no path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallError {
    pub kind: InstallErrorKind,
    pub message: String,
}

impl InstallError {
    pub fn new(kind: InstallErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        self.kind.code()
    }
}

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code(), self.message)
    }
}

impl std::error::Error for InstallError {}

use InstallErrorKind as K;

fn err(kind: InstallErrorKind, message: impl Into<String>) -> InstallError {
    InstallError::new(kind, message)
}

/// A disk error, by its kind only (the path stays out).
fn file_err(what: &str, e: &io::Error) -> InstallError {
    err(K::File, format!("{what} ({:?})", e.kind()))
}

/// One path component safe to put in a URL and on disk: 1–128 of
/// `[A-Za-z0-9._+-]`, not starting with a dot.
fn safe_component(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && !s.starts_with('.')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-'))
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `https:`, or `http:` to a loopback IP address (the tests' server); no
/// credentials: a [`ReleaseSource::new`] base.
#[cfg(any(test, debug_assertions, feature = "testing"))]
fn allowed_url(u: &url::Url) -> bool {
    if !u.username().is_empty() || u.password().is_some() {
        return false;
    }
    match u.scheme() {
        "https" => u.host().is_some(),
        "http" => match u.host() {
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            _ => false,
        },
        _ => false,
    }
}

fn is_loopback(u: &url::Url) -> bool {
    match u.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    }
}

/// Where releases are: the metadata API (`<api>/v<version>`) and the
/// downloads (`<download>/v<version>/<name>`).
#[derive(Clone)]
pub struct ReleaseSource {
    api: url::Url,
    download: url::Url,
}

impl fmt::Debug for ReleaseSource {
    /// The hosts only.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReleaseSource")
            .field("api", &self.api.host_str())
            .field("download", &self.download.host_str())
            .finish()
    }
}

impl ReleaseSource {
    /// Seaquel's GitHub releases.
    pub fn github() -> Self {
        Self {
            api: url::Url::parse("https://api.github.com/repos/webstonehq/seaquel/releases/tags")
                .expect("a fixed URL"),
            download: url::Url::parse("https://github.com/webstonehq/seaquel/releases/download")
                .expect("a fixed URL"),
        }
    }

    /// Another source: tests and debug builds' hooks only (review I1:
    /// a release build, without the `testing` feature, has no way to point
    /// the download elsewhere). Each base must be `https:`, or `http:` to a
    /// loopback IP address, with no credentials, query or fragment.
    #[cfg(any(test, debug_assertions, feature = "testing"))]
    pub fn new(api: &str, download: &str) -> Result<Self, InstallError> {
        let parse = |u: &str| {
            url::Url::parse(u)
                .ok()
                .filter(|u| allowed_url(u) && u.query().is_none() && u.fragment().is_none())
                .ok_or_else(|| {
                    err(
                        K::InvalidArgument,
                        "a release source must be an https address (or http to a loopback address)",
                    )
                })
        };
        Ok(Self {
            api: parse(api)?,
            download: parse(download)?,
        })
    }

    fn is_loopback(&self) -> bool {
        is_loopback(&self.api) && is_loopback(&self.download)
    }

    /// Where a download may be sent on (review I1): `https:` anywhere, and
    /// plain `http:` only to a loopback address when this source is itself
    /// loopback (a test's server). No credentials.
    fn allows_download_hop(&self, u: &url::Url) -> bool {
        if !u.username().is_empty() || u.password().is_some() || u.host().is_none() {
            return false;
        }
        match u.scheme() {
            "https" => true,
            "http" => self.is_loopback() && is_loopback(u),
            _ => false,
        }
    }

    /// Where the metadata request may be sent on (review M7): as a
    /// download, and only on the API's own origin (`api.github.com`).
    fn allows_metadata_hop(&self, u: &url::Url) -> bool {
        self.allows_download_hop(u) && u.origin() == self.api.origin()
    }
}

/// Which redirects a request follows.
#[derive(Clone, Copy)]
enum Hops {
    Metadata,
    Download,
}

/// `base` with `segments` appended, each percent-encoded as one segment.
fn join(base: &url::Url, segments: &[&str]) -> url::Url {
    let mut u = base.clone();
    if let Ok(mut path) = u.path_segments_mut() {
        path.pop_if_empty();
        for s in segments {
            path.push(s);
        }
    }
    u
}

/// One asset of a release, as its metadata describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    /// The release's version (its tag without the `v`).
    pub version: String,
    /// Compressed, in bytes.
    pub size: u64,
    /// Of the compressed bytes, 64 lowercase hex digits.
    pub sha256: String,
}

/// Where an asset goes: `<root>/<folders…>/<file_name>`.
///
/// `root` (the app's own folder, `<data_local_dir>/<identifier>`) is made
/// if missing (0700) and must be a real folder owned by this user; group and
/// world write are taken off it. Each of `folders` is made 0700, or
/// tightened to 0700 when it exists, and must be a real folder owned by this
/// user. Symlinks are refused at every level from `root` down
/// (`O_NOFOLLOW`), and each folder is tightened before the one below it is
/// looked at. Folders above `root` aren't checked (Decision 9's residual
/// gap). Windows checks only that each is a folder and not a symlink.
#[derive(Clone)]
pub struct InstallTarget {
    pub root: PathBuf,
    pub folders: Vec<String>,
    pub file_name: String,
}

impl fmt::Debug for InstallTarget {
    /// The folder and file names below the root, never the root.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InstallTarget")
            .field("folders", &self.folders.len())
            .finish_non_exhaustive()
    }
}

impl InstallTarget {
    /// The folder the file goes in.
    pub fn dir(&self) -> PathBuf {
        let mut dir = self.root.clone();
        for f in &self.folders {
            dir.push(f);
        }
        dir
    }

    /// The installed file.
    pub fn path(&self) -> PathBuf {
        self.dir().join(&self.file_name)
    }

    fn record_path(&self) -> PathBuf {
        self.dir()
            .join(format!("{}{RECORD_SUFFIX}", self.file_name))
    }

    fn check_names(&self) -> Result<(), InstallError> {
        if self.folders.iter().all(|f| safe_component(f)) && safe_component(&self.file_name) {
            Ok(())
        } else {
            Err(err(
                K::InvalidArgument,
                "an install folder or file name isn't a single plain name",
            ))
        }
    }
}

/// An installed file.
#[derive(Clone, PartialEq, Eq)]
pub struct Installed {
    pub path: PathBuf,
    /// Of the installed (decompressed) file.
    pub sha256: String,
    pub size: u64,
}

impl fmt::Debug for Installed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Installed")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

/// How far a download is: compressed bytes received, of `total`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub bytes: u64,
    pub total: u64,
}

/// What [`installed`] reads back.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    sha256: String,
    size: u64,
    /// The asset it came from and its digest, for a person reading it.
    asset: Option<String>,
    asset_sha256: Option<String>,
}

#[derive(Deserialize)]
struct ReleaseJson {
    assets: Vec<AssetJson>,
}

#[derive(Deserialize)]
struct AssetJson {
    name: String,
    size: u64,
    digest: Option<String>,
}

/// Fetches release metadata and assets.
pub struct Fetcher {
    client: LazyClient,
    source: ReleaseSource,
    user_agent: String,
    /// The most an asset may decompress to ([`MAX_INSTALLED_BYTES`]).
    pub max_installed_bytes: u64,
    /// The whole metadata request, redirects and body included
    /// ([`METADATA_TIMEOUT`]).
    pub metadata_timeout: Duration,
}

impl fmt::Debug for Fetcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Fetcher")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

impl Fetcher {
    /// A fetcher for `source`, sending `Seaquel/<app_version>` as its
    /// User-Agent. A loopback source (tests) is reached without a proxy.
    pub fn new(source: ReleaseSource, app_version: &str) -> Self {
        let extra_roots = std::env::var_os("NODE_EXTRA_CA_CERTS")
            .filter(|p| !p.is_empty())
            .map(|p| load_extra_roots_with(Path::new(&p), ACTIVITY, ShowPath::No))
            .unwrap_or_default();
        // A proxy that takes nothing: reqwest's automatic proxies (the
        // environment's, the OS's) are then off.
        let proxy = source
            .is_loopback()
            .then(|| reqwest::Proxy::custom(|_| None::<reqwest::Url>));
        let client = LazyClient::new(
            "release download client",
            ClientOptions {
                connect_timeout: Some(CONNECT_TIMEOUT),
                read_timeout: Some(IDLE_TIMEOUT),
                extra_roots,
                // Followed by hand, to check each hop.
                no_redirects: true,
                proxy,
                activity: ACTIVITY,
                ..ClientOptions::default()
            },
        );
        let version = if safe_component(app_version) {
            app_version
        } else {
            "unknown"
        };
        Self {
            client,
            source,
            user_agent: format!("Seaquel/{version}"),
            max_installed_bytes: MAX_INSTALLED_BYTES,
            metadata_timeout: METADATA_TIMEOUT,
        }
    }

    /// GET `url`, following redirects to allowed addresses only.
    async fn get(
        &self,
        url: url::Url,
        accept: &str,
        hops: Hops,
    ) -> Result<reqwest::Response, InstallError> {
        let client = self
            .client
            .get()
            .map_err(|_| err(K::Network, "the HTTP client couldn't be set up (TLS)"))?;
        let mut url = url;
        for hop in 0..=MAX_REDIRECTS {
            let resp = client
                .get(url.clone())
                .header(reqwest::header::USER_AGENT, &self.user_agent)
                .header(reqwest::header::ACCEPT, accept)
                .send()
                .await
                .map_err(network)?;
            if !resp.status().is_redirection() {
                return Ok(resp);
            }
            if hop == MAX_REDIRECTS {
                break;
            }
            let next = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|l| l.to_str().ok())
                .and_then(|l| url.join(l).ok())
                .ok_or_else(|| err(K::RedirectRefused, "a redirect named no valid address"))?;
            let allowed = match hops {
                Hops::Metadata => self.source.allows_metadata_hop(&next),
                Hops::Download => self.source.allows_download_hop(&next),
            };
            if !allowed {
                return Err(err(
                    K::RedirectRefused,
                    match hops {
                        Hops::Metadata => {
                            "a redirect of the release metadata away from its server was refused"
                        }
                        Hops::Download => "a redirect to an address that isn't https was refused",
                    },
                ));
            }
            url = next;
        }
        Err(err(K::RedirectRefused, "too many redirects"))
    }

    /// The asset `name` of release `v<version>`: its size and digest.
    /// The whole request is bounded by [`Fetcher::metadata_timeout`].
    pub async fn asset(&self, version: &str, name: &str) -> Result<Asset, InstallError> {
        tokio::time::timeout(self.metadata_timeout, self.asset_unbounded(version, name))
            .await
            .unwrap_or_else(|_| Err(err(K::Network, "the release server didn't answer in time")))
    }

    async fn asset_unbounded(&self, version: &str, name: &str) -> Result<Asset, InstallError> {
        if !safe_component(version) || !safe_component(name) {
            return Err(err(
                K::InvalidArgument,
                "a release version or asset name isn't a single plain name",
            ));
        }
        let url = join(&self.source.api, &[&format!("v{version}")]);
        let mut resp = self
            .get(url, "application/vnd.github+json", Hops::Metadata)
            .await?;
        let status = resp.status().as_u16();
        if status == 404 {
            return Err(err(
                K::ReleaseNotFound,
                format!("there's no Seaquel {version} release"),
            ));
        }
        if !resp.status().is_success() {
            return Err(err(
                K::Http,
                format!("the release server answered {status}"),
            ));
        }
        let mut body = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(network)? {
            if body.len() + chunk.len() > MAX_METADATA_BYTES {
                return Err(err(K::MetadataInvalid, "the release metadata is too large"));
            }
            body.extend_from_slice(&chunk);
        }
        let release: ReleaseJson = serde_json::from_slice(&body)
            .map_err(|_| err(K::MetadataInvalid, "the release metadata can't be read"))?;
        let found = release
            .assets
            .into_iter()
            .find(|a| a.name == name)
            .ok_or_else(|| {
                err(
                    K::AssetNotFound,
                    format!("the Seaquel {version} release has no {name}"),
                )
            })?;
        let sha256 = found
            .digest
            .as_deref()
            .and_then(|d| d.strip_prefix("sha256:"))
            .ok_or_else(|| err(K::DigestMissing, "the release asset has no SHA-256 digest"))?;
        if !is_hex64(sha256) {
            return Err(err(
                K::DigestInvalid,
                "the release asset's SHA-256 digest isn't valid",
            ));
        }
        if found.size == 0 || found.size > MAX_ASSET_BYTES {
            return Err(err(K::SizeInvalid, "the release asset's size isn't valid"));
        }
        Ok(Asset {
            name: found.name,
            version: version.to_string(),
            size: found.size,
            sha256: sha256.to_ascii_lowercase(),
        })
    }

    /// Downloads `asset` (gzip) and installs it at `target`, reporting
    /// progress about every [`PROGRESS_STEP`] compressed bytes and at the
    /// end. Nothing is at the target unless every check passed; dropping
    /// the future deletes the partial file.
    pub async fn install(
        &self,
        asset: &Asset,
        target: &InstallTarget,
        progress: &mut (dyn FnMut(Progress) + Send),
    ) -> Result<Installed, InstallError> {
        if !safe_component(&asset.version)
            || !safe_component(&asset.name)
            || !is_hex64(&asset.sha256)
        {
            return Err(err(K::InvalidArgument, "the asset isn't valid"));
        }
        if asset.size == 0 || asset.size > MAX_ASSET_BYTES {
            return Err(err(K::SizeInvalid, "the release asset's size isn't valid"));
        }
        target.check_names()?;
        let dir = prepare(target)?;
        let mut sink = Sink::new(&dir, Decode::Gzip, self.max_installed_bytes)?;
        let url = join(
            &self.source.download,
            &[&format!("v{}", asset.version), &asset.name],
        );
        let mut resp = self
            .get(url, "application/octet-stream", Hops::Download)
            .await?;
        if !resp.status().is_success() {
            return Err(err(
                K::Http,
                format!("the download server answered {}", resp.status().as_u16()),
            ));
        }
        if let Some(len) = resp.content_length() {
            if len != asset.size {
                return Err(size_mismatch(len, asset.size));
            }
        }
        let total = asset.size;
        progress(Progress { bytes: 0, total });
        let mut reported = 0;
        while let Some(chunk) = resp.chunk().await.map_err(network)? {
            let received = sink.received + chunk.len() as u64;
            if received > total {
                return Err(size_mismatch(received, total));
            }
            sink.feed(&chunk)?;
            if received - reported >= PROGRESS_STEP || received == total {
                reported = received;
                progress(Progress {
                    bytes: received,
                    total,
                });
            }
        }
        if sink.received != total {
            return Err(size_mismatch(sink.received, total));
        }
        sink.finish(&asset.sha256, target, Some(&asset.name))
    }
}

fn size_mismatch(got: u64, want: u64) -> InstallError {
    err(
        K::SizeMismatch,
        format!("the download is {got} bytes; the release says {want}"),
    )
}

/// reqwest's error by its kind only: its `Display` can name the URL.
fn network(e: reqwest::Error) -> InstallError {
    let what = if e.is_timeout() {
        "the release server didn't answer in time"
    } else if e.is_connect() {
        "couldn't connect to the release server (or the proxy)"
    } else if e.is_body() || e.is_decode() {
        "the connection was lost during the download"
    } else {
        "the request to the release server failed"
    };
    err(K::Network, what)
}

/// How a [`Sink`] reads its input.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Decode {
    /// Always gzip (a release asset).
    Gzip,
    /// Gzip when it starts with gzip's magic, else the bytes as they are
    /// (a file the user copied, maybe gunzipped already).
    Detect,
}

/// The temporary file's writer: counts and hashes what it writes, and
/// stops at the cap. Remembers why it failed, since the gzip decoder
/// passes its errors on as plain `io::Error`s.
struct Out {
    file: File,
    written: u64,
    cap: u64,
    hasher: Sha256,
    failure: Option<InstallError>,
}

impl Write for Out {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.written + buf.len() as u64 > self.cap {
            self.failure = Some(err(
                K::TooLarge,
                "the release asset decompresses to more than allowed",
            ));
            return Err(io::Error::other("the cap"));
        }
        match self.file.write(buf) {
            Ok(n) => {
                self.hasher.update(&buf[..n]);
                self.written += n as u64;
                Ok(n)
            }
            Err(e) => {
                self.failure = Some(file_err("the download couldn't be written", &e));
                Err(e)
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

enum Writer {
    /// [`Decode::Detect`] before the first two bytes.
    Undecided(Out, Vec<u8>),
    Gzip(Box<GzDecoder<Out>>),
    Plain(Out),
    /// After a gzip error: the input is still hashed, so a corrupted
    /// download says so rather than "not gzip".
    Broken,
}

/// The input streamed into a temporary file in the target's folder, and
/// the checks at the end.
struct Sink {
    temp: tempfile::TempPath,
    writer: Writer,
    hasher: Sha256,
    received: u64,
    gzip_error: bool,
}

impl Sink {
    fn new(dir: &Path, decode: Decode, cap: u64) -> Result<Self, InstallError> {
        // 0600 on Unix (tempfile's default): never executable while partial.
        let temp = tempfile::Builder::new()
            .prefix(TEMP_PREFIX)
            .suffix(TEMP_SUFFIX)
            .tempfile_in(dir)
            .map_err(|e| file_err("the download's temporary file couldn't be made", &e))?;
        let (file, temp) = temp.into_parts();
        let out = Out {
            file,
            written: 0,
            cap,
            hasher: Sha256::new(),
            failure: None,
        };
        let writer = match decode {
            Decode::Gzip => Writer::Gzip(Box::new(GzDecoder::new(out))),
            Decode::Detect => Writer::Undecided(out, Vec::new()),
        };
        Ok(Self {
            temp,
            writer,
            hasher: Sha256::new(),
            received: 0,
            gzip_error: false,
        })
    }

    fn feed(&mut self, chunk: &[u8]) -> Result<(), InstallError> {
        self.received += chunk.len() as u64;
        self.hasher.update(chunk);
        self.write(chunk)
    }

    fn write(&mut self, chunk: &[u8]) -> Result<(), InstallError> {
        let writer = std::mem::replace(&mut self.writer, Writer::Broken);
        self.writer = match writer {
            Writer::Broken => Writer::Broken,
            Writer::Undecided(out, mut head) => {
                head.extend_from_slice(chunk);
                if head.len() < 2 {
                    Writer::Undecided(out, head)
                } else {
                    self.writer = if head.starts_with(&[0x1f, 0x8b]) {
                        Writer::Gzip(Box::new(GzDecoder::new(out)))
                    } else {
                        Writer::Plain(out)
                    };
                    return self.write(&head);
                }
            }
            Writer::Plain(mut out) => match out.write_all(chunk) {
                Ok(()) => Writer::Plain(out),
                Err(_) => return Err(out_failure(out)),
            },
            Writer::Gzip(mut gz) => match gz.write_all(chunk) {
                Ok(()) => Writer::Gzip(gz),
                Err(_) => {
                    if let Some(f) = gz.get_mut().failure.take() {
                        return Err(f);
                    }
                    self.gzip_error = true;
                    Writer::Broken
                }
            },
        };
        Ok(())
    }

    /// Checks the digest (of the input, as received) and the gzip trailer,
    /// then makes the file 0700, syncs it, renames it over the target and
    /// writes the record.
    fn finish(
        self,
        sha256: &str,
        target: &InstallTarget,
        asset: Option<&str>,
    ) -> Result<Installed, InstallError> {
        let Sink {
            temp,
            writer,
            hasher,
            gzip_error,
            ..
        } = self;
        if format!("{:x}", hasher.finalize()) != sha256.to_ascii_lowercase() {
            return Err(err(
                K::DigestMismatch,
                "the download failed its SHA-256 check",
            ));
        }
        let not_gzip = || err(K::NotGzip, "the release asset isn't valid gzip");
        if gzip_error {
            return Err(not_gzip());
        }
        let out = match writer {
            Writer::Broken => return Err(not_gzip()),
            Writer::Undecided(mut out, head) => {
                // Shorter than gzip's magic: as it is.
                out.write_all(&head)
                    .map_err(|_| out_failure_ref(&mut out))?;
                out
            }
            Writer::Plain(out) => out,
            Writer::Gzip(mut gz) => match gz.try_finish() {
                Ok(()) => gz.finish().map_err(|_| not_gzip())?,
                Err(_) => {
                    return Err(gz.get_mut().failure.take().unwrap_or_else(not_gzip));
                }
            },
        };
        let Out {
            file,
            written,
            hasher,
            ..
        } = out;
        let installed_sha = format!("{:x}", hasher.finalize());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o700))
                .map_err(|e| file_err("the download couldn't be made executable", &e))?;
        }
        file.sync_all()
            .map_err(|e| file_err("the download couldn't be synced", &e))?;
        drop(file);
        let path = target.path();
        if let Err(e) = persist_retrying(temp, &path) {
            // Windows can't replace a running file: one that is already
            // this exact file (another install got there first) will do.
            if sha256_of(&path).ok().as_deref() != Some(installed_sha.as_str()) {
                return Err(file_err("the download couldn't be put in place", &e.error));
            }
        }
        sync_dir(&target.dir());
        write_record(
            target,
            &Record {
                sha256: installed_sha.clone(),
                size: written,
                asset: asset.map(str::to_string),
                asset_sha256: Some(sha256.to_ascii_lowercase()),
            },
        )?;
        Ok(Installed {
            path,
            sha256: installed_sha,
            size: written,
        })
    }
}

fn out_failure(mut out: Out) -> InstallError {
    out_failure_ref(&mut out)
}

fn out_failure_ref(out: &mut Out) -> InstallError {
    out.failure
        .take()
        .unwrap_or_else(|| err(K::File, "the download couldn't be written"))
}

/// Renames `temp` over `path`. On Windows a rename over a file another
/// process has open (a helper starting, an antivirus scan) fails for a
/// moment, so it is retried for about a second (review M3).
fn persist_retrying(
    temp: tempfile::TempPath,
    path: &Path,
) -> Result<(), tempfile::PathPersistError> {
    #[cfg(windows)]
    {
        let mut temp = temp;
        for _ in 0..10 {
            match temp.persist(path) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    temp = e.path;
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
        temp.persist(path)
    }
    #[cfg(not(windows))]
    {
        temp.persist(path)
    }
}

/// The record, written to a temporary file and renamed into place.
fn write_record(target: &InstallTarget, record: &Record) -> Result<(), InstallError> {
    let dir = target.dir();
    let mut temp = tempfile::Builder::new()
        .prefix(TEMP_PREFIX)
        .suffix(TEMP_SUFFIX)
        .tempfile_in(&dir)
        .map_err(|e| file_err("the install record couldn't be written", &e))?;
    let json = serde_json::to_vec(record).expect("a record serializes");
    temp.write_all(&json)
        .and_then(|()| temp.as_file().sync_all())
        .map_err(|e| file_err("the install record couldn't be written", &e))?;
    temp.persist(target.record_path())
        .map_err(|e| file_err("the install record couldn't be put in place", &e.error))?;
    sync_dir(&dir);
    Ok(())
}

/// The SHA-256 of the file at `path`.
fn sha256_of(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// Best effort: a rename is durable once its folder is synced (Unix).
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// The install at `target`, if its record is there and the file's SHA-256
/// is the one it records. Reads the whole file (about 35 MB for the
/// helper): for an install that wants to skip a download, not for every
/// start.
pub fn installed(target: &InstallTarget) -> Option<Installed> {
    target.check_names().ok()?;
    let path = target.path();
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if !meta.file_type().is_file() {
        return None;
    }
    let record = File::open(target.record_path()).ok()?;
    let mut text = Vec::new();
    record.take(MAX_RECORD_BYTES).read_to_end(&mut text).ok()?;
    let record: Record = serde_json::from_slice(&text).ok()?;
    if record.size != meta.len() || !is_hex64(&record.sha256) {
        return None;
    }
    let sha = sha256_of(&path).ok()?;
    (sha == record.sha256).then_some(Installed {
        path,
        sha256: sha,
        size: record.size,
    })
}

/// Installs `file`, a copy of the release asset the user brought over
/// (gzipped, or already gunzipped), checked against `sha256` (of the file
/// as given, as the release page shows it).
pub fn install_file(
    file: &Path,
    sha256: &str,
    target: &InstallTarget,
) -> Result<Installed, InstallError> {
    install_file_cancellable(file, sha256, target, &AtomicBool::new(false))
}

/// [`install_file`], stopping with `CANCELLED` between reads once `cancel`
/// is set (review I2: a caller dropping its wait sets it, so the blocking
/// copy ends and its partial file goes). A read that blocks (a pipe) is
/// only noticed once it returns.
pub fn install_file_cancellable(
    file: &Path,
    sha256: &str,
    target: &InstallTarget,
    cancel: &AtomicBool,
) -> Result<Installed, InstallError> {
    if !is_hex64(sha256) {
        return Err(err(
            K::DigestInvalid,
            "the SHA-256 given isn't 64 hex digits",
        ));
    }
    target.check_names()?;
    let mut source = File::open(file).map_err(|e| file_err("the file can't be read", &e))?;
    let dir = prepare(target)?;
    let mut sink = Sink::new(&dir, Decode::Detect, MAX_INSTALLED_BYTES)?;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(err(K::Cancelled, "the install was stopped"));
        }
        let n = source
            .read(&mut buf)
            .map_err(|e| file_err("the file can't be read", &e))?;
        if n == 0 {
            break;
        }
        if sink.received + n as u64 > MAX_INSTALLED_BYTES {
            return Err(err(K::TooLarge, "the file is larger than allowed"));
        }
        sink.feed(&buf[..n])?;
    }
    sink.finish(sha256, target, None)
}

/// The release target for an OS and architecture (`std::env::consts`'s
/// names), as `release.yml` names the assets.
pub fn target_triple(os: &str, arch: &str) -> Option<&'static str> {
    Some(match (os, arch) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("windows", "aarch64") => "aarch64-pc-windows-msvc",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        _ => return None,
    })
}

/// The target's folders made (or tightened) as [`InstallTarget`] says,
/// with no download: an intact install in a folder that went loose is
/// fixed this way, offline (review M5). Returns the folder the file goes
/// in.
pub fn prepare_target(target: &InstallTarget) -> Result<PathBuf, InstallError> {
    target.check_names()?;
    prepare(target)
}

/// [`prepare_target`] without the name check (done by the callers).
fn prepare(target: &InstallTarget) -> Result<PathBuf, InstallError> {
    let root = &target.root;
    if root.as_os_str().is_empty() {
        return Err(err(K::InvalidArgument, "the install has no folder"));
    }
    if std::fs::symlink_metadata(root).is_err() {
        if let Some(parent) = root.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .map_err(|e| file_err("the install folder couldn't be made", &e))?;
        }
        make_dir(root)?;
    }
    secure(root, Level::Root)?;
    let mut dir = root.clone();
    for f in &target.folders {
        dir.push(f);
        make_dir(&dir)?;
        secure(&dir, Level::Private)?;
    }
    sweep_stale_parts(&dir, STALE_PART_AGE);
    Ok(dir)
}

/// Removes `.seaquel-download-*.part` files in `dir` last written more
/// than `age` ago: what an install killed mid-copy left (review I2).
/// Fresh ones may be a concurrent install's and are kept; symlinks and
/// other names are left alone. Best effort, logged by error kind only.
fn sweep_stale_parts(dir: &Path, age: Duration) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with(TEMP_PREFIX) || !name.ends_with(TEMP_SUFFIX) {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        let old = meta
            .modified()
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|elapsed| elapsed > age);
        if meta.file_type().is_file() && old {
            if let Err(e) = std::fs::remove_file(&path) {
                log::warn!(activity = ACTIVITY, event = "sweep", kind = format!("{:?}", e.kind()).as_str(); "a stale partial download couldn't be removed");
            }
        }
    }
}

/// A folder made 0700 (Unix); one already there is left to [`secure`].
fn make_dir(path: &Path) -> Result<(), InstallError> {
    let mut b = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    match b.create(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(file_err("an install folder couldn't be made", &e)),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    /// The app's folder: no group or world write.
    Root,
    /// `bin` and below: exactly 0700.
    Private,
}

fn unsafe_folder() -> InstallError {
    err(
        K::UnsafeFolder,
        "an install folder is a link, isn't a folder or belongs to another user",
    )
}

/// The mode a folder of `level` must have, given its current one; `None`
/// when it's right as it is.
#[cfg(unix)]
fn wanted_mode(level: Level, mode: u32) -> Option<u32> {
    let mode = mode & 0o7777;
    let want = match level {
        Level::Root => mode & !0o022,
        Level::Private => 0o700,
    };
    (want != mode).then_some(want)
}

/// Opens `path` without following a symlink, checks it is a folder this
/// user owns, and fixes its mode through the open handle (so a swap after
/// the check can't redirect the `chmod`).
#[cfg(unix)]
fn secure(path: &Path, level: Level) -> Result<(), InstallError> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    let dir = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(path)
        .map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => file_err("an install folder disappeared", &e),
            _ => unsafe_folder(),
        })?;
    let meta = dir
        .metadata()
        .map_err(|e| file_err("an install folder can't be read", &e))?;
    // SAFETY: `geteuid` has no preconditions and can't fail.
    let me = unsafe { libc::geteuid() };
    if !meta.is_dir() || meta.uid() != me {
        return Err(unsafe_folder());
    }
    if let Some(mode) = wanted_mode(level, meta.mode()) {
        dir.set_permissions(std::fs::Permissions::from_mode(mode))
            .map_err(|e| file_err("an install folder's permissions couldn't be set", &e))?;
    }
    Ok(())
}

/// Windows: a folder and not a link (the profile's ACLs protect it;
/// Decision 9, M4).
#[cfg(not(unix))]
fn secure(path: &Path, _level: Level) -> Result<(), InstallError> {
    let meta = std::fs::symlink_metadata(path)
        .map_err(|e| file_err("an install folder can't be read", &e))?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(unsafe_folder());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_are_plain_names() {
        for ok in [
            "2026.10.1",
            "seaquel-duckdb-x86_64-pc-windows-msvc.exe.gz",
            "a+b",
        ] {
            assert!(safe_component(ok), "{ok}");
        }
        for bad in [
            "",
            ".",
            "..",
            ".hidden",
            "a/b",
            "a\\b",
            "a b",
            "a?b",
            "é",
            &"a".repeat(129),
        ] {
            assert!(!safe_component(bad), "{bad}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn modes_are_tightened_never_loosened() {
        assert_eq!(wanted_mode(Level::Private, 0o40700), None);
        assert_eq!(wanted_mode(Level::Private, 0o775), Some(0o700));
        assert_eq!(wanted_mode(Level::Private, 0o500), Some(0o700));
        assert_eq!(wanted_mode(Level::Root, 0o755), None);
        assert_eq!(wanted_mode(Level::Root, 0o700), None);
        assert_eq!(wanted_mode(Level::Root, 0o777), Some(0o755));
        assert_eq!(wanted_mode(Level::Root, 0o1777), Some(0o1755));
    }

    /// Review guard: a folder another user owns is refused. A test can't
    /// make one, but `/` is root's (and the tests don't run as root).
    #[cfg(unix)]
    #[test]
    fn a_folder_of_another_user_is_refused() {
        // SAFETY: as in `secure`.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let e = secure(Path::new("/"), Level::Root).unwrap_err();
        assert_eq!(e.kind, K::UnsafeFolder);
    }

    #[test]
    fn urls_are_https_or_loopback_http() {
        let ok = |u: &str| allowed_url(&url::Url::parse(u).unwrap());
        assert!(ok("https://objects.githubusercontent.com/x"));
        assert!(ok("http://127.0.0.1:8/x"));
        assert!(ok("http://127.9.9.9/x"));
        assert!(ok("http://[::1]/x"));
        assert!(!ok("http://localhost/x"));
        assert!(!ok("http://10.0.0.1/x"));
        assert!(!ok("https://user:pw@example.com/x"));
        assert!(!ok("ftp://127.0.0.1/x"));
        assert!(!ok("file:///etc/passwd"));
    }

    fn u(s: &str) -> url::Url {
        url::Url::parse(s).unwrap()
    }

    /// Review I1: plain `http:` only when the source itself is loopback (a
    /// test's server); from GitHub every hop is `https:`.
    #[test]
    fn plain_http_hops_only_from_a_loopback_source() {
        let github = ReleaseSource::github();
        assert!(github.allows_download_hop(&u("https://objects.githubusercontent.com/x")));
        assert!(!github.allows_download_hop(&u("http://127.0.0.1:8/x")));
        assert!(!github.allows_download_hop(&u("http://[::1]:8/x")));
        assert!(!github.allows_download_hop(&u("http://objects.githubusercontent.com/x")));
        let local = ReleaseSource::new("http://127.0.0.1:9/api", "http://127.0.0.1:9/dl").unwrap();
        assert!(local.allows_download_hop(&u("http://127.0.0.1:7/x")));
        assert!(local.allows_download_hop(&u("https://example.com/x")));
        assert!(!local.allows_download_hop(&u("http://10.0.0.1/x")));
        // An https source with a loopback-looking download host still
        // gets no plain http.
        let https =
            ReleaseSource::new("https://example.com/api", "https://example.com/dl").unwrap();
        assert!(!https.allows_download_hop(&u("http://127.0.0.1/x")));
    }

    /// Review M7: the metadata's redirects stay on the API's origin.
    #[test]
    fn metadata_hops_stay_on_the_api_origin() {
        let github = ReleaseSource::github();
        assert!(github.allows_metadata_hop(&u("https://api.github.com/repositories/1/releases/2")));
        assert!(!github.allows_metadata_hop(&u("https://evil.example/x")));
        assert!(!github.allows_metadata_hop(&u("http://api.github.com/x")));
        assert!(!github.allows_metadata_hop(&u("https://api.github.com:8443/x")));
        let local = ReleaseSource::new("http://127.0.0.1:9/api", "http://127.0.0.1:9/dl").unwrap();
        assert!(local.allows_metadata_hop(&u("http://127.0.0.1:9/elsewhere")));
        assert!(!local.allows_metadata_hop(&u("http://127.0.0.1:10/api")));
    }

    /// Review M6: the terminal binaries' extra-roots warnings carry the
    /// error's kind, not the file's path.
    #[test]
    fn extra_roots_warnings_can_leave_the_path_out() {
        let missing = Path::new("/nonexistent/secret-place/ca.pem");
        assert!(
            crate::client::load_extra_roots_with(missing, "t", crate::client::ShowPath::No)
                .is_empty()
        );
        let text = crate::client::unreadable_roots_message(
            missing,
            crate::client::ShowPath::No,
            "NotFound",
        );
        assert!(!text.contains("secret-place"), "{text}");
        assert!(text.contains("NotFound"), "{text}");
        let text = crate::client::unreadable_roots_message(
            missing,
            crate::client::ShowPath::Yes,
            "NotFound",
        );
        assert!(text.contains("secret-place"), "{text}");
    }

    #[test]
    fn joined_segments_are_encoded() {
        let base = url::Url::parse("https://h/a/b/").unwrap();
        assert_eq!(
            join(&base, &["v1", "n.gz"]).as_str(),
            "https://h/a/b/v1/n.gz"
        );
        let base = url::Url::parse("https://h/a").unwrap();
        assert_eq!(join(&base, &["x y"]).as_str(), "https://h/a/x%20y");
    }
}
