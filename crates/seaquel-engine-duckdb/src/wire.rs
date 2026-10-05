//! The DuckDB helper's wire (the DuckDB helper plan, Decisions 3 and 5):
//! frames on the helper's stdin and stdout, shared by the client (`remote`)
//! and the helper (`helper`).
//!
//! A frame is `[len u32 LE][kind u8][call u32 LE][payload]`, `len` counting
//! the kind, the call id and the payload. A frame whose `len` passes
//! [`MAX_FRAME`] is refused on both sides, before anything is written or
//! allocated: it is a protocol error that ends the process on either side
//! (`HELPER_PROTOCOL`).
//!
//! - **Control** frames carry one JSON [`Request`] (client to helper) or
//!   [`Reply`] (helper to client).
//! - **Rows** go as Arrow IPC stream messages: one [`FrameKind::Schema`]
//!   frame, then [`FrameKind::Batch`] frames of at most [`MAX_BATCH_FRAME`]
//!   bytes each, read on the client by [`crate::ipc`]. The schema frame
//!   also carries each column's [`Kind`], made in the helper from DuckDB's
//!   logical types ([`schema_payload`]), which the client decodes by
//!   instead of guessing from the Arrow fields.
//!
//! Nothing here ever shows a payload: SQL, bound values, paths and DuckDB's
//! messages (which can quote SQL) stay out of every `Debug` and every error.
//! A control message that doesn't parse is reported by position, never with
//! serde's text, which quotes the value it choked on.
//!
//! The helper (`helper.rs`) uses the sync frames, decodes requests and
//! encodes replies; the remote driver (`remote/`) uses the async frames
//! and the other direction. Items only one side uses are marked so the
//! other side's build doesn't warn about them.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Read, Write};

use seaquel_engine::{BatchStatement, ConnectConfig, DbError, DriverType, Value};
use serde::{Deserialize, Serialize};

use crate::decode::Kind;

/// The protocol version both sides speak; `hello` carries it. 2: schema
/// frames carry the columns' kinds ([`schema_payload`], Checkpoint H-1).
pub(crate) const PROTOCOL: u32 = 2;

/// The largest `len` a frame may have: its kind, call id and payload.
pub(crate) const MAX_FRAME: usize = 16 * 1024 * 1024;

/// The largest IPC message the helper puts in one batch frame. A batch is
/// sliced to stay under it; a single row larger than this goes alone, up to
/// [`MAX_FRAME`].
#[cfg_attr(not(feature = "helper"), allow(dead_code))] // the helper's
pub(crate) const MAX_BATCH_FRAME: usize = 8 * 1024 * 1024;

/// Batch frames a streaming call may have in flight ahead of the client's
/// `credit` frames (what the native driver held in flight, while there was
/// one).
// Both sides' (dead only in a test build with neither).
#[cfg_attr(not(any(feature = "remote", feature = "helper")), allow(dead_code))]
pub(crate) const STREAM_CREDIT: u32 = 2;

/// Read-only calls (`readOnly`, `explainReadOnly`) a helper runs at once,
/// each on a DuckDB clone and a thread of its own; past it the helper
/// answers `TOO_MANY_REQUESTS`. The client sends at most this many and
/// queues the rest (the desktop DuckDB helper plan, Decision 5), so the
/// helper's refusal is only a backstop.
#[cfg_attr(not(any(feature = "remote", feature = "helper")), allow(dead_code))]
pub(crate) const MAX_READ_ONLY_CALLS: usize = 16;

/// The error code of a broken wire: a frame over the limit, a kind or a
/// control message that doesn't parse.
pub(crate) const HELPER_PROTOCOL: &str = "HELPER_PROTOCOL";

/// The kind byte and the call id.
const HEADER: usize = 5;

/// The largest payload a frame can carry.
// Both sides' (dead only in a test build with neither).
#[cfg_attr(not(any(feature = "remote", feature = "helper")), allow(dead_code))]
pub(crate) const MAX_PAYLOAD: usize = MAX_FRAME - HEADER;

/// What a frame's payload holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum FrameKind {
    /// One JSON [`Request`] or [`Reply`].
    Control = 0,
    /// An IPC stream's schema message.
    Schema = 1,
    /// One IPC record batch message.
    Batch = 2,
}

impl FrameKind {
    fn of(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(FrameKind::Control),
            1 => Some(FrameKind::Schema),
            2 => Some(FrameKind::Batch),
            _ => None,
        }
    }
}

