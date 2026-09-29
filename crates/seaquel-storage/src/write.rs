//! One write transaction per Core call (phase 5d Decision 3), and the reader
//! that lets a query run either inside it or on the pool.

use std::ops::{Deref, DerefMut};

use sqlx::{Sqlite, SqliteConnection, Transaction};
use tokio::sync::OwnedMutexGuard;

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
    // Dropped first: the rollback is queued on the connection before the
    // next writer may begin.
    tx: Transaction<'static, Sqlite>,
    _turn: OwnedMutexGuard<()>,
}

impl std::fmt::Debug for WriteTx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WriteTx")
    }
}

impl WriteTx {
    /// Makes every write in it durable.
    pub async fn commit(self) -> Result<(), StorageError> {
        Ok(self.tx.commit().await?)
    }

    /// Undoes every write in it. Dropping the transaction does the same.
    pub async fn rollback(self) -> Result<(), StorageError> {
        Ok(self.tx.rollback().await?)
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
        sqlx::query(sql).execute(self.conn()).await?;
        Ok(())
    }

    pub(crate) fn conn(&mut self) -> &mut SqliteConnection {
        &mut self.tx
    }
}

impl Deref for WriteTx {
    type Target = SqliteConnection;
    fn deref(&self) -> &SqliteConnection {
        &self.tx
    }
}

impl DerefMut for WriteTx {
    fn deref_mut(&mut self) -> &mut SqliteConnection {
        &mut self.tx
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
        let turn = tokio::time::timeout(self.write_wait(), self.write_lock().lock_owned())
            .await
            .map_err(|_| StorageError::WriteLockTimeout {
                path: self.path().to_path_buf(),
                waited: self.write_wait(),
            })?;
        Ok(WriteTx {
            tx: self.pool().begin_with("BEGIN IMMEDIATE").await?,
            _turn: turn,
        })
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
        sqlx::query("VACUUM").execute(&mut *conn).await?;
        Ok(())
    }

    /// Moves every WAL page into the file and empties the WAL
    /// (`wal_checkpoint(TRUNCATE)`). `false` when SQLite answered busy (a
    /// reader or writer elsewhere kept it from finishing).
    pub async fn checkpoint(&self) -> Result<bool, StorageError> {
        let _turn = self.exclusive_turn().await?;
        let mut conn = self.pool().acquire().await?;
        let (busy,): (i64,) = sqlx::query_as("PRAGMA wal_checkpoint(TRUNCATE)")
            .fetch_one(&mut *conn)
            .await?;
        Ok(busy == 0)
    }

    /// This process's write turn, for a statement that can't run in a
    /// [`WriteTx`].
    async fn exclusive_turn(&self) -> Result<tokio::sync::OwnedMutexGuard<()>, StorageError> {
        if self.is_read_only() {
            return Err(StorageError::ReadOnly {
                path: self.path().to_path_buf(),
            });
        }
        tokio::time::timeout(self.write_wait(), self.write_lock().lock_owned())
            .await
            .map_err(|_| StorageError::WriteLockTimeout {
                path: self.path().to_path_buf(),
                waited: self.write_wait(),
            })
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
    Own(Transaction<'static, Sqlite>),
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
