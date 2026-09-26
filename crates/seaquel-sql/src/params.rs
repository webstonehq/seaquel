//! `{{param}}` extraction and substitution (Task 4). Port of
//! `src/lib/db/query-params.ts` (deleted in phase 2b), with literal and comment
//! boundaries taken from the tokenizer in [`crate::scan`]. The fix 13 rules are
//! the recorder's `params-model.ts`, kept as
//! `docs/plans/artifacts/2026-09-27-sql-recorder-params-model.ts.txt`.
//!
//! Per engine (`substituteParameters`):
//!
//! - **Postgres and SQLite, bound** (fix 13):
//!   `$n` numbered by first use. Inside a `'…'` literal `' || $n || '`, inside
//!   `E'…'` `' || $n || E'` so the rest keeps its escapes. Inside a
//!   `$tag$…$tag$` string the value's raw text (open question 4: an accepted
//!   injection path, since the user writes both the query and the value; the
//!   tag check only stops a value from closing the string). Comments and
//!   quoted names are copied as they are.
//! - **Forced inline** on Postgres, SQLite, MySQL and MariaDB (fix 13,
//!   extended): `escapeValueForInline` with the same contexts; a negative
//!   number in code is parenthesized, `E'…'` doubles `\`, a MySQL string
//!   doubles `\` and its own quote, and a MySQL executable comment is code.
//! - **MySQL and MariaDB, bound**: `?` per occurrence. A string literal (with
//!   the adjacent literals MySQL joins to it, and a charset introducer in
//!   front) holding parameters becomes one `?` bound to its CONCAT text.
//! - **SQL Server and DuckDB**: inlined, never bound.
//!
//! Fix 13's value rules keep a value inside its literal:
//!
//! - A `$tag$` string must still end where it did once its values are in: a
//!   value can form the tag with the text next to it (`$$Cost: ${{p}}$$`
//!   with `$ || 'INJ' --`), which the "contains the tag" check doesn't see.
//! - In a string that takes backslash escapes (`E'…'`, MySQL `'…'` and
//!   `"…"`), a `{{` after an odd run of `\` is escaped text, not a parameter:
//!   filling it would leave the `\` to escape the value's first quote.
//! - A `Decimal` whose text isn't a finite decimal number is refused wherever
//!   its text goes into the SQL.
//! - Integers inline as their digits (negatives in parentheses in code).
//! - A value inlined in code is spaced off its neighbours ([`Out`]): a quoted
//!   value right after a quote would merge with that literal through `''`
//!   (and a `\` in it could then end an `E'…'` string early), and a number,
//!   `NULL` or boolean followed by a word character or `$` would fuse into
//!   one word (`NULL$$…`, `1$t$`).
//!
//! Values follow decision 11: a `Value` is what JavaScript held before
//! `encodeParam` (`Decimal` a `SqlDecimal`). `Bytes`, `Json` and `Array`
//! values are refused.

use std::collections::HashMap;
use std::fmt;

use seaquel_types::Value;

use crate::js_ws::{is_js_space, js_trim};
use crate::scan::{scan, ScanOptions, Token, TokenKind};
use crate::SqlEngine;

/// Substituted SQL and the values bound to its placeholders, in order. The TS
/// `{ sql, bindValues }`.
#[derive(Debug, Clone, PartialEq)]
pub struct Substituted {
    pub sql: String,
    pub bind_values: Vec<Value>,
}

/// Why a query couldn't be substituted. The wrapper throws it as a
/// `ParameterSubstitutionError`; `message` is word for word the TS text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubstitutionError {
    pub message: String,
}

impl fmt::Display for SubstitutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SubstitutionError {}

/// One `{{name}}`: the TS `PARAM_REGEX`, `\{\{([a-zA-Z_][a-zA-Z0-9_]*)\}\}`.
#[derive(Debug, Clone, Copy)]
struct Param<'a> {
    start: usize,
    end: usize,
    name: &'a str,
}

