//! Result formatting: cells as JSON the way the app shows them, and an
//! EXPLAIN plan as text.
//!
//! Cells follow `cellText` in `src/lib/values.ts`, applied to what the GUI
//! holds after `decodeCell`. A cell JavaScript holds as a plain JSON value
//! (null, a boolean, text, a number that is exact in a `number`) stays that
//! JSON value, so a model still tells `null` from `""` and `1` from `"1"`.
//! Every other cell (what the wire tags, and arrays) becomes `cellText`'s
//! string:
//!
//! | cell                          | JSON                                   |
//! |-------------------------------|----------------------------------------|
//! | `Int` beyond ±(2^53−1)        | its digits (a `bigint`'s `String`)     |
//! | `Float` NaN, ±inf             | `"NaN"`, `"Infinity"`, `"-Infinity"`   |
//! | `Decimal`                     | its text (`SqlDecimal.toString`)       |
//! | `Bytes`                       | `\x` and lowercase hex (`toHex`)        |
//! | `Json` object                 | `JSON.stringify` text                  |
//! | `Json` array                  | elements' `cellText` joined with `,`   |
//! | `Json` string / number / bool | the string / `String(n)` / `"true"`    |
//! | `Json` null                   | `""`                                   |
//! | `Array`                       | elements' `cellText` joined with `,`   |
//!
//! `String(n)` for a JavaScript number is `ryu_js`, and a JSON cell's object
//! keys come out in the order `JSON.parse` gives them (array-index keys
//! first, ascending).

use std::fmt::Write as _;

use seaquel_types::{ExplainPlanNode, ExplainResult, Value, MAX_SAFE_INTEGER};
use serde_json::{Map, Value as Json};

/// The most bytes of text one cell renders to. Longer text is cut (at a
/// character boundary) and the cell becomes a [`truncated_cell`] object.
pub const MAX_CELL_BYTES: usize = 64 * 1024;

/// One cell of a `run_query` result, with its text cut at
/// [`MAX_CELL_BYTES`]. Also returns whether it was cut.
pub fn cell(v: &Value) -> (Json, bool) {
    match v {
        Value::Null => (Json::Null, false),
        Value::Bool(b) => (Json::Bool(*b), false),
        Value::Int(i) if i.unsigned_abs() <= MAX_SAFE_INTEGER as u64 => (Json::from(*i), false),
        Value::Float(f) if f.is_finite() => (Json::from(*f), false),
        Value::Text(s) if s.len() <= MAX_CELL_BYTES => (Json::String(s.clone()), false),
        other => {
            let mut out = Capped::new(MAX_CELL_BYTES);
            write_cell_text(other, &mut out);
            if out.total > MAX_CELL_BYTES {
                (truncated_cell(out.text, out.total), true)
            } else {
                (Json::String(out.text), false)
            }
        }
    }
}

/// A cell whose text was cut: `{"truncated": true, "bytes": <the whole
/// text's length in UTF-8 bytes>, "text": <its first bytes>}`. No other cell
/// is an object (JSON cells render as text), so it can't be mistaken for one.
pub fn truncated_cell(text: String, bytes: usize) -> Json {
    serde_json::json!({ "truncated": true, "bytes": bytes, "text": text })
}

/// `cellText` of a decoded cell.
pub fn cell_text(v: &Value) -> String {
    let mut out = Capped::new(usize::MAX);
    write_cell_text(v, &mut out);
    out.text
}

/// Text that keeps at most `cap` bytes, cut at a character boundary, while
/// counting every byte pushed (`total`), so a huge cell costs no more memory
/// than the cap.
struct Capped {
    text: String,
    total: usize,
    cap: usize,
}

impl Capped {
    fn new(cap: usize) -> Self {
        Self {
            text: String::new(),
            total: 0,
            cap,
        }
    }

    fn push_str(&mut self, s: &str) {
        self.total = self.total.saturating_add(s.len());
        let room = self.cap.saturating_sub(self.text.len());
        if s.len() <= room {
            self.text.push_str(s);
        } else if room > 0 {
            let mut end = room;
            while !s.is_char_boundary(end) {
                end -= 1;
            }
            self.text.push_str(&s[..end]);
            // Nothing more fits, even a shorter character.
            self.cap = self.text.len();
        }
    }

    fn push(&mut self, c: char) {
        self.push_str(c.encode_utf8(&mut [0; 4]));
    }
}

