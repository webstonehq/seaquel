//! Tool errors: what a tool call returns when Core, storage or the tool
//! itself refuses. They reach the host as a tool result with `isError: true`
//! and the text `CODE: message`, never as a JSON-RPC error (hosts show those
//! opaquely). Messages come from Core, which keeps secrets out of them.

use std::fmt;

use rmcp::model::{CallToolResult, ContentBlock};
use seaquel_core::CoreError;
use seaquel_types::DbError;

/// A tool's failure: Core's `code` and `message`, or one of this crate's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError {
    pub code: String,
    pub message: String,
}

impl ToolError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    /// The tool result: `isError: true`, one text block `CODE: message`.
    pub fn into_result(self) -> CallToolResult {
        CallToolResult::error(vec![ContentBlock::text(self.to_string())])
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ToolError {}

impl From<CoreError> for ToolError {
    fn from(e: CoreError) -> Self {
        Self::new(e.code, e.message)
    }
}

impl From<seaquel_core::ai::tools::ToolError> for ToolError {
    fn from(e: seaquel_core::ai::tools::ToolError) -> Self {
        Self::new(e.code, e.message)
    }
}

impl From<DbError> for ToolError {
    fn from(e: DbError) -> Self {
        Self::new(e.code, e.message)
    }
}

impl From<seaquel_core::storage::StorageError> for ToolError {
    fn from(e: seaquel_core::storage::StorageError) -> Self {
        CoreError::from(e).into()
    }
}

/// The registry's codes: an argument no tool call can use, and the
/// connection's schema or data sharing off (after the global default).
pub use seaquel_core::ai::tools::{DATA_SHARING_OFF, INVALID_ARGUMENT, SCHEMA_SHARING_OFF};
/// A tool call ran past the per-call timeout; its query was cancelled.
pub const TIMEOUT: &str = "TIMEOUT";
