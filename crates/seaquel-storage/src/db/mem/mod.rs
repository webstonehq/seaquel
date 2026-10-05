//! The in-memory executor: the part of sqlx's API
//! this crate uses, over one SQLite connection to an in-memory database
//! (`ffi.rs`). The demo's metadata file lives here; the page keeps
//! snapshots of it ([`SqlitePool::snapshot`]).
//!
//! It copies sqlx 0.8.6's behaviour where storage can see it:
//! - **Statements.** The text is trimmed and its statements run in order,
//!   each prepared after the previous one ran. Values are consumed in order
//!   across them; `?NNN` and `$NNN` pick a value by number; a parameter past
//!   the values is NULL and extra values are ignored. `rows_affected` sums
//!   `sqlite3_changes` after each statement, as sqlx's does (so a `SELECT`
//!   after a write repeats that write's count). `fetch_optional` and
//!   `fetch_one` stop at the first row, and a later statement in the text
//!   never runs.
//! - **Reads.** Rows keep a copy of each cell. `try_get` checks the cell's
//!   storage class against the Rust type with sqlx's `compatible` rules
//!   (`String` only from TEXT, `f64` only from REAL, integers and `bool`
//!   from INTEGER, bytes from BLOB or TEXT; NULL always passes) and then
//!   decodes with SQLite's own conversions; `try_get_unchecked` skips the
//!   check. Columns are found by name (the last of a repeated name) or
//!   index. Text that isn't UTF-8 is a decode error.
//! - **Errors** keep SQLite's extended code and message, with sqlx's
//!   wording and kinds, so `StorageError::code` (`STORAGE_FULL`, the
//!   corrupt check) and the "no transaction is active" match read them the
//!   same way.
//! - **The pool** is the one connection behind an async mutex: a caller
//!   holds it from `acquire` (or `begin`) until it drops it, and the next
//!   caller waits, as on a sqlx pool of one. In the browser a wait past
//!   30 s (sqlx's acquire timeout) fails with `PoolTimedOut`. A connection
//!   or transaction dropped inside a transaction is rolled back there and
//!   then, synchronously.
//!
//! Every call is synchronous behind its `async` signature: nothing here
//! awaits anything but the pool's mutex.

#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

mod ffi;
#[cfg(test)]
mod tests;

use std::borrow::Cow;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::lock::{self, Mutex, OwnedMutexGuard};

use ffi::{Commits, Db, RawError, Step};
pub use ffi::{Kind, Value};

/// Any error a decoder or encoder returns, as sqlx's `BoxDynError`.
pub type BoxDynError = Box<dyn std::error::Error + Send + Sync + 'static>;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// sqlx's `Error`, for the variants this executor can produce, with the
/// same messages.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("error returned from database: {0}")]
    Database(#[source] Box<SqliteError>),
    #[error("encountered unexpected or invalid data: {0}")]
    Protocol(String),
    #[error("no rows returned by a query that expected to return at least one row")]
    RowNotFound,
    #[error("column index out of bounds: the len is {len}, but the index is {index}")]
    ColumnIndexOutOfBounds { index: usize, len: usize },
    #[error("no column found for name: {0}")]
    ColumnNotFound(String),
    #[error("error occurred while decoding column {index}: {source}")]
    ColumnDecode {
        index: String,
        #[source]
        source: BoxDynError,
    },
    #[error("error occurred while encoding a value: {0}")]
    Encode(#[source] BoxDynError),
    #[error("error occurred while decoding: {0}")]
    Decode(#[source] BoxDynError),
    #[error("pool timed out while waiting for an open connection")]
    PoolTimedOut,
    #[error("attempted to acquire a connection on a closed pool")]
    PoolClosed,
}

impl From<RawError> for Error {
    fn from(e: RawError) -> Self {
        Error::Database(Box::new(SqliteError {
            code: e.code,
            message: e.message,
        }))
    }
}

/// sqlx's `SqliteError`: SQLite's extended result code and message.
#[derive(Debug)]
pub struct SqliteError {
    code: i32,
    message: String,
}

impl SqliteError {
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The extended result code as text, as sqlx gives it.
    pub fn code(&self) -> Option<Cow<'_, str>> {
        Some(Cow::Owned(self.code.to_string()))
    }

    pub fn kind(&self) -> ErrorKind {
        match self.code {
            2067 | 1555 => ErrorKind::UniqueViolation,
            787 => ErrorKind::ForeignKeyViolation,
            1299 => ErrorKind::NotNullViolation,
            275 => ErrorKind::CheckViolation,
            _ => ErrorKind::Other,
        }
    }
}

impl std::fmt::Display for SqliteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "(code: {}) {}", self.code, self.message)
    }
}