/// Every `{{name}}` in `text`, left to right, not overlapping (the regex with
/// the `g` flag). All ASCII, so every range is on char boundaries.
fn params(text: &str) -> Vec<Param<'_>> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < b.len() {
        if b[i] == b'{' && b[i + 1] == b'{' {
            let s = i + 2;
            if b.get(s)
                .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')
            {
                let mut j = s + 1;
                while b
                    .get(j)
                    .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
                {
                    j += 1;
                }
                if b.get(j) == Some(&b'}') && b.get(j + 1) == Some(&b'}') {
                    out.push(Param {
                        start: i,
                        end: j + 2,
                        name: &text[s..j],
                    });
                    i = j + 2;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

/// `text` with each parameter replaced by `f(name)`.
fn fill<E>(text: &str, mut f: impl FnMut(&str) -> Result<String, E>) -> Result<String, E> {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for p in params(text) {
        out.push_str(&text[last..p.start]);
        out.push_str(&f(p.name)?);
        last = p.end;
    }
    out.push_str(&text[last..]);
    Ok(out)
}

/// Every `{{name}}` in the text, unique, in order of first appearance.
pub fn extract_parameters(sql: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in params(sql) {
        if !out.iter().any(|n| n == p.name) {
            out.push(p.name.to_string());
        }
    }
    out
}

/// Whether the text holds a `{{name}}`.
pub fn has_parameters(sql: &str) -> bool {
    !params(sql).is_empty()
}

// --- Values as JavaScript held them ----------------------------------------

/// A parameter value as the TS saw it after `decodeCell`.
#[derive(Debug, Clone, Copy)]
enum Js<'a> {
    /// `null` or `undefined` (a name with no value); the TS treats both alike.
    Null,
    /// A `number` holding an integer, or a `bigint`: both inline as digits.
    Int(i64),
    /// A `number`, possibly NaN or ±Infinity.
    Float(f64),
    /// A `SqlDecimal`.
    Decimal(&'a str),
    Bool(bool),
    Str(&'a str),
}

impl<'a> Js<'a> {
    /// `String(value)` for a `number`, `bigint` or `SqlDecimal`; `None` for
    /// the others, which each caller spells out.
    fn number_text(self) -> Option<String> {
        match self {
            Js::Int(i) => Some(i.to_string()),
            Js::Float(f) => Some(js_number(f)),
            Js::Decimal(s) => Some(s.to_string()),
            _ => None,
        }
    }

    /// `String(value)`: what `rawText` and `concatText` fall back to.
    fn string(self) -> String {
        match self {
            Js::Null => "null".to_string(),
            Js::Bool(b) => b.to_string(),
            Js::Str(s) => s.to_string(),
            other => other.number_text().unwrap_or_default(),
        }
    }
}

/// `String(n)` for a JavaScript number: `1e+21`, `0.1`, `-0` as `0`, `NaN`,
/// `Infinity`.
fn js_number(f: f64) -> String {
    ryu_js::Buffer::new().format(f).to_string()
}

/// The values by name, with `Map` semantics: the last duplicate wins.
struct Values<'a> {
    map: HashMap<&'a str, &'a Value>,
}

const NULL: Value = Value::Null;

impl<'a> Values<'a> {
    fn new(values: &'a [(String, Value)]) -> Self {
        let mut map = HashMap::with_capacity(values.len());
        for (name, value) in values {
            map.insert(name.as_str(), value);
        }
        Values { map }
    }

    /// The value for `name` (NULL when there's none), refusing the kinds
    /// decision 11 excludes. Bound values go out as they are.
    fn value(&self, name: &str) -> Result<&'a Value, SubstitutionError> {
        let v = self.map.get(name).copied().unwrap_or(&NULL);
        match v {
            Value::Bytes(_) | Value::Json(_) | Value::Array(_) => Err(SubstitutionError {
                message: format!(
                    "The value for {{{{{name}}}}} is binary, JSON or an array; only text, numbers, booleans and NULL can be substituted"
                ),
            }),
            _ => Ok(v),
        }
    }

    /// The value for `name` as JavaScript held it, for text that's bound
    /// (MySQL's CONCAT text).
    fn js(&self, name: &str) -> Result<Js<'a>, SubstitutionError> {
        Ok(match self.value(name)? {
            Value::Bool(b) => Js::Bool(*b),
            Value::Int(i) => Js::Int(*i),
            Value::Float(f) => Js::Float(*f),
            Value::Decimal(s) => Js::Decimal(s),
            Value::Text(s) => Js::Str(s),
            _ => Js::Null,
        })
    }

    /// The value for `name`, for text that goes into the SQL: a decimal must
    /// be a finite decimal number.
    fn inline(&self, name: &str) -> Result<Js<'a>, SubstitutionError> {
        let v = self.js(name)?;
        match v {
            Js::Decimal(s) if !is_decimal(s) => Err(SubstitutionError {
                message: format!("The value for {{{{{name}}}}} is not a finite decimal number"),
            }),
            _ => Ok(v),
        }
    }
}

/// `^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$`: decimal text that inlines as
/// a number (no `NaN`, no `Infinity`, nothing else).
fn is_decimal(s: &str) -> bool {
    let b = s.as_bytes();
    let digits = |mut k: usize| {
        while b.get(k).is_some_and(u8::is_ascii_digit) {
            k += 1;
        }
        k
    };
    let mut k = usize::from(matches!(b.first(), Some(b'+' | b'-')));
    let int_end = digits(k);
    let has_int = int_end > k;
    k = int_end;
    if b.get(k) == Some(&b'.') {
        let frac_end = digits(k + 1);
        if !has_int && frac_end == k + 1 {
            return false;
        }
        k = frac_end;
    } else if !has_int {
        return false;
    }
    if matches!(b.get(k), Some(b'e' | b'E')) {
        k += 1;
        if matches!(b.get(k), Some(b'+' | b'-')) {
            k += 1;
        }
        let exp_end = digits(k);
        if exp_end == k {
            return false;
        }
        k = exp_end;
    }
    k == b.len()
}

/// `escapeValueForInline(value, insideString)`.
fn escape_inline(v: Js<'_>, inside: bool) -> String {
    match v {
        Js::Null => (if inside { "" } else { "NULL" }).to_string(),
        Js::Float(f) if !f.is_finite() => (if inside { "" } else { "NULL" }).to_string(),
        Js::Int(i) => i.to_string(),
        Js::Float(f) => js_number(f),
        Js::Bool(b) => (if b { "1" } else { "0" }).to_string(),
        // `String(value)` with `'` doubled, quoted unless inside a string.
        other => {
            let s = other.string().replace('\'', "''");
            if inside {
                s
            } else {
                format!("'{s}'")
            }
        }
    }
}

/// `concatText`: the text MySQL's CONCAT makes of a value; `None` for NULL.
fn concat_text(v: Js<'_>) -> Option<String> {
    match v {
        Js::Null => None,
        Js::Float(f) if !f.is_finite() => None,
        Js::Bool(b) => Some((if b { "1" } else { "0" }).to_string()),
        other => Some(other.string()),
    }
}

/// A value's raw text in a dollar-quoted string (`rawText`, DuckDB's rule).
fn raw_text(v: Js<'_>) -> String {
    match v {
        Js::Null => String::new(),
        Js::Decimal(s) => plain_decimal(s),
        other => other.string(),
    }
}

/// Wraps text that starts with `-` in parentheses, so `1-{{p}}` with −1
/// can't become the comment `1--1`.
fn paren(text: String) -> String {
    if text.starts_with('-') {
        format!("({text})")
    } else {
        text
    }
}

/// Past this many digits of padding, [`plain_decimal`] leaves the exponent
/// as it is instead of writing out the zeros.
const MAX_DECIMAL_PADDING: u64 = 1 << 20;

