//! Binds `Value` parameters onto a Postgres query.

use sqlx::postgres::PgArguments;
use sqlx::query::Query;
use sqlx::Postgres;

use seaquel_engine::{DbError, Value};

use crate::numeric::Numeric;

type PgQuery<'q> = Query<'q, Postgres, PgArguments>;

/// - `Int` binds as INT8 and `Float` as FLOAT8, so integers are exact.
/// - `Decimal` binds as NUMERIC in the binary wire format, exactly: any
///   scale, NaN, ±Infinity, more than 28 digits.
/// - `Null` binds as a TEXT NULL, `Bytes` as BYTEA, `Json` as JSONB.
/// - `Array` binds as the matching one-dimensional array (see [`bind_array`]).
///   Nested arrays and mixed element kinds are `QUERY_ERROR`.
pub fn bind_value<'q>(query: PgQuery<'q>, value: &'q Value) -> Result<PgQuery<'q>, DbError> {
    Ok(match value {
        // sqlx types a bare NULL by the Rust type. TEXT casts explicitly to
        // every type, so `CAST($1 AS date)` works (bug fix 6: the old JSONB
        // NULL couldn't cast to date, timestamp, uuid, inet, bytea, …).
        Value::Null => query.bind(None::<&str>),
        Value::Bool(b) => query.bind(*b),
        Value::Int(i) => query.bind(*i),
        Value::Float(f) => query.bind(*f),
        Value::Decimal(s) => query.bind(parse_decimal(s)?),
        Value::Text(s) => query.bind(s.as_str()),
        Value::Bytes(b) => query.bind(b.as_slice()),
        Value::Json(j) => query.bind(j),
        Value::Array(items) => bind_array(query, items)?,
    })
}

/// The element type of an array parameter.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Bool,
    Int,
    Float,
    Decimal,
    Text,
    Bytes,
    Json,
}

fn kind(v: &Value) -> Result<Option<Kind>, DbError> {
    Ok(Some(match v {
        Value::Null => return Ok(None),
        Value::Bool(_) => Kind::Bool,
        Value::Int(_) => Kind::Int,
        Value::Float(_) => Kind::Float,
        Value::Decimal(_) => Kind::Decimal,
        Value::Text(_) => Kind::Text,
        Value::Bytes(_) => Kind::Bytes,
        Value::Json(_) => Kind::Json,
        Value::Array(_) => return Err(DbError::query_error("nested arrays can't be bound")),
    }))
}

/// One element kind for the whole array. Integers mixed with floats bind as
/// FLOAT8[], and with decimals as NUMERIC[] (JavaScript's `[1, 2.5]` arrives
/// as `Int` and `Float`). Any other mix is an error.
fn element_kind(items: &[Value]) -> Result<Option<Kind>, DbError> {
    let mut acc: Option<Kind> = None;
    for item in items {
        let Some(k) = kind(item)? else { continue };
        acc = Some(match (acc, k) {
            (None, k) => k,
            (Some(a), k) if a == k => a,
            (Some(Kind::Int), Kind::Float) | (Some(Kind::Float), Kind::Int) => Kind::Float,
            (Some(Kind::Int), Kind::Decimal) | (Some(Kind::Decimal), Kind::Int) => Kind::Decimal,
            (Some(a), k) => {
                return Err(DbError::query_error(format!(
                    "array parameter mixes {a:?} and {k:?} elements"
                )))
            }
        });
    }
    Ok(acc)
}

/// Collect the elements as `T`, keeping NULLs. `f` sees only the element
/// kinds [`element_kind`] allowed.
fn collect<'q, T>(
    items: &'q [Value],
    f: impl Fn(&'q Value) -> Result<T, DbError>,
) -> Result<Vec<Option<T>>, DbError> {
    items
        .iter()
        .map(|v| {
            if v.is_null() {
                Ok(None)
            } else {
                f(v).map(Some)
            }
        })
        .collect()
}

fn unexpected(v: &Value) -> DbError {
    DbError::query_error(format!("unexpected array element {v:?}"))
}

/// A one-dimensional array of one element type.
///
/// Integers bind as INT4[] when every element fits in 32 bits, else INT8[].
/// Postgres has no cross-width array equality, so this is a trade-off:
/// `$1 = ARRAY[1, 2]` (an INT4[]) works, but `$1 = ARRAY[1, 2]::int8[]` only
/// works with a cast on the parameter, `$1::int8[]`. `col = ANY($1)` works
/// with either width, and assignment (`INSERT … VALUES ($1)`) casts.
///
/// An empty or all-NULL array carries no element type and binds as TEXT[].
/// Cast it where the type matters (`$1::int[]`); TEXT[] casts to other array
/// types explicitly.
fn bind_array<'q>(query: PgQuery<'q>, items: &'q [Value]) -> Result<PgQuery<'q>, DbError> {
    Ok(match element_kind(items)? {
        None | Some(Kind::Text) => query.bind(collect(items, |v| match v {
            Value::Text(s) => Ok(s.as_str()),
            other => Err(unexpected(other)),
        })?),
        Some(Kind::Bool) => query.bind(collect(items, |v| match v {
            Value::Bool(b) => Ok(*b),
            other => Err(unexpected(other)),
        })?),
        // INT4[] when every element fits: Postgres has no `int8[] = int4[]`,
        // and `ARRAY[1, 2]` is an INT4[]. `= ANY($1)` works with either.
        Some(Kind::Int)
            if items
                .iter()
                .all(|v| !matches!(v, Value::Int(i) if i32::try_from(*i).is_err())) =>
        {
            query.bind(collect(items, |v| match v {
                Value::Int(i) => i32::try_from(*i).map_err(|_| unexpected(v)),
                other => Err(unexpected(other)),
            })?)
        }
        Some(Kind::Int) => query.bind(collect(items, |v| match v {
            Value::Int(i) => Ok(*i),
            other => Err(unexpected(other)),
        })?),
        Some(Kind::Float) => query.bind(collect(items, |v| match v {
            Value::Float(f) => Ok(*f),
            // Mixed with floats: FLOAT8[] can't do better.
            Value::Int(i) => Ok(*i as f64),
            other => Err(unexpected(other)),
        })?),
        Some(Kind::Decimal) => query.bind(collect(items, |v| match v {
            Value::Decimal(s) => parse_decimal(s),
            Value::Int(i) => parse_decimal(&i.to_string()),
            other => Err(unexpected(other)),
        })?),
        Some(Kind::Bytes) => query.bind(collect(items, |v| match v {
            Value::Bytes(b) => Ok(b.as_slice()),
            other => Err(unexpected(other)),
        })?),
        Some(Kind::Json) => query.bind(collect(items, |v| match v {
            Value::Json(j) => Ok(j),
            other => Err(unexpected(other)),
        })?),
    })
}

fn parse_decimal(s: &str) -> Result<Numeric, DbError> {
    Numeric::parse(s)
        .map_err(|e| DbError::query_error(format!("decimal parameter can't be bound: {e}")))
}
