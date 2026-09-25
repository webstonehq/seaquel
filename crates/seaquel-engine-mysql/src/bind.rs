//! Binds `Value` parameters onto a MySQL/MariaDB query.

use sqlx::encode::{Encode, IsNull};
use sqlx::error::BoxDynError;
use sqlx::mysql::{MySqlArguments, MySqlTypeInfo};
use sqlx::query::Query;
use sqlx::{MySql, Type};

use seaquel_engine::{DbError, Value};

type MySqlQuery<'q> = Query<'q, MySql, MySqlArguments>;

/// `Int` binds as BIGINT and `Float` as DOUBLE, so integers are exact.
/// `Decimal` binds as a DECIMAL parameter carrying its exact text (see
/// [`DecimalText`]); `Bytes` as a binary string; `Json` as its JSON text.
pub fn bind_value<'q>(query: MySqlQuery<'q>, value: &'q Value) -> Result<MySqlQuery<'q>, DbError> {
    Ok(match value {
        // sqlx types a bare NULL by the Rust type; this keeps what the old
        // JSON binder sent. The server assigns it to any column, binary,
        // geometry and BIT included.
        Value::Null => query.bind(None::<serde_json::Value>),
        // sqlx encodes bool as TINYINT 0/1, which MySQL accepts for tinyint
        // columns (JSON text "true" would be rejected).
        Value::Bool(b) => query.bind(*b),
        Value::Int(i) => query.bind(*i),
        Value::Float(f) => query.bind(*f),
        Value::Decimal(s) if is_decimal_literal(s) => query.bind(DecimalText(s)),
        // Not a number MySQL's DECIMAL can hold (e.g. Postgres's `NaN`): the
        // server converts the text itself, as it would a typed-in value.
        Value::Decimal(s) => query.bind(s.as_str()),
        Value::Text(s) => query.bind(s.as_str()),
        Value::Bytes(b) => query.bind(b.as_slice()),
        Value::Json(j) => query.bind(j),
        Value::Array(_) => return Err(DbError::query_error("array parameters are not supported")),
    })
}

/// Exact decimal text sent as a DECIMAL (NEWDECIMAL) parameter, which is how
/// sqlx sends `rust_decimal` too: the protocol carries DECIMAL as text. Unlike
/// `rust_decimal` (28 digits), nothing is rounded: DECIMAL(65,30) and
/// BIGINT UNSIGNED above `i64::MAX` bind exactly, and `=` compares as
/// DECIMAL (MariaDB compares a plain string with a DECIMAL as DOUBLE).
struct DecimalText<'a>(&'a str);

impl Type<MySql> for DecimalText<'_> {
    fn type_info() -> MySqlTypeInfo {
        <rust_decimal::Decimal as Type<MySql>>::type_info()
    }
}

impl<'q> Encode<'q, MySql> for DecimalText<'q> {
    fn encode_by_ref(&self, buf: &mut Vec<u8>) -> Result<IsNull, BoxDynError> {
        <&str as Encode<MySql>>::encode_by_ref(&self.0, buf)
    }
}

/// `-12.50`, `10000`, `.5`: what MySQL reads as an exact DECIMAL.
fn is_decimal_literal(s: &str) -> bool {
    let digits = s.strip_prefix(['-', '+']).unwrap_or(s);
    let (int, frac) = digits.split_once('.').unwrap_or((digits, ""));
    !(int.is_empty() && frac.is_empty())
        && int.bytes().all(|b| b.is_ascii_digit())
        && frac.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_literals() {
        for ok in [
            "0",
            "-12.50",
            "+1",
            "18446744073709551615",
            ".5",
            "5.",
            "0.000000000000000000000000000001",
        ] {
            assert!(is_decimal_literal(ok), "{ok}");
        }
        for bad in [
            "", "-", ".", "NaN", "Infinity", "1e5", "1.2.3", "1 ", "0x10",
        ] {
            assert!(!is_decimal_literal(bad), "{bad}");
        }
    }
}