/// `plainDecimal`: decimal text without an exponent (`1e5` → `100000`,
/// `-1.5E-3` → `-0.0015`), so it inlines as an exact numeric, not a float.
/// Text that isn't a decimal with an exponent comes back unchanged.
fn plain_decimal(text: &str) -> String {
    let unchanged = || text.to_string();
    let t = js_trim(text);
    let b = t.as_bytes();
    let digits_from = |mut k: usize| {
        while b.get(k).is_some_and(u8::is_ascii_digit) {
            k += 1;
        }
        k
    };
    let mut k = 0;
    let negative = match b.first() {
        Some(b'-') => {
            k = 1;
            true
        }
        Some(b'+') => {
            k = 1;
            false
        }
        _ => false,
    };
    let int_end = digits_from(k);
    let int = &t[k..int_end];
    k = int_end;
    let mut frac = "";
    if b.get(k) == Some(&b'.') {
        let end = digits_from(k + 1);
        frac = &t[k + 1..end];
        k = end;
    }
    if !matches!(b.get(k), Some(b'e' | b'E')) {
        return unchanged();
    }
    k += 1;
    let exp_negative = match b.get(k) {
        Some(b'-') => {
            k += 1;
            true
        }
        Some(b'+') => {
            k += 1;
            false
        }
        _ => false,
    };
    let exp_end = digits_from(k);
    if exp_end == k || exp_end != b.len() || (int.is_empty() && frac.is_empty()) {
        return unchanged();
    }
    // Saturating: anything this large is refused below anyway.
    let exp = t[k..exp_end].bytes().fold(0u64, |acc, d| {
        acc.saturating_mul(10)
            .saturating_add(u64::from(d - b'0'))
            .min(u64::MAX / 4)
    });
    let mut digits = format!("{int}{frac}");
    let len = digits.len() as u64;
    let int_len = int.len() as u64;
    // Position of the decimal point in `digits`.
    let point: u64 = if exp_negative {
        match int_len.checked_sub(exp) {
            Some(p) => p,
            None => {
                let pad = exp - int_len;
                if pad > MAX_DECIMAL_PADDING {
                    return unchanged();
                }
                digits.insert_str(0, &"0".repeat(pad as usize));
                0
            }
        }
    } else {
        let p = int_len + exp;
        if p > len {
            if p - len > MAX_DECIMAL_PADDING {
                return unchanged();
            }
            digits.push_str(&"0".repeat((p - len) as usize));
        }
        p
    };
    let point = point as usize;
    let whole = digits[..point].trim_start_matches('0');
    let whole = if whole.is_empty() { "0" } else { whole };
    let rest = &digits[point..];
    let mut out = String::with_capacity(digits.len() + 3);
    if negative {
        out.push('-');
    }
    out.push_str(whole);
    if !rest.is_empty() {
        out.push('.');
        out.push_str(rest);
    }
    out
}

// --- Substitution -------------------------------------------------------------

/// Replaces each `{{name}}` with a placeholder or the value's literal text, as
/// the engine needs. `values` pairs a parameter name with its value, with the
/// TS `Map` semantics: on a duplicate name the last value wins, and a name
/// with no value becomes NULL.
pub fn substitute(
    sql: &str,
    values: &[(String, Value)],
    engine: SqlEngine,
    force_inline: bool,
) -> Result<Substituted, SubstitutionError> {
    let values = Values::new(values);
    match engine {
        SqlEngine::Mysql | SqlEngine::Mariadb if !force_inline => {
            substitute_mysql(sql, &values, engine)
        }
        SqlEngine::Mssql => substitute_mssql(sql, &values),
        SqlEngine::Duckdb => substitute_duckdb(sql, &values),
        _ => substitute_tokens(sql, &values, engine, force_inline),
    }
}

/// Every token of `sql`, comments included, with MySQL executable comments
/// read as code.
fn all_tokens(sql: &str, engine: SqlEngine) -> Vec<Token> {
    scan(
        sql,
        engine,
        ScanOptions {
            exec_comments: true,
            ..ScanOptions::default()
        },
    )
}

/// Whether the `{{` at `at` follows an odd run of `\\` inside the token
/// starting at `token_start`: escaped text in a string that takes backslash
/// escapes, not a parameter.
fn escaped(text: &str, token_start: usize, at: usize) -> bool {
    let run = text.as_bytes()[token_start..at]
        .iter()
        .rev()
        .take_while(|&&c| c == b'\\')
        .count();
    run % 2 == 1
}

/// Whether a quoted token is a string that takes backslash escapes: `E'…'`
/// on Postgres and DuckDB, `'…'` and `"…"` on MySQL/MariaDB.
fn backslash_string(text: &str, engine: SqlEngine) -> bool {
    let b = text.as_bytes();
    if engine.is_mysql() {
        matches!(b.first(), Some(b'\'' | b'"'))
    } else {
        matches!(engine, SqlEngine::Postgres | SqlEngine::Duckdb)
            && matches!(b.first(), Some(b'E' | b'e'))
            && b.get(1) == Some(&b'\'')
    }
}

/// The parameters in a string token's `text`, without the escaped ones when
/// it takes backslash escapes.
fn token_params(text: &str, backslashes: bool) -> Vec<Param<'_>> {
    let mut ps = params(text);
    if backslashes {
        ps.retain(|p| !escaped(text, 0, p.start));
    }
    ps
}

