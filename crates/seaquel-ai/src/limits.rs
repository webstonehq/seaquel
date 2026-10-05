//! The assistant's and the MCP server's limits: the ones every interface
//! applies as constants, and the ones
//! only the web sets as [`AiLimits`].

use std::fmt;
use std::time::Duration;

use crate::tools::{ToolError, INVALID_ARGUMENT};

/// `max_rows` when the call passes none.
pub const DEFAULT_MAX_ROWS: u32 = 100;
/// The largest `max_rows` a call may ask for.
pub const MAX_MAX_ROWS: u32 = 1000;
/// The byte budget the driver gets (Core's `QueryOptions::max_bytes`), for
/// both profiles: it stops fetching once the decoded rows it kept reach
/// this.
pub const MAX_FETCH_BYTES: usize = 8 * 1024 * 1024;
/// How long the assistant's tool call may take (its turn's timer) and its
/// query may run (Core's `QueryOptions::timeout` and EXPLAIN's). MCP's
/// default call timeout is the same 60 s, but its server's own
/// (`ServerOptions::call_timeout`), which Core passes to the database in
/// place of this (`ToolContext::timeout`).
pub const CALL_TIMEOUT: Duration = Duration::from_secs(60);
/// About the most bytes an MCP result's JSON may take.
pub const MCP_RESULT_BYTES: usize = 4 * 1024 * 1024;
/// About the most bytes an assistant result's JSON may take (about 64k
/// tokens).
pub const ASSISTANT_RESULT_BYTES: usize = 256 * 1024;
/// A stored tool result is cut at this many bytes.
pub const PART_RESULT_BYTES: usize = 16 * 1024;
/// What a turn's history may cost.
pub const HISTORY_BYTES: usize = 512 * 1024;
/// The schema context's cap, whole tables only.
pub const SCHEMA_CONTEXT_BYTES: usize = 128 * 1024;
/// Tool calls per turn, counted across rounds; the next ends the turn with
/// `TOOL_LIMIT`.
pub const MAX_TOOL_CALLS_PER_TURN: usize = 20;

/// The most a reply may hold, in bytes, whatever else is enforced:
/// past it, or past the chat's message limit when that is lower,
/// Core stops reading the provider and stores the reply cut.
pub const MAX_REPLY_BYTES: usize = 1024 * 1024;

/// The limits on turns (`CoreBuilder::ai_limits`). By default only the
/// reply's ceiling, [`MAX_REPLY_BYTES`]; the web sets the rest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiLimits {
    /// Turns running at once per workspace (web: 4; past it
    /// `TOO_MANY_REQUESTS`).
    pub max_turns_in_flight: Option<usize>,
    /// The user's message, in bytes (web: 1 MiB).
    pub max_message_bytes: Option<usize>,
    /// The reply's text, in bytes: [`MAX_REPLY_BYTES`] unless set lower.
    /// The chat's `max_message_bytes` (`StateLimits`) caps it too.
    pub max_reply_bytes: usize,
}

impl AiLimits {
    /// No limits but the reply's ceiling.
    pub const DEFAULT: Self = Self {
        max_turns_in_flight: None,
        max_message_bytes: None,
        max_reply_bytes: MAX_REPLY_BYTES,
    };
}

impl Default for AiLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// `max_rows` checked: 1 to [`MAX_MAX_ROWS`], [`DEFAULT_MAX_ROWS`] when
/// absent.
pub fn max_rows(requested: Option<u32>) -> Result<usize, ToolError> {
    match requested.unwrap_or(DEFAULT_MAX_ROWS) {
        n @ 1..=MAX_MAX_ROWS => Ok(n as usize),
        n => Err(ToolError::new(
            INVALID_ARGUMENT,
            format!("max_rows must be between 1 and {MAX_MAX_ROWS}, got {n}"),
        )),
    }
}

/// How a tool's query runs: read-only, with these
/// rows, this fetch budget and this timeout.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct QuerySpec {
    pub max_rows: usize,
    pub max_bytes: usize,
    pub timeout: Duration,
}

impl QuerySpec {
    /// `max_rows` with [`MAX_FETCH_BYTES`] and [`CALL_TIMEOUT`]. Core sets
    /// `timeout` to the call's `ToolContext::timeout` before it runs the
    /// query, so for MCP it is the server's call timeout.
    pub fn new(max_rows: usize) -> Self {
        Self {
            max_rows,
            max_bytes: MAX_FETCH_BYTES,
            timeout: CALL_TIMEOUT,
        }
    }
}

impl fmt::Debug for QuerySpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QuerySpec")
            .field("max_rows", &self.max_rows)
            .field("max_bytes", &self.max_bytes)
            .field("timeout_ms", &self.timeout.as_millis())
            .finish()
    }
}
