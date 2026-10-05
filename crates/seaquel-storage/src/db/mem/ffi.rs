//! The thin safe wrapper over SQLite's C API that the in-memory executor
//! runs on. This file holds every `unsafe` block in
//! the crate. Everything above it handles owned Rust values only.
//!
//! On wasm32 the C API is `sqlite-wasm-rs`'s (SQLite compiled to wasm,
//! single-threaded); natively, for this module's own tests, it is
//! `libsqlite3-sys`, the library sqlx links. Both are bindgen output of
//! the same `sqlite3.h`, so one wrapper serves both.
//!
//! Rules every function here keeps:
//! - every pointer SQLite hands back is checked for NULL before use;
//! - a prepared statement is finalized and a duplicated value freed on
//!   every path (their `Drop`), and memory from `sqlite3_serialize` goes
//!   back through `sqlite3_free`;
//! - text and blobs are bound with `SQLITE_TRANSIENT`, so SQLite copies
//!   them before the call returns and no Rust buffer has to outlive it;
//! - nothing panics on what SQLite or the caller hands in: sizes past the
//!   C API's `int` are errors, and text that isn't UTF-8 is returned as
//!   bytes for the caller to judge.

use std::ffi::{c_char, c_int, c_void, CStr};
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

#[cfg(not(target_arch = "wasm32"))]
use libsqlite3_sys as ffi;
#[cfg(target_arch = "wasm32")]
use sqlite_wasm_rs as ffi;

// The wrapper's `Send`/`Sync` below rest on there being one thread. A
// wasm32 build with shared memory and threads would break that.
#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
compile_error!("the in-memory storage executor assumes a single-threaded wasm32 build");

/// An error SQLite reported: its extended result code and message, as
/// sqlx's `SqliteError` keeps them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawError {
    pub code: c_int,
    pub message: String,
}

impl RawError {
    /// An error with SQLite's own text for `code` (`sqlite3_errstr`), for a
    /// failure that has no connection to ask.
    fn from_code(code: c_int) -> Self {
        // SAFETY: `sqlite3_errstr` takes any code and returns a pointer to
        // a static NUL-terminated string, or NULL.
        let text = unsafe { ffi::sqlite3_errstr(code) };
        Self {
            code,
            message: if text.is_null() {
                "<error message unavailable>".to_string()
            } else {
                // SAFETY: non-NULL, static and NUL-terminated (above).
                unsafe { CStr::from_ptr(text) }
                    .to_string_lossy()
                    .into_owned()
            },
        }
    }

    /// An error of the wrapper's own (an argument SQLite's API can't take),
    /// with SQLite's generic code, as sqlx's `SqliteError::generic`.
    pub(crate) fn generic(message: impl Into<String>) -> Self {
        Self {
            code: ffi::SQLITE_ERROR,
            message: message.into(),
        }
    }
}

/// What the commit hook shares with the connection: set by the hook when
/// SQLite is about to commit, and the count of commits that then succeeded.
#[derive(Default)]
pub(crate) struct Commits {
    hooked: AtomicBool,
    count: AtomicU64,
}

impl Commits {
    pub(crate) fn get(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }
}

/// SQLite calls this right before a transaction commits (an explicit
/// `COMMIT`, or a write in autocommit). It only notes that a commit is
/// under way; [`Db::settle`] counts it once the statement has succeeded,
/// so a commit that then fails isn't counted.
unsafe extern "C" fn on_commit(arg: *mut c_void) -> c_int {
    // SAFETY: `arg` is `Arc::as_ptr` of the `Commits` the `Db` holds, and
    // the `Db` keeps that `Arc` alive until after `sqlite3_close` (its
    // `Drop` closes the handle before the fields drop), so the hook never
    // sees a dangling pointer.
    let commits = unsafe { &*(arg as *const Commits) };
    commits.hooked.store(true, Ordering::Relaxed);
    0
}

/// One SQLite connection.
pub(crate) struct Db {
    handle: NonNull<ffi::sqlite3>,
    commits: Arc<Commits>,
}

// SAFETY: moving a connection to another thread is sound when nothing else
// touches it meanwhile, which ownership guarantees (`Db` isn't `Sync`, so
// no `&Db` is shared across threads). On wasm32 there is one thread anyway:
// SQLite is built with `-DSQLITE_THREADSAFE=0` and the `compile_error!`
// above refuses a build with `atomics`. Natively (this module's tests) the
// bundled SQLite is serialized-threadsafe. `Send` lets the pool keep it in
// its async mutex, which is `Sync` only for `Send` contents.
unsafe impl Send for Db {}