/// `text` with each of `ps` (parameters in it) replaced by `f(name)`.
fn fill_params(
    text: &str,
    ps: &[Param<'_>],
    mut f: impl FnMut(&str) -> Result<String, SubstitutionError>,
) -> Result<String, SubstitutionError> {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for p in ps {
        out.push_str(&text[last..p.start]);
        out.push_str(&f(p.name)?);
        last = p.end;
    }
    out.push_str(&text[last..]);
    Ok(out)
}

/// Postgres and SQLite (bound or forced inline), and MySQL/MariaDB forced
/// inline: fix 13, `substituteByToken` in the recorder's `params-model.ts`.
fn substitute_tokens(
    sql: &str,
    values: &Values<'_>,
    engine: SqlEngine,
    force: bool,
) -> Result<Substituted, SubstitutionError> {
    let mysql = engine.is_mysql();
    let tokens: Vec<Token> = all_tokens(sql, engine)
        .into_iter()
        .filter(|t| t.kind != TokenKind::Comment)
        .collect();
    let mut bind_values: Vec<Value> = Vec::new();
    let mut positions: HashMap<String, usize> = HashMap::new();
    // `$n` for `name`, numbered by first bound use.
    let mut bind = |name: &str, bind_values: &mut Vec<Value>| -> Result<usize, SubstitutionError> {
        if let Some(&n) = positions.get(name) {
            return Ok(n);
        }
        bind_values.push(values.value(name)?.clone());
        positions.insert(name.to_string(), bind_values.len());
        Ok(bind_values.len())
    };
    // A value in code: a negative number parenthesized; on MySQL `\\` doubled
    // in a quoted value (the default sql_mode reads it as an escape).
    let in_code = |v: Js<'_>| {
        let mut text = escape_inline(v, false);
        if mysql && text.starts_with('\'') {
            text = text.replace('\\', "\\\\");
        }
        paren(text)
    };

    let mut out = Out::default();
    let mut last = 0;
    let mut k = 0;
    for p in params(sql) {
        if p.start < last {
            continue; // in a `$tag$` string already filled
        }
        while k < tokens.len() && tokens[k].end <= p.start {
            k += 1;
        }
        let token = tokens.get(k).filter(|t| t.start <= p.start);
        let whole = &sql[p.start..p.end];
        let replacement = match token {
            // In a comment.
            None => whole.to_string(),
            Some(t) if t.kind != TokenKind::Quoted => {
                if force {
                    out.text(&sql[last..p.start]);
                    out.value(&in_code(values.inline(p.name)?), None, true);
                    last = p.end;
                    continue;
                }
                format!("${}", bind(p.name, &mut bind_values)?)
            }
            Some(t) => {
                let text = t.text(sql);
                let first = text.as_bytes().first().copied();
                let e_string =
                    matches!(first, Some(b'E' | b'e')) && text.as_bytes().get(1) == Some(&b'\'');
                let string = first == Some(b'\'') || e_string || (mysql && first == Some(b'"'));
                if string && backslash_string(text, engine) && escaped(sql, t.start, p.start) {
                    // `\{{p}}`: escaped text, not a parameter.
                    whole.to_string()
                } else if string && mysql {
                    // Only forced inline gets here on MySQL: `\\` and the
                    // string's own quote doubled.
                    let quote = if first == Some(b'"') { "\"" } else { "'" };
                    escape_inline(values.inline(p.name)?, true)
                        .replace("''", "'")
                        .replace('\\', "\\\\")
                        .replace(quote, &quote.repeat(2))
                } else if string && !e_string {
                    if force {
                        escape_inline(values.inline(p.name)?, true)
                    } else {
                        format!("' || ${} || '", bind(p.name, &mut bind_values)?)
                    }
                } else if string {
                    if force {
                        escape_inline(values.inline(p.name)?, true).replace('\\', "\\\\")
                    } else {
                        format!("' || ${} || E'", bind(p.name, &mut bind_values)?)
                    }
                } else if let Some(tag) = dollar_tag(text) {
                    // The whole string at once, so its end can be checked.
                    out.text(&sql[last..t.start]);
                    out.text(&fill_tag(values, text, tag)?);
                    last = t.end;
                    continue;
                } else {
                    // In a quoted name.
                    whole.to_string()
                }
            }
        };
        out.text(&sql[last..p.start]);
        out.text(&replacement);
        last = p.end;
    }
    out.text(&sql[last..]);
    Ok(Substituted {
        sql: out.sql,
        bind_values,
    })
}

/// The opening tag of a dollar-quoted string token (`$$`, `$tag$`).
fn dollar_tag(text: &str) -> Option<&str> {
    let rest = text.strip_prefix('$')?;
    rest.find('$').map(|k| &text[..k + 2])
}

/// A value's raw text inside a `$tag$` string. It can't hold the closing tag,
/// which would end the string.
fn dollar_text(values: &Values<'_>, name: &str, tag: &str) -> Result<String, SubstitutionError> {
    let text = raw_text(values.inline(name)?);
    if text.contains(tag) {
        return Err(SubstitutionError {
            message: format!(
                "The value for {{{{{name}}}}} contains the dollar-quote tag {tag}; use a different tag or a '…' string"
            ),
        });
    }
    Ok(text)
}

/// A `$tag$` string token (`text`, opening with `tag`) with its parameters'
/// raw text in. Each value is checked for the tag, and the filled string must
/// still end where the original did (or not at all when it was
/// unterminated): a value can form the tag with the text next to it.
fn fill_tag(values: &Values<'_>, text: &str, tag: &str) -> Result<String, SubstitutionError> {
    let mut out = String::with_capacity(text.len());
    let mut ranges: Vec<(usize, usize, &str)> = Vec::new();
    let mut last = 0;
    for p in params(text) {
        out.push_str(&text[last..p.start]);
        let s = out.len();
        out.push_str(&dollar_text(values, p.name, tag)?);
        ranges.push((s, out.len(), p.name));
        last = p.end;
    }
    out.push_str(&text[last..]);
    let n = tag.len();
    let terminated = text[n..].find(tag).map(|k| k + n) == Some(text.len().saturating_sub(n))
        && text.len() >= 2 * n;
    let first = out[n..].find(tag).map(|k| k + n);
    let expected = terminated.then(|| out.len() - n);
    if first == expected {
        return Ok(out);
    }
    let at = first.unwrap_or(out.len());
    let hit = ranges
        .iter()
        .find(|&&(s, e, _)| s < at + n && e > at)
        .or_else(|| ranges.iter().find(|&&(s, e, _)| s <= at + n && e >= at))
        .or(ranges.first());
    match hit {
        Some(&(_, _, name)) => Err(SubstitutionError {
            message: format!(
                "The value for {{{{{name}}}}} forms the dollar-quote tag {tag} with the text next to it; use a different tag or a '…' string"
            ),
        }),
        // No parameter, no change: the string ends where it did.
        None => Ok(out),
    }
}

/// MySQL's backslash escapes in string literals (the default sql_mode). `%`
/// and `_` keep their backslash, so LIKE patterns still see an escaped one.
fn mysql_escape(c: char) -> &'static str {
    match c {
        '0' => "\0",
        'b' => "\u{8}",
        'n' => "\n",
        'r' => "\r",
        't' => "\t",
        'Z' => "\u{1a}",
        '%' => "\\%",
        '_' => "\\_",
        _ => "",
    }
}