impl std::fmt::Write for Capped {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.push_str(s);
        Ok(())
    }
}

/// serde_json writes a string literal as whole `&str` pieces (text runs and
/// escapes), so each piece is UTF-8 on its own.
impl std::io::Write for Capped {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.push_str(&String::from_utf8_lossy(buf));
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn write_cell_text(v: &Value, out: &mut Capped) {
    match v {
        Value::Null => {}
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Int(i) => {
            let _ = write!(out, "{i}");
        }
        Value::Float(f) => out.push_str(&js_number(*f)),
        Value::Decimal(s) | Value::Text(s) => out.push_str(s),
        Value::Bytes(b) => to_hex_into(b, out),
        Value::Json(j) => json_cell_text(j, out),
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_cell_text(item, out);
            }
        }
    }
}

/// `cellText` of a JSON cell's value (what `decodeCell` leaves for a `json`
/// tag).
fn json_cell_text(j: &Json, out: &mut Capped) {
    match j {
        Json::Null => {}
        Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Json::Number(n) => out.push_str(&js_number(n.as_f64().unwrap_or(f64::NAN))),
        Json::String(s) => out.push_str(s),
        Json::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                json_cell_text(item, out);
            }
        }
        Json::Object(map) => stringify_object(map, out),
    }
}

/// JavaScript's `String(n)` for a number.
fn js_number(f: f64) -> String {
    ryu_js::Buffer::new().format(f).to_string()
}

/// `toHex` in `values.ts`: `\x` then two lowercase hex digits per byte.
pub fn to_hex(bytes: &[u8]) -> String {
    let mut out = Capped::new(usize::MAX);
    to_hex_into(bytes, &mut out);
    out.text
}

fn to_hex_into(bytes: &[u8], out: &mut Capped) {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    out.push_str("\\x");
    // In chunks, so a long value isn't formatted byte by byte past the cap.
    let mut chunk = String::with_capacity(512);
    for piece in bytes.chunks(256) {
        chunk.clear();
        for b in piece {
            chunk.push(DIGITS[usize::from(b >> 4)] as char);
            chunk.push(DIGITS[usize::from(b & 0xf)] as char);
        }
        out.push_str(&chunk);
    }
}

/// `JSON.stringify` of a value `JSON.parse` produced.
fn stringify(j: &Json, out: &mut Capped) {
    match j {
        Json::Null => out.push_str("null"),
        Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        // A JSON number is finite, and JSON.parse rounds it to a double.
        Json::Number(n) => out.push_str(&js_number(n.as_f64().unwrap_or(0.0))),
        Json::String(s) => push_json_string(s, out),
        Json::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                stringify(item, out);
            }
            out.push(']');
        }
        Json::Object(map) => stringify_object(map, out),
    }
}

fn stringify_object(map: &Map<String, Json>, out: &mut Capped) {
    // A JavaScript object lists array-index keys first, ascending, then the
    // rest in insertion order.
    let mut index_keys: Vec<(u32, &String, &Json)> = Vec::new();
    let mut other_keys: Vec<(&String, &Json)> = Vec::new();
    for (k, v) in map {
        match array_index(k) {
            Some(i) => index_keys.push((i, k, v)),
            None => other_keys.push((k, v)),
        }
    }
    index_keys.sort_by_key(|(i, _, _)| *i);
    out.push('{');
    let entries = index_keys
        .into_iter()
        .map(|(_, k, v)| (k, v))
        .chain(other_keys);
    for (n, (k, v)) in entries.enumerate() {
        if n > 0 {
            out.push(',');
        }
        push_json_string(k, out);
        out.push(':');
        stringify(v, out);
    }
    out.push('}');
}

/// A key that is an array index: the canonical decimal text of an integer
/// below 2^32 − 1.
fn array_index(k: &str) -> Option<u32> {
    if k.is_empty() || (k.len() > 1 && k.starts_with('0')) {
        return None;
    }
    if !k.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    k.parse::<u32>().ok().filter(|i| *i < u32::MAX)
}

/// A JSON string literal. serde_json escapes the same characters
/// `JSON.stringify` does for text without lone surrogates (which a Rust
/// `String` can't hold): `"`, `\`, and control characters, with the short
/// forms for `\b \t \n \f \r` and lowercase `\u00XX` for the rest.
fn push_json_string(s: &str, out: &mut Capped) {
    let _ = serde_json::to_writer(out, s);
}