impl std::error::Error for SqliteError {}

/// sqlx's `ErrorKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    UniqueViolation,
    ForeignKeyViolation,
    NotNullViolation,
    CheckViolation,
    Other,
}

/// sqlx's `MigrateError`, for the variants the in-memory migrator
/// produces, with the same messages.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MigrateError {
    #[error("while executing migrations: {0}")]
    Execute(#[from] Error),
    #[error("while executing migration {1}: {0}")]
    ExecuteMigration(#[source] Error, i64),
    #[error("migration {0} was previously applied but has been modified")]
    VersionMismatch(i64),
    #[error(
        "migration {0} is partially applied; fix and remove row from `_sqlx_migrations` table"
    )]
    Dirty(i64),
}

// ---------------------------------------------------------------------------
// Values in
// ---------------------------------------------------------------------------

/// One bound value, as sqlx's `SqliteArgumentValue` holds it.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    Null,
    Int(i64),
    Double(f64),
    Text(String),
    Blob(Vec<u8>),
}

/// A Rust value storage binds, encoded as sqlx encodes it for SQLite.
pub trait Encode {
    fn encode(&self) -> Arg;
}

impl<T: Encode + ?Sized> Encode for &T {
    fn encode(&self) -> Arg {
        (**self).encode()
    }
}

impl<T: Encode> Encode for Option<T> {
    fn encode(&self) -> Arg {
        self.as_ref().map_or(Arg::Null, Encode::encode)
    }
}

macro_rules! encode_int {
    ($($t:ty),*) => {$(
        impl Encode for $t {
            fn encode(&self) -> Arg {
                Arg::Int(i64::from(*self))
            }
        }
    )*};
}
encode_int!(i8, i16, i32, i64, u8, u16, u32, bool);

impl Encode for f64 {
    fn encode(&self) -> Arg {
        Arg::Double(*self)
    }
}

impl Encode for f32 {
    fn encode(&self) -> Arg {
        Arg::Double(f64::from(*self))
    }
}

impl Encode for str {
    fn encode(&self) -> Arg {
        Arg::Text(self.to_string())
    }
}

impl Encode for String {
    fn encode(&self) -> Arg {
        Arg::Text(self.clone())
    }
}

impl Encode for Box<str> {
    fn encode(&self) -> Arg {
        Arg::Text(self.to_string())
    }
}

impl Encode for Cow<'_, str> {
    fn encode(&self) -> Arg {
        Arg::Text(self.to_string())
    }
}

impl Encode for [u8] {
    fn encode(&self) -> Arg {
        Arg::Blob(self.to_vec())
    }
}

impl Encode for Vec<u8> {
    fn encode(&self) -> Arg {
        Arg::Blob(self.clone())
    }
}

impl Encode for Cow<'_, [u8]> {
    fn encode(&self) -> Arg {
        Arg::Blob(self.to_vec())
    }
}

// ---------------------------------------------------------------------------
// Values out
// ---------------------------------------------------------------------------

/// sqlx's `Type::compatible` for SQLite, given a cell's storage class.
pub trait Type {
    fn compatible(kind: Kind) -> bool;
    /// The SQL type sqlx names for it, for the mismatch message.
    fn sql_name() -> &'static str;
}

/// sqlx's `Decode` for SQLite: SQLite's conversions, as sqlx calls them.
pub trait Decode: Sized {
    fn decode(v: &Value) -> Result<Self, BoxDynError>;
}

impl<T: Type> Type for Option<T> {
    fn compatible(kind: Kind) -> bool {
        kind == Kind::Null || T::compatible(kind)
    }
    fn sql_name() -> &'static str {
        T::sql_name()
    }
}

impl<T: Decode> Decode for Option<T> {
    fn decode(v: &Value) -> Result<Self, BoxDynError> {
        if v.kind() == Kind::Null {
            Ok(None)
        } else {
            T::decode(v).map(Some)
        }
    }
}

