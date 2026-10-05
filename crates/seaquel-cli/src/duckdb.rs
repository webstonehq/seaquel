//! `seaquel-cli duckdb status` and `seaquel-cli duckdb install`.
//!
//! The CLI runs DuckDB in the `seaquel-duckdb` helper, a separate download
//! matched to this version. `install` fetches it from this version's GitHub
//! release (size and SHA-256 checked, gzipped), or installs a copy of the
//! release asset with `--from FILE --sha256 HEX`, into
//! `<data_local_dir>/<identifier>/bin/duckdb/<version>/` (under
//! `SEAQUEL_DATA_DIR` when that is set), the folder the MCP server, the TUI
//! and the app's "Install Command Line Tool…" all use.
//!
//! stdout carries only the answer: the installed path, or the status word.
//! Progress, hints and failures go to stderr. Exit codes: 0 done (or
//! installed), 1 not installed or failed, 2 a usage error (clap), 130
//! stopped by a signal (the partial download is deleted).
//!
//! **Test hooks, debug builds only** (`SEAQUEL_CLI_TEST_*`, as for `mcp`):
//! `_DUCKDB_RELEASES` points the download at a release server on
//! 127.0.0.1, `_DUCKDB_HELPER` links a built helper in place.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Subcommand};
use seaquel_core::{Core, CoreError, DuckdbHelperProgress, DuckdbHelperStatus};
use seaquel_terminal::TestHooks;

use crate::session::{stopped, TEST_HOOKS_PREFIX};
use crate::VERSION;

/// How long a stopped install may take to clean up before the process
/// exits anyway. A `.part` left past it (a read blocked on a pipe or a
/// stalled disk) is swept by a later install once it is stale.
const SHUTDOWN_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// One helper per version is kept, the two newest at a time (Task 4's pruning).
const KEPT: &str = "One DuckDB helper is kept per Seaquel version, the two newest at a time, so \
                    an older seaquel-cli may need to install again after a newer one has.";

#[derive(Debug, Args)]
pub struct DuckdbArgs {
    #[command(subcommand)]
    pub command: DuckdbCommand,
}

#[derive(Debug, Subcommand)]
pub enum DuckdbCommand {
    /// Say whether DuckDB support for this seaquel-cli is installed.
    ///
    /// Prints `installed <path>`, `missing`, `outdated` (only another
    /// version's helper is there) or `unsafe` (its folder is a link or can
    /// be written by other users; an install makes it private again).
    /// Exits 0 only when installed.
    Status,
    /// Download and install DuckDB support for this seaquel-cli.
    ///
    /// Fetches the `seaquel-duckdb` helper from this version's GitHub
    /// release, checks its size and SHA-256, and installs it beside the
    /// command line tool. Prints the installed path on stdout and progress
    /// on stderr. Nothing is fetched when it is already installed.
    ///
    /// Without a network, download the release asset
    /// (seaquel-duckdb-<platform>.gz) elsewhere, copy it over and pass
    /// --from with the SHA-256 the release page shows.
    ///
    /// One DuckDB helper is kept per Seaquel version, the two newest at a
    /// time: installing for an older seaquel-cli is safe, but may need
    /// repeating after a newer one has installed its own.
    Install(InstallArgs),
}

#[derive(Debug, Args)]
pub struct InstallArgs {
    /// Install this copy of the release asset instead of downloading it.
    #[arg(long, value_name = "FILE", requires = "sha256")]
    pub from: Option<PathBuf>,
    /// The asset's SHA-256, as the release page shows it (64 hex digits).
    #[arg(long, value_name = "HEX", requires = "from")]
    pub sha256: Option<String>,
}

pub fn run(args: DuckdbArgs) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("seaquel-cli duckdb: can't start the runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let hooks = TestHooks::from_env(TEST_HOOKS_PREFIX);
    let core =
        seaquel_terminal::core_builder(seaquel_terminal::CoreOptions::default().with_hooks(&hooks))
            .build();
    let code = match args.command {
        DuckdbCommand::Status => status(&core),
        DuckdbCommand::Install(args) => runtime.block_on(install_until_stopped(&core, args)),
    };
    // A stopped `install --from` is still copying on the blocking pool
    // until its next read sees the cancel flag; give it a bounded moment so
    // its partial file is deleted. Otherwise nothing is left
    // running and this returns at once.
    runtime.shutdown_timeout(SHUTDOWN_WAIT);
    code
}

