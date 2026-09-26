//! Binds `Value` parameters as `@P1…` of an `sp_executesql` call.
//!
//! | `Value` | SQL Server parameter |
//! |---|---|
//! | `Null` | the `NULL` literal in the statement (see [`inline_nulls`]) |
//! | `Bool` | bit |
//! | `Int` | bigint |
//! | `Float` | float |
//! | `Decimal` | numeric(p,s), exact; nvarchar when it doesn't fit (more than 38 digits, scale above 37, an exponent) |
//! | `Text` | nvarchar |
//! | `Bytes` | varbinary |
//! | `Json` | nvarchar, its JSON text (SQL Server has no JSON type) |
//! | `Array` | rejected (`QUERY_ERROR`) |
//!
//! Dates, times and uniqueidentifiers arrive as `Text`; SQL Server converts
//! nvarchar to the column's type on comparison and assignment, and the
//! decoder's text forms convert back exactly.

use std::borrow::Cow;

use tiberius::numeric::Numeric;
use tiberius::Query;

use seaquel_engine::{DbError, Value};

/// Binds `v` as the next parameter of `query`.
pub(crate) fn bind_mssql_param(query: &mut Query<'_>, v: &Value) -> Result<(), DbError> {
    match v {
        // Not in the SQL any more (see `inline_nulls`); still bound so the
        // numbering of the later parameters holds.
        Value::Null => query.bind(None::<&str>),
        Value::Bool(b) => query.bind(*b),
        Value::Int(i) => query.bind(*i),
        Value::Float(f) => query.bind(*f),
        Value::Decimal(s) => match numeric(s) {
            Some(n) => query.bind(n),
            None => query.bind(s.clone()),
        },
        Value::Text(s) => query.bind(s.clone()),
        Value::Bytes(b) => query.bind(b.clone()),
        Value::Json(j) => query.bind(j.to_string()),
        Value::Array(_) => {
            return Err(DbError::query_error("array parameters are not supported"));
        }
    }
    Ok(())
}

/// The type tiberius declares for `v` once [`bind_mssql_param`] has bound
/// it (tiberius 0.12.3, `ColumnData::type_name`), for a nested
/// `sp_executesql` that forwards the parameters with the same types. Its
/// string and binary limits count bytes (UTF-8 for strings), as tiberius's
/// do. `None` for what can't be bound.
pub(crate) fn declared_type(v: &Value) -> Option<Cow<'static, str>> {
    let string = |len: usize| {
        if len <= 4000 {
            "nvarchar(4000)"
        } else {
            "nvarchar(max)"
        }
    };
    Some(match v {
        Value::Null => "nvarchar(4000)".into(),
        Value::Bool(_) => "bit".into(),
        Value::Int(_) => "bigint".into(),
        Value::Float(_) => "float(53)".into(),
        Value::Decimal(s) => match numeric(s) {
            Some(n) => format!("numeric({},{})", n.precision(), n.scale()).into(),
            None => string(s.len()).into(),
        },
        Value::Text(s) => string(s.len()).into(),
        Value::Bytes(b) if b.len() <= 8000 => "varbinary(8000)".into(),
        Value::Bytes(_) => "varbinary(max)".into(),
        Value::Json(j) => string(j.to_string().len()).into(),
        Value::Array(_) => return None,
    })
}

/// Decimal text (`-12.50`, `.5`, `7.`) as a `Numeric` with its scale, when
/// SQL Server's numeric holds it: at most 38 significant digits and a scale
/// below 38.
pub(crate) fn numeric(s: &str) -> Option<Numeric> {
    let (negative, rest) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let (int, frac) = rest.split_once('.').unwrap_or((rest, ""));
    if int.is_empty() && frac.is_empty()
        || !int.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let int = int.trim_start_matches('0');
    if frac.len() > 37 || int.len() + frac.len() > 38 {
        return None;
    }
    let digits: i128 = format!("0{int}{frac}").parse().ok()?;
    let scale = u8::try_from(frac.len()).ok()?;
    Some(Numeric::new_with_scale(
        if negative { -digits } else { digits },
        scale,
    ))
}

/// Words that start a new statement, ending an `EXEC` argument list (see
/// [`inline_nulls`]).
const ENDS_EXEC_ARGS: &[&str] = &[
    "ALTER",
    "BEGIN",
    "BREAK",
    "CLOSE",
    "COMMIT",
    "CONTINUE",
    "CREATE",
    "DEALLOCATE",
    "DECLARE",
    "DELETE",
    "DENY",
    "DROP",
    "ELSE",
    "END",
    "FETCH",
    "GOTO",
    "GRANT",
    "IF",
    "INSERT",
    "MERGE",
    "OPEN",
    "PRINT",
    "RAISERROR",
    "RETURN",
    "REVOKE",
    "ROLLBACK",
    "SAVE",
    "SELECT",
    "SET",
    "THROW",
    "TRUNCATE",
    "UPDATE",
    "USE",
    "WAITFOR",
    "WHILE",
    "WITH",
];