macro_rules! decode_int {
    ($($t:ty),*) => {$(
        impl Type for $t {
            fn compatible(kind: Kind) -> bool {
                kind == Kind::Integer
            }
            fn sql_name() -> &'static str {
                "INTEGER"
            }
        }
        impl Decode for $t {
            fn decode(v: &Value) -> Result<Self, BoxDynError> {
                Ok(<$t>::try_from(v.int64())?)
            }
        }
    )*};
}
decode_int!(i8, i16, i32, u8, u16, u32, u64);

impl Type for i64 {
    fn compatible(kind: Kind) -> bool {
        kind == Kind::Integer
    }
    fn sql_name() -> &'static str {
        "INTEGER"
    }
}

impl Decode for i64 {
    fn decode(v: &Value) -> Result<Self, BoxDynError> {
        Ok(v.int64())
    }
}

impl Type for bool {
    fn compatible(kind: Kind) -> bool {
        kind == Kind::Integer
    }
    fn sql_name() -> &'static str {
        "BOOLEAN"
    }
}

impl Decode for bool {
    fn decode(v: &Value) -> Result<Self, BoxDynError> {
        Ok(v.int64() != 0)
    }
}

impl Type for f64 {
    fn compatible(kind: Kind) -> bool {
        kind == Kind::Float
    }
    fn sql_name() -> &'static str {
        "REAL"
    }
}

impl Decode for f64 {
    fn decode(v: &Value) -> Result<Self, BoxDynError> {
        Ok(v.double())
    }
}

impl Type for String {
    fn compatible(kind: Kind) -> bool {
        kind == Kind::Text
    }
    fn sql_name() -> &'static str {
        "TEXT"
    }
}

impl Decode for String {
    fn decode(v: &Value) -> Result<Self, BoxDynError> {
        Ok(String::from_utf8(v.bytes())?)
    }
}

impl Type for Vec<u8> {
    fn compatible(kind: Kind) -> bool {
        matches!(kind, Kind::Blob | Kind::Text)
    }
    fn sql_name() -> &'static str {
        "BLOB"
    }
}

impl Decode for Vec<u8> {
    fn decode(v: &Value) -> Result<Self, BoxDynError> {
        Ok(v.bytes())
    }
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Integer => "INTEGER",
        Kind::Float => "REAL",
        Kind::Text => "TEXT",
        Kind::Blob => "BLOB",
        Kind::Null => "NULL",
    }
}

/// A column by index or name, as sqlx's `ColumnIndex`.
pub trait ColumnIndex: std::fmt::Debug {
    fn index(&self, row: &SqliteRow) -> Result<usize, Error>;
}

impl ColumnIndex for usize {
    fn index(&self, row: &SqliteRow) -> Result<usize, Error> {
        if *self >= row.values.len() {
            return Err(Error::ColumnIndexOutOfBounds {
                index: *self,
                len: row.values.len(),
            });
        }
        Ok(*self)
    }
}

impl ColumnIndex for &str {
    fn index(&self, row: &SqliteRow) -> Result<usize, Error> {
        row.names
            .get(*self)
            .copied()
            .ok_or_else(|| Error::ColumnNotFound((*self).to_string()))
    }
}

/// One row: a copy of each cell and the statement's column names.
pub struct SqliteRow {
    values: Box<[Value]>,
    names: Arc<HashMap<String, usize>>,
}

impl std::fmt::Debug for SqliteRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SqliteRow({} columns)", self.values.len())
    }
}

/// sqlx's `Row`, for the methods storage calls.
pub trait Row {
    fn try_get<T, I>(&self, index: I) -> Result<T, Error>
    where
        T: Decode + Type,
        I: ColumnIndex;
    fn try_get_unchecked<T, I>(&self, index: I) -> Result<T, Error>
    where
        T: Decode,
        I: ColumnIndex;
    // Only `Storage::debug_rows` (the `test-hooks` feature) and the tests
    // read it; Core's wasm32 build links storage without either.
    #[cfg_attr(not(any(test, feature = "test-hooks")), allow(dead_code))]
    fn len(&self) -> usize;
}

impl SqliteRow {
    fn value<I: ColumnIndex>(&self, index: &I) -> Result<&Value, Error> {
        let i = index.index(self)?;
        self.values.get(i).ok_or(Error::ColumnIndexOutOfBounds {
            index: i,
            len: self.values.len(),
        })
    }
}

