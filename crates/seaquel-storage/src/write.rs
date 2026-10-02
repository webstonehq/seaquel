//! One write transaction per Core call (phase 5d Decision 3), and the reader
//! that lets a query run either inside it or on the pool.

use std::ops::{Deref, DerefMut};

use crate::db::{self, PoolConnection, SqliteConnection, Transaction};
use crate::lock::OwnedMutexGuard;

use crate::{Storage, StorageError};

/// A write transaction on the metadata file.
///
/// - **In this process,** writers queue on the storage's async write mutex
///   (shared by every clone of the [`Storage`]) before they take a pool
///   connection. So a writer waiting its turn holds no connection, and a
///   small pool (the web's 2 per user) can't be drained by waiting writers
///   while readers starve or a writer times out on `database is locked`.
/// - **Across processes,** it begins with `BEGIN IMMEDIATE`: SQLite's write
///   lock is taken up front, so a writer in another pool or process waits
///   up to the busy timeout instead of failing when it upgrades a read lock.
///
/// The targeted write functions in the query modules take `&mut WriteTx`,
/// so a call reads what it needs, checks it, writes and commits in one
/// place. Dropping it without [`WriteTx::commit`] rolls everything back.
///
/// **Inside a write, read only through `&mut tx`.** A read through the
/// storage (`&storage`) while a `WriteTx` is open takes a second pool
/// connection: it doesn't see the transaction's writes, and on a pool of one
/// it waits for the connection the transaction holds, which never comes
/// back while the caller awaits the read.
pub struct WriteTx {
    /// `None` once committed or handed to the rollback in `Drop`.
    conn: Option<PoolConnection>,
    /// Held until the transaction has ended (a rollback on drop keeps it
    /// until the `ROLLBACK` ran), so the next writer never begins on top.
    turn: Option<OwnedMutexGuard<()>>,
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
/// which would leave the pooled connection unusable for every later
/// transaction (phase 5d-2 review, the size cap).
async fn rollback_on(conn: &mut SqliteConnection) -> Result<(), StorageError> {
    match db::query("ROLLBACK").execute(conn).await {
        Ok(_) => Ok(()),
        Err(db::Error::Database(e)) if e.message().contains("no transaction is active") => Ok(()),
        Err(e) => Err(e.into()),
    }
}

impl WriteTx {
    /// Makes every write in it durable. If the commit fails, the
    /// transaction is rolled back.
    pub async fn commit(mut self) -> Result<(), StorageError> {
        let Some(conn) = self.conn.as_mut() else {
            return Ok(());
        };
        db::query("COMMIT").execute(&mut **conn).await?;
        self.conn = None;
        Ok(())
    }

    /// Undoes every write in it. Dropping the transaction does the same.
    pub async fn rollback(mut self) -> Result<(), StorageError> {
        let Some(mut conn) = self.conn.take() else {
            return Ok(());
        };
        let done = rollback_on(&mut conn).await;
        if done.is_err() {
            // Leave no connection in an unknown state in the pool.
            conn.close_on_drop();
        }
        done
    }

    /// Turns `PRAGMA secure_delete` on or off for this transaction's
    /// connection. While it's on, the space deletes and overwrites free is
    /// zeroed, so text they remove doesn't linger in the pages written: the
    /// upgrade that strips secrets from stored connection strings (phase
    /// 5d, Decision 12a) turns it on around its row update and off again
    /// before the commit, so the pooled connection goes back to the pool as
    /// it came.
    pub async fn secure_delete(&mut self, on: bool) -> Result<(), StorageError> {
        let sql = if on {
            "PRAGMA secure_delete = ON"
        } else {
            "PRAGMA secure_delete = OFF"
        };
        db::query(sql).execute(self.conn()).await?;
        Ok(())
    }

