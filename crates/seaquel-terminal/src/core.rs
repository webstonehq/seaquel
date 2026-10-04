//! Where the terminal binaries read from, and the Core they build.

use std::path::PathBuf;
use std::sync::Arc;

use seaquel_core::{CoreBuilder, CoreError, LocalFiles};

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
}

/// A terminal binary's Core, as the desktop app builds its own: every
/// engine, `ConnectPolicy::Unrestricted` (it connects wherever its user
/// asks) and `TokioExecutor`, plus `options`. Each binary adds what only it
/// has (the CLI's import paths, the TUI's AI client) on the builder this
/// returns.
pub fn core_builder(options: CoreOptions) -> CoreBuilder {
    let mut builder = seaquel_core::with_default_plugins()
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .executor(Arc::new(seaquel_runtime::TokioExecutor));
    if let Some(local_files) = options.local_files {
        builder = builder.local_files(local_files);
    }
    if let Some(path) = options.known_hosts {
        builder = builder.ssh_known_hosts(path);
    }
    builder
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

    #[test]
    fn every_engine_is_registered() {
        let core = core_builder(CoreOptions::default()).build();
        let mut ids = core.engine_ids();
        ids.sort_unstable();
        assert_eq!(ids, ["duckdb", "mssql", "mysql", "postgres", "sqlite"]);
    }
}
