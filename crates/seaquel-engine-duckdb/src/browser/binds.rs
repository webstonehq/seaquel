//! Bound values written into the SQL as literals (phase 8, Decision 9).
//!
//! DuckDB-WASM sends a prepared statement's parameters as JSON, so it can't
//! bind a `bigint` (or bytes) at all. The browser driver therefore runs every
//! statement unprepared, with each placeholder replaced by the value's DuckDB
//! literal from [`seaquel_sql::params::duckdb_bind_literal`].
//!
//! Placeholders are found with DuckDB's own scanner rules
//! ([`seaquel_sql::scan`]): only in code, never inside a string (`'…'`,
//! `E'…'`, `$tag$…$tag$`), a comment or a quoted name. They are `?` (in
//! order), or `?N` / `$N` (numbered from 1), not mixed. Each literal is
//! written with a space on both sides, so it can't join the text around it:
//! no `'a''b'` from a string right before it, no `E'…'` from an `E` before
//! it, no `--` from a minus before a negative number (which is also
//! parenthesized).
//!
//! The SQL that comes out may hold no NUL: DuckDB-WASM passes the text as a
//! C string and silently drops everything after one, which could cut a
//! statement short (`… WHERE id = 1\0 AND …`). A NUL inside a bound text
//! value is written as `chr(0)`; one in the caller's SQL is refused.

use seaquel_engine::{DbError, Value};
use seaquel_sql::params::duckdb_bind_literal;
use seaquel_sql::scan::{scan, ScanOptions, TokenKind};
use seaquel_sql::SqlEngine;

/// `sql` with each placeholder replaced by its value's literal. With no
/// values the SQL is returned as it is (Core's runs and pages bind nothing),
/// but still refused if it holds a NUL.
pub(crate) fn inline_binds(sql: &str, params: &[Value]) -> Result<String, DbError> {
    if sql.contains('\0') {
        return Err(DbError::query_error(
            "SQL holding a NUL character can't run on DuckDB in the browser",
        ));
    }
    if params.is_empty() {
        return Ok(sql.to_string());
    }
    let marks = placeholders(sql);
    let numbered = marks.iter().filter(|m| m.number.is_some()).count();
    if numbered > 0 && numbered < marks.len() {
        return Err(DbError::query_error(
            "The statement mixes numbered placeholders (?1, $1) with plain ?",
        ));
    }
    if numbered == 0 && marks.len() != params.len() {
        return Err(DbError::query_error(format!(
            "{} for {}",
            plural(params.len(), "value"),
            plural(marks.len(), "placeholder")
        )));
    }
    if numbered > 0 {
        let highest = marks.iter().filter_map(|m| m.number).max().unwrap_or(0);
        if let Some(m) = marks
            .iter()
            .find(|m| !matches!(m.number, Some(n) if (1..=params.len()).contains(&n)))
        {
            return Err(DbError::query_error(format!(
                "{} has no value ({} given)",
                &sql[m.start..m.end],
                plural(params.len(), "value")
            )));
        }
        if highest != params.len() {
            return Err(DbError::query_error(format!(
                "{} for placeholders up to {}",
                plural(params.len(), "value"),
                highest
            )));
        }
    }

    let literals = params
        .iter()
        .map(|v| duckdb_bind_literal(v).map_err(|e| DbError::query_error(e.message)))
        .collect::<Result<Vec<_>, _>>()?;
    let mut out =
        String::with_capacity(sql.len() + literals.iter().map(|l| l.len() + 2).sum::<usize>());
    let mut last = 0;
    for (i, m) in marks.iter().enumerate() {
        let index = m.number.map_or(i, |n| n - 1);
        out.push_str(&sql[last..m.start]);
        // Checked above: `index` is in range.
        let literal = literals.get(index).map_or("NULL", String::as_str);
        out.push(' ');
        out.push_str(literal);
        out.push(' ');
        last = m.end;
    }
    out.push_str(&sql[last..]);
    Ok(out)
}

fn plural(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

/// A placeholder's byte range, and its number for `?N` and `$N`.
struct Mark {
    start: usize,
    end: usize,
    number: Option<usize>,
}

/// Every placeholder in code: the text between DuckDB's strings, comments
/// and quoted names.
fn placeholders(sql: &str) -> Vec<Mark> {
    let mut marks = Vec::new();
    let mut plain = 0;
    for t in scan(sql, SqlEngine::Duckdb, ScanOptions::default()) {
        if matches!(t.kind, TokenKind::Comment | TokenKind::Quoted) {
            find_marks(sql, plain, t.start, &mut marks);
            plain = t.end;
        }
    }
    find_marks(sql, plain, sql.len(), &mut marks);
    marks
}

/// Whether `b` can continue a word, so a `$` after it is the word's
/// (`a$1`). Any non-ASCII byte counts: names can hold letters of any script.
fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80
}