/// The size of `value` as compact JSON, without building the text.
pub fn json_len(value: &Json) -> usize {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0 += buf.len();
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    let _ = serde_json::to_writer(&mut count, value);
    count.0
}

/// An EXPLAIN result as indented text, one node per line with its
/// estimates, and its conditions on the lines below it.
pub fn explain_text(result: &ExplainResult) -> String {
    let mut out = String::new();
    explain_node(&result.plan, 0, &mut out);
    if result.planning_time > 0.0 {
        let _ = writeln!(out, "Planning time: {} ms", js_number(result.planning_time));
    }
    if let Some(t) = result.execution_time {
        let _ = writeln!(out, "Execution time: {} ms", js_number(t));
    }
    out.truncate(out.trim_end().len());
    out
}

fn explain_node(node: &ExplainPlanNode, depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    let _ = write!(out, "{indent}{}", node.node_type);
    if let Some(rel) = &node.relation_name {
        let _ = write!(out, " on {rel}");
        if let Some(alias) = node.alias.as_ref().filter(|a| *a != rel) {
            let _ = write!(out, " {alias}");
        }
    }
    if let Some(index) = &node.index_name {
        let _ = write!(out, " using {index}");
    }
    let mut est = Vec::new();
    match (node.startup_cost, node.total_cost) {
        (Some(s), Some(t)) => est.push(format!("cost={}..{}", fixed2(s), fixed2(t))),
        (None, Some(t)) => est.push(format!("cost={}", fixed2(t))),
        _ => {}
    }
    if let Some(rows) = node.plan_rows {
        est.push(format!("rows={}", js_number(rows)));
    }
    if let Some(width) = node.plan_width {
        est.push(format!("width={width}"));
    }
    if !est.is_empty() {
        let _ = write!(out, " ({})", est.join(" "));
    }
    out.push('\n');
    let detail = format!("{indent}    ");
    for (label, value) in [
        ("Join type", &node.join_type),
        ("Index cond", &node.index_cond),
        ("Hash cond", &node.hash_cond),
        ("Filter", &node.filter),
    ] {
        if let Some(value) = value {
            let _ = writeln!(out, "{detail}{label}: {value}");
        }
    }
    if let Some(keys) = node.sort_key.as_ref().filter(|k| !k.is_empty()) {
        let _ = writeln!(out, "{detail}Sort key: {}", keys.join(", "));
    }
    for child in &node.children {
        explain_node(child, depth + 1, out);
    }
}