impl Db {
    /// A connection to a fresh in-memory database.
    pub(crate) fn open_memory() -> Result<Self, RawError> {
        Self::open(
            c":memory:",
            ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_CREATE,
        )
    }

    /// A connection to `uri` (`file:` URIs allowed), for tests that need
    /// two connections to one `memdb` file.
    #[cfg(test)]
    pub(crate) fn open_uri(uri: &str) -> Result<Self, RawError> {
        let uri = std::ffi::CString::new(uri)
            .map_err(|_| RawError::generic("a database name can't hold a NUL byte"))?;
        Self::open(
            &uri,
            ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_CREATE | ffi::SQLITE_OPEN_URI,
        )
    }

    fn open(name: &CStr, flags: c_int) -> Result<Self, RawError> {
        let mut raw: *mut ffi::sqlite3 = ptr::null_mut();
        // SAFETY: `name` is NUL-terminated and outlives the call, `raw` is a
        // valid out-pointer, and a NULL VFS name selects the default VFS.
        let rc = unsafe { ffi::sqlite3_open_v2(name.as_ptr(), &mut raw, flags, ptr::null()) };
        let Some(handle) = NonNull::new(raw) else {
            // Only an allocation failure leaves no handle.
            return Err(RawError::from_code(if rc == ffi::SQLITE_OK {
                ffi::SQLITE_NOMEM
            } else {
                rc
            }));
        };
        let db = Self {
            handle,
            commits: Arc::default(),
        };
        if rc != ffi::SQLITE_OK {
            // `db`'s Drop closes the handle SQLite returned with the error.
            return Err(db.last_error());
        }
        // SAFETY: `handle` is an open connection. Extended result codes, as
        // sqlx turns on for its connections, so step errors carry them.
        unsafe { ffi::sqlite3_extended_result_codes(handle.as_ptr(), 1) };
        // SAFETY: `handle` is open; the hook's argument is the `Commits`
        // this `Db` keeps alive until after the handle is closed (Drop).
        unsafe {
            ffi::sqlite3_commit_hook(
                handle.as_ptr(),
                Some(on_commit),
                Arc::as_ptr(&db.commits) as *mut c_void,
            )
        };
        Ok(db)
    }

    pub(crate) fn commits(&self) -> Arc<Commits> {
        Arc::clone(&self.commits)
    }

    /// The connection's current error: extended code and message.
    pub(crate) fn last_error(&self) -> RawError {
        // SAFETY: `handle` is open (it lives as long as `self`).
        let code = unsafe { ffi::sqlite3_extended_errcode(self.handle.as_ptr()) };
        // SAFETY: as above; `sqlite3_errmsg` returns a NUL-terminated string
        // owned by the connection, valid until the next call on it, and we
        // copy it at once.
        let msg = unsafe { ffi::sqlite3_errmsg(self.handle.as_ptr()) };
        if msg.is_null() {
            return RawError::from_code(code);
        }
        RawError {
            code,
            // SAFETY: non-NULL and NUL-terminated (above).
            message: unsafe { CStr::from_ptr(msg) }
                .to_string_lossy()
                .into_owned(),
        }
    }

    /// The error for a call that returned `rc`: the connection's, unless
    /// it reports none (then SQLite's text for `rc`).
    fn error_for(&self, rc: c_int) -> RawError {
        let e = self.last_error();
        if e.code == ffi::SQLITE_OK {
            RawError::from_code(rc)
        } else {
            e
        }
    }