/// One frame read off the wire.
pub(crate) struct Frame {
    pub kind: FrameKind,
    /// The call it belongs to, chosen by the client.
    pub call: u32,
    pub payload: Vec<u8>,
}

impl fmt::Debug for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Frame")
            .field("kind", &self.kind)
            .field("call", &self.call)
            .field("bytes", &self.payload.len())
            .finish()
    }
}

/// A broken wire, as an I/O error of kind `InvalidData`.
fn protocol(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// A broken wire, as a [`DbError`].
pub(crate) fn protocol_error(message: impl Into<String>) -> DbError {
    DbError {
        message: message.into(),
        code: HELPER_PROTOCOL.to_string(),
    }
}

/// The frame's `len` for a payload, refused past [`MAX_FRAME`].
fn frame_len(payload: usize) -> io::Result<u32> {
    payload
        .checked_add(HEADER)
        .filter(|len| *len <= MAX_FRAME)
        .and_then(|len| u32::try_from(len).ok())
        .ok_or_else(|| {
            protocol(format!(
                "a frame of {payload} bytes is over the {MAX_FRAME}-byte limit"
            ))
        })
}

/// A frame's header: `len`, kind and call id.
fn header(kind: FrameKind, call: u32, payload: usize) -> io::Result<[u8; 9]> {
    let len = frame_len(payload)?;
    let mut out = [0u8; 9];
    out[..4].copy_from_slice(&len.to_le_bytes());
    out[4] = kind as u8;
    out[5..].copy_from_slice(&call.to_le_bytes());
    Ok(out)
}

/// The payload length a frame's `len` announces, refused before anything is
/// allocated when it is too short or past [`MAX_FRAME`].
fn payload_len(len: [u8; 4]) -> io::Result<usize> {
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(protocol(format!(
            "a frame of {len} bytes is over the {MAX_FRAME}-byte limit"
        )));
    }
    len.checked_sub(HEADER)
        .ok_or_else(|| protocol(format!("a frame of {len} bytes has no header")))
}

/// Splits a frame's kind byte and call id.
fn kind_and_call(head: [u8; HEADER]) -> io::Result<(FrameKind, u32)> {
    let kind = FrameKind::of(head[0])
        .ok_or_else(|| protocol(format!("unknown frame kind {}", head[0])))?;
    let call = u32::from_le_bytes([head[1], head[2], head[3], head[4]]);
    Ok((kind, call))
}

/// Writes one frame and flushes it. A payload too large for a frame is
/// refused before anything is written.
// The client's (the helper writes `write_frame_buffered` and flushes once
// per turn); the helper's tests use it too.
#[cfg_attr(not(feature = "remote"), allow(dead_code))]
pub(crate) fn write_frame(
    out: &mut impl Write,
    kind: FrameKind,
    call: u32,
    payload: &[u8],
) -> io::Result<()> {
    write_frame_buffered(out, kind, call, payload)?;
    out.flush()
}

/// [`write_frame`] without the flush, for a writer that flushes once after
/// several frames.
#[cfg_attr(not(feature = "helper"), allow(dead_code))] // the helper's
pub(crate) fn write_frame_buffered(
    out: &mut impl Write,
    kind: FrameKind,
    call: u32,
    payload: &[u8],
) -> io::Result<()> {
    let head = header(kind, call, payload.len())?;
    out.write_all(&head)?;
    out.write_all(payload)
}