impl Row for SqliteRow {
    fn try_get<T, I>(&self, index: I) -> Result<T, Error>
    where
        T: Decode + Type,
        I: ColumnIndex,
    {
        let value = self.value(&index)?;
        let kind = value.kind();
        if kind != Kind::Null && !T::compatible(kind) {
            return Err(Error::ColumnDecode {
                index: format!("{index:?}"),
                source: format!(
                    "mismatched types; Rust type `{}` (as SQL type `{}`) is not compatible with \
                     SQL type `{}`",
                    std::any::type_name::<T>(),
                    T::sql_name(),
                    kind_name(kind)
                )
                .into(),
            });
        }
        T::decode(value).map_err(|source| Error::ColumnDecode {
            index: format!("{index:?}"),
            source,
        })
    }

    fn try_get_unchecked<T, I>(&self, index: I) -> Result<T, Error>
    where
        T: Decode,
        I: ColumnIndex,
    {
        let value = self.value(&index)?;
        T::decode(value).map_err(|source| Error::ColumnDecode {
            index: format!("{index:?}"),
            source,
        })
    }

    fn len(&self) -> usize {
        self.values.len()
    }
}

/// What a cell holds, for the flag codecs (as `native::Cell`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cell {
    Null,
    Integer(i64),
    Real(f64),
    Other,
}

/// The [`Cell`] in `col`.
pub fn cell(row: &SqliteRow, col: &str) -> Result<Cell, Error> {
    let v = row.value(&col)?;
    Ok(match v.kind() {
        Kind::Null => Cell::Null,
        Kind::Integer => Cell::Integer(v.int64()),
        Kind::Float => Cell::Real(v.double()),
        Kind::Text | Kind::Blob => Cell::Other,
    })
}

/// sqlx's `FromRow` for the tuples `query_as` reads: each element by
/// position, checked.
pub trait FromRow: Sized {
    fn from_row(row: &SqliteRow) -> Result<Self, Error>;
}

macro_rules! from_row_tuple {
    ($($i:tt $t:ident),+) => {
        impl<$($t: Decode + Type),+> FromRow for ($($t,)+) {
            fn from_row(row: &SqliteRow) -> Result<Self, Error> {
                Ok(($(row.try_get::<$t, usize>($i)?,)+))
            }
        }
    };
}
from_row_tuple!(0 A);
from_row_tuple!(0 A, 1 B);
from_row_tuple!(0 A, 1 B, 2 C);
from_row_tuple!(0 A, 1 B, 2 C, 3 D);
from_row_tuple!(0 A, 1 B, 2 C, 3 D, 4 E);
from_row_tuple!(0 A, 1 B, 2 C, 3 D, 4 E, 5 F);
from_row_tuple!(0 A, 1 B, 2 C, 3 D, 4 E, 5 F, 6 G);
from_row_tuple!(0 A, 1 B, 2 C, 3 D, 4 E, 5 F, 6 G, 7 H);
from_row_tuple!(0 A, 1 B, 2 C, 3 D, 4 E, 5 F, 6 G, 7 H, 8 I);
from_row_tuple!(0 A, 1 B, 2 C, 3 D, 4 E, 5 F, 6 G, 7 H, 8 I, 9 J);

// ---------------------------------------------------------------------------
// The connection
// ---------------------------------------------------------------------------

/// What a statement run returned: sqlx's `SqliteQueryResult`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SqliteQueryResult {
    changes: u64,
}

impl SqliteQueryResult {
    pub fn rows_affected(&self) -> u64 {
        self.changes
    }
}

/// Which rows a run keeps.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Keep {
    /// None (`execute`).
    None,
    /// Every row (`fetch_all`).
    All,
    /// The first, then stop (`fetch_optional`, `fetch_one`).
    First,
}

/// One SQLite connection.
pub struct SqliteConnection {
    db: Db,
}

impl std::fmt::Debug for SqliteConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SqliteConnection")
    }
}

impl SqliteConnection {
    /// A connection to `uri`, for tests that need two connections to one
    /// `memdb` file.
    #[cfg(test)]
    pub(crate) fn open_uri(uri: &str) -> Result<Self, Error> {
        Ok(Self {
            db: Db::open_uri(uri)?,
        })
    }

