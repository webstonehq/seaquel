//! The native executor: sqlx, re-exported. Every name here is sqlx's own
//! item or an alias of it, so a call through `db` is the call it was.

pub use sqlx::error::DatabaseError;
pub use sqlx::migrate::{MigrateError, Migrator};
pub use sqlx::sqlite::{SqliteArguments, SqliteRow};
pub use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
pub use sqlx::{
    query, query_as, query_scalar, Error, Row, SqliteConnection, SqliteExecutor, SqlitePool,
};
pub use sqlx::{ConnectOptions, Connection};

use sqlx::{Decode, Sqlite, TypeInfo, ValueRef};

/// The numbered migrations in `migrations/`, embedded at compile time.
pub fn embedded_migrator() -> Migrator {
    sqlx::migrate!("./migrations")
}

/// A transaction begun on the pool.
pub type Transaction = sqlx::Transaction<'static, Sqlite>;

/// A query with its bound arguments, for the helpers that bind conditionally.
pub type SqliteQuery<'q> = sqlx::query::Query<'q, Sqlite, SqliteArguments<'q>>;

/// What a cell holds, for the flag codecs (`codec::flag`): its storage
/// class, with the number when it is one. Read exactly as before: a NULL is
/// checked first (its type info is the column's declared type), then the
/// value's own type decides.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cell {
    Null,
    Integer(i64),
    Real(f64),
    Other,
}

/// The [`Cell`] in `col`.
pub fn cell(row: &SqliteRow, col: &str) -> Result<Cell, Error> {
    let v = row.try_get_raw(col)?;
    if v.is_null() {
        return Ok(Cell::Null);
    }
    let kind = v.type_info();
    Ok(match kind.name() {
        "INTEGER" => Cell::Integer(<i64 as Decode<Sqlite>>::decode(v).map_err(Error::Decode)?),
        "REAL" => Cell::Real(<f64 as Decode<Sqlite>>::decode(v).map_err(Error::Decode)?),
        _ => Cell::Other,
    })
}
