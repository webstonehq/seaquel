//! Helpers the engines' introspection parsers share: reading catalog rows by
//! column name, JavaScript's number coercion (the parsers port TypeScript
//! that read rows as objects), and building EXPLAIN plan nodes.
//!
//! Engine-specific cell readings (Postgres' printed scalars, MySQL's
//! byte-array catalog text) stay in those engine crates, as extension traits
//! on [`Row`] whose method names differ from `Row`'s own (an inherent method
//! wins over a trait method of the same name).

use seaquel_types::{ExplainPlanNode, QueryResult, SchemaColumn, SchemaIndex, Value};

static NULL: Value = Value::Null;

/// One row of a result, read by column name like the TypeScript row objects.
#[derive(Clone, Copy)]
pub struct Row<'a> {
    pub columns: &'a [String],
    pub cells: &'a [Value],
}

/// The rows of `result`, in order.
pub fn rows(result: &QueryResult) -> impl Iterator<Item = Row<'_>> {
    result.rows.iter().map(move |cells| Row {
        columns: &result.columns,
        cells,
    })
}

impl<'a> Row<'a> {
    /// A cell by column name; a missing column reads as `Null` (TS
    /// `undefined`). With duplicate names, the first wins.
    pub fn get(&self, column: &str) -> &'a Value {
        self.columns
            .iter()
            .position(|c| c == column)
            .and_then(|i| self.cells.get(i))
            .unwrap_or(&NULL)
    }

    /// Whether the result has `column` (TS `column in row`).
    pub fn has(&self, column: &str) -> bool {
        self.columns.iter().any(|c| c == column)
    }

    /// A text cell; anything else is `None`.
    pub fn str(&self, column: &str) -> Option<&'a str> {
        match self.get(column) {
            Value::Text(s) => Some(s),
            _ => None,
        }
    }

    /// A text cell, `""` for anything else.
    pub fn text(&self, column: &str) -> String {
        self.str(column).unwrap_or_default().to_string()
    }

    /// A non-empty text cell (TS `row[column] || undefined` on a text column).
    pub fn non_empty(&self, column: &str) -> Option<&'a str> {
        self.str(column).filter(|s| !s.is_empty())
    }

    /// JS truthiness of the cell (`!!row[column]`, see [`truthy`]).
    pub fn truthy(&self, column: &str) -> bool {
        truthy(self.get(column))
    }

    /// An integer cell; anything else is `None`.
    pub fn int(&self, column: &str) -> Option<i64> {
        match self.get(column) {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// TS `Number(cell) || 0`, truncated (see [`number_or_zero`]).
    pub fn number(&self, column: &str) -> i64 {
        number_or_zero(self.get(column))
    }
}

/// JS truthiness of a decoded cell (`a || b` picks `b` when `a` is falsy).
/// Decimals, bytes, JSON and arrays decode to objects in TS, which are always
/// truthy.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Int(i) => *i != 0,
        Value::Float(f) => *f != 0.0 && !f.is_nan(),
        Value::Text(s) => !s.is_empty(),
        Value::Decimal(_) | Value::Bytes(_) | Value::Json(_) | Value::Array(_) => true,
    }
}

/// TS `Number(x) || 0`, truncated to an integer: `Int`, `Bool`, `Float`,
/// `Decimal` and numeric `Text` (read as JS `Number(string)` does) count;
/// `Null`, NaN, ±Infinity and anything unparsable are 0.
pub fn number_or_zero(v: &Value) -> i64 {
    let f = match v {
        Value::Int(i) => return *i,
        Value::Bool(b) => return i64::from(*b),
        Value::Float(f) => *f,
        Value::Decimal(s) | Value::Text(s) => {
            if let Ok(i) = js_trim(s).parse::<i64>() {
                return i;
            }
            js_string_to_number(s)
        }
        _ => f64::NAN,
    };
    if f.is_finite() {
        // `as` saturates beyond the i64 range.
        f.trunc() as i64
    } else {
        0
    }
}