    /// Counts a commit the hook saw during the statement that just ended,
    /// if that statement succeeded; forgets it otherwise.
    fn settle(&self, ok: bool) {
        if self.commits.hooked.swap(false, Ordering::Relaxed) && ok {
            self.commits.count.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Whether no transaction is open (`sqlite3_get_autocommit`).
    pub(crate) fn autocommit(&self) -> bool {
        // SAFETY: `handle` is open.
        unsafe { ffi::sqlite3_get_autocommit(self.handle.as_ptr()) != 0 }
    }

    /// The rows the most recent INSERT, UPDATE or DELETE changed
    /// (`sqlite3_changes`), as sqlx reads it after each statement.
    pub(crate) fn changes(&self) -> u64 {
        // SAFETY: `handle` is open.
        let n = unsafe { ffi::sqlite3_changes(self.handle.as_ptr()) };
        u64::try_from(n).unwrap_or(0)
    }

    /// Compiles the first statement in `sql`. Returns it (`None` when that
    /// part held only whitespace or comments) and how many bytes of `sql`
    /// it used. A NUL byte ends the text, as SQLite reads it.
    pub(crate) fn prepare(&self, sql: &[u8]) -> Result<(Option<Stmt<'_>>, usize), RawError> {
        let len = c_int::try_from(sql.len()).map_err(|_| {
            RawError::generic(format!(
                "query string too large for SQLite3 API ({} bytes); try breaking it into \
                 smaller chunks (< 2 GiB), executed separately",
                sql.len()
            ))
        })?;
        let mut raw: *mut ffi::sqlite3_stmt = ptr::null_mut();
        let mut tail: *const c_char = ptr::null();
        // SAFETY: `sql` is valid for `len` bytes (SQLite reads at most that
        // many and doesn't need a NUL terminator when given the length), and
        // `raw` and `tail` are valid out-pointers.
        let rc = unsafe {
            ffi::sqlite3_prepare_v2(
                self.handle.as_ptr(),
                sql.as_ptr() as *const c_char,
                len,
                &mut raw,
                &mut tail,
            )
        };
        if rc != ffi::SQLITE_OK {
            // On failure SQLite sets `raw` to NULL; nothing to finalize.
            return Err(self.error_for(rc));
        }
        // `tail` points into `sql`, just past the statement compiled. Any
        // pointer outside it (it never is) counts as the whole text.
        let start = sql.as_ptr() as usize;
        let used = (tail as usize)
            .checked_sub(start)
            .filter(|n| *n <= sql.len())
            .unwrap_or(sql.len());
        let stmt = NonNull::new(raw).map(|raw| Stmt { raw, db: self });
        Ok((stmt, used))
    }

    /// The `main` database as one file image (`sqlite3_serialize`).
    pub(crate) fn serialize(&self) -> Result<Vec<u8>, RawError> {
        let mut size: ffi::sqlite3_int64 = 0;
        // SAFETY: `handle` is open, the schema name is NUL-terminated, and
        // `size` is a valid out-pointer. Flags 0: SQLite allocates a copy.
        let data =
            unsafe { ffi::sqlite3_serialize(self.handle.as_ptr(), c"main".as_ptr(), &mut size, 0) };
        if data.is_null() {
            // An empty database may serialize as nothing.
            return if size <= 0 {
                Ok(Vec::new())
            } else {
                Err(RawError::from_code(ffi::SQLITE_NOMEM))
            };
        }
        let bytes = usize::try_from(size).map(|n| {
            // SAFETY: SQLite returned `size` bytes at `data`, owned by us
            // until `sqlite3_free`; they are copied before it.
            unsafe { std::slice::from_raw_parts(data, n) }.to_vec()
        });
        // SAFETY: `data` came from `sqlite3_serialize` without
        // `SQLITE_SERIALIZE_NOCOPY`, so it is ours to free, exactly once.
        unsafe { ffi::sqlite3_free(data as *mut c_void) };
        bytes.map_err(|_| RawError::from_code(ffi::SQLITE_NOMEM))
    }

    /// Replaces the `main` database with `image` (`sqlite3_deserialize`),
    /// resizable, in memory SQLite owns. An empty image does nothing.
    pub(crate) fn deserialize(&self, image: &[u8]) -> Result<(), RawError> {
        if image.is_empty() {
            return Ok(());
        }
        let size = ffi::sqlite3_int64::try_from(image.len())
            .map_err(|_| RawError::from_code(ffi::SQLITE_TOOBIG))?;
        let size_u = u64::try_from(size).map_err(|_| RawError::from_code(ffi::SQLITE_TOOBIG))?;
        // SAFETY: `sqlite3_malloc64` takes any size and returns NULL or a
        // buffer of at least that many bytes.
        let buf = unsafe { ffi::sqlite3_malloc64(size_u) } as *mut u8;
        if buf.is_null() {
            return Err(RawError::from_code(ffi::SQLITE_NOMEM));
        }
        // SAFETY: `buf` holds `image.len()` bytes (above) and doesn't
        // overlap `image`, which is valid for that many.
        unsafe { ptr::copy_nonoverlapping(image.as_ptr(), buf, image.len()) };
        // SAFETY: `handle` is open and `buf` came from `sqlite3_malloc64`,
        // as `SQLITE_DESERIALIZE_FREEONCLOSE` requires: SQLite owns it from
        // here and frees it, on failure too.
        let rc = unsafe {
            ffi::sqlite3_deserialize(
                self.handle.as_ptr(),
                c"main".as_ptr(),
                buf,
                size,
                size,
                ffi::SQLITE_DESERIALIZE_FREEONCLOSE | ffi::SQLITE_DESERIALIZE_RESIZEABLE,
            )
        };
        if rc != ffi::SQLITE_OK {
            return Err(self.error_for(rc));
        }
        Ok(())
    }
}

impl Drop for Db {
    fn drop(&mut self) {
        // SAFETY: `handle` is open and closed only here. Every statement
        // borrows the `Db`, so all are finalized by now and the close can't
        // be refused as busy. The commit hook's `Arc` is dropped after this,
        // with the struct's fields.
        unsafe { ffi::sqlite3_close(self.handle.as_ptr()) };
    }
}

/// A prepared statement, finalized on drop.
pub(crate) struct Stmt<'db> {
    raw: NonNull<ffi::sqlite3_stmt>,
    db: &'db Db,
}

/// What [`Stmt::step`] found.
pub(crate) enum Step {
    Row,
    Done,
}

impl Stmt<'_> {
    pub(crate) fn bind_count(&self) -> usize {
        // SAFETY: `raw` is a live statement (finalized only in Drop).
        let n = unsafe { ffi::sqlite3_bind_parameter_count(self.raw.as_ptr()) };
        usize::try_from(n).unwrap_or(0)
    }