/// A MySQL string literal token's text with its escapes resolved
/// (`mysqlLiteralText`), with each of `ps` (its parameters, not escaped) as
/// `fill(name)`.
fn mysql_literal_text(
    token: &str,
    ps: &[Param<'_>],
    mut fill: impl FnMut(&str) -> Result<String, SubstitutionError>,
) -> Result<String, SubstitutionError> {
    let Some(quote) = token.chars().next() else {
        return Ok(String::new());
    };
    let mut out = String::with_capacity(token.len());
    let mut ps = ps.iter().peekable();
    let mut i = quote.len_utf8();
    while let Some(c) = token[i..].chars().next() {
        while ps.peek().is_some_and(|p| p.start < i) {
            ps.next();
        }
        if let Some(p) = ps.next_if(|p| p.start == i) {
            out.push_str(&fill(p.name)?);
            i = p.end;
        } else if c == '\\' {
            match token[i + 1..].chars().next() {
                Some(e) => {
                    match mysql_escape(e) {
                        "" => out.push(e),
                        s => out.push_str(s),
                    }
                    i += 1 + e.len_utf8();
                }
                None => {
                    out.push('\\');
                    i += 1;
                }
            }
        } else if c == quote {
            if token[i + 1..].starts_with(quote) {
                out.push(quote);
                i += 2;
            } else {
                break;
            }
        } else {
            out.push(c);
            i += c.len_utf8();
        }
    }
    Ok(out)
}

/// Where a charset introducer in front of the string run at `tokens[first]`
/// starts, if there is one: the previous token is a word, either `N`/`n`
/// touching the quote or `_charset` (`^_[A-Za-z0-9_]+$`) with only whitespace
/// in between. A comment token in between is no introducer.
fn introducer_start(sql: &str, tokens: &[Token], first: usize) -> Option<usize> {
    let prev = tokens.get(first.checked_sub(1)?)?;
    if prev.kind != TokenKind::Word {
        return None;
    }
    let start = tokens[first].start;
    let word = prev.text(sql);
    let gap = &sql[prev.end..start];
    let n = matches!(word, "N" | "n") && gap.is_empty();
    let charset = word.len() >= 2
        && word.starts_with('_')
        && word.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
        && gap.chars().all(is_js_space);
    (n || charset).then_some(prev.start)
}

fn is_mysql_string(sql: &str, t: &Token) -> bool {
    t.kind == TokenKind::Quoted && matches!(sql.as_bytes()[t.start], b'\'' | b'"')
}

/// MySQL/MariaDB bound (`substituteMysqlParameters`): `?` placeholders, one
/// bind value per occurrence. A string literal holding parameters, with the
/// adjacent literals MySQL joins to it, becomes one `?` bound to its text with
/// the values filled in as CONCAT would (NULL if any value is NULL), and the
/// `?` swallows a charset introducer in front of it. Comments and backtick
/// names are copied as they are; an executable comment is SQL.
fn substitute_mysql(
    sql: &str,
    values: &Values<'_>,
    engine: SqlEngine,
) -> Result<Substituted, SubstitutionError> {
    let tokens = all_tokens(sql, engine);
    let mut out = String::with_capacity(sql.len());
    let mut bind_values: Vec<Value> = Vec::new();
    let mut plain = 0;
    let flush = |out: &mut String,
                 bind_values: &mut Vec<Value>,
                 plain: &mut usize,
                 end: usize|
     -> Result<(), SubstitutionError> {
        out.push_str(&fill(&sql[*plain..end], |name| {
            bind_values.push(values.value(name)?.clone());
            Ok("?".to_string())
        })?);
        *plain = end;
        Ok(())
    };

    let mut k = 0;
    while k < tokens.len() {
        let t = &tokens[k];
        if is_mysql_string(sql, t) {
            let mut m = k + 1;
            while m < tokens.len() && is_mysql_string(sql, &tokens[m]) {
                m += 1;
            }
            let end = tokens[m - 1].end;
            // Only a `{{p}}` written in the literals counts (not an escaped
            // `\{{p}}`); the CONCAT text is the literals' text with the
            // escapes around them resolved.
            let run: Vec<(&str, Vec<Param<'_>>)> = tokens[k..m]
                .iter()
                .map(|t| {
                    let text = t.text(sql);
                    (text, token_params(text, true))
                })
                .collect();
            if run.iter().any(|(_, ps)| !ps.is_empty()) {
                let start = introducer_start(sql, &tokens, k).unwrap_or(t.start);
                flush(&mut out, &mut bind_values, &mut plain, start)?;
                let mut null = false;
                let mut filled = String::new();
                for (text, ps) in &run {
                    filled.push_str(&mysql_literal_text(text, ps, |name| {
                        Ok(concat_text(values.js(name)?).unwrap_or_else(|| {
                            null = true;
                            String::new()
                        }))
                    })?);
                }
                bind_values.push(if null {
                    Value::Null
                } else {
                    Value::Text(filled)
                });
                out.push('?');
            } else {
                flush(&mut out, &mut bind_values, &mut plain, t.start)?;
                out.push_str(&sql[t.start..end]);
            }
            plain = end;
            k = m;
            continue;
        }
        if t.kind == TokenKind::Comment || t.kind == TokenKind::Quoted {
            // A comment (or an executable comment's opener or `*/`) or a
            // backtick name.
            flush(&mut out, &mut bind_values, &mut plain, t.start)?;
            out.push_str(t.text(sql));
            plain = t.end;
        }
        k += 1;
    }
    flush(&mut out, &mut bind_values, &mut plain, sql.len())?;
    Ok(Substituted {
        sql: out,
        bind_values,
    })
}

/// The substituted SQL as it's written, spacing each value inlined in code
/// off its neighbours (fix 13, `Out` in `params-model.ts`).
#[derive(Default)]
struct Out {
    sql: String,
    /// The last output was a value ending in a word character: the next
    /// output gets a space before it if it starts with one or `$`.
    pending: bool,
}

impl Out {
    /// Template text, or a value's placeholder or text that isn't inlined in
    /// code.
    fn text(&mut self, t: &str) {
        if let Some(c) = t.chars().next() {
            if self.pending && (c.is_ascii_alphanumeric() || c == '_' || c == '$' || !c.is_ascii())
            {
                self.sql.push(' ');
            }
            self.pending = false;
        }
        self.sql.push_str(t);
    }

    /// A value inlined in code: a space before it when it's quoted and follows
    /// a quote (they'd merge through `''`), or, with `is_word` (SQL Server's
    /// and DuckDB's rule), when both sides are words. `inlined`: false for SQL
    /// Server's `N` prefix, which isn't a value.
    fn value(&mut self, t: &str, is_word: Option<fn(char) -> bool>, inlined: bool) {
        let last = self.sql.chars().next_back();
        let words = is_word.is_some_and(|w| last.is_some_and(w) && t.chars().next().is_some_and(w));
        let quotes = inlined && t.starts_with('\'') && matches!(last, Some('\'' | '"' | '`'));
        if words || quotes {
            self.sql.push(' ');
            self.pending = false;
        }
        self.text(t);
        self.pending = inlined
            && t.chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
    }
}

/// Copies SQL text outside literals, comments and names, with its parameters
/// inlined by `literal` and spaced off the words around them.
fn plain_text(
    out: &mut Out,
    text: &str,
    is_word: fn(char) -> bool,
    mut literal: impl FnMut(&str) -> Result<String, SubstitutionError>,
) -> Result<(), SubstitutionError> {
    let mut last = 0;
    for p in params(text) {
        out.text(&text[last..p.start]);
        let lit = literal(p.name)?;
        out.value(&lit, Some(is_word), true);
        last = p.end;
    }
    out.text(&text[last..]);
    Ok(())
}

/// Whether `c` continues a T-SQL word (identifier, keyword, number, `@var`).
fn is_tsql_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '@' | '#' | '$')
}