/// `String.prototype.trim`: ECMAScript WhiteSpace and LineTerminator. That is
/// Rust's `White_Space` minus U+0085 (NEL), plus U+FEFF (BOM).
pub fn js_trim(s: &str) -> &str {
    s.trim_matches(|c: char| (c.is_whitespace() && c != '\u{0085}') || c == '\u{FEFF}')
}

/// JS `Number(string)`: trimmed, `""` is 0, `0x`/`0o`/`0b` prefixes,
/// `Infinity`; anything else unparsable (including Rust-only spellings such
/// as `inf` or `nan`) is NaN.
pub fn js_string_to_number(s: &str) -> f64 {
    let t = js_trim(s);
    if t.is_empty() {
        return 0.0;
    }
    for (prefix, radix) in [
        ("0x", 16),
        ("0X", 16),
        ("0o", 8),
        ("0O", 8),
        ("0b", 2),
        ("0B", 2),
    ] {
        if let Some(digits) = t.strip_prefix(prefix) {
            return u64::from_str_radix(digits, radix).map_or(f64::NAN, |n| n as f64);
        }
    }
    let unsigned = t.trim_start_matches(['+', '-']);
    match unsigned {
        "Infinity" if t.len() - unsigned.len() <= 1 => {
            if t.starts_with('-') {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            }
        }
        _ if unsigned.starts_with(|c: char| c.is_ascii_digit() || c == '.')
            && t.len() - unsigned.len() <= 1
            && unsigned
                .chars()
                .all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-')) =>
        {
            t.parse::<f64>().unwrap_or(f64::NAN)
        }
        _ => f64::NAN,
    }
}

/// `node-0`, `node-1`, … in creation order (TS `makeNodeIdFactory`).
#[derive(Debug, Default)]
pub struct Ids(usize);

impl Ids {
    pub fn new() -> Self {
        Self(0)
    }

    /// The next id. Not an `Iterator`: it never ends.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> String {
        let id = format!("node-{}", self.0);
        self.0 += 1;
        id
    }
}

/// A plan node with only its type set and an empty id: fill in the rest
/// with `ExplainPlanNode { id: ids.next(), …, ..node("Scan") }`.
pub fn node(node_type: impl Into<String>) -> ExplainPlanNode {
    ExplainPlanNode {
        id: String::new(),
        node_type: node_type.into(),
        relation_name: None,
        alias: None,
        startup_cost: None,
        total_cost: None,
        plan_rows: None,
        plan_width: None,
        actual_startup_time: None,
        actual_total_time: None,
        actual_rows: None,
        actual_loops: None,
        filter: None,
        index_name: None,
        index_cond: None,
        join_type: None,
        hash_cond: None,
        sort_key: None,
        children: vec![],
    }
}

/// TS `formatBytes`: `parseFloat((bytes / 1024 ** i).toFixed(2)) + " " + unit`,
/// `"0 bytes"` for 0. Beyond TB the TS printed `undefined` as the unit; this
/// stays in TB. (A copy of the SQLite port's, shared from DuckDB on.)
pub fn format_bytes(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    if bytes == 0 {
        return "0 bytes".to_string();
    }
    let b = bytes as f64;
    let i = (b.ln() / 1024f64.ln()).floor();
    // A negative size has no logarithm (NaN): JS indexes `sizes[NaN]`.
    let i = if i.is_nan() {
        0
    } else {
        (i.max(0.0) as usize).min(UNITS.len() - 1)
    };
    let scaled = b / 1024f64.powi(i as i32);
    format!("{} {}", js_to_fixed_2_trimmed(scaled), UNITS[i])
}

