//! One write transaction per Core call (phase 5d Decision 3), the one
//! connection every write goes through (phase 7a Decision 5), and the
//! reader that lets a query run either inside a write or on the pool.

use std::ops::{Deref, DerefMut};

#[cfg(target_arch = "wasm32")]
use crate::db::PoolConnection;
use crate::db::{self, SqliteConnection, Transaction};
#[cfg(not(target_arch = "wasm32"))]
use crate::db::{ConnectOptions, Connection, SqliteConnectOptions};
use crate::lock::OwnedMutexGuard;

use crate::{Storage, StorageError};

/// The connection a write runs on: natively the storage's one writer
/// connection ([`Writer`]), on wasm32 the in-memory pool's only connection.
#[cfg(not(target_arch = "wasm32"))]
type TxConn = SqliteConnection;
#[cfg(target_arch = "wasm32")]
type TxConn = PoolConnection;

fn as_conn(c: &mut TxConn) -> &mut SqliteConnection {
    c
}

fn as_conn_ref(c: &TxConn) -> &SqliteConnection {
    c
}

/// What the write mutex guards (phase 7a Decision 5).
///
/// Natively it owns the storage's **writer connection**: one SQLite
/// connection outside the pool that every [`WriteTx`], `VACUUM` and
/// checkpoint of this storage runs on, so nothing this storage commits ever
/// shows up as another connection's change. That's what makes
/// [`Storage::external_version`] exact: `PRAGMA data_version` on the writer
/// connection moves only when some *other* connection (another `Storage`,
/// another process) committed.
///
/// The connection opens on first use with the pool's connect options. One
/// left in an unknown state (a `ROLLBACK` that failed) is closed, and the
/// next write opens a fresh one. With an idle timeout (the web) it closes
/// once it has gone unused that long (a reaper task on the runtime), so an
/// idle user holds no file handle, as the pool's idle timeout does for
/// reads, while a burst of writes costs one open.
///
/// On wasm32 it guards nothing: the in-memory pool has one connection, and
/// writes take it from the pool as before.
#[derive(Default)]
pub(crate) struct Writer {
    #[cfg(not(target_arch = "wasm32"))]
    conn: Option<SqliteConnection>,
    /// How to open the connection; `None` only for [`Writer::default`].
    #[cfg(not(target_arch = "wasm32"))]
    connect: Option<SqliteConnectOptions>,
    /// Close the connection once it has gone unused this long; `None`
    /// keeps it open.
    #[cfg(not(target_arch = "wasm32"))]
    idle: Option<std::time::Duration>,
    /// When the connection was last handed back.
    #[cfg(not(target_arch = "wasm32"))]
    last_use: Option<tokio::time::Instant>,
    /// A reaper task is waiting to close the idle connection.
    #[cfg(not(target_arch = "wasm32"))]
    reaper: bool,
    /// The `data_version` the last poll read on the open connection.
    #[cfg(not(target_arch = "wasm32"))]
    seen: Option<i64>,
    /// A polled connection was closed: the next poll's fresh connection
    /// can't be compared with it, so it counts as a change.
    #[cfg(not(target_arch = "wasm32"))]
    lost: bool,
    /// What [`Storage::external_version`] answers.
    #[cfg(not(target_arch = "wasm32"))]
    version: i64,
}

impl std::fmt::Debug for Writer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Writer")
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Writer {
    /// A writer that opens its connection with `connect` and, with
    /// `idle`, closes it once it has gone unused that long.
    pub(crate) fn new(connect: SqliteConnectOptions, idle: Option<std::time::Duration>) -> Self {
        Self {
            connect: Some(connect),
            idle,
            ..Self::default()
        }
    }

    /// The open connection, or a new one.
    async fn take(&mut self, path: &std::path::Path) -> Result<SqliteConnection, StorageError> {
        if let Some(conn) = self.conn.take() {
            return Ok(conn);
        }
        let Some(connect) = &self.connect else {
            return Err(StorageError::Sqlx(db::Error::PoolClosed));
        };
        connect
            .connect()
            .await
            .map_err(|e| crate::open::classify(path, e, false))
    }

    /// Closes `conn`, which is gone from the slot.
    async fn discard(&mut self, conn: SqliteConnection) {
        self.lose();
        // A failed close changes nothing: the connection is gone either way,
        // and SQLite rolls back what it left open.
        let _ = conn.close().await;
    }

    /// The open connection is gone (closed, or about to be).
    fn lose(&mut self) {
        if self.seen.take().is_some() {
            self.lost = true;
        }
    }

    /// Counts a change when `data_version` differs from the last poll's.
    fn observe(&mut self, data_version: i64) {
        match self.seen {
            Some(seen) if seen != data_version => self.version += 1,
            None if self.lost => self.version += 1,
            _ => {}
        }
        self.lost = false;
        self.seen = Some(data_version);
    }

    /// Closes the connection (`Storage::close`), for good.
    pub(crate) async fn close(&mut self) {
        self.connect = None;
        if let Some(conn) = self.conn.take() {
            self.lose();
            let _ = conn.close().await;
        }
    }
}