/// `sql` with each `@Pn` whose parameter is `Null` replaced by `NULL`.
///
/// tiberius declares every parameter with a type, and no type converts to
/// every column: an nvarchar NULL (what `None::<&str>` binds as) can't be
/// assigned to varbinary or image (error 257), a varbinary NULL not to
/// float, date or time columns, and so on. The literal `NULL` goes
/// anywhere. Only whole `@Pn` words outside strings, quoted names and
/// comments are replaced, case-insensitively, as T-SQL compares variable
/// names.
///
/// In an `EXEC` argument list, a `@Pn` followed by `=` names the
/// procedure's parameter (`EXEC p @P1 = @P2`, or the return-status variable
/// in `EXEC @P1 = p`) and is left alone; the value after `=` is still
/// replaced. The list runs from `EXEC`/`EXECUTE` to a `;` or the next
/// statement keyword ([`ENDS_EXEC_ARGS`]).
///
/// What changes, when a parameter is NULL:
/// - `SELECT @P1` returns an int NULL instead of an nvarchar one, and so
///   does a column made from it: `SELECT @P1 AS c INTO t` creates an int
///   column, and in a `UNION` it types the column as int.
/// - `COALESCE(@P1, @P2)` with both NULL is error 4127, and `CASE`/`IIF`
///   whose every result is such a NULL is error 8133 (no result that isn't
///   the NULL constant).
/// - A statement that assigns to a parameter (`SET @P1 = 1`) fails, and so
///   does passing it as an `OUTPUT` argument: `EXEC p @x = @P1 OUTPUT`
///   becomes `EXEC p @x = NULL OUTPUT` (error 179, a constant can't be
///   `OUTPUT`).
pub(crate) fn inline_nulls<'a>(sql: &'a str, params: &[Value]) -> Cow<'a, str> {
    if !params.iter().any(Value::is_null) {
        return Cow::Borrowed(sql);
    }
    let is_null = |word: &str| {
        let n = word
            .strip_prefix("@P")
            .or_else(|| word.strip_prefix("@p"))
            .filter(|d| {
                !d.is_empty() && !d.starts_with('0') && d.bytes().all(|b| b.is_ascii_digit())
            })
            .and_then(|d| d.parse::<usize>().ok());
        n.is_some_and(|n| params.get(n - 1).is_some_and(Value::is_null))
    };
    let word = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '@' | '#' | '$');
    let s: Vec<(usize, char)> = sql.char_indices().collect();
    let n = s.len();
    let at = |i: usize| s.get(i).map(|&(_, c)| c);
    let byte = |i: usize| s.get(i).map_or(sql.len(), |&(b, _)| b);
    let mut out = String::with_capacity(sql.len());
    let mut copied = 0; // bytes of `sql` already in `out`
    let mut exec_args = false;
    let mut i = 0;
    while i < n {
        let c = s[i].1;
        if c == ';' {
            exec_args = false;
            i += 1;
        } else if c == '-' && at(i + 1) == Some('-') {
            while i < n && s[i].1 != '\n' {
                i += 1;
            }
        } else if c == '/' && at(i + 1) == Some('*') {
            let mut level = 1;
            i += 2;
            while i < n && level > 0 {
                if s[i].1 == '/' && at(i + 1) == Some('*') {
                    level += 1;
                    i += 2;
                } else if s[i].1 == '*' && at(i + 1) == Some('/') {
                    level -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
        } else if matches!(c, '\'' | '"' | '[') {
            let close = if c == '[' { ']' } else { c };
            i += 1;
            while i < n {
                if s[i].1 == close {
                    if at(i + 1) == Some(close) {
                        i += 2;
                        continue;
                    }
                    break;
                }
                i += 1;
            }
            i += 1;
        } else if word(c) {
            let start = i;
            while i < n && word(s[i].1) {
                i += 1;
            }
            let (from, to) = (byte(start), byte(i));
            let w = &sql[from..to];
            if w.eq_ignore_ascii_case("EXEC") || w.eq_ignore_ascii_case("EXECUTE") {
                exec_args = true;
            } else if ENDS_EXEC_ARGS.iter().any(|k| w.eq_ignore_ascii_case(k)) {
                exec_args = false;
            }
            let names_a_parameter = exec_args && {
                let mut j = i;
                while j < n && s[j].1.is_whitespace() {
                    j += 1;
                }
                at(j) == Some('=')
            };
            if is_null(w) && !names_a_parameter {
                out.push_str(&sql[copied..from]);
                out.push_str("NULL");
                copied = to;
            }
        } else {
            i += 1;
        }
    }
    out.push_str(&sql[copied..]);
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(s: &str) -> Option<(i128, u8)> {
        numeric(s).map(|n| (n.value(), n.scale()))
    }

    #[test]
    fn decimals_bind_as_numeric_when_they_fit() {
        assert_eq!(num("12.50"), Some((1250, 2)));
        assert_eq!(num("-0.5"), Some((-5, 1)));
        assert_eq!(num("+3"), Some((3, 0)));
        assert_eq!(num(".5"), Some((5, 1)));
        assert_eq!(num("7."), Some((7, 0)));
        assert_eq!(num("0000123.4"), Some((1234, 1)));
        assert_eq!(
            num("9999999999999999999999999999.9999999999"),
            Some((99_999_999_999_999_999_999_999_999_999_999_999_999, 10))
        );
        assert_eq!(
            num("-0.1234567890123456789012345678901234567"),
            Some((-1_234_567_890_123_456_789_012_345_678_901_234_567, 37))
        );
        // Too wide, scale 38, an exponent, not a number: bound as text.
        assert_eq!(num("123456789012345678901234567890123456789"), None);
        assert_eq!(num("0.12345678901234567890123456789012345678"), None);
        assert_eq!(num("1e5"), None);
        assert_eq!(num(""), None);
        assert_eq!(num("-"), None);
        assert_eq!(num("."), None);
        assert_eq!(num("1.2.3"), None);
        assert_eq!(num("NaN"), None);
    }

    #[test]
    fn null_parameters_become_literals() {
        let p = [Value::Null, Value::Int(1), Value::Null];
        assert_eq!(
            inline_nulls("UPDATE t SET b = @P1 WHERE id = @P2 AND x = @p3", &p),
            "UPDATE t SET b = NULL WHERE id = @P2 AND x = NULL"
        );
        // Strings, names, comments, longer names and other variables stay.
        assert_eq!(
            inline_nulls(
                "SELECT '@P1', [@P1], \"@P1\", @P10, @P1x, @@P1, @P01 -- @P1\n/* @P1 */, @P1",
                &p
            ),
            "SELECT '@P1', [@P1], \"@P1\", @P10, @P1x, @@P1, @P01 -- @P1\n/* @P1 */, NULL"
        );
        assert_eq!(
            inline_nulls("SELECT N'東京', @P1", &p),
            "SELECT N'東京', NULL"
        );
        // In an EXEC argument list, a `@Pn` followed by `=` names the
        // procedure's parameter (or the return-status variable) and stays;
        // the value after `=` is replaced, and so is everything once the
        // list ends.
        let exec = |sql: &str| inline_nulls(sql, &p).into_owned();
        assert_eq!(exec("EXEC p @P1 = @P2"), "EXEC p @P1 = @P2");
        assert_eq!(exec("EXEC p @P2 = @P1"), "EXEC p @P2 = NULL");
        assert_eq!(exec("EXEC p @a=@P1,@b=@P3"), "EXEC p @a=NULL,@b=NULL");
        assert_eq!(exec("EXEC p @P1=@P3, @P3 = 1"), "EXEC p @P1=NULL, @P3 = 1");
        assert_eq!(exec("EXEC @P1 = p @P3"), "EXEC @P1 = p NULL");
        assert_eq!(
            exec("EXECUTE sp_executesql N'SELECT @P1', N'@P1 int', @P1 = @P1"),
            "EXECUTE sp_executesql N'SELECT @P1', N'@P1 int', @P1 = NULL"
        );
        assert_eq!(exec("EXEC p @x = @P1 OUTPUT"), "EXEC p @x = NULL OUTPUT");
        assert_eq!(
            exec("EXEC p @P1 = 1; SELECT @P1 WHERE @P3 = x"),
            "EXEC p @P1 = 1; SELECT NULL WHERE NULL = x"
        );
        assert_eq!(
            exec("EXEC p @P1 = 1\nSELECT @P1 = x"),
            "EXEC p @P1 = 1\nSELECT NULL = x"
        );
        // Nothing NULL: untouched, not copied.
        assert!(matches!(
            inline_nulls("SELECT @P1", &[Value::Int(1)]),
            Cow::Borrowed(_)
        ));
    }
}