/// `mssqlLiteral`: a value as a T-SQL literal, strings as `N'…'`.
fn mssql_literal(v: Js<'_>) -> String {
    match v {
        Js::Null => "NULL".to_string(),
        Js::Decimal(s) => paren(plain_decimal(s)),
        Js::Str(_) => format!("N{}", escape_inline(v, false)),
        other => paren(escape_inline(other, false)),
    }
}

/// `mssqlLiteralText`: a value's text inside an existing literal.
fn mssql_literal_text(v: Js<'_>) -> String {
    match v {
        Js::Decimal(s) => plain_decimal(s),
        other => escape_inline(other, true),
    }
}

/// SQL Server (`substituteMssqlInline`): inlined, not bound. Strings become
/// `N'…'`; a `{{p}}` inside a `'…'` literal is filled into it, and a literal
/// that then holds non-ASCII text gets the `N` prefix. Comments and quoted
/// names are copied as they are.
fn substitute_mssql(sql: &str, values: &Values<'_>) -> Result<Substituted, SubstitutionError> {
    let tokens = scan(sql, SqlEngine::Mssql, ScanOptions::default());
    let mut out = Out::default();
    let mut plain = 0;
    for t in &tokens {
        if !matches!(t.kind, TokenKind::Comment | TokenKind::Quoted) {
            continue;
        }
        plain_text(&mut out, &sql[plain..t.start], is_tsql_word, |name| {
            Ok(mssql_literal(values.inline(name)?))
        })?;
        let text = t.text(sql);
        if t.kind == TokenKind::Quoted && text.starts_with('\'') && has_parameters(text) {
            let filled = fill(text, |name| Ok(mssql_literal_text(values.inline(name)?)))?;
            // Already N'…' (the N ends the text before, not a longer word)?
            let mut back = out.sql.chars().rev();
            let prefixed = matches!(back.next(), Some('N' | 'n'))
                && back.next().is_none_or(|c| !is_tsql_word(c));
            if !prefixed && !filled.is_ascii() {
                out.value("N", Some(is_tsql_word), false);
            }
            out.text(&filled);
        } else {
            out.text(text);
        }
        plain = t.end;
    }
    plain_text(&mut out, &sql[plain..], is_tsql_word, |name| {
        Ok(mssql_literal(values.inline(name)?))
    })?;
    Ok(Substituted {
        sql: out.sql,
        bind_values: Vec::new(),
    })
}

/// Whether `c` continues a DuckDB word (identifier, keyword, number).
fn is_duckdb_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '$')
}

/// `duckdbLiteral`: a value as a DuckDB literal.
fn duckdb_literal(v: Js<'_>) -> String {
    match v {
        Js::Decimal(s) => paren(plain_decimal(s)),
        other => paren(escape_inline(other, false)),
    }
}

/// `duckdbLiteralText`: a value's text inside an existing literal, `\`
/// doubled too in an `E'…'` literal.
fn duckdb_literal_text(v: Js<'_>, backslash_escapes: bool) -> String {
    let text = match v {
        Js::Decimal(s) => plain_decimal(s),
        other => escape_inline(other, true),
    };
    if backslash_escapes {
        text.replace('\\', "\\\\")
    } else {
        text
    }
}