/// Reads one frame. `None` when the input ended cleanly, between frames; an
/// input that ends inside a frame is `UnexpectedEof`.
#[cfg_attr(not(feature = "helper"), allow(dead_code))] // the helper's
pub(crate) fn read_frame(input: &mut impl Read) -> io::Result<Option<Frame>> {
    let mut len = [0u8; 4];
    let mut filled = 0;
    while filled < len.len() {
        match input.read(&mut len[filled..]) {
            Ok(0) if filled == 0 => return Ok(None),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let size = payload_len(len)?;
    let mut head = [0u8; HEADER];
    input.read_exact(&mut head)?;
    let (kind, call) = kind_and_call(head)?;
    let mut payload = vec![0u8; size];
    input.read_exact(&mut payload)?;
    Ok(Some(Frame {
        kind,
        call,
        payload,
    }))
}

/// [`write_frame`] on an async writer.
#[cfg(any(feature = "remote", test))]
#[cfg_attr(not(feature = "remote"), allow(dead_code))] // the client's
pub(crate) async fn write_frame_async(
    out: &mut (impl tokio::io::AsyncWrite + Unpin),
    kind: FrameKind,
    call: u32,
    payload: &[u8],
) -> io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let head = header(kind, call, payload.len())?;
    out.write_all(&head).await?;
    out.write_all(payload).await?;
    out.flush().await
}

/// [`read_frame`] on an async reader.
#[cfg(any(feature = "remote", test))]
#[cfg_attr(not(feature = "remote"), allow(dead_code))] // the client's
pub(crate) async fn read_frame_async(
    input: &mut (impl tokio::io::AsyncRead + Unpin),
) -> io::Result<Option<Frame>> {
    use tokio::io::AsyncReadExt;
    let mut len = [0u8; 4];
    let mut filled = 0;
    while filled < len.len() {
        match input.read(&mut len[filled..]).await? {
            0 if filled == 0 => return Ok(None),
            0 => return Err(io::ErrorKind::UnexpectedEof.into()),
            n => filled += n,
        }
    }
    let size = payload_len(len)?;
    let mut head = [0u8; HEADER];
    input.read_exact(&mut head).await?;
    let (kind, call) = kind_and_call(head)?;
    let mut payload = vec![0u8; size];
    input.read_exact(&mut payload).await?;
    Ok(Some(Frame {
        kind,
        call,
        payload,
    }))
}

/// A schema frame's payload: `[kinds_len u32 LE][kinds JSON][IPC schema
/// message]`. The kinds are the result's columns' [`Kind`]s from DuckDB's
/// logical types (`kinds::of`), one per column; with them the client
/// decodes by DuckDB's types even where the Arrow field
/// is ambiguous (a session that reset `arrow_lossless_conversion` sends
/// UHUGEINT as `Decimal128(38, 0)` and BIT as plain binary).
#[cfg_attr(not(feature = "helper"), allow(dead_code))] // the helper's
pub(crate) fn schema_payload(kinds: &[Kind], ipc: &[u8]) -> Result<Vec<u8>, DbError> {
    let json = encode(&kinds)?;
    let len = u32::try_from(json.len())
        .map_err(|_| protocol_error("a result's column kinds are too large to send"))?;
    let mut out = Vec::with_capacity(4 + json.len() + ipc.len());
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&json);
    out.extend_from_slice(ipc);
    Ok(out)
}

/// Splits a schema frame's payload ([`schema_payload`]) into the kinds and
/// the IPC schema message. A payload that doesn't hold them is
/// `HELPER_PROTOCOL`, reported by position only.
#[cfg_attr(not(feature = "remote"), allow(dead_code))] // the client's
pub(crate) fn read_schema_payload(mut payload: Vec<u8>) -> Result<(Vec<Kind>, Vec<u8>), DbError> {
    let Some(len) = payload
        .first_chunk::<4>()
        .map(|b| u32::from_le_bytes(*b) as usize)
    else {
        return Err(protocol_error("a schema frame without its column kinds"));
    };
    let end = 4usize
        .checked_add(len)
        .filter(|end| *end <= payload.len())
        .ok_or_else(|| protocol_error("a schema frame's column kinds run past its end"))?;
    let kinds = decode(&payload[4..end])?;
    payload.drain(..end);
    Ok((kinds, payload))
}

/// What `open` needs of a [`ConnectConfig`]: the DuckDB fields only, so no
/// password or connection string ever reaches the helper. `duckdb_config`
/// may still hold credentials (`s3_secret_access_key` and the like), so
/// `Debug` shows only how many options there are, and the path only as
/// present or not.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct OpenParams {
    pub path: Option<String>,
    pub create_if_missing: Option<bool>,
    pub restricted: Option<bool>,
    pub duckdb_config: Option<BTreeMap<String, String>>,
}

impl OpenParams {
    #[cfg_attr(not(feature = "remote"), allow(dead_code))] // the client's
    pub(crate) fn of(config: &ConnectConfig) -> Self {
        OpenParams {
            path: config.path.clone(),
            create_if_missing: config.create_if_missing,
            restricted: config.restricted,
            duckdb_config: config.duckdb_config.clone(),
        }
    }

    /// The config the helper opens DuckDB with.
    #[cfg_attr(not(feature = "helper"), allow(dead_code))] // the helper's
    pub(crate) fn config(&self) -> ConnectConfig {
        ConnectConfig {
            driver: DriverType::Duckdb,
            connection_string: None,
            host: None,
            port: None,
            database: None,
            username: None,
            password: None,
            encrypt: None,
            trust_cert: None,
            path: self.path.clone(),
            create_if_missing: self.create_if_missing,
            restricted: self.restricted,
            tls_server_name: None,
            duckdb_config: self.duckdb_config.clone(),
        }
    }
}