fn fixed2(f: f64) -> String {
    format!("{f:.2}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cell(v: &Value) -> Json {
        let (json, cut) = super::cell(v);
        assert!(!cut);
        json
    }

    #[test]
    fn plain_cells_stay_json() {
        assert_eq!(cell(&Value::Null), Json::Null);
        assert_eq!(cell(&Value::Bool(true)), json!(true));
        assert_eq!(cell(&Value::Text("".into())), json!(""));
        assert_eq!(cell(&Value::Int(MAX_SAFE_INTEGER)), json!(MAX_SAFE_INTEGER));
        assert_eq!(
            cell(&Value::Int(-MAX_SAFE_INTEGER)),
            json!(-MAX_SAFE_INTEGER)
        );
        assert_eq!(cell(&Value::Float(1.5)), json!(1.5));
    }

    #[test]
    fn tagged_cells_are_cell_text() {
        assert_eq!(
            cell(&Value::Int(MAX_SAFE_INTEGER + 1)),
            json!("9007199254740992")
        );
        assert_eq!(cell(&Value::Int(i64::MIN)), json!("-9223372036854775808"));
        assert_eq!(cell(&Value::Float(f64::NAN)), json!("NaN"));
        assert_eq!(cell(&Value::Float(f64::INFINITY)), json!("Infinity"));
        assert_eq!(cell(&Value::Float(f64::NEG_INFINITY)), json!("-Infinity"));
        assert_eq!(cell(&Value::Decimal("12.50".into())), json!("12.50"));
        assert_eq!(
            cell(&Value::Bytes(vec![0, 1, 0xab, 0xff])),
            json!("\\x0001abff")
        );
        assert_eq!(cell(&Value::Bytes(vec![])), json!("\\x"));
    }

    #[test]
    fn json_cells_follow_cell_text() {
        let j = |v: Json| cell(&Value::Json(v));
        assert_eq!(
            j(json!({"b": 1, "a": [1, "x", null]})),
            json!(r#"{"a":[1,"x",null],"b":1}"#)
        );
        assert_eq!(
            j(json!({"01": 4, "10": 2, "2": 3, "b": 1})),
            json!(r#"{"2":3,"10":2,"01":4,"b":1}"#)
        );
        assert_eq!(
            j(json!([1, "a", null, true, {"k": 1.0}])),
            json!(r#"1,a,,true,{"k":1}"#)
        );
        assert_eq!(j(json!("text")), json!("text"));
        assert_eq!(j(json!(1e21)), json!("1e+21"));
        assert_eq!(j(json!(2.0)), json!("2"));
        assert_eq!(j(json!(false)), json!("false"));
        assert_eq!(j(Json::Null), json!(""));
        assert_eq!(
            j(json!({"s": "q\"\n\u{1}"})),
            json!("{\"s\":\"q\\\"\\n\\u0001\"}")
        );
    }

    #[test]
    fn arrays_join_their_elements_cell_text() {
        let a = Value::Array(vec![
            Value::Int(1),
            Value::Null,
            Value::Float(0.1),
            Value::Float(f64::NAN),
            Value::Bool(false),
            Value::Decimal("1.0".into()),
            Value::Bytes(vec![1]),
            Value::Json(json!({"k": [1]})),
            Value::Array(vec![Value::Text("x".into()), Value::Text("y".into())]),
        ]);
        assert_eq!(
            cell(&a),
            json!(r#"1,,0.1,NaN,false,1.0,\x01,{"k":[1]},x,y"#)
        );
    }

    #[test]
    fn long_cells_are_cut_and_marked() {
        let (j, cut) = super::cell(&Value::Text("x".repeat(MAX_CELL_BYTES)));
        assert!(!cut);
        assert_eq!(j.as_str().unwrap().len(), MAX_CELL_BYTES);

        // Cut at a character boundary: 'é' is two bytes.
        let long = format!("a{}", "é".repeat(MAX_CELL_BYTES));
        let (j, cut) = super::cell(&Value::Text(long.clone()));
        assert!(cut);
        assert_eq!(j["truncated"], true);
        assert_eq!(j["bytes"], long.len());
        let text = j["text"].as_str().unwrap();
        assert_eq!(text.len(), MAX_CELL_BYTES - 1);
        assert!(long.starts_with(text));

        let (j, cut) = super::cell(&Value::Bytes(vec![0xab; MAX_CELL_BYTES]));
        assert!(cut);
        assert_eq!(j["bytes"], 2 + 2 * MAX_CELL_BYTES);
        assert!(j["text"].as_str().unwrap().starts_with("\\xabab"));
        assert_eq!(j["text"].as_str().unwrap().len(), MAX_CELL_BYTES);

        let big = Value::Json(json!({ "k": "q\"".repeat(MAX_CELL_BYTES) }));
        let (j, cut) = super::cell(&big);
        assert!(cut);
        assert_eq!(j["bytes"], cell_text(&big).len());
        assert!(j["text"].as_str().unwrap().starts_with("{\"k\":\"q\\\"q"));

        let arr = Value::Array(vec![Value::Text("y".repeat(40_000)); 2]);
        let (j, cut) = super::cell(&arr);
        assert!(cut);
        assert_eq!(j["bytes"], 80_001);
    }

    #[test]
    fn json_len_is_the_compact_length() {
        let v = json!({ "a": [1, "é\n", null], "b": { "c": true } });
        assert_eq!(json_len(&v), v.to_string().len());
    }

    #[test]
    fn explain_renders_a_tree() {
        let node = |t: &str, children| ExplainPlanNode {
            id: "1".into(),
            node_type: t.into(),
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
            children,
        };
        let mut scan = node("Seq Scan", vec![]);
        scan.relation_name = Some("users".into());
        scan.alias = Some("u".into());
        scan.startup_cost = Some(0.0);
        scan.total_cost = Some(35.5);
        scan.plan_rows = Some(2550.0);
        scan.plan_width = Some(36);
        scan.filter = Some("(id > 1)".into());
        let result = ExplainResult {
            plan: node("Limit", vec![scan]),
            planning_time: 0.25,
            execution_time: None,
            is_analyze: false,
        };
        assert_eq!(
            explain_text(&result),
            "Limit\n  Seq Scan on users u (cost=0.00..35.50 rows=2550 width=36)\n      Filter: (id > 1)\nPlanning time: 0.25 ms"
        );
    }
}