    /// Runs `sql`'s statements in order with `args`, as sqlx's
    /// `ExecuteIter` does (see the module docs).
    fn run(
        &mut self,
        sql: &str,
        args: &[Arg],
        keep: Keep,
        rows: &mut Vec<SqliteRow>,
    ) -> Result<SqliteQueryResult, Error> {
        let mut rest = sql.trim().as_bytes();
        let mut used = 0;
        let mut done = SqliteQueryResult::default();
        while !rest.is_empty() {
            let (stmt, n) = self.db.prepare(rest)?;
            rest = &rest[n..];
            let Some(mut stmt) = stmt else {
                if n == 0 {
                    // A NUL ends the text, as SQLite reads it.
                    break;
                }
                continue;
            };
            used += bind(&mut stmt, args, used)?;
            let mut names: Option<Arc<HashMap<String, usize>>> = None;
            loop {
                match stmt.step()? {
                    Step::Row => {
                        if keep == Keep::None {
                            continue;
                        }
                        let names = names
                            .get_or_insert_with(|| {
                                let mut map = HashMap::new();
                                for i in 0..stmt.column_count() {
                                    map.insert(stmt.column_name(i), i);
                                }
                                Arc::new(map)
                            })
                            .clone();
                        let values = (0..stmt.column_count())
                            .map(|i| stmt.column_value(i))
                            .collect::<Result<Box<[Value]>, _>>()?;
                        rows.push(SqliteRow { values, names });
                        if keep == Keep::First {
                            return Ok(done);
                        }
                    }
                    Step::Done => {
                        done.changes += self.db.changes();
                        break;
                    }
                }
            }
        }
        Ok(done)
    }

    /// Runs `sql` with no values and no rows kept: for the executor's own
    /// `BEGIN`, `COMMIT` and `ROLLBACK`.
    fn exec(&mut self, sql: &str) -> Result<(), Error> {
        self.run(sql, &[], Keep::None, &mut Vec::new()).map(drop)
    }

    /// `ROLLBACK`, when a transaction is open. "no transaction is active"
    /// can't happen, since it checks first.
    fn rollback_if_open(&mut self) -> Result<(), Error> {
        if self.db.autocommit() {
            return Ok(());
        }
        self.exec("ROLLBACK")
    }
}

/// Binds the statement's parameters from `args`, starting after the
/// `offset` values earlier statements used, as sqlx's
/// `SqliteArguments::bind`. Returns how many positional values it used.
fn bind(stmt: &mut ffi::Stmt<'_>, args: &[Arg], offset: usize) -> Result<usize, Error> {
    let mut next = offset;
    for param in 1..=stmt.bind_count() {
        let n = match stmt.bind_name(param) {
            Some(name) => numbered(&name)?,
            None => {
                next += 1;
                next
            }
        };
        // SQLite treats unbound variables as NULL, as sqlx leaves them.
        if n > args.len() {
            break;
        }
        let Some(arg) = n.checked_sub(1).and_then(|i| args.get(i)) else {
            return Err(Error::Protocol(format!("parameter {n} is out of range")));
        };
        match arg {
            Arg::Null => stmt.bind_null(param)?,
            Arg::Int(v) => stmt.bind_int64(param, *v)?,
            Arg::Double(v) => stmt.bind_double(param, *v)?,
            Arg::Text(v) => stmt.bind_text(param, v)?,
            Arg::Blob(v) => stmt.bind_blob(param, v)?,
        }
    }
    Ok(next - offset)
}

/// The value number of a named parameter: `?NNN` or `$NNN` (the leading
/// digits, as sqlx's `atoi` reads them). Other forms are refused, as sqlx
/// refuses them.
fn numbered(name: &str) -> Result<usize, Error> {
    let digits = |s: &str| -> Option<usize> {
        let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
        s[..end].parse().ok()
    };
    if let Some(rest) = name.strip_prefix('?') {
        digits(rest).ok_or_else(|| Error::Protocol(format!("parameter of the form ?NNN: {name}")))
    } else if let Some(rest) = name.strip_prefix('$') {
        digits(rest).ok_or_else(|| {
            Error::Protocol(format!(
                "parameters with non-integer names are not currently supported: {rest}"
            ))
        })
    } else {
        Err(Error::Protocol(format!(
            "unsupported SQL parameter format: {name}"
        )))
    }
}

