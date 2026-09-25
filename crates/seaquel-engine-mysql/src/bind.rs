//! Binds `Value` parameters onto a MySQL/MariaDB query.

use sqlx::mysql::MySqlArguments;
use sqlx::query::Query;
use sqlx::MySql;

use seaquel_engine::{DbError, Value};

type MySqlQuery<'q> = Query<'q, MySql, MySqlArguments>;

/// `Int` binds as BIGINT and `Float` as DOUBLE, so integers are exact.
/// `Decimal` binds as DECIMAL through `rust_decimal`, or as its text when
/// `rust_decimal` can't hold it (MySQL converts the string itself).
pub fn bind_value<'q>(query: MySqlQuery<'q>, value: &'q Value) -> Result<MySqlQuery<'q>, DbError> {
    Ok(match value {
        // sqlx types a bare NULL by the Rust type; this keeps what the old
        // JSON binder sent.
        Value::Null => query.bind(None::<serde_json::Value>),
        // sqlx encodes bool as TINYINT 0/1, which MySQL accepts for tinyint
        // columns (JSON text "true" would be rejected).
        Value::Bool(b) => query.bind(*b),
        Value::Int(i) => query.bind(*i),
        Value::Float(f) => query.bind(*f),
        Value::Decimal(s) => match s.parse::<rust_decimal::Decimal>() {
            Ok(d) => query.bind(d),
            Err(_) => query.bind(s.as_str()),
        },
        Value::Text(s) => query.bind(s.as_str()),
        Value::Bytes(b) => query.bind(b.as_slice()),
        Value::Json(j) => query.bind(j),
        Value::Array(_) => {
            return Err(DbError::query_error("array parameters are not supported"))
        }
    })
}
