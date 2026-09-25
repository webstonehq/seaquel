//! Binds `Value` parameters onto a SQLite query.

use sqlx::query::Query;
use sqlx::sqlite::SqliteArguments;
use sqlx::Sqlite;

use seaquel_engine::{DbError, Value};

type SqliteQuery<'q> = Query<'q, Sqlite, SqliteArguments<'q>>;

/// `Int` binds as INTEGER and `Float` as REAL, so integers are exact.
/// SQLite has no decimal type: `Decimal` binds as its text.
pub fn bind_value<'q>(query: SqliteQuery<'q>, value: &'q Value) -> Result<SqliteQuery<'q>, DbError> {
    Ok(match value {
        Value::Null => query.bind(None::<serde_json::Value>),
        Value::Bool(b) => query.bind(*b),
        Value::Int(i) => query.bind(*i),
        Value::Float(f) => query.bind(*f),
        Value::Decimal(s) | Value::Text(s) => query.bind(s.as_str()),
        Value::Bytes(b) => query.bind(b.as_slice()),
        Value::Json(j) => query.bind(j),
        Value::Array(_) => {
            return Err(DbError::query_error("array parameters are not supported"))
        }
    })
}
