//! Decodes SQLite cells into `Value`.
//!
//! SQLite is dynamically typed, so a cell is decoded by its **storage class**
//! (`typeof(x)`), never by the declared column type: a TEXT value in an
//! untyped column stays `Text`, and a BLOB in a TEXT column is `Bytes`.
//! sqlx's `SqliteValueRef::type_info()` is exactly that: it reads
//! `sqlite3_value_type` and only falls back to the declared type for NULL.

use sqlx::{sqlite::SqliteValueRef, TypeInfo, ValueRef};

use seaquel_engine::{DbError, Value};

pub fn to_value(v: SqliteValueRef) -> Result<Value, DbError> {
    if v.is_null() {
        return Ok(Value::Null);
    }
    let class = v.type_info().name().to_string();
    let decode_error = |e: sqlx::error::BoxDynError| {
        DbError::query_error(format!("can't decode a {class} value: {e}"))
    };
    Ok(match class.as_str() {
        // An INTEGER is an i64; the wire format tags it `bigint` beyond 2^53.
        "INTEGER" => {
            Value::Int(<i64 as sqlx::Decode<sqlx::Sqlite>>::decode(v).map_err(decode_error)?)
        }
        // Infinity is storable (`9e999`); NaN isn't, SQLite stores NULL.
        "REAL" => {
            Value::Float(<f64 as sqlx::Decode<sqlx::Sqlite>>::decode(v).map_err(decode_error)?)
        }
        "BLOB" => {
            Value::Bytes(<Vec<u8> as sqlx::Decode<sqlx::Sqlite>>::decode(v).map_err(decode_error)?)
        }
        // TEXT that isn't valid UTF-8 (`CAST(x'ff' AS TEXT)`) shows U+FFFD
        // for the bad bytes instead of failing the whole query. Not `Bytes`:
        // SQLite tells TEXT from BLOB, so an edit keyed by it would bind a
        // BLOB and could hit a BLOB row with the same bytes. The lossy text
        // matches no row instead.
        "TEXT" => {
            let bytes = <Vec<u8> as sqlx::Decode<sqlx::Sqlite>>::decode(v).map_err(decode_error)?;
            Value::Text(String::from_utf8_lossy(&bytes).into_owned())
        }
        other => {
            return Err(DbError {
                message: format!("Unsupported datatype: {other}"),
                code: "UNSUPPORTED_TYPE".to_string(),
            })
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::{Connection, Row, SqliteConnection};

    async fn cells(sql: &str) -> Vec<Value> {
        let mut conn = SqliteConnection::connect("sqlite::memory:").await.unwrap();
        let row = sqlx::query(sql).fetch_one(&mut conn).await.unwrap();
        (0..row.len())
            .map(|i| to_value(row.try_get_raw(i).unwrap()).unwrap())
            .collect()
    }

    #[tokio::test]
    async fn decodes_by_storage_class() {
        assert_eq!(
            cells("SELECT 1, 1.5, 'a', x'00ff', NULL, 9e999, -9e999, 9007199254740993, CAST(x'ff41' AS TEXT)")
                .await,
            vec![
                Value::Int(1),
                Value::Float(1.5),
                Value::Text("a".into()),
                Value::Bytes(vec![0, 255]),
                Value::Null,
                Value::Float(f64::INFINITY),
                Value::Float(f64::NEG_INFINITY),
                Value::Int(9007199254740993),
                Value::Text("\u{fffd}A".into()),
            ]
        );
    }
}
