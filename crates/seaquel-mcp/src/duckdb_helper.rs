//! What the server says when a DuckDB connection's helper can't be used.
//!
//! `seaquel-cli` runs DuckDB in the `seaquel-duckdb` helper, which it
//! downloads separately. The MCP server can't ask the user anything, so a
//! DuckDB connection without the helper is a tool error saying how to
//! install it, and [`McpServer::duckdb_helper_notice`] gives the CLI one
//! startup line saying the same.
//!
//! When the helper is installed (it passes the start's check) but still
//! refused, installing again wouldn't help (`seaquel-cli duckdb install`
//! finds it intact and fetches nothing), so the error points at
//! `seaquel-cli duckdb status` instead (the TUI's `after_install` rule, in
//! the CLI's terms).
//!
//! Neither text names the connection or a path.

use seaquel_core::{Core, DuckdbHelperStatus};

use crate::error::ToolError;
use crate::exposed::Exposed;
use crate::server::McpServer;

/// The engine id and the code the remote DuckDB engine refuses with.
const DUCKDB: &str = "duckdb";
pub const ENGINE_NOT_INSTALLED: &str = "ENGINE_NOT_INSTALLED";

/// The tool error's message and the startup line, for `version`'s CLI.
pub fn not_installed_message(version: &str) -> String {
    format!(
        "DuckDB support isn't installed for seaquel-cli {version}. Run \"seaquel-cli duckdb \
         install\", or use Install Command Line Tool in the Seaquel app."
    )
}

/// The message for a helper that is installed but was refused: Core's
/// reason (no path), and where to look.
fn refused_message(version: &str, reason: &str) -> String {
    format!(
        "DuckDB support for seaquel-cli {version} is installed but can't be used ({reason}). Run \
         \"seaquel-cli duckdb status\" to check it."
    )
}

/// Whether `status` is one an install fixes (an install also makes a loose
/// folder private again).
fn install_fixes(status: &DuckdbHelperStatus) -> bool {
    matches!(
        status,
        DuckdbHelperStatus::Missing | DuckdbHelperStatus::Outdated | DuckdbHelperStatus::Unsafe
    )
}

/// `e`, reworded when it is a DuckDB connection's helper that is missing
/// or refused (`engine` is the saved row's `type`). For `seaquel-cli`'s
/// commands as for the MCP tools. Other errors, and a Core with no helper
/// locator, keep Core's words.
pub fn reword_connect_error(core: &Core, engine: &str, version: &str, e: ToolError) -> ToolError {
    if engine != DUCKDB || e.code != ENGINE_NOT_INSTALLED {
        return e;
    }
    match core.duckdb_helper_status() {
        Ok(status) if install_fixes(&status) => {
            ToolError::new(ENGINE_NOT_INSTALLED, not_installed_message(version))
        }
        Ok(DuckdbHelperStatus::Installed { .. }) => {
            let message = refused_message(version, &e.message);
            ToolError::new(ENGINE_NOT_INSTALLED, message)
        }
        _ => e,
    }
}

/// [`reword_connect_error`] for an exposed connection.
pub(crate) fn connect_error(core: &Core, c: &Exposed, version: &str, e: ToolError) -> ToolError {
    reword_connect_error(core, &c.engine, version, e)
}

impl McpServer {
    /// One line for stderr at startup when an exposed connection is DuckDB
    /// and its helper isn't installed (or its folder needs an install to
    /// be private again). `None` otherwise. The server works either way;
    /// only the DuckDB connections' tools fail.
    pub fn duckdb_helper_notice(&self) -> Option<String> {
        if !self.exposed().iter().any(|c| c.engine == DUCKDB) {
            return None;
        }
        match self.inner.core.duckdb_helper_status() {
            Ok(status) if install_fixes(&status) => {
                Some(not_installed_message(&self.inner.options.version))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_messages_name_the_command_and_the_version() {
        let m = not_installed_message("2026.10.1");
        assert!(m.contains("seaquel-cli 2026.10.1"), "{m}");
        assert!(m.contains("\"seaquel-cli duckdb install\""), "{m}");
        let m = refused_message("2026.10.1", "the DuckDB helper stopped before answering");
        assert!(m.contains("\"seaquel-cli duckdb status\""), "{m}");
        assert!(!m.contains("duckdb install"), "{m}");
    }

    #[test]
    fn only_a_missing_outdated_or_loose_helper_is_an_install() {
        assert!(install_fixes(&DuckdbHelperStatus::Missing));
        assert!(install_fixes(&DuckdbHelperStatus::Outdated));
        assert!(install_fixes(&DuckdbHelperStatus::Unsafe));
        assert!(!install_fixes(&DuckdbHelperStatus::Installed {
            path: "/x".into()
        }));
    }
}