/// JS `parseFloat(x.toFixed(2))` as a string, for finite `x` below 1e21:
/// `toFixed` rounds the exact binary value half up (Rust's `{:.2}` rounds
/// ties to even: 1.125 → "1.12", JS "1.13"), then `parseFloat` and `String`
/// drop trailing zeros.
fn js_to_fixed_2_trimmed(x: f64) -> String {
    // Exact decimal expansion: every f64 here has at most 52 fractional bits.
    let exact = format!("{:.60}", x.abs());
    let (int, frac) = exact.split_once('.').expect("fixed notation");
    let mut digits: Vec<u8> = format!("{int}{}", &frac[..2]).into_bytes();
    let rest = &frac[2..];
    if rest.as_bytes()[0] >= b'5' {
        // Round half up on the digit string.
        let mut i = digits.len();
        loop {
            if i == 0 {
                digits.insert(0, b'1');
                break;
            }
            i -= 1;
            if digits[i] == b'9' {
                digits[i] = b'0';
            } else {
                digits[i] += 1;
                break;
            }
        }
    }
    let s = String::from_utf8(digits).expect("ascii digits");
    let (int, frac) = s.split_at(s.len() - 2);
    let int = int.trim_start_matches('0');
    let int = if int.is_empty() { "0" } else { int };
    let frac = frac.trim_end_matches('0');
    let sign = if x < 0.0 && (int != "0" || !frac.is_empty()) {
        "-"
    } else {
        ""
    };
    if frac.is_empty() {
        format!("{sign}{int}")
    } else {
        format!("{sign}{int}.{frac}")
    }
}