fn status(core: &Core) -> ExitCode {
    match core.duckdb_helper_status() {
        Ok(DuckdbHelperStatus::Installed { path }) => {
            println!("installed {}", path.display());
            ExitCode::SUCCESS
        }
        Ok(DuckdbHelperStatus::Missing) => {
            println!("missing");
            eprintln!(
                "DuckDB support for seaquel-cli {VERSION} isn't installed. Run \"seaquel-cli \
                 duckdb install\" to download it."
            );
            ExitCode::FAILURE
        }
        Ok(DuckdbHelperStatus::Outdated) => {
            println!("outdated");
            eprintln!(
                "DuckDB support is installed for another version of Seaquel, not for \
                 seaquel-cli {VERSION}. Run \"seaquel-cli duckdb install\" to download it. {KEPT}"
            );
            ExitCode::FAILURE
        }
        Ok(DuckdbHelperStatus::Unsafe) => {
            println!("unsafe");
            eprintln!(
                "The DuckDB helper's folder is a link or can be written by other users, so it \
                 isn't started. Run \"seaquel-cli duckdb install\" to make it private again."
            );
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("seaquel-cli duckdb status: {}: {}", e.code, e.message);
            ExitCode::FAILURE
        }
    }
}

/// [`install`], dropped on a signal: dropping Core's install deletes the
/// partial download, which the default action (exit) wouldn't.
async fn install_until_stopped(core: &Core, args: InstallArgs) -> ExitCode {
    tokio::select! {
        code = install(core, args) => code,
        () = stopped() => {
            eprintln!("seaquel-cli duckdb install: stopped; nothing was installed");
            ExitCode::from(130)
        }
    }
}

async fn install(core: &Core, args: InstallArgs) -> ExitCode {
    let from_file = args.from.is_some();
    let result = match (args.from, args.sha256) {
        (Some(file), Some(sha256)) => {
            core.duckdb_helper_install_from_file(&file, Some(&sha256))
                .await
        }
        _ => {
            let mut progress = ProgressLines::default();
            core.duckdb_helper_install(&mut |p| progress.report(p))
                .await
        }
    };
    match result {
        Ok(done) => {
            if done.downloaded {
                eprintln!("Installed DuckDB support for seaquel-cli {VERSION}.");
            } else {
                eprintln!("DuckDB support for seaquel-cli {VERSION} is already installed.");
            }
            if done.pruned > 0 {
                eprintln!("Removed {} older version(s) of it. {KEPT}", done.pruned);
            }
            println!("{}", done.path.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{}", failure_text(&e, from_file));
            ExitCode::FAILURE
        }
    }
}

/// A line on stderr per 10% of the download.
#[derive(Default)]
struct ProgressLines {
    /// The last tenth reported; `None` before the first report.
    tenth: Option<u64>,
}

impl ProgressLines {
    fn report(&mut self, p: DuckdbHelperProgress) {
        if self.tenth.is_none() {
            eprintln!(
                "Downloading DuckDB support for seaquel-cli {VERSION} ({})",
                size(p.total)
            );
            self.tenth = Some(0);
        }
        let tenth = (p.bytes.saturating_mul(10))
            .checked_div(p.total)
            .unwrap_or(10)
            .min(10);
        if Some(tenth) > self.tenth {
            self.tenth = Some(tenth);
            eprintln!("  {}% ({} of {})", tenth * 10, size(p.bytes), size(p.total));
        }
    }
}

/// A byte count in decimal units, as release pages show sizes: `11.7 MB`,
/// `640 KB`, `12 bytes`.
fn size(bytes: u64) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1e6)
    } else if bytes >= 1_000 {
        format!("{} KB", bytes / 1_000)
    } else {
        format!("{bytes} bytes")
    }
}

/// The failure as the TUI's dialog splits it (`text::install_failure`):
/// what happened, what to do, then Core's code and message (no path or
/// URL).
fn failure_text(e: &CoreError, from_file: bool) -> String {
    let (title, hint) = failure(&e.code, from_file);
    format!(
        "seaquel-cli duckdb install: {title}.\n{hint}\n{}: {}",
        e.code, e.message
    )
}