/// Hands `conn` back to the writer after a write. A healthy one stays open
/// (until it has been idle for the writer's idle timeout); any other is
/// closed.
#[cfg(not(target_arch = "wasm32"))]
async fn put_back(writer: &mut OwnedMutexGuard<Writer>, conn: SqliteConnection, healthy: bool) {
    if !healthy {
        writer.discard(conn).await;
        return;
    }
    writer.conn = Some(conn);
    let Some(idle) = writer.idle else { return };
    writer.last_use = Some(tokio::time::Instant::now());
    if writer.reaper {
        return;
    }
    // No runtime (a write finishing at process exit): it stays open.
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    writer.reaper = true;
    let lock = std::sync::Arc::downgrade(OwnedMutexGuard::mutex(writer));
    rt.spawn(reap(lock, idle));
}

/// Closes the writer's connection once it has gone unused for `idle`,
/// waiting again while writes keep it in use. It holds only a weak
/// reference, so a dropped storage ends it, and it ends when the connection
/// is gone (closed or lost); the next write's `put_back` starts another.
#[cfg(not(target_arch = "wasm32"))]
async fn reap(lock: std::sync::Weak<crate::lock::Mutex<Writer>>, idle: std::time::Duration) {
    let mut wait = idle;
    loop {
        tokio::time::sleep(wait).await;
        let Some(lock) = lock.upgrade() else { return };
        let mut writer = lock.lock().await;
        let Some(last_use) = writer.last_use.filter(|_| writer.conn.is_some()) else {
            writer.reaper = false;
            return;
        };
        let unused = last_use.elapsed();
        if unused >= idle {
            writer.reaper = false;
            if let Some(conn) = writer.conn.take() {
                writer.discard(conn).await;
            }
            return;
        }
        wait = idle - unused;
    }
}

/// A write transaction on the metadata file.
///
/// - **In this process,** writers queue on the storage's async write mutex
///   (shared by every clone of the [`Storage`]), which owns the one writer
///   connection (natively; see `Writer`). So a writer waiting its turn holds
///   no connection, and no write ever takes a pool connection: readers
///   never wait for writers in this process, and on the web's two
///   connections per user one reads while the other writes.
/// - **Across processes,** it begins with `BEGIN IMMEDIATE`: SQLite's write
///   lock is taken up front, so a writer in another pool or process waits
///   up to the busy timeout instead of failing when it upgrades a read lock.
///
/// The targeted write functions in the query modules take `&mut WriteTx`,
/// so a call reads what it needs, checks it, writes and commits in one
/// place. Dropping it without [`WriteTx::commit`] rolls everything back.
///
/// **Inside a write, read only through `&mut tx`.** A read through the
/// storage (`&storage`) while a `WriteTx` is open runs on a pool
/// connection: it doesn't see the transaction's writes (and on wasm32,
/// whose pool is the writer's one connection, it waits for the
/// transaction, which never ends while the caller awaits the read).
pub struct WriteTx {
    /// `None` once committed or handed to the rollback in `Drop`.
    conn: Option<TxConn>,
    /// Held until the transaction has ended (a rollback on drop keeps it
    /// until the `ROLLBACK` ran), so the next writer never begins on top.
    turn: Option<OwnedMutexGuard<Writer>>,
    /// [`WriteTx::fail_next_rollback_for_tests`].
    fail_rollback: bool,
    /// [`WriteTx::secure_delete`] turned it on: it's turned off again before
    /// the connection is handed back, whether the write committed or not.
    secure_delete_on: bool,
}