/// Sets `is_unique` and `in_unique_constraint` on `columns` from their
/// table's `indexes`, for engines whose column query doesn't report UNIQUE
/// (Task 18): every column of a unique index is `in_unique_constraint`, and
/// the column of a single-column one is `is_unique` unless it is the primary
/// key. The primary key's own index (a unique index on exactly the primary
/// key columns) doesn't count. A UNIQUE constraint and a unique index count
/// alike: each engine's ALTER TABLE drops either when the box is unchecked.
pub fn apply_unique_indexes(columns: &mut [SchemaColumn], indexes: &[SchemaIndex]) {
    let mut pk: Vec<&str> = columns
        .iter()
        .filter(|c| c.is_primary_key)
        .map(|c| c.name.as_str())
        .collect();
    pk.sort_unstable();
    let mut flags: Vec<(String, bool)> = Vec::new();
    for idx in indexes.iter().filter(|i| i.unique && !i.columns.is_empty()) {
        let mut cols: Vec<&str> = idx.columns.iter().map(String::as_str).collect();
        cols.sort_unstable();
        if cols == pk {
            continue;
        }
        let single = idx.columns.len() == 1;
        flags.extend(idx.columns.iter().map(|c| (c.clone(), single)));
    }
    for (name, single) in flags {
        if let Some(c) = columns.iter_mut().find(|c| c.name == name) {
            c.in_unique_constraint = true;
            if single && !c.is_primary_key {
                c.is_unique = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_read_cells_by_name() {
        let result = QueryResult {
            columns: vec!["a".into(), "b".into(), "a".into()],
            rows: vec![vec![Value::Int(1), Value::from("x"), Value::Int(3)]],
        };
        let r = rows(&result).next().unwrap();
        assert_eq!(r.get("a"), &Value::Int(1));
        assert_eq!(r.get("missing"), &Value::Null);
        assert!(r.has("b") && !r.has("c"));
        assert_eq!(r.str("b"), Some("x"));
        assert_eq!(r.str("a"), None);
        assert_eq!(r.int("a"), Some(1));
        assert_eq!(r.number("b"), 0);
        assert_eq!(r.text("b"), "x");
        assert_eq!(r.text("a"), "");
        assert_eq!(r.non_empty("b"), Some("x"));
        assert_eq!(r.non_empty("a"), None);
        assert!(r.truthy("a") && r.truthy("b") && !r.truthy("missing"));
    }

    #[test]
    fn truthiness_as_js_reads_it() {
        for v in [
            Value::Bool(true),
            Value::Int(-1),
            Value::Float(0.5),
            Value::from("0"),
            Value::Decimal("0".into()),
            Value::Bytes(vec![]),
            Value::Array(vec![]),
        ] {
            assert!(truthy(&v), "{v:?}");
        }
        for v in [
            Value::Null,
            Value::Bool(false),
            Value::Int(0),
            Value::Float(0.0),
            Value::Float(f64::NAN),
            Value::from(""),
        ] {
            assert!(!truthy(&v), "{v:?}");
        }
    }

    #[test]
    fn numbers_as_js_reads_them() {
        let n = |v: Value| number_or_zero(&v);
        assert_eq!(n(Value::Int(7)), 7);
        assert_eq!(n(Value::Bool(true)), 1);
        assert_eq!(n(Value::Float(2.9)), 2);
        assert_eq!(n(Value::Float(f64::NAN)), 0);
        assert_eq!(n(Value::Float(f64::INFINITY)), 0);
        assert_eq!(n(Value::Decimal("12.5".into())), 12);
        assert_eq!(n(Value::from(" 42 ")), 42);
        assert_eq!(n(Value::from("")), 0);
        assert_eq!(n(Value::from("  ")), 0);
        assert_eq!(n(Value::from("1e3")), 1000);
        assert_eq!(n(Value::from("+5")), 5);
        assert_eq!(n(Value::from("abc")), 0);
        assert_eq!(n(Value::from("inf")), 0);
        assert_eq!(n(Value::from("0x10")), 16);
        assert_eq!(n(Value::Null), 0);
        assert_eq!(n(Value::Float(1e300)), i64::MAX);
    }

    #[test]
    fn js_numbers() {
        assert_eq!(js_string_to_number(" 4 "), 4.0);
        assert_eq!(js_string_to_number(""), 0.0);
        assert_eq!(js_string_to_number("0x1A"), 26.0);
        assert_eq!(js_string_to_number("1e3"), 1000.0);
        assert_eq!(js_string_to_number("-Infinity"), f64::NEG_INFINITY);
        assert!(js_string_to_number("inf").is_nan());
        assert!(js_string_to_number("nan").is_nan());
        assert!(js_string_to_number("x").is_nan());
        assert!(js_string_to_number("1.2.3").is_nan());
        // JS trim: BOM is whitespace, NEL is not.
        assert_eq!(js_trim("\u{FEFF}\t1\n"), "1");
        assert_eq!(js_trim("\u{0085}1"), "\u{0085}1");
    }

    #[test]
    fn bytes_as_js_formats_them() {
        assert_eq!(format_bytes(0), "0 bytes");
        assert_eq!(format_bytes(1023), "1023 bytes");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(6029312), "5.75 MB");
        assert_eq!(format_bytes(1610612736), "1.5 GB");
        // Half up on the exact value, as toFixed: 1.125 KB.
        assert_eq!(format_bytes(1152), "1.13 KB");
    }

    #[test]
    fn ids_count_from_zero() {
        let mut ids = Ids::new();
        assert_eq!((ids.next(), ids.next()), ("node-0".into(), "node-1".into()));
        let n = node("Scan");
        assert_eq!((n.id.as_str(), n.node_type.as_str()), ("", "Scan"));
        assert!(n.children.is_empty() && n.filter.is_none());
    }

    fn column(name: &str, pk: bool) -> SchemaColumn {
        SchemaColumn {
            name: name.into(),
            ty: "int".into(),
            cast_type: None,
            nullable: !pk,
            default_value: None,
            is_primary_key: pk,
            is_foreign_key: false,
            foreign_key_ref: None,
            is_unique: false,
            in_unique_constraint: false,
            collation: None,
        }
    }

    fn index(name: &str, columns: &[&str], unique: bool) -> SchemaIndex {
        SchemaIndex {
            name: name.into(),
            columns: columns.iter().map(|c| c.to_string()).collect(),
            unique,
            ty: "btree".into(),
        }
    }

    #[test]
    fn unique_flags_from_indexes() {
        let mut cols = vec![
            column("id", true),
            column("email", false),
            column("a", false),
            column("b", false),
            column("tag", false),
        ];
        apply_unique_indexes(
            &mut cols,
            &[
                index("t_pkey", &["id"], true),
                index("t_email_key", &["email"], true),
                index("t_a_b_key", &["b", "a"], true),
                index("t_tag_idx", &["tag"], false),
                // A second unique index on the primary key column.
                index("t_id_uq", &["id", "email"], true),
            ],
        );
        let got: Vec<_> = cols
            .iter()
            .map(|c| (c.name.as_str(), c.is_unique, c.in_unique_constraint))
            .collect();
        assert_eq!(
            got,
            vec![
                ("id", false, true),
                ("email", true, true),
                ("a", false, true),
                ("b", false, true),
                ("tag", false, false),
            ]
        );
    }
}