fn failure(code: &str, from_file: bool) -> (&'static str, &'static str) {
    if from_file {
        match code {
            "DIGEST_MISMATCH" | "DIGEST_INVALID" => {
                return (
                    "The file doesn't match the SHA-256 given",
                    "Nothing was installed. Compare the hash with the release page's, and the \
                     file with the asset for this platform.",
                )
            }
            "GZIP_ERROR" | "SIZE_MISMATCH" | "ASSET_TOO_LARGE" => {
                return (
                    "The file isn't a complete release asset",
                    "Nothing was installed. Copy the asset over again.",
                )
            }
            "FILE_ERROR" => {
                return (
                    "Couldn't read the file or save DuckDB support",
                    "Check the file's path, that the disk has room and that Seaquel's data \
                     folder can be written.",
                )
            }
            _ => {}
        }
    }
    match code {
        "NETWORK_ERROR" => (
            "Couldn't reach the download server",
            "Check the network connection, or the proxy in HTTPS_PROXY, then try again. Without \
             a network, copy the release asset over and use --from FILE --sha256 HEX.",
        ),
        "RELEASE_NOT_FOUND" | "ASSET_NOT_FOUND" => (
            "DuckDB support isn't published for this version",
            "This seaquel-cli has no DuckDB download for this platform yet. Try again later, or \
             install the command line tool from the Seaquel app.",
        ),
        "DIGEST_MISMATCH" | "SIZE_MISMATCH" | "GZIP_ERROR" => (
            "The download was damaged",
            "Nothing was installed. Run the command again to download it again.",
        ),
        "DIGEST_MISSING"
        | "DIGEST_INVALID"
        | "SIZE_INVALID"
        | "ASSET_TOO_LARGE"
        | "RELEASE_METADATA_INVALID"
        | "HTTP_ERROR"
        | "REDIRECT_REFUSED" => (
            "The download server's answer can't be used",
            "Nothing was installed. Try again later.",
        ),
        "FILE_ERROR" => (
            "Couldn't save DuckDB support",
            "Check that the disk has room and that Seaquel's data folder can be written, then \
             try again.",
        ),
        "UNSAFE_FOLDER" => (
            "Seaquel's data folder can't be used",
            "A folder on the way to the DuckDB helper belongs to another user or is a link. \
             Remove the bin/duckdb folder in Seaquel's data folder and install again; if \
             SEAQUEL_DATA_DIR is set, point it at a folder of your own on this computer.",
        ),
        "NOT_SUPPORTED" => (
            "DuckDB support can't be installed here",
            "This platform has no DuckDB download.",
        ),
        _ => (
            "Couldn't install DuckDB support",
            "Try again, or look up the code below.",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_decimal() {
        assert_eq!(size(11_700_000), "11.7 MB");
        assert_eq!(size(640_123), "640 KB");
        assert_eq!(size(12), "12 bytes");
    }

    #[test]
    fn a_from_file_mismatch_blames_the_hash_given() {
        assert_eq!(
            failure("DIGEST_MISMATCH", true).0,
            "The file doesn't match the SHA-256 given"
        );
        assert_eq!(
            failure("DIGEST_MISMATCH", false).0,
            "The download was damaged"
        );
        // Codes a file can't cause keep the download's words.
        assert_eq!(
            failure("UNSAFE_FOLDER", true),
            failure("UNSAFE_FOLDER", false)
        );
    }

    /// A folder the install won't touch: the line says what to do.
    #[test]
    fn an_unsafe_folder_says_what_to_do() {
        let (_, hint) = failure("UNSAFE_FOLDER", false);
        assert!(hint.contains("install again"), "{hint}");
        assert!(hint.contains("SEAQUEL_DATA_DIR"), "{hint}");
    }

    #[test]
    fn every_failure_line_keeps_the_code() {
        let e = CoreError::new("NETWORK_ERROR", "connect failed");
        let text = failure_text(&e, false);
        assert!(text.starts_with("seaquel-cli duckdb install: Couldn't reach"));
        assert!(text.ends_with("NETWORK_ERROR: connect failed"), "{text}");
    }
}