/// DuckDB (`substituteDuckdbInline`): inlined, not bound. A `{{p}}` inside a
/// `'…'` literal is filled into it (`E'…'` doubles `\`); one inside a
/// `$tag$…$tag$` string gets the value's raw text. Comments and quoted names
/// are copied as they are.
fn substitute_duckdb(sql: &str, values: &Values<'_>) -> Result<Substituted, SubstitutionError> {
    let tokens = scan(sql, SqlEngine::Duckdb, ScanOptions::default());
    let mut out = Out::default();
    let mut plain = 0;
    for t in &tokens {
        if !matches!(t.kind, TokenKind::Comment | TokenKind::Quoted) {
            continue;
        }
        plain_text(&mut out, &sql[plain..t.start], is_duckdb_word, |name| {
            Ok(duckdb_literal(values.inline(name)?))
        })?;
        let text = t.text(sql);
        let filled = if t.kind == TokenKind::Comment || text.starts_with('"') {
            text.to_string()
        } else if let Some(tag) = dollar_tag(text) {
            fill_tag(values, text, tag)?
        } else {
            // '…', or E'…' with backslash escapes (an escaped `\{{p}}` stays).
            let escapes = !text.starts_with('\'');
            fill_params(text, &token_params(text, escapes), |name| {
                Ok(duckdb_literal_text(values.inline(name)?, escapes))
            })?
        };
        out.text(&filled);
        plain = t.end;
    }
    plain_text(&mut out, &sql[plain..], is_duckdb_word, |name| {
        Ok(duckdb_literal(values.inline(name)?))
    })?;
    Ok(Substituted {
        sql: out.sql,
        bind_values: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_number_formatting() {
        for (f, want) in [
            (1e21, "1e+21"),
            (1e20, "100000000000000000000"),
            (-0.0, "0"),
            (0.1, "0.1"),
            (1.5e-7, "1.5e-7"),
            (0.000001, "0.000001"),
            (-1.5, "-1.5"),
            (123456789012.0, "123456789012"),
            (f64::NAN, "NaN"),
            (f64::INFINITY, "Infinity"),
            (f64::NEG_INFINITY, "-Infinity"),
            (5e-324, "5e-324"),
            (f64::MAX, "1.7976931348623157e+308"),
        ] {
            assert_eq!(js_number(f), want, "{f:?}");
        }
    }

    #[test]
    fn plain_decimals() {
        for (text, want) in [
            ("1e5", "100000"),
            ("-1.5E-3", "-0.0015"),
            ("1.25e1", "12.5"),
            ("12.50", "12.50"),
            ("1e-3", "0.001"),
            ("+1e2", "100"),
            ("-0e0", "-0"),
            ("007e1", "70"),
            (".5e1", "5"),
            ("5.e0", "5"),
            (".e5", ".e5"),
            ("e5", "e5"),
            ("1e", "1e"),
            ("1e+", "1e+"),
            (" 1e2 ", "100"),
            ("1e99999999999999999999999", "1e99999999999999999999999"),
            ("1e-99999999999999999999999", "1e-99999999999999999999999"),
            ("", ""),
            ("-", "-"),
        ] {
            assert_eq!(plain_decimal(text), want, "{text:?}");
        }
    }

    #[test]
    fn param_regex() {
        fn names(s: &str) -> Vec<&str> {
            params(s).iter().map(|p| p.name).collect()
        }
        assert_eq!(names("{{{a}}} {{1a}} {{_b1}}{{c}}"), ["a", "_b1", "c"]);
        assert_eq!(names("{{a }} {{ a}} {{a}"), Vec::<&str>::new());
        assert_eq!(extract_parameters("{{a}} {{b}} {{a}}"), ["a", "b"]);
        assert!(!has_parameters("{{"));
    }

    #[test]
    fn introducers() {
        let bound = |sql: &str| {
            substitute(
                sql,
                &[("p".into(), Value::Text("x".into()))],
                SqlEngine::Mysql,
                false,
            )
            .unwrap()
            .sql
        };
        for (sql, want) in [
            ("SELECT N'{{p}}'", "SELECT ?"),
            ("SELECT n'{{p}}'", "SELECT ?"),
            ("SELECT N '{{p}}'", "SELECT N ?"),
            ("SELECT xN'{{p}}'", "SELECT xN?"),
            ("SELECT _utf8mb4 '{{p}}'", "SELECT ?"),
            ("SELECT _utf8mb4\n'{{p}}'", "SELECT ?"),
            ("SELECT _utf8mb4'{{p}}'", "SELECT ?"),
            ("SELECT x_utf8 '{{p}}'", "SELECT x_utf8 ?"),
            ("SELECT é_u '{{p}}'", "SELECT é_u ?"),
            ("SELECT _ '{{p}}'", "SELECT _ ?"),
            ("SELECT _u /* c */ '{{p}}'", "SELECT _u /* c */ ?"),
            ("N'{{p}}' '{{p}}'", "?"),
            ("SELECT '{{p' '}}'", "SELECT '{{p' '}}'"),
        ] {
            assert_eq!(bound(sql), want, "{sql:?}");
        }
    }

    #[test]
    fn map_semantics_and_refused_values() {
        let v = |pairs: &[(&str, Value)]| {
            pairs
                .iter()
                .map(|(n, v)| (n.to_string(), v.clone()))
                .collect::<Vec<_>>()
        };
        let s = substitute(
            "SELECT {{a}}, {{b}}",
            &v(&[("a", Value::Int(1)), ("a", Value::Int(2))]),
            SqlEngine::Postgres,
            false,
        )
        .unwrap();
        assert_eq!(s.bind_values, [Value::Int(2), Value::Null]);

        let bytes = v(&[("a", Value::Bytes(vec![1]))]);
        for e in SqlEngine::ALL {
            for force in [false, true] {
                let err = substitute("SELECT {{a}}", &bytes, e, force).unwrap_err();
                assert!(err.message.starts_with("The value for {{a}} is binary"));
                // Not used: no error.
                assert!(substitute("SELECT 1 -- {{a}}", &bytes, e, force).is_ok());
            }
        }
    }

    fn run(sql: &str, p: Value, e: SqlEngine, force: bool) -> Result<String, String> {
        substitute(sql, &[("p".into(), p)], e, force)
            .map(|s| s.sql)
            .map_err(|e| e.message)
    }

    #[test]
    fn decimals_are_checked() {
        for ok in ["1", "-1.5", "+1.", ".5", "1e5", "-.5E-3", "007"] {
            assert!(is_decimal(ok), "{ok}");
        }
        for bad in [
            "", "-", ".", "e5", ".e5", "1e", "1e+", "NaN", "Infinity", " 1", "1 ", "1; x", "0x1",
        ] {
            assert!(!is_decimal(bad), "{bad}");
        }
        let nan = Value::Decimal("NaN".into());
        let msg = "The value for {{p}} is not a finite decimal number";
        for e in [SqlEngine::Mssql, SqlEngine::Duckdb] {
            assert_eq!(run("SELECT {{p}}", nan.clone(), e, false).unwrap_err(), msg);
        }
        // Bound, it goes out as it is.
        assert_eq!(
            run("SELECT {{p}}", nan, SqlEngine::Postgres, false).unwrap(),
            "SELECT $1"
        );
    }

    #[test]
    fn integers_inline_as_digits() {
        let big = Value::Int(-9_007_199_254_740_993);
        assert_eq!(
            run("SELECT 1-{{p}}, '{{p}}'", big, SqlEngine::Postgres, true).unwrap(),
            "SELECT 1-(-9007199254740993), '-9007199254740993'"
        );
        let e18 = Value::Int(1_000_000_000_000_000_000);
        assert_eq!(
            run("SELECT {{p}}", e18, SqlEngine::Mysql, true).unwrap(),
            "SELECT 1000000000000000000"
        );
    }

    #[test]
    fn dollar_tag_edges() {
        let t = |s: &str| Value::Text(s.into());
        let formed = |tag: &str| {
            format!("The value for {{{{p}}}} forms the dollar-quote tag {tag} with the text next to it; use a different tag or a '…' string")
        };
        for e in [SqlEngine::Postgres, SqlEngine::Duckdb] {
            for force in [false, true] {
                for (sql, v, tag) in [
                    ("SELECT $$Cost: ${{p}}$$", "$ || 'INJ' --", "$$"),
                    ("SELECT $${{p}}$ tail$$", "x$", "$$"),
                    ("SELECT $tag$ $ta{{p}} $tag$", "g$ x", "$tag$"),
                    ("SELECT $tag$ {{p}}ag$ $tag$", "$t", "$tag$"),
                    ("SELECT $$ ${{p}}", "$x", "$$"),
                ] {
                    assert_eq!(
                        run(sql, t(v), e, force).unwrap_err(),
                        formed(tag),
                        "{sql} {v}"
                    );
                }
                // Next to the text, but no tag formed.
                assert_eq!(
                    run("SELECT $$Cost: ${{p}}$$", t("5"), e, force).unwrap(),
                    "SELECT $$Cost: $5$$"
                );
                assert_eq!(
                    run("SELECT $$ {{p}}", t("x$"), e, force).unwrap(),
                    "SELECT $$ x$"
                );
                // The value ends the string exactly where it ended: allowed.
                assert_eq!(
                    run("SELECT $t${{p}}$t$", t("$ta"), e, force).unwrap(),
                    "SELECT $t$$ta$t$"
                );
            }
        }
    }

    #[test]
    fn escaped_braces_in_backslash_strings() {
        let v = || Value::Text("' OR 1=1 --".into());
        let sql = r"SELECT E'\{{p}}', E'\\{{p}}', E'\\\{{p}}'";
        assert_eq!(
            run(sql, v(), SqlEngine::Postgres, true).unwrap(),
            r"SELECT E'\{{p}}', E'\\'' OR 1=1 --', E'\\\{{p}}'"
        );
        assert_eq!(
            run(sql, v(), SqlEngine::Postgres, false).unwrap(),
            r"SELECT E'\{{p}}', E'\\' || $1 || E'', E'\\\{{p}}'"
        );
        assert_eq!(
            run(sql, v(), SqlEngine::Duckdb, false).unwrap(),
            r"SELECT E'\{{p}}', E'\\'' OR 1=1 --', E'\\\{{p}}'"
        );
        // Plain '…' takes no backslash escapes on Postgres and DuckDB.
        assert_eq!(
            run(r"SELECT '\{{p}}'", v(), SqlEngine::Duckdb, false).unwrap(),
            r"SELECT '\'' OR 1=1 --'"
        );
        for e in [SqlEngine::Mysql, SqlEngine::Mariadb] {
            let sql = r#"SELECT '\{{p}}', "\{{p}}", '\\{{p}}'"#;
            assert_eq!(
                run(sql, v(), e, true).unwrap(),
                r#"SELECT '\{{p}}', "\{{p}}", '\\'' OR 1=1 --'"#
            );
            let s = substitute(sql, &[("p".into(), v())], e, false).unwrap();
            assert_eq!(s.sql, r#"SELECT '\{{p}}', "\{{p}}", ?"#);
            assert_eq!(s.bind_values, [Value::Text(r"\' OR 1=1 --".into())]);
        }
    }

    #[test]
    fn values_are_spaced_off_their_neighbours() {
        let t = |s: &str| Value::Text(s.into());
        // A quoted value after a quote would merge with that literal.
        for e in [SqlEngine::Postgres, SqlEngine::Duckdb, SqlEngine::Sqlite] {
            let force = e != SqlEngine::Duckdb;
            assert_eq!(
                run(r"SELECT E'a'{{p}}", t(r"\') OR 1=1 --"), e, force).unwrap(),
                r"SELECT E'a' '\'') OR 1=1 --'"
            );
        }
        assert_eq!(
            run("SELECT 'a'{{p}}", t("x"), SqlEngine::Mssql, false).unwrap(),
            "SELECT 'a'N'x'"
        );
        // A number, NULL or boolean before a word character or `$`.
        for (v, text) in [
            (Value::Null, "NULL"),
            (Value::Int(1), "1"),
            (Value::Bool(true), "1"),
        ] {
            for e in SqlEngine::ALL {
                let force = !matches!(e, SqlEngine::Mssql | SqlEngine::Duckdb);
                assert_eq!(
                    run(
                        "SELECT {{p}}$$ x $$, {{p}}AS a, {{p}}{{p}}",
                        v.clone(),
                        e,
                        force
                    )
                    .unwrap(),
                    format!("SELECT {text} $$ x $$, {text} AS a, {text} {text}"),
                    "{e}"
                );
            }
        }
        // Closed values, and a `$n` placeholder, are left as they were.
        assert_eq!(
            run(
                "SELECT {{p}}AS a",
                Value::Int(-1),
                SqlEngine::Postgres,
                true
            )
            .unwrap(),
            "SELECT (-1)AS a"
        );
        assert_eq!(
            run("SELECT {{p}}AS a", t("s"), SqlEngine::Postgres, true).unwrap(),
            "SELECT 's'AS a"
        );
        assert_eq!(
            run("SELECT {{p}}AS a", t("s"), SqlEngine::Postgres, false).unwrap(),
            "SELECT $1AS a"
        );
    }
}