impl std::fmt::Debug for WriteTx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WriteTx")
    }
}

/// Runs `ROLLBACK`, ignoring SQLite's "no transaction is active": after
/// some errors (`SQLITE_FULL`, `SQLITE_IOERR`, …) SQLite has already rolled
/// the whole transaction back itself. `BEGIN`, `COMMIT` and `ROLLBACK` are
/// sent as plain statements, not through sqlx's `Transaction`, because sqlx
/// counts transaction depth and never lowers it when that `ROLLBACK` fails,
/// which would leave the connection unusable for every later transaction
/// (phase 5d-2 review, the size cap).
///
/// `fail` sends a statement that fails instead (tests only), so the path a
/// failed `ROLLBACK` takes can be tested.
async fn rollback_on(conn: &mut SqliteConnection, fail: bool) -> Result<(), StorageError> {
    let sql = if fail {
        "ROLLBACK TO seaquel_no_such_savepoint"
    } else {
        "ROLLBACK"
    };
    match db::query(sql).execute(conn).await {
        Ok(_) => Ok(()),
        Err(db::Error::Database(e)) if e.message().contains("no transaction is active") => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Turns `secure_delete` off again when `on`; `false` when it wouldn't.
async fn secure_delete_off(conn: &mut SqliteConnection, on: bool) -> bool {
    !on || db::query("PRAGMA secure_delete = OFF")
        .execute(conn)
        .await
        .is_ok()
}

impl WriteTx {
    /// Makes every write in it durable. If the commit fails, the
    /// transaction is rolled back.
    pub async fn commit(mut self) -> Result<(), StorageError> {
        let Some(conn) = self.conn.as_mut() else {
            return Ok(());
        };
        db::query("COMMIT").execute(as_conn(conn)).await?;
        if let Some(conn) = self.conn.take() {
            self.release(conn, true).await;
        }
        Ok(())
    }

    /// Undoes every write in it. Dropping the transaction does the same.
    /// When the `ROLLBACK` itself fails, the connection is closed (SQLite
    /// then discards the transaction) and the next write gets a fresh one.
    pub async fn rollback(mut self) -> Result<(), StorageError> {
        let Some(mut conn) = self.conn.take() else {
            return Ok(());
        };
        let done = rollback_on(as_conn(&mut conn), self.fail_rollback).await;
        self.release(conn, done.is_ok()).await;
        done
    }

    /// Hands the connection back once the transaction has ended, with
    /// `secure_delete` off again. One in an unknown state (`healthy: false`,
    /// or `secure_delete` that won't turn off) is closed instead.
    #[cfg(not(target_arch = "wasm32"))]
    async fn release(&mut self, mut conn: TxConn, healthy: bool) {
        let healthy = healthy && secure_delete_off(as_conn(&mut conn), self.secure_delete_on).await;
        match self.turn.as_mut() {
            Some(turn) => put_back(turn, conn, healthy).await,
            None => drop(conn),
        }
    }

    #[cfg(target_arch = "wasm32")]
    async fn release(&mut self, mut conn: TxConn, healthy: bool) {
        let healthy = healthy && secure_delete_off(as_conn(&mut conn), self.secure_delete_on).await;
        if !healthy {
            // Leave no connection in an unknown state in the pool.
            conn.close_on_drop();
        }
        drop(conn);
    }

    /// Turns `PRAGMA secure_delete` on or off for this transaction's
    /// connection. While it's on, the space deletes and overwrites free is
    /// zeroed, so text they remove doesn't linger in the pages written: the
    /// upgrade that strips secrets from stored connection strings (phase
    /// 5d, Decision 12a) turns it on around its row update and off again
    /// before the commit, so the writer connection goes on as it came.
    pub async fn secure_delete(&mut self, on: bool) -> Result<(), StorageError> {
        let sql = if on {
            "PRAGMA secure_delete = ON"
        } else {
            "PRAGMA secure_delete = OFF"
        };
        db::query(sql).execute(self.conn()).await?;
        self.secure_delete_on = on;
        Ok(())
    }

    /// Makes this transaction's next `ROLLBACK` (an explicit
    /// [`WriteTx::rollback`] or the one on drop) fail, for the tests of
    /// what a failed `ROLLBACK` does to the writer connection.
    #[doc(hidden)]
    pub fn fail_next_rollback_for_tests(&mut self) {
        self.fail_rollback = true;
    }

    pub(crate) fn conn(&mut self) -> &mut SqliteConnection {
        as_conn(
            self.conn
                .as_mut()
                .expect("a WriteTx has its connection until it's consumed"),
        )
    }
}

impl Drop for WriteTx {
    /// Rolls back a transaction that wasn't committed (or a `BEGIN` still
    /// in flight). The `ROLLBACK` runs on a task that keeps the connection
    /// and this process's write turn until it has, so the next writer
    /// begins after it; sqlx runs it after anything already queued on the
    /// connection. A failed `ROLLBACK` closes the connection.
    ///
    /// Without a Tokio runtime (a `WriteTx` outliving it at process exit)
    /// nothing can run the `ROLLBACK`, and sqlx can't close the connection
    /// either (that needs a runtime). The connection is leaked with the
    /// process; SQLite discards the uncommitted transaction when the file
    /// is next opened.
    #[cfg(not(target_arch = "wasm32"))]
    fn drop(&mut self) {
        let Some(mut conn) = self.conn.take() else {
            return;
        };
        let mut turn = self.turn.take();
        let fail = self.fail_rollback;
        let secure_delete_on = self.secure_delete_on;
        match tokio::runtime::Handle::try_current() {
            // The native executor only: on wasm32 the `ROLLBACK` is
            // synchronous (below).
            Ok(rt) => {
                rt.spawn(async move {
                    let healthy = rollback_on(&mut conn, fail).await.is_ok()
                        && secure_delete_off(&mut conn, secure_delete_on).await;
                    match turn.as_mut() {
                        Some(writer) => put_back(writer, conn, healthy).await,
                        None => drop(conn),
                    }
                    drop(turn);
                });
            }
            Err(_) => {
                std::mem::forget(conn);
                if let Some(writer) = turn.as_mut() {
                    writer.lose();
                }
                drop(turn);
            }
        }
    }

    /// Rolls back a transaction that wasn't committed (phase 8 Decision
    /// 5). The in-memory executor is synchronous, so the `ROLLBACK` runs
    /// here, before the connection and the write turn are released; "no
    /// transaction is active" (SQLite already rolled back) is fine, and any
    /// other failure is logged by code.
    #[cfg(target_arch = "wasm32")]
    fn drop(&mut self) {
        let Some(mut conn) = self.conn.take() else {
            return;
        };
        if let Err(e) = conn.rollback_now() {
            let code = StorageError::from(e).code();
            log::warn!(activity = "storage.rollback", code = code; "Rolling back a dropped write failed");
        }
        drop(conn);
        drop(self.turn.take());
    }
}

impl Deref for WriteTx {
    type Target = SqliteConnection;
    fn deref(&self) -> &SqliteConnection {
        as_conn_ref(
            self.conn
                .as_ref()
                .expect("a WriteTx has its connection until it's consumed"),
        )
    }
}

impl DerefMut for WriteTx {
    fn deref_mut(&mut self) -> &mut SqliteConnection {
        self.conn()
    }
}

impl Storage {
    /// Begins a write transaction ([`WriteTx`]): waits for this process's
    /// earlier writers, then `BEGIN IMMEDIATE` on the writer connection.
    ///
    /// On storage opened with `StorageOptions::read_only` it fails at once
    /// with [`StorageError::ReadOnly`] (`STORAGE_READ_ONLY`). When the
    /// earlier writers don't finish within [`crate::WRITE_WAIT`] (30 s) it
    /// fails with [`StorageError::WriteLockTimeout`] (`STORAGE_ERROR`), so a
    /// write begun while the caller holds a `WriteTx` fails instead of
    /// hanging. After [`Storage::close`] it fails as a closed pool does.
    pub async fn write(&self) -> Result<WriteTx, StorageError> {
        if self.is_read_only() {
            return Err(StorageError::ReadOnly {
                path: self.path().to_path_buf(),
            });
        }
        #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
        let mut turn = self.turn().await?;
        #[cfg(not(target_arch = "wasm32"))]
        let conn = {
            self.check_open()?;
            turn.take(self.path()).await?
        };
        #[cfg(target_arch = "wasm32")]
        let conn = self.pool().acquire().await?;
        // The guard exists before `BEGIN` is sent: sqlx's worker runs a
        // statement even when its future is dropped, so a caller cancelled
        // (or a `BEGIN` that failed) while it waits leaves the connection
        // to `Drop`, whose `ROLLBACK` ends that transaction or finds none.
        let mut tx = WriteTx {
            conn: Some(conn),
            turn: Some(turn),
            fail_rollback: false,
            secure_delete_on: false,
        };
        db::query("BEGIN IMMEDIATE").execute(tx.conn()).await?;
        Ok(tx)
    }
}

impl Storage {
    /// Rebuilds the file (`VACUUM`), so bytes deleted earlier don't linger
    /// in free space or pages nothing rewrote since. For the upgrade that
    /// strips secrets from stored connection strings (phase 5d, Decision
    /// 12a). It waits for this process's writers first, runs on the writer
    /// connection, and fails with `STORAGE_READ_ONLY` on read-only storage.
    pub async fn vacuum(&self) -> Result<(), StorageError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (mut turn, mut conn) = self.writer_conn().await?;
            let ran = db::query("VACUUM").execute(&mut conn).await;
            put_back(&mut turn, conn, true).await;
            ran?;
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _turn = self.exclusive_turn().await?;
            let mut conn = self.pool().acquire().await?;
            db::query("VACUUM").execute(&mut *conn).await?;
        }
        Ok(())
    }

    /// Moves every WAL page into the file and empties the WAL
    /// (`wal_checkpoint(TRUNCATE)`), on the writer connection. `false` when
    /// SQLite answered busy (a reader or writer elsewhere kept it from
    /// finishing).
    pub async fn checkpoint(&self) -> Result<bool, StorageError> {
        let sql = "PRAGMA wal_checkpoint(TRUNCATE)";
        #[cfg(not(target_arch = "wasm32"))]
        let (busy,): (i64,) = {
            let (mut turn, mut conn) = self.writer_conn().await?;
            let ran = db::query_as(sql).fetch_one(&mut conn).await;
            put_back(&mut turn, conn, true).await;
            ran?
        };
        #[cfg(target_arch = "wasm32")]
        let (busy,): (i64,) = {
            let _turn = self.exclusive_turn().await?;
            let mut conn = self.pool().acquire().await?;
            db::query_as(sql).fetch_one(&mut *conn).await?
        };
        Ok(busy == 0)
    }

    /// The write turn and the writer connection, for a statement that
    /// can't run in a [`WriteTx`]. Hand the connection back with
    /// `put_back`.
    #[cfg(not(target_arch = "wasm32"))]
    async fn writer_conn(
        &self,
    ) -> Result<(OwnedMutexGuard<Writer>, SqliteConnection), StorageError> {
        let mut turn = self.exclusive_turn().await?;
        self.check_open()?;
        let conn = turn.take(self.path()).await?;
        Ok((turn, conn))
    }

    /// This process's write turn, for a statement that can't run in a
    /// [`WriteTx`].
    async fn exclusive_turn(&self) -> Result<OwnedMutexGuard<Writer>, StorageError> {
        if self.is_read_only() {
            return Err(StorageError::ReadOnly {
                path: self.path().to_path_buf(),
            });
        }
        self.turn().await
    }

    /// This process's write turn, waiting at most [`crate::WRITE_WAIT`]
    /// for the earlier writers. The wait races the executor's `sleep` when
    /// Core gave storage one ([`Storage::with_executor`]); otherwise
    /// tokio's timer natively, as before, and the page's timer on wasm32.
    async fn turn(&self) -> Result<OwnedMutexGuard<Writer>, StorageError> {
        let timed_out = || StorageError::WriteLockTimeout {
            path: self.path().to_path_buf(),
            waited: self.write_wait(),
        };
        let lock = self.write_lock();
        let acquire = lock.lock_owned();
        if let Some(clock) = self.clock() {
            return race(acquire, clock.0.sleep(self.write_wait()))
                .await
                .ok_or_else(timed_out);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            tokio::time::timeout(self.write_wait(), acquire)
                .await
                .map_err(|_| timed_out())
        }
        // No tokio timer in the browser: the wait races the page's own
        // timer (phase 8 Decision 5), the one Core's `WasmExecutor` uses.
        #[cfg(target_arch = "wasm32")]
        {
            use seaquel_runtime::Executor as _;
            race(
                acquire,
                seaquel_runtime::WasmExecutor.sleep(self.write_wait()),
            )
            .await
            .ok_or_else(timed_out)
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Storage {
    /// A number that changes whenever another connection to the file (in
    /// this process or another) has committed since the last call (phase
    /// 7a Decision 5). `PRAGMA data_version` on the writer connection, which
    /// this storage's own writes never move, since they all run on it.
    ///
    /// - `None` while a write holds the write turn (this process's writers
    ///   come first); poll again later.
    /// - Only a change between two calls means anything: the first call is
    ///   the baseline. Commits between two calls count once.
    /// - A writer connection closed since the last call (a failed
    ///   `ROLLBACK`, or the web's close after each write) can't be
    ///   compared with its successor, so the next call counts a change: a
    ///   reload too many, never one too few.
    /// - Read-only storage answers too, on a connection of its own, since
    ///   every change there is another's.
    ///
    /// Native only: the browser's storage is in memory and no one else
    /// writes it.
    pub async fn external_version(&self) -> Result<Option<i64>, StorageError> {
        let Some(mut writer) = crate::lock::try_lock_owned(&self.write_lock()) else {
            return Ok(None);
        };
        self.check_open()?;
        let mut conn = writer.take(self.path()).await?;
        let polled: Result<i64, _> = db::query_scalar("PRAGMA data_version")
            .fetch_one(&mut conn)
            .await;
        match polled {
            Ok(data_version) => {
                put_back(&mut writer, conn, true).await;
                writer.observe(data_version);
                Ok(Some(writer.version))
            }
            Err(e) => {
                writer.discard(conn).await;
                Err(e.into())
            }
        }
    }
}

/// `acquire`'s guard, or `None` if `wait` ends first. `acquire` is polled
/// first, so a free lock wins over a wait that has already ended.
async fn race<G>(
    acquire: impl std::future::Future<Output = G>,
    wait: seaquel_runtime::BoxFuture<'static, ()>,
) -> Option<G> {
    use futures::future::{select, Either};
    match select(Box::pin(acquire), wait).await {
        Either::Left((guard, _)) => Some(guard),
        Either::Right(_) => None,
    }
}

/// Where a read runs: on the pool (in a read transaction of its own, so a
/// read made of several queries sees one version of the file), or inside a
/// [`WriteTx`], where it sees that transaction's own writes.
///
/// Every read function takes `impl Into<Reader>`, so both `&storage` and
/// `&mut tx` can be passed.
pub enum Reader<'a> {
    Pool(&'a Storage),
    Tx(&'a mut WriteTx),
}

impl<'a> From<&'a Storage> for Reader<'a> {
    fn from(st: &'a Storage) -> Self {
        Reader::Pool(st)
    }
}

impl<'a> From<&'a mut WriteTx> for Reader<'a> {
    fn from(tx: &'a mut WriteTx) -> Self {
        Reader::Tx(tx)
    }
}

impl<'a> Reader<'a> {
    /// A connection to run the read's queries on.
    pub(crate) async fn conn(self) -> Result<ReadConn<'a>, StorageError> {
        Ok(match self {
            // A deferred `BEGIN`: it takes no write lock, and dropping it
            // rolls the (empty) transaction back.
            Reader::Pool(st) => ReadConn::Own(st.pool().begin().await?),
            Reader::Tx(tx) => ReadConn::Borrowed(tx.conn()),
        })
    }
}

pub(crate) enum ReadConn<'a> {
    Own(Transaction),
    Borrowed(&'a mut SqliteConnection),
}

impl Deref for ReadConn<'_> {
    type Target = SqliteConnection;
    fn deref(&self) -> &SqliteConnection {
        match self {
            ReadConn::Own(tx) => tx,
            ReadConn::Borrowed(conn) => conn,
        }
    }
}

impl DerefMut for ReadConn<'_> {
    fn deref_mut(&mut self) -> &mut SqliteConnection {
        match self {
            ReadConn::Own(tx) => tx,
            ReadConn::Borrowed(conn) => conn,
        }
    }
}