    /// The name of parameter `i` (1-based) as written (`?3`, `$1`, `:x`),
    /// or `None` for a plain `?`.
    pub(crate) fn bind_name(&self, i: usize) -> Option<String> {
        let i = c_int::try_from(i).ok()?;
        // SAFETY: `raw` is live; an out-of-range index returns NULL. The
        // name is owned by the statement and copied at once.
        let name = unsafe { ffi::sqlite3_bind_parameter_name(self.raw.as_ptr(), i) };
        if name.is_null() {
            return None;
        }
        // SAFETY: non-NULL and NUL-terminated.
        Some(
            unsafe { CStr::from_ptr(name) }
                .to_string_lossy()
                .into_owned(),
        )
    }

    fn check(&self, rc: c_int) -> Result<(), RawError> {
        if rc == ffi::SQLITE_OK {
            Ok(())
        } else {
            Err(self.db.error_for(rc))
        }
    }

    fn index(i: usize) -> Result<c_int, RawError> {
        c_int::try_from(i).map_err(|_| RawError::from_code(ffi::SQLITE_RANGE))
    }

    pub(crate) fn bind_null(&mut self, i: usize) -> Result<(), RawError> {
        let i = Self::index(i)?;
        // SAFETY: `raw` is live; SQLite checks the index.
        self.check(unsafe { ffi::sqlite3_bind_null(self.raw.as_ptr(), i) })
    }

    pub(crate) fn bind_int64(&mut self, i: usize, v: i64) -> Result<(), RawError> {
        let i = Self::index(i)?;
        // SAFETY: as `bind_null`.
        self.check(unsafe { ffi::sqlite3_bind_int64(self.raw.as_ptr(), i, v) })
    }

    pub(crate) fn bind_double(&mut self, i: usize, v: f64) -> Result<(), RawError> {
        let i = Self::index(i)?;
        // SAFETY: as `bind_null`.
        self.check(unsafe { ffi::sqlite3_bind_double(self.raw.as_ptr(), i, v) })
    }

    pub(crate) fn bind_text(&mut self, i: usize, v: &str) -> Result<(), RawError> {
        let i = Self::index(i)?;
        // SAFETY: `v` is valid for `v.len()` bytes for the call, and
        // `SQLITE_TRANSIENT` makes SQLite copy them before it returns. An
        // empty `&str`'s pointer is dangling but non-NULL, so SQLite binds
        // empty text rather than NULL.
        self.check(unsafe {
            ffi::sqlite3_bind_text64(
                self.raw.as_ptr(),
                i,
                v.as_ptr() as *const c_char,
                v.len() as u64,
                ffi::SQLITE_TRANSIENT(),
                ffi::SQLITE_UTF8 as u8,
            )
        })
    }

    pub(crate) fn bind_blob(&mut self, i: usize, v: &[u8]) -> Result<(), RawError> {
        let i = Self::index(i)?;
        // SAFETY: as `bind_text`: copied before return, and an empty
        // slice's non-NULL pointer binds an empty blob, not NULL.
        self.check(unsafe {
            ffi::sqlite3_bind_blob64(
                self.raw.as_ptr(),
                i,
                v.as_ptr() as *const c_void,
                v.len() as u64,
                ffi::SQLITE_TRANSIENT(),
            )
        })
    }

    /// Runs the statement to its next row or its end.
    pub(crate) fn step(&mut self) -> Result<Step, RawError> {
        // SAFETY: `raw` is live.
        let rc = unsafe { ffi::sqlite3_step(self.raw.as_ptr()) };
        let out = match rc {
            ffi::SQLITE_ROW => Ok(Step::Row),
            ffi::SQLITE_DONE => Ok(Step::Done),
            rc => Err(self.db.error_for(rc)),
        };
        // A commit happens only as a statement ends.
        if !matches!(out, Ok(Step::Row)) {
            self.db.settle(out.is_ok());
        }
        out
    }