impl fmt::Debug for OpenParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenParams")
            .field("path", &self.path.is_some())
            .field("create_if_missing", &self.create_if_missing)
            .field("restricted", &self.restricted)
            .field(
                "options",
                &self.duckdb_config.as_ref().map_or(0, BTreeMap::len),
            )
            .finish()
    }
}

/// A control message from the client. The call id is in the frame.
#[derive(Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum Request {
    /// The first frame: the client's protocol and app version.
    Hello { protocol: u32, version: String },
    /// Opens the database (once, after `hello`).
    Open(OpenParams),
    /// `Driver::query`: the rows as IPC frames, then `done`.
    Query { sql: String, params: Vec<Value> },
    /// `Driver::query_stream`: streaming execution, rows under credit.
    Stream { sql: String, params: Vec<Value> },
    /// `Driver::execute`: answered with `executed`.
    Execute { sql: String, params: Vec<Value> },
    /// `Driver::transaction`: answered with `committed`.
    Transaction { statements: Vec<BatchStatement> },
    /// `Driver::query_read_only` on a clone of its own; DuckDB reads at most
    /// `limit + 1` rows (the client's `RowCap` decides what that row means).
    ReadOnly { sql: String, limit: usize },
    /// `Driver::explain_read_only`: `sql` is the user's statement. The
    /// helper refuses more than one (`READ_ONLY`), makes the EXPLAIN with
    /// `introspect::explain_sql` and runs it in a read-only transaction on
    /// a clone of its own.
    ExplainReadOnly { sql: String, params: Vec<Value> },
    /// Stops the frame's call. Ignored for a call that isn't running.
    Cancel,
    /// Lets a streaming call send `frames` more batch frames.
    Credit { frames: u32 },
    /// Closes the database and ends the helper.
    Close,
}

/// A control message from the helper. The call id is in the frame.
#[derive(Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum Reply {
    /// The answer to `hello`: the helper's protocol, app version and DuckDB
    /// version.
    HelloOk {
        protocol: u32,
        version: String,
        duckdb: String,
    },
    /// The database is open.
    Opened,
    /// `execute`'s rows affected.
    Executed { rows_affected: u64 },
    /// `transaction`'s rows affected, one per statement.
    Committed { rows_affected: Vec<u64> },
    /// A call's rows ended.
    Done,
    /// A call failed. `index` is `TransactionError::index` for a
    /// transaction, `None` otherwise.
    Error {
        error: DbError,
        #[serde(default)]
        index: Option<usize>,
    },
}

/// Writes a control message as JSON.
fn encode(message: &impl Serialize) -> Result<Vec<u8>, DbError> {
    serde_json::to_vec(message).map_err(|e| {
        protocol_error(format!(
            "couldn't write a control message ({:?})",
            e.classify()
        ))
    })
}

/// Reads a control message, reporting a failure by position only.
fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, DbError> {
    serde_json::from_slice(bytes).map_err(|e| {
        protocol_error(format!(
            "a control message didn't parse ({:?} at line {}, column {})",
            e.classify(),
            e.line(),
            e.column()
        ))
    })
}

impl Request {
    #[cfg_attr(not(feature = "remote"), allow(dead_code))] // the client's
    pub(crate) fn encode(&self) -> Result<Vec<u8>, DbError> {
        encode(self)
    }

    #[cfg_attr(not(feature = "helper"), allow(dead_code))] // the helper's
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, DbError> {
        decode(bytes)
    }
}

impl Reply {
    #[cfg_attr(not(feature = "helper"), allow(dead_code))] // the helper's
    pub(crate) fn encode(&self) -> Result<Vec<u8>, DbError> {
        encode(self)
    }

    #[cfg_attr(not(feature = "remote"), allow(dead_code))] // the client's
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, DbError> {
        decode(bytes)
    }
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sql = |name: &str, f: &mut fmt::Formatter<'_>, sql: &str, params: usize| {
            f.debug_struct(name)
                .field("sql_bytes", &sql.len())
                .field("params", &params)
                .finish()
        };
        match self {
            Request::Hello { protocol, version } => f
                .debug_struct("Hello")
                .field("protocol", protocol)
                .field("version", version)
                .finish(),
            Request::Open(params) => f.debug_tuple("Open").field(params).finish(),
            Request::Query { sql: s, params } => sql("Query", f, s, params.len()),
            Request::Stream { sql: s, params } => sql("Stream", f, s, params.len()),
            Request::Execute { sql: s, params } => sql("Execute", f, s, params.len()),
            Request::Transaction { statements } => f
                .debug_struct("Transaction")
                .field("statements", &statements.len())
                .finish(),
            Request::ReadOnly { sql, limit } => f
                .debug_struct("ReadOnly")
                .field("sql_bytes", &sql.len())
                .field("limit", limit)
                .finish(),
            Request::ExplainReadOnly { sql: s, params } => {
                sql("ExplainReadOnly", f, s, params.len())
            }
            Request::Cancel => f.write_str("Cancel"),
            Request::Credit { frames } => f.debug_struct("Credit").field("frames", frames).finish(),
            Request::Close => f.write_str("Close"),
        }
    }
}