/// The placeholders in `sql[from..to]`, which holds code only.
fn find_marks(sql: &str, from: usize, to: usize, marks: &mut Vec<Mark>) {
    let b = sql.as_bytes();
    let digits_end = |mut k: usize| {
        while k < to && b[k].is_ascii_digit() {
            k += 1;
        }
        k
    };
    let mut i = from;
    while i < to {
        let numbered_dollar = b[i] == b'$'
            && (i == 0 || !is_word_byte(b[i - 1]))
            && i + 1 < to
            && b[i + 1].is_ascii_digit();
        if b[i] == b'?' || numbered_dollar {
            let end = digits_end(i + 1);
            let number = (end > i + 1).then(|| sql[i + 1..end].parse().unwrap_or(usize::MAX));
            marks.push(Mark {
                start: i,
                end,
                number,
            });
            i = end;
        } else {
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn one(v: Value) -> String {
        inline_binds("SELECT ?", &[v]).unwrap()
    }

    #[test]
    fn binds_inline_every_value_kind() {
        let cases: Vec<(Value, &str)> = vec![
            (Value::Null, "SELECT  NULL "),
            (Value::Bool(true), "SELECT  TRUE "),
            (Value::Bool(false), "SELECT  FALSE "),
            (Value::Int(42), "SELECT  42 "),
            (Value::Int(-5), "SELECT  (-5) "),
            (Value::Int(i64::MIN), "SELECT  (-9223372036854775808) "),
            (
                Value::Int(9_007_199_254_740_993),
                "SELECT  9007199254740993 ",
            ),
            (Value::Float(1.5), "SELECT  CAST('1.5' AS DOUBLE) "),
            (Value::Float(0.1), "SELECT  CAST('0.1' AS DOUBLE) "),
            (Value::Float(-0.0), "SELECT  CAST('-0.0' AS DOUBLE) "),
            (
                Value::Float(f64::from(1e-40_f32)),
                "SELECT  CAST('9.99994610111476e-41' AS DOUBLE) ",
            ),
            (Value::Float(f64::NAN), "SELECT  CAST('nan' AS DOUBLE) "),
            (
                Value::Float(f64::INFINITY),
                "SELECT  CAST('inf' AS DOUBLE) ",
            ),
            (
                Value::Float(f64::NEG_INFINITY),
                "SELECT  CAST('-inf' AS DOUBLE) ",
            ),
            // DECIMAL as the helper binds it: width and scale from
            // the digits, one integer digit more than the scale.
            (
                Value::Decimal("1.50".into()),
                "SELECT  CAST('1.50' AS DECIMAL(3, 2)) ",
            ),
            (
                Value::Decimal("-0.05".into()),
                "SELECT  CAST('-0.05' AS DECIMAL(3, 2)) ",
            ),
            (
                Value::Decimal(".5".into()),
                "SELECT  CAST('0.5' AS DECIMAL(2, 1)) ",
            ),
            (
                Value::Decimal("+007".into()),
                "SELECT  CAST('7' AS DECIMAL(1, 0)) ",
            ),
            (
                Value::Decimal("0".into()),
                "SELECT  CAST('0' AS DECIMAL(1, 0)) ",
            ),
            (
                Value::Decimal("9".repeat(38)),
                &*format!("SELECT  CAST('{}' AS DECIMAL(38, 0)) ", "9".repeat(38)).leak(),
            ),
            // Past 38 digits, or not plain digits: text, which DuckDB casts
            // to the other side's type (the helper binds it as text).
            (
                Value::Decimal("340282366920938463463374607431768211455".into()),
                "SELECT  '340282366920938463463374607431768211455' ",
            ),
            (Value::Decimal("1e5".into()), "SELECT  '1e5' "),
            (Value::Decimal("NaN".into()), "SELECT  'NaN' "),
            (
                Value::Decimal("1'; DROP TABLE t; --".into()),
                "SELECT  '1''; DROP TABLE t; --' ",
            ),
            // Text: standard strings, where `\` is just a character.
            (Value::Text("héllo €".into()), "SELECT  'héllo €' "),
            (Value::Text(String::new()), "SELECT  '' "),
            (Value::Text("it's \\'".into()), "SELECT  'it''s \\''' "),
            (
                Value::Text("a\0b".into()),
                "SELECT  ('a' || chr(0) || 'b') ",
            ),
            (Value::Text("\0".into()), "SELECT  ('' || chr(0) || '') "),
            // Bytes, exactly.
            (
                Value::Bytes(vec![0, 0xff, b'a']),
                "SELECT  from_hex('00FF61') ",
            ),
            (Value::Bytes(vec![]), "SELECT  from_hex('') "),
            // JSON as JSON, its text escaped.
            (
                Value::Json(json!({"a": [1, "it's"], "z": null})),
                "SELECT  CAST('{\"a\":[1,\"it''s\"],\"z\":null}' AS JSON) ",
            ),
            (
                Value::Json(json!("\u{0}")),
                "SELECT  CAST('\"\\u0000\"' AS JSON) ",
            ),
            // Arrays as list literals, element by element.
            (
                Value::Array(vec![
                    Value::Int(1),
                    Value::Int(-2),
                    Value::Null,
                    Value::Text("x'".into()),
                    Value::Array(vec![Value::Float(f64::NAN)]),
                    Value::Array(vec![]),
                ]),
                "SELECT  [1, (-2), NULL, 'x''', [CAST('nan' AS DOUBLE)], []] ",
            ),
        ];
        for (value, want) in cases {
            assert_eq!(one(value.clone()), want, "{value:?}");
        }
    }

    #[test]
    fn binds_skip_question_marks_in_strings_comments_and_quoted_names() {
        let v = |i: i64| Value::Int(i);
        let sql = "SELECT '?', E'\\'?', $$?$$, $t$ ? $t$, \"a?\", ? -- ?\n /* ? */ + ?";
        assert_eq!(
            inline_binds(sql, &[v(1), v(2)]).unwrap(),
            "SELECT '?', E'\\'?', $$?$$, $t$ ? $t$, \"a?\",  1  -- ?\n /* ? */ +  2 "
        );
        // An unterminated string or comment runs to the end: nothing in it
        // is a placeholder, so the count is wrong.
        let e = inline_binds("SELECT ?, '?", &[v(1), v(2)]).unwrap_err();
        assert!(
            e.message.contains("2 values for 1 placeholder"),
            "{}",
            e.message
        );
        // Numbered, used twice.
        assert_eq!(
            inline_binds("SELECT ?2 + ?1 + $2", &[v(1), v(2)]).unwrap(),
            "SELECT  2  +  1  +  2 "
        );
        // `$1` inside a word is the word's.
        assert_eq!(
            inline_binds("SELECT a$1, ?", &[v(7)]).unwrap(),
            "SELECT a$1,  7 "
        );
    }

    #[test]
    fn binds_never_join_their_neighbours() {
        // A string right before: `'a''x'` would be one string.
        assert_eq!(
            inline_binds("SELECT 'a'?", &[Value::Text("x".into())]).unwrap(),
            "SELECT 'a' 'x' "
        );
        // An `E` right before would make an escape string of the value.
        assert_eq!(
            inline_binds("SELECT E?", &[Value::Text("\\'".into())]).unwrap(),
            "SELECT E '\\''' "
        );
        // A minus right before a negative number would start a comment.
        assert_eq!(
            inline_binds("SELECT 1-?", &[Value::Int(-1)]).unwrap(),
            "SELECT 1- (-1) "
        );
    }

    #[test]
    fn hostile_text_stays_inside_its_literal() {
        let value = "'); DROP TABLE t; -- \\ \u{0} /* $$ \" ?";
        let sql = inline_binds("SELECT ? AS v", &[Value::Text(value.into())]).unwrap();
        assert_eq!(
            sql,
            "SELECT  ('''); DROP TABLE t; -- \\ ' || chr(0) || ' /* $$ \" ?')  AS v"
        );
        // DuckDB's scanner sees one parenthesized expression: two strings
        // and `chr(0)`, no comment, no second statement.
        let tokens = seaquel_sql::scan::scan(
            &sql,
            seaquel_sql::SqlEngine::Duckdb,
            seaquel_sql::scan::ScanOptions::default(),
        );
        assert!(tokens
            .iter()
            .all(|t| t.kind != seaquel_sql::scan::TokenKind::Comment));
        assert_eq!(
            seaquel_sql::scan::split_statements(&sql, seaquel_sql::SqlEngine::Duckdb).len(),
            1
        );
    }

    #[test]
    fn counts_and_mixing_are_checked() {
        let e = inline_binds("SELECT ?, ?", &[Value::Int(1)]).unwrap_err();
        assert!(
            e.message.contains("1 value for 2 placeholders"),
            "{}",
            e.message
        );
        let e = inline_binds("SELECT ?1, ?", &[Value::Int(1)]).unwrap_err();
        assert!(e.message.contains("numbered"), "{}", e.message);
        let e = inline_binds("SELECT ?3", &[Value::Int(1)]).unwrap_err();
        assert!(e.message.contains("?3"), "{}", e.message);
        let e = inline_binds("SELECT ?0", &[Value::Int(1)]).unwrap_err();
        assert!(e.message.contains("?0"), "{}", e.message);
    }

    #[test]
    fn nul_in_the_sql_is_refused() {
        // With or without values: DuckDB-WASM would drop what follows it.
        for params in [vec![], vec![Value::Int(1)]] {
            let e = inline_binds("SELECT 1\0; DROP TABLE t", &params).unwrap_err();
            assert_eq!(e.code, "QUERY_ERROR");
            assert!(e.message.contains("NUL"), "{}", e.message);
        }
        // Without values the SQL is passed on untouched, `?` and all.
        assert_eq!(inline_binds("SELECT '?', ?", &[]).unwrap(), "SELECT '?', ?");
    }
}