    pub(crate) fn column_count(&self) -> usize {
        // SAFETY: `raw` is live.
        let n = unsafe { ffi::sqlite3_column_count(self.raw.as_ptr()) };
        usize::try_from(n).unwrap_or(0)
    }

    pub(crate) fn column_name(&self, i: usize) -> String {
        let Ok(i) = c_int::try_from(i) else {
            return String::new();
        };
        // SAFETY: `raw` is live; an index out of range or an allocation
        // failure returns NULL. The name is copied at once.
        let name = unsafe { ffi::sqlite3_column_name(self.raw.as_ptr(), i) };
        if name.is_null() {
            return String::new();
        }
        // SAFETY: non-NULL and NUL-terminated.
        unsafe { CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned()
    }

    /// A copy of column `i` of the current row, which outlives the step.
    pub(crate) fn column_value(&self, i: usize) -> Result<Value, RawError> {
        let i = Self::index(i)?;
        // SAFETY: `raw` is live and on a row (callers only read after
        // `Step::Row`); the value it returns is valid until the next step,
        // and `sqlite3_value_dup` copies it before then. A NULL from either
        // is an allocation failure.
        let dup = unsafe {
            let v = ffi::sqlite3_column_value(self.raw.as_ptr(), i);
            if v.is_null() {
                ptr::null_mut()
            } else {
                ffi::sqlite3_value_dup(v)
            }
        };
        NonNull::new(dup)
            .map(|raw| Value { raw })
            .ok_or_else(|| RawError::from_code(ffi::SQLITE_NOMEM))
    }
}

impl Drop for Stmt<'_> {
    fn drop(&mut self) {
        // SAFETY: `raw` is live and finalized only here. Its return code
        // repeats the last step's error, which the caller already has, or
        // reports the reset of a statement stopped at a row.
        let rc = unsafe { ffi::sqlite3_finalize(self.raw.as_ptr()) };
        // A write stopped at a row (`fetch_optional` on `… RETURNING`)
        // commits as it is reset here, in autocommit.
        self.db.settle(rc == ffi::SQLITE_OK);
    }
}

/// A protected copy of one cell (`sqlite3_value_dup`), freed on drop. It
/// reads with SQLite's own conversions, as sqlx's row values do.
pub struct Value {
    raw: NonNull<ffi::sqlite3_value>,
}

/// SQLite's storage classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Integer,
    Float,
    Text,
    Blob,
    Null,
}

impl Value {
    /// The value's current storage class (`sqlite3_value_type`).
    pub fn kind(&self) -> Kind {
        // SAFETY: `raw` is a live duplicated value.
        match unsafe { ffi::sqlite3_value_type(self.raw.as_ptr()) } {
            ffi::SQLITE_INTEGER => Kind::Integer,
            ffi::SQLITE_FLOAT => Kind::Float,
            ffi::SQLITE_TEXT => Kind::Text,
            ffi::SQLITE_BLOB => Kind::Blob,
            _ => Kind::Null,
        }
    }

    pub fn int64(&self) -> i64 {
        // SAFETY: `raw` is live; SQLite converts as needed.
        unsafe { ffi::sqlite3_value_int64(self.raw.as_ptr()) }
    }

    pub fn double(&self) -> f64 {
        // SAFETY: as `int64`.
        unsafe { ffi::sqlite3_value_double(self.raw.as_ptr()) }
    }

    /// The value as bytes: a blob's, or the text SQLite gives a number or
    /// text, without its terminator. Empty for NULL and empty values.
    pub fn bytes(&self) -> Vec<u8> {
        // SAFETY: `raw` is live. `sqlite3_value_blob` first (it may convert
        // the value), then `sqlite3_value_bytes` for that form's length; the
        // pointer stays valid until the value changes, and we copy at once.
        unsafe {
            let data = ffi::sqlite3_value_blob(self.raw.as_ptr());
            let len = ffi::sqlite3_value_bytes(self.raw.as_ptr());
            match usize::try_from(len) {
                Ok(len) if len > 0 && !data.is_null() => {
                    std::slice::from_raw_parts(data as *const u8, len).to_vec()
                }
                _ => Vec::new(),
            }
        }
    }
}

impl Drop for Value {
    fn drop(&mut self) {
        // SAFETY: `raw` came from `sqlite3_value_dup` and is freed only here.
        unsafe { ffi::sqlite3_value_free(self.raw.as_ptr()) };
    }
}