// ---------------------------------------------------------------------------
// The pool
// ---------------------------------------------------------------------------

/// How long `acquire` waits in the browser before `PoolTimedOut`: sqlx's
/// default acquire timeout.
#[cfg(target_arch = "wasm32")]
const ACQUIRE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

struct PoolInner {
    conn: Arc<Mutex<SqliteConnection>>,
    commits: Arc<Commits>,
    closed: AtomicBool,
}

/// The one connection, shared: sqlx's `SqlitePool` with one connection.
#[derive(Clone)]
pub struct SqlitePool {
    inner: Arc<PoolInner>,
}

impl std::fmt::Debug for SqlitePool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SqlitePool(in memory)")
    }
}

/// The page size of a new file: SQLite's usual default, which every
/// desktop and web file has (the wasm build's default is 8192).
const NEW_FILE_PAGE_SIZE: u32 = 4096;

impl SqlitePool {
    /// A connection to SQLite's memory database, holding `image` when one
    /// is given (an empty image is a new database), with foreign keys on.
    ///
    /// An image whose header says WAL (a desktop file) is opened as a
    /// rollback-journal file: the memory database has no WAL, and the
    /// header is the only difference. Bytes that aren't a database open,
    /// and the first read fails with SQLite's "file is not a database".
    pub fn open_in_memory(image: Option<&[u8]>) -> Result<Self, Error> {
        let db = Db::open_memory()?;
        match image.filter(|i| !i.is_empty()) {
            Some(image) => {
                let mut image = image.to_vec();
                if image.starts_with(crate::open::SQLITE_MAGIC) && image.len() > 19 {
                    for b in &mut image[18..20] {
                        if *b == 2 {
                            *b = 1;
                        }
                    }
                }
                db.deserialize(&image)?;
            }
            None => {
                let mut conn = SqliteConnection { db };
                conn.exec(&format!("PRAGMA page_size = {NEW_FILE_PAGE_SIZE}"))?;
                return Self::finish(conn);
            }
        }
        Self::finish(SqliteConnection { db })
    }

    fn finish(mut conn: SqliteConnection) -> Result<Self, Error> {
        conn.exec("PRAGMA foreign_keys = ON")?;
        let commits = conn.db.commits();
        Ok(Self {
            inner: Arc::new(PoolInner {
                conn: Arc::new(Mutex::new(conn)),
                commits,
                closed: AtomicBool::new(false),
            }),
        })
    }

    /// The connection, once no one else holds it.
    pub async fn acquire(&self) -> Result<PoolConnection, Error> {
        if self.inner.closed.load(Ordering::Relaxed) {
            return Err(Error::PoolClosed);
        }
        let conn = Arc::clone(&self.inner.conn);
        if let Some(guard) = lock::try_lock_owned(&conn) {
            return Ok(PoolConnection { guard });
        }
        #[cfg(target_arch = "wasm32")]
        {
            use futures::future::{select, Either};
            use seaquel_runtime::Executor as _;
            let wait = seaquel_runtime::WasmExecutor.sleep(ACQUIRE_TIMEOUT);
            match select(Box::pin(conn.lock_owned()), wait).await {
                Either::Left((guard, _)) => Ok(PoolConnection { guard }),
                Either::Right(_) => Err(Error::PoolTimedOut),
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            Ok(PoolConnection {
                guard: conn.lock_owned().await,
            })
        }
    }

    /// A deferred transaction (`BEGIN`) on the connection.
    pub async fn begin(&self) -> Result<Transaction, Error> {
        self.begin_with("BEGIN").await
    }

    /// A transaction begun with `sql` (`BEGIN IMMEDIATE`).
    pub async fn begin_with(&self, sql: &str) -> Result<Transaction, Error> {
        let mut conn = self.acquire().await?;
        conn.exec(sql)?;
        Ok(Transaction { conn: Some(conn) })
    }

    /// Refuses every later `acquire`. The database lives until the last
    /// clone is dropped.
    pub async fn close(&self) {
        self.inner.closed.store(true, Ordering::Relaxed);
    }

    /// The database as one file image (`sqlite3_serialize`), for the page
    /// to keep. Refused while someone holds the connection or a transaction
    /// is open, since the image would hold uncommitted rows: try again
    /// after the call in flight.
    pub fn snapshot(&self) -> Result<Vec<u8>, Error> {
        let busy = || {
            Error::from(RawError {
                code: 5,
                message: "database is locked: a call is still using it".to_string(),
            })
        };
        let conn = lock::try_lock(&self.inner.conn).ok_or_else(busy)?;
        if !conn.db.autocommit() {
            return Err(busy());
        }
        Ok(conn.db.serialize()?)
    }

    /// How many transactions have committed since the pool opened.
    pub fn commits(&self) -> u64 {
        self.inner.commits.get()
    }
}

/// The connection, held until dropped (sqlx's `PoolConnection`). Dropped
/// inside a transaction, it rolls that back first.
pub struct PoolConnection {
    guard: OwnedMutexGuard<SqliteConnection>,
}

impl PoolConnection {
    /// sqlx closes such a connection instead of pooling it; the one
    /// in-memory connection can't be replaced, and `Drop` rolls back
    /// whatever was left open, so this does nothing.
    pub fn close_on_drop(&mut self) {}