    pub(crate) fn conn(&mut self) -> &mut SqliteConnection {
        self.conn
            .as_deref_mut()
            .expect("a WriteTx has its connection until it's consumed")
    }
}

impl Drop for WriteTx {
    /// Rolls back a transaction that wasn't committed (or a `BEGIN` still
    /// in flight). The `ROLLBACK` runs on a task that keeps the connection
    /// and this process's write turn until it has, so the next writer
    /// begins after it; sqlx runs it after anything already queued on the
    /// connection.
    ///
    /// Without a Tokio runtime (a `WriteTx` outliving it at process exit)
    /// nothing can run the `ROLLBACK`, and sqlx can't return or close the
    /// connection either (both need a runtime). The connection is leaked
    /// with the process; SQLite discards the uncommitted transaction when
    /// the file is next opened.
    #[cfg(not(target_arch = "wasm32"))]
    fn drop(&mut self) {
        let Some(mut conn) = self.conn.take() else {
            return;
        };
        let turn = self.turn.take();
        match tokio::runtime::Handle::try_current() {
            // The native executor only: on wasm32 the `ROLLBACK` is
            // synchronous (below).
            Ok(rt) => {
                rt.spawn(async move {
                    if rollback_on(&mut conn).await.is_err() {
                        conn.close_on_drop();
                    }
                    drop(conn);
                    drop(turn);
                });
            }
            Err(_) => {
                std::mem::forget(conn);
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
        self.conn
            .as_deref()
            .expect("a WriteTx has its connection until it's consumed")
    }
}

impl DerefMut for WriteTx {
    fn deref_mut(&mut self) -> &mut SqliteConnection {
        self.conn()
    }
}

impl Storage {
    /// Begins a write transaction ([`WriteTx`]): waits for this process's
    /// earlier writers, then takes a connection and `BEGIN IMMEDIATE`.
    ///
    /// On storage opened with `StorageOptions::read_only` it fails at once
    /// with [`StorageError::ReadOnly`] (`STORAGE_READ_ONLY`). When the
    /// earlier writers don't finish within [`crate::WRITE_WAIT`] (30 s) it
    /// fails with [`StorageError::WriteLockTimeout`] (`STORAGE_ERROR`), so a
    /// write begun while the caller holds a `WriteTx` fails instead of
    /// hanging.
    pub async fn write(&self) -> Result<WriteTx, StorageError> {
        if self.is_read_only() {
            return Err(StorageError::ReadOnly {
                path: self.path().to_path_buf(),
            });
        }
        let turn = self.turn().await?;
        let conn = self.pool().acquire().await?;
        // The guard exists before `BEGIN` is sent: sqlx's worker runs a
        // statement even when its future is dropped, so a caller cancelled
        // (or a `BEGIN` that failed) while it waits leaves the connection
        // to `Drop`, whose `ROLLBACK` ends that transaction or finds none.
        let mut tx = WriteTx {
            conn: Some(conn),
            turn: Some(turn),
        };
        db::query("BEGIN IMMEDIATE").execute(tx.conn()).await?;
        Ok(tx)
    }
}

impl Storage {
    /// Rebuilds the file (`VACUUM`), so bytes deleted earlier don't linger
    /// in free space or pages nothing rewrote since. For the upgrade that
    /// strips secrets from stored connection strings (phase 5d, Decision
    /// 12a). It waits for this process's writers first, and fails with
    /// `STORAGE_READ_ONLY` on read-only storage.
    pub async fn vacuum(&self) -> Result<(), StorageError> {
        let _turn = self.exclusive_turn().await?;
        let mut conn = self.pool().acquire().await?;
        db::query("VACUUM").execute(&mut *conn).await?;
        Ok(())
    }

    /// Moves every WAL page into the file and empties the WAL
    /// (`wal_checkpoint(TRUNCATE)`). `false` when SQLite answered busy (a
    /// reader or writer elsewhere kept it from finishing).
    pub async fn checkpoint(&self) -> Result<bool, StorageError> {
        let _turn = self.exclusive_turn().await?;
        let mut conn = self.pool().acquire().await?;
        let (busy,): (i64,) = db::query_as("PRAGMA wal_checkpoint(TRUNCATE)")
            .fetch_one(&mut *conn)
            .await?;
        Ok(busy == 0)
    }

    /// This process's write turn, for a statement that can't run in a
    /// [`WriteTx`].
    async fn exclusive_turn(&self) -> Result<OwnedMutexGuard<()>, StorageError> {
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
    async fn turn(&self) -> Result<OwnedMutexGuard<()>, StorageError> {
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
