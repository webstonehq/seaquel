//! The `.seaquel` projection of a shared project (phase 5e):
//! the five file formats, file names, the content hash, pairing and the
//! sync and publish plans. Pure: no I/O, never panics on input, builds for
//! wasm32. Core (`shared.rs`) scans the directory through
//! `seaquel-git`'s `tree`, hands the scan and the rows here, and applies
//! what comes back.
//!
//! - [`format`], the readers and writers, ports of
//!   `query-file-parser.ts`, `dashboard-file-parser.ts`,
//!   `config-file-parser.ts` and `yaml-utils.ts` with CRLF and a BOM
//!   accepted, the quoting fixed so every value reads back, and the stable
//!   file id. Each kind's `*_content` is the canonical text its hash
//!   is taken over.
//! - [`names`], with [`names::file_stem`], [`names::free_path`] and the
//!   path rules.
//! - [`plan`], with [`plan::plan_sync`] (pairing and the three-way rule table)
//!   and [`plan::plan_publish`].
//!
//! Paths in rows, scans and file operations are repo-relative
//! (`.seaquel/projects/<dir>/…`); paths in notices are relative to
//! `.seaquel/`. No `Debug` here shows a name, a path, a host
//! or a file's text.

pub mod format;
mod json;
pub mod names;
pub mod plan;
mod yaml;

pub use plan::{
    file_hash, pick_project_dir, plan_publish, plan_sync, DirScan, FileOp, IdSource, Kind, Limits,
    Link, LinkUpdate, LinkedConnection, LinkedDashboard, LinkedQuery, NoticeMemory, ProjectLink,
    PublishContext, PublishOutcome, PublishPlan, PublishStatus, RawFile, ReplacedValues, RowChange,
    RowOp, SharedRows, SkipReason, Skipped, SyncNotice, SyncPlan,
};

/// The repo-relative directory every shared file lives under.
pub const SEAQUEL_DIR: &str = ".seaquel";

/// A file name's byte cap (I8): 255, the usual file system limit.
pub const MAX_FILE_NAME_BYTES: usize = 255;

/// A stem's byte cap: 255 less the longest extension Core writes
/// (`.json`, `.yaml`).
pub const MAX_STEM_BYTES: usize = 250;

/// A repo-relative path's byte cap.
pub const MAX_PATH_BYTES: usize = 1024;

/// SHA-256 of `canonical`, as lowercase hex: the base and the hashes
/// the sync compares. `canonical` is a kind's `*_content` text.
pub fn content_hash(canonical: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(canonical.as_bytes());
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(b & 15), 16).unwrap_or('0'));
    }
    out
}