impl fmt::Debug for Reply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Reply::HelloOk {
                protocol,
                version,
                duckdb,
            } => f
                .debug_struct("HelloOk")
                .field("protocol", protocol)
                .field("version", version)
                .field("duckdb", duckdb)
                .finish(),
            Reply::Opened => f.write_str("Opened"),
            Reply::Executed { rows_affected } => f
                .debug_struct("Executed")
                .field("rows_affected", rows_affected)
                .finish(),
            Reply::Committed { rows_affected } => f
                .debug_struct("Committed")
                .field("statements", &rows_affected.len())
                .finish(),
            Reply::Done => f.write_str("Done"),
            // DuckDB's message can quote the SQL: the code only.
            Reply::Error { error, index } => f
                .debug_struct("Error")
                .field("code", &error.code)
                .field("index", index)
                .finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::Kind;
    use seaquel_engine::ExpectRows;

    const MARKER: &str = "MARKER_7f3a";

    fn values() -> Vec<Value> {
        vec![
            Value::Null,
            Value::Bool(true),
            Value::Int(i64::MIN),
            Value::Int(42),
            Value::Float(0.1),
            Value::Float(f64::NAN),
            Value::Float(f64::NEG_INFINITY),
            Value::Decimal("-12.50".into()),
            Value::Text(format!("{MARKER} é \u{0}")),
            Value::Bytes(vec![0, 255, 7]),
            Value::Json(serde_json::json!({"a": [1, null]})),
            Value::Array(vec![Value::Int(1), Value::Text("x".into())]),
        ]
    }

    fn requests() -> Vec<Request> {
        let sql = format!("SELECT '{MARKER}'");
        vec![
            Request::Hello {
                protocol: PROTOCOL,
                version: "2026.10.1".into(),
            },
            Request::Open(OpenParams {
                path: Some(format!("/tmp/{MARKER}.duckdb")),
                create_if_missing: Some(true),
                restricted: Some(false),
                duckdb_config: Some(BTreeMap::from([(
                    "s3_secret_access_key".to_string(),
                    MARKER.to_string(),
                )])),
            }),
            Request::Open(OpenParams::default()),
            Request::Query {
                sql: sql.clone(),
                params: values(),
            },
            Request::Stream {
                sql: sql.clone(),
                params: values(),
            },
            Request::Execute {
                sql: sql.clone(),
                params: vec![],
            },
            Request::Transaction {
                statements: vec![
                    BatchStatement {
                        sql: sql.clone(),
                        params: values(),
                        expect_rows: Some(ExpectRows { min: 1 }),
                    },
                    BatchStatement {
                        sql: sql.clone(),
                        params: vec![],
                        expect_rows: None,
                    },
                ],
            },
            Request::ReadOnly {
                sql: sql.clone(),
                limit: 100_000,
            },
            Request::ExplainReadOnly {
                sql,
                params: values(),
            },
            Request::Cancel,
            Request::Credit { frames: 2 },
            Request::Close,
        ]
    }

    fn replies() -> Vec<Reply> {
        vec![
            Reply::HelloOk {
                protocol: PROTOCOL,
                version: "2026.10.1".into(),
                duckdb: "v1.5.0".into(),
            },
            Reply::Opened,
            Reply::Executed { rows_affected: 7 },
            Reply::Committed {
                rows_affected: vec![1, 0, u64::MAX],
            },
            Reply::Done,
            Reply::Error {
                error: DbError::query_error(format!("near \"{MARKER}\"")),
                index: None,
            },
            Reply::Error {
                error: DbError::execute_error(MARKER),
                index: Some(3),
            },
        ]
    }

    /// Every control message reads back as itself: encoding what was
    /// decoded gives the same bytes, and the fields that matter compare.
    #[test]
    fn control_messages_round_trip() {
        for request in requests() {
            let bytes = request.encode().unwrap();
            let back = Request::decode(&bytes).unwrap();
            assert_eq!(back.encode().unwrap(), bytes, "{request:?}");
        }
        for reply in replies() {
            let bytes = reply.encode().unwrap();
            let back = Reply::decode(&bytes).unwrap();
            assert_eq!(back.encode().unwrap(), bytes, "{reply:?}");
        }

        let bytes = Request::Query {
            sql: "SELECT ?".into(),
            params: values(),
        }
        .encode()
        .unwrap();
        let Request::Query { sql, params } = Request::decode(&bytes).unwrap() else {
            panic!("not a query");
        };
        assert_eq!(sql, "SELECT ?");
        assert_eq!(params.len(), values().len());
        for (got, want) in params.iter().zip(values()) {
            match (got, &want) {
                (Value::Float(a), Value::Float(b)) => assert_eq!(a.to_bits(), b.to_bits()),
                _ => assert_eq!(got, &want),
            }
        }

        let bytes = Reply::Error {
            error: DbError {
                message: "boom".into(),
                code: "EXECUTE_ERROR".into(),
            },
            index: Some(3),
        }
        .encode()
        .unwrap();
        let Reply::Error { error, index } = Reply::decode(&bytes).unwrap() else {
            panic!("not an error");
        };
        assert_eq!(
            (error.code.as_str(), error.message.as_str(), index),
            ("EXECUTE_ERROR", "boom", Some(3))
        );

        let open = OpenParams::of(
            &serde_json::from_value::<ConnectConfig>(serde_json::json!({
                "driver": "duckdb",
                "path": "/data/x.duckdb",
                "password": "hunter2",
                "restricted": true,
                "duckdb_config": {"threads": "1"}
            }))
            .unwrap(),
        );
        let bytes = Request::Open(open).encode().unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("hunter2"));
        let Request::Open(open) = Request::decode(&bytes).unwrap() else {
            panic!("not an open");
        };
        let config = open.config();
        assert_eq!(config.path.as_deref(), Some("/data/x.duckdb"));
        assert_eq!(config.restricted, Some(true));
        assert!(config.password.is_none());
        assert_eq!(
            config.duckdb_config,
            Some(BTreeMap::from([("threads".to_string(), "1".to_string())]))
        );
    }

    /// No `Debug` shows SQL, a value, a path, an option's value or DuckDB's
    /// message; neither does a control message that doesn't parse.
    #[test]
    fn debug_and_errors_never_show_a_payload() {
        for request in requests() {
            let debug = format!("{request:?}");
            assert!(!debug.contains(MARKER), "{debug}");
        }
        for reply in replies() {
            let debug = format!("{reply:?}");
            assert!(!debug.contains(MARKER), "{debug}");
        }
        let frame = Frame {
            kind: FrameKind::Control,
            call: 9,
            payload: Request::Query {
                sql: MARKER.into(),
                params: vec![],
            }
            .encode()
            .unwrap(),
        };
        assert!(!format!("{frame:?}").contains(MARKER));

        let bad = format!(r#"{{"type": "query", "sql": 5, "params": ["{MARKER}"]}}"#);
        let e = Request::decode(bad.as_bytes()).unwrap_err();
        assert_eq!(e.code, HELPER_PROTOCOL);
        assert!(!e.message.contains(MARKER), "{}", e.message);
        let bad = format!(r#"{{"type": "{MARKER}"}}"#);
        let e = Reply::decode(bad.as_bytes()).unwrap_err();
        assert!(!e.message.contains(MARKER), "{}", e.message);
    }

    /// One message of each kind, as JSON: the wire's field names are
    /// camelCase throughout, like the rest of Seaquel's JSON.
    #[test]
    fn control_messages_have_one_json_shape() {
        let json = |bytes: Vec<u8>| String::from_utf8(bytes).unwrap();
        let q = |sql: &str| sql.to_string();
        let requests = [
            (
                Request::Hello {
                    protocol: 1,
                    version: "2026.10.1".into(),
                },
                r#"{"type":"hello","protocol":1,"version":"2026.10.1"}"#,
            ),
            (
                Request::Open(OpenParams {
                    path: Some("a.duckdb".into()),
                    create_if_missing: Some(true),
                    restricted: Some(false),
                    duckdb_config: Some(BTreeMap::from([("threads".into(), "1".into())])),
                }),
                r#"{"type":"open","path":"a.duckdb","createIfMissing":true,"restricted":false,"duckdbConfig":{"threads":"1"}}"#,
            ),
            (
                Request::Query {
                    sql: q("SELECT ?"),
                    params: vec![Value::Int(1)],
                },
                r#"{"type":"query","sql":"SELECT ?","params":[1]}"#,
            ),
            (
                Request::Stream {
                    sql: q("SELECT 1"),
                    params: vec![],
                },
                r#"{"type":"stream","sql":"SELECT 1","params":[]}"#,
            ),
            (
                Request::Execute {
                    sql: q("DELETE FROM t"),
                    params: vec![],
                },
                r#"{"type":"execute","sql":"DELETE FROM t","params":[]}"#,
            ),
            (
                Request::Transaction {
                    statements: vec![BatchStatement {
                        sql: q("UPDATE t SET a = ?"),
                        params: vec![Value::Null],
                        expect_rows: Some(ExpectRows { min: 1 }),
                    }],
                },
                r#"{"type":"transaction","statements":[{"sql":"UPDATE t SET a = ?","params":[null],"expectRows":{"min":1}}]}"#,
            ),
            (
                Request::ReadOnly {
                    sql: q("SELECT 1"),
                    limit: 1000,
                },
                r#"{"type":"readOnly","sql":"SELECT 1","limit":1000}"#,
            ),
            (
                Request::ExplainReadOnly {
                    sql: q("EXPLAIN SELECT 1"),
                    params: vec![],
                },
                r#"{"type":"explainReadOnly","sql":"EXPLAIN SELECT 1","params":[]}"#,
            ),
            (Request::Cancel, r#"{"type":"cancel"}"#),
            (
                Request::Credit { frames: 2 },
                r#"{"type":"credit","frames":2}"#,
            ),
            (Request::Close, r#"{"type":"close"}"#),
        ];
        for (request, want) in requests {
            assert_eq!(json(request.encode().unwrap()), want);
            assert!(Request::decode(want.as_bytes()).is_ok(), "{want}");
        }
        let replies = [
            (
                Reply::HelloOk {
                    protocol: 1,
                    version: "2026.10.1".into(),
                    duckdb: "v1.5.0".into(),
                },
                r#"{"type":"helloOk","protocol":1,"version":"2026.10.1","duckdb":"v1.5.0"}"#,
            ),
            (Reply::Opened, r#"{"type":"opened"}"#),
            (
                Reply::Executed { rows_affected: 3 },
                r#"{"type":"executed","rowsAffected":3}"#,
            ),
            (
                Reply::Committed {
                    rows_affected: vec![1, 2],
                },
                r#"{"type":"committed","rowsAffected":[1,2]}"#,
            ),
            (Reply::Done, r#"{"type":"done"}"#),
            (
                Reply::Error {
                    error: DbError {
                        message: "m".into(),
                        code: "C".into(),
                    },
                    index: Some(0),
                },
                r#"{"type":"error","error":{"message":"m","code":"C"},"index":0}"#,
            ),
        ];
        for (reply, want) in replies {
            assert_eq!(json(reply.encode().unwrap()), want);
            assert!(Reply::decode(want.as_bytes()).is_ok(), "{want}");
        }
    }

    /// A schema frame carries the columns' kinds (from DuckDB's logical
    /// types) ahead of the IPC schema message, nested
    /// kinds included. A payload that doesn't hold them is a broken wire,
    /// reported without its bytes.
    #[test]
    fn schema_frames_carry_the_columns_kinds() {
        let kinds = vec![
            Kind::Plain,
            Kind::UHugeInt,
            Kind::List(Box::new(Kind::Bit)),
            Kind::Struct(vec![
                Kind::Json,
                Kind::Map(
                    Box::new(Kind::Uuid),
                    Box::new(Kind::Union(vec![
                        Kind::TimeTz,
                        Kind::Bignum,
                        Kind::HugeInt,
                        Kind::Bool,
                    ])),
                ),
            ]),
        ];
        let ipc = b"the IPC schema message".to_vec();
        let payload = schema_payload(&kinds, &ipc).unwrap();
        let (back, rest) = read_schema_payload(payload).unwrap();
        assert_eq!(back, kinds);
        assert_eq!(rest, ipc);
        let (none, rest) = read_schema_payload(schema_payload(&[], b"x").unwrap()).unwrap();
        assert!(none.is_empty());
        assert_eq!(rest, b"x");

        let with_len = |len: u32, body: &str| {
            let mut p = len.to_le_bytes().to_vec();
            p.extend_from_slice(body.as_bytes());
            p
        };
        for bad in [
            vec![],
            vec![1, 0, 0],
            with_len(100, MARKER),
            with_len(u32::MAX, MARKER),
            with_len(MARKER.len() as u32, MARKER),
            with_len(12, r#"["NotAKind"]"#),
        ] {
            let e = read_schema_payload(bad).unwrap_err();
            assert_eq!(e.code, HELPER_PROTOCOL, "{e:?}");
            assert!(!e.message.contains(MARKER), "{}", e.message);
        }
    }

    #[test]
    fn frames_round_trip() {
        let mut wire = Vec::new();
        write_frame(&mut wire, FrameKind::Control, 1, b"{}").unwrap();
        write_frame(&mut wire, FrameKind::Schema, 2, b"").unwrap();
        write_frame(&mut wire, FrameKind::Batch, u32::MAX, &[7; 1000]).unwrap();
        assert_eq!(&wire[..9], &[7, 0, 0, 0, 0, 1, 0, 0, 0]);

        let mut input = &wire[..];
        let a = read_frame(&mut input).unwrap().unwrap();
        assert_eq!(
            (a.kind, a.call, &a.payload[..]),
            (FrameKind::Control, 1, &b"{}"[..])
        );
        let b = read_frame(&mut input).unwrap().unwrap();
        assert_eq!((b.kind, b.call, b.payload.len()), (FrameKind::Schema, 2, 0));
        let c = read_frame(&mut input).unwrap().unwrap();
        assert_eq!(
            (c.kind, c.call, c.payload.len()),
            (FrameKind::Batch, u32::MAX, 1000)
        );
        assert!(read_frame(&mut input).unwrap().is_none());

        // Cut anywhere inside a frame: an error, not a clean end.
        for cut in [1, 4, 8, 9, 10] {
            let e = read_frame(&mut &wire[..cut]).unwrap_err();
            assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof, "cut at {cut}");
        }
        // An unknown kind, and a `len` too short for the header.
        let e = read_frame(&mut &[5u8, 0, 0, 0, 9, 0, 0, 0, 0][..]).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        let e = read_frame(&mut &[4u8, 0, 0, 0, 0, 0, 0, 0][..]).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
    }

    /// A frame over [`MAX_FRAME`] is refused on write (nothing written) and
    /// on read (before its payload is allocated or read); one exactly at the
    /// limit passes both ways.
    #[test]
    fn a_frame_over_the_limit_is_refused_both_ways() {
        let largest = vec![0u8; MAX_FRAME - HEADER];
        let mut wire = Vec::new();
        write_frame(&mut wire, FrameKind::Batch, 1, &largest).unwrap();
        let back = read_frame(&mut &wire[..]).unwrap().unwrap();
        assert_eq!(back.payload.len(), MAX_FRAME - HEADER);

        let mut out = Vec::new();
        let e = write_frame(
            &mut out,
            FrameKind::Batch,
            1,
            &vec![0u8; MAX_FRAME - HEADER + 1],
        )
        .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        assert!(out.is_empty(), "wrote {} bytes", out.len());

        // A header announcing one byte too many, and no payload behind it:
        // refused on the header alone.
        let mut over = ((MAX_FRAME + 1) as u32).to_le_bytes().to_vec();
        over.extend([2, 1, 0, 0, 0]);
        let e = read_frame(&mut &over[..]).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        let e = read_frame(&mut &u32::MAX.to_le_bytes()[..]).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn async_frames_match_sync_frames() {
        let mut wire = Vec::new();
        write_frame_async(&mut wire, FrameKind::Control, 3, b"{\"type\":\"done\"}")
            .await
            .unwrap();
        let mut sync = Vec::new();
        write_frame(&mut sync, FrameKind::Control, 3, b"{\"type\":\"done\"}").unwrap();
        assert_eq!(wire, sync);

        let mut input = &wire[..];
        let frame = read_frame_async(&mut input).await.unwrap().unwrap();
        assert_eq!((frame.kind, frame.call), (FrameKind::Control, 3));
        assert!(matches!(Reply::decode(&frame.payload), Ok(Reply::Done)));
        assert!(read_frame_async(&mut input).await.unwrap().is_none());

        let mut out = Vec::new();
        let e = write_frame_async(&mut out, FrameKind::Batch, 1, &vec![0u8; MAX_FRAME])
            .await
            .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        assert!(out.is_empty());
        let mut over = ((MAX_FRAME + 1) as u32).to_le_bytes().to_vec();
        over.extend([2, 1, 0, 0, 0]);
        let e = read_frame_async(&mut &over[..]).await.unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        let e = read_frame_async(&mut &wire[..6]).await.unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof);
    }
}