    /// `ROLLBACK` now, if a transaction is open (a dropped `WriteTx`).
    pub fn rollback_now(&mut self) -> Result<(), Error> {
        self.guard.rollback_if_open()
    }
}

impl Drop for PoolConnection {
    fn drop(&mut self) {
        if let Err(e) = self.guard.rollback_if_open() {
            let code = match &e {
                Error::Database(db) => db.code,
                _ => 0,
            };
            log::warn!(activity = "storage.release", sqlite_code = code; "Rolling back a released connection failed");
        }
    }
}

impl Deref for PoolConnection {
    type Target = SqliteConnection;
    fn deref(&self) -> &SqliteConnection {
        &self.guard
    }
}

impl DerefMut for PoolConnection {
    fn deref_mut(&mut self) -> &mut SqliteConnection {
        &mut self.guard
    }
}

/// A transaction on the pool's connection (sqlx's `Transaction`):
/// `commit` ends it, and dropping it rolls it back at once.
pub struct Transaction {
    conn: Option<PoolConnection>,
}

impl Transaction {
    pub async fn commit(mut self) -> Result<(), Error> {
        if let Some(mut conn) = self.conn.take() {
            conn.exec("COMMIT")?;
        }
        Ok(())
    }

    pub async fn rollback(mut self) -> Result<(), Error> {
        if let Some(mut conn) = self.conn.take() {
            conn.exec("ROLLBACK")?;
        }
        Ok(())
    }
}

impl Deref for Transaction {
    type Target = SqliteConnection;
    fn deref(&self) -> &SqliteConnection {
        self.conn
            .as_deref()
            .expect("a Transaction has its connection until it's consumed")
    }
}

impl DerefMut for Transaction {
    fn deref_mut(&mut self) -> &mut SqliteConnection {
        self.conn
            .as_deref_mut()
            .expect("a Transaction has its connection until it's consumed")
    }
}

// ---------------------------------------------------------------------------
// Executors and queries
// ---------------------------------------------------------------------------

/// Where a query runs: the pool (it takes the connection for the call) or
/// a connection the caller holds. sqlx's `SqliteExecutor`.
#[allow(async_fn_in_trait)]
pub trait SqliteExecutor<'c> {
    #[doc(hidden)]
    async fn with<R>(
        self,
        f: impl FnOnce(&mut SqliteConnection) -> Result<R, Error>,
    ) -> Result<R, Error>;
}

impl<'c> SqliteExecutor<'c> for &'c SqlitePool {
    async fn with<R>(
        self,
        f: impl FnOnce(&mut SqliteConnection) -> Result<R, Error>,
    ) -> Result<R, Error> {
        let mut conn = self.acquire().await?;
        f(&mut conn)
    }
}

impl<'c> SqliteExecutor<'c> for &'c mut SqliteConnection {
    async fn with<R>(
        self,
        f: impl FnOnce(&mut SqliteConnection) -> Result<R, Error>,
    ) -> Result<R, Error> {
        f(self)
    }
}

/// A statement and its values (sqlx's `Query`).
#[must_use = "a query does nothing until it is executed"]
pub struct Query<'q> {
    sql: &'q str,
    args: Vec<Arg>,
}

/// The facade's name for a query being built.
pub type SqliteQuery<'q> = Query<'q>;

/// sqlx's `query`.
pub fn query(sql: &str) -> Query<'_> {
    Query {
        sql,
        args: Vec::new(),
    }
}

impl<'q> Query<'q> {
    pub fn bind<T: Encode>(mut self, value: T) -> Self {
        self.args.push(value.encode());
        self
    }

    async fn run<'c, E: SqliteExecutor<'c>>(
        self,
        e: E,
        keep: Keep,
    ) -> Result<(SqliteQueryResult, Vec<SqliteRow>), Error> {
        e.with(|conn| {
            let mut rows = Vec::new();
            let done = conn.run(self.sql, &self.args, keep, &mut rows)?;
            Ok((done, rows))
        })
        .await
    }

    pub async fn execute<'c, E: SqliteExecutor<'c>>(
        self,
        e: E,
    ) -> Result<SqliteQueryResult, Error> {
        Ok(self.run(e, Keep::None).await?.0)
    }

    pub async fn fetch_all<'c, E: SqliteExecutor<'c>>(self, e: E) -> Result<Vec<SqliteRow>, Error> {
        Ok(self.run(e, Keep::All).await?.1)
    }

    pub async fn fetch_optional<'c, E: SqliteExecutor<'c>>(
        self,
        e: E,
    ) -> Result<Option<SqliteRow>, Error> {
        Ok(self.run(e, Keep::First).await?.1.into_iter().next())
    }

    pub async fn fetch_one<'c, E: SqliteExecutor<'c>>(self, e: E) -> Result<SqliteRow, Error> {
        self.fetch_optional(e).await?.ok_or(Error::RowNotFound)
    }
}

/// A query whose rows are read as `O` (sqlx's `QueryAs`).
#[must_use = "a query does nothing until it is executed"]
pub struct QueryAs<'q, O> {
    inner: Query<'q>,
    out: PhantomData<fn() -> O>,
}

/// sqlx's `query_as`.
pub fn query_as<O: FromRow>(sql: &str) -> QueryAs<'_, O> {
    QueryAs {
        inner: query(sql),
        out: PhantomData,
    }
}

impl<'q, O: FromRow> QueryAs<'q, O> {
    pub fn bind<T: Encode>(mut self, value: T) -> Self {
        self.inner = self.inner.bind(value);
        self
    }

    pub async fn fetch_all<'c, E: SqliteExecutor<'c>>(self, e: E) -> Result<Vec<O>, Error> {
        self.inner
            .fetch_all(e)
            .await?
            .iter()
            .map(O::from_row)
            .collect()
    }

    pub async fn fetch_optional<'c, E: SqliteExecutor<'c>>(self, e: E) -> Result<Option<O>, Error> {
        self.inner
            .fetch_optional(e)
            .await?
            .as_ref()
            .map(O::from_row)
            .transpose()
    }

    pub async fn fetch_one<'c, E: SqliteExecutor<'c>>(self, e: E) -> Result<O, Error> {
        O::from_row(&self.inner.fetch_one(e).await?)
    }
}

/// A query whose rows are read as their first column (sqlx's
/// `QueryScalar`).
#[must_use = "a query does nothing until it is executed"]
pub struct QueryScalar<'q, O> {
    inner: Query<'q>,
    out: PhantomData<fn() -> O>,
}

/// sqlx's `query_scalar`.
pub fn query_scalar<O: Decode + Type>(sql: &str) -> QueryScalar<'_, O> {
    QueryScalar {
        inner: query(sql),
        out: PhantomData,
    }
}

impl<'q, O: Decode + Type> QueryScalar<'q, O> {
    pub fn bind<T: Encode>(mut self, value: T) -> Self {
        self.inner = self.inner.bind(value);
        self
    }

    pub async fn fetch_all<'c, E: SqliteExecutor<'c>>(self, e: E) -> Result<Vec<O>, Error> {
        self.inner
            .fetch_all(e)
            .await?
            .iter()
            .map(|r| r.try_get(0))
            .collect()
    }

    pub async fn fetch_optional<'c, E: SqliteExecutor<'c>>(self, e: E) -> Result<Option<O>, Error> {
        self.inner
            .fetch_optional(e)
            .await?
            .as_ref()
            .map(|r| r.try_get(0))
            .transpose()
    }

    pub async fn fetch_one<'c, E: SqliteExecutor<'c>>(self, e: E) -> Result<O, Error> {
        self.inner.fetch_one(e).await?.try_get(0)
    }
}
