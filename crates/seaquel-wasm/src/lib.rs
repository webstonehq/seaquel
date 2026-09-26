//! `seaquel-sql` for the Svelte app, as one WebAssembly module built with
//! `wasm-bindgen --target web`. `src/lib/sql` wraps it with the TS signatures
//! the call sites use.
//!
//! Exports are thin, synchronous free functions over strings (the glue guard
//! in `scripts/build-wasm.mjs` refuses exported structs). Every export that
//! returns JSON returns an envelope, `{"ok": …}` or `{"error": "…"}`. An
//! `error` means the call itself was bad (an unknown engine id, malformed JSON
//! arguments, a value that can't be substituted); results such as a parse
//! error are inside `ok`.
//!
//! Every position that crosses the boundary is a UTF-16 offset, converted in
//! [`offsets`]; no byte offset or sqlparser `Location` leaves this crate.
//! Statement text isn't sent back: the wrapper slices the caller's string with
//! the returned ranges, so a lone surrogate typed mid-edit survives (it
//! arrives here as U+FFFD, also one UTF-16 unit).
//!
//! Nothing here may panic on user input: the module is built with
//! `panic = "abort"`, so a panic is a trap.

pub mod offsets;

use seaquel_sql::ast::TutorialSchema;
use seaquel_sql::scan::Statement;
use seaquel_sql::{ast, create_table, params, read_only, scan, statements, SqlEngine};
use seaquel_types::Value;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

use crate::offsets::Utf16Cursor;

/// The crate version, so the app can check which module it loaded.
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

// --- envelope -------------------------------------------------------------------

#[derive(Serialize)]
struct OkEnvelope<'a, T> {
    ok: &'a T,
}

#[derive(Serialize)]
struct ErrorEnvelope<'a> {
    error: &'a str,
}

fn error(message: &str) -> String {
    serde_json::to_string(&ErrorEnvelope { error: message })
        .unwrap_or_else(|_| r#"{"error":"seaquel-wasm: couldn't serialize an error"}"#.to_string())
}

fn respond<T: Serialize>(result: Result<T, String>) -> String {
    match result {
        Ok(value) => serde_json::to_string(&OkEnvelope { ok: &value }).unwrap_or_else(|e| {
            error(&format!("seaquel-wasm: couldn't serialize the result: {e}"))
        }),
        Err(message) => error(&message),
    }
}

fn engine(id: &str) -> Result<SqlEngine, String> {
    id.parse()
        .map_err(|e: seaquel_sql::UnknownEngine| e.to_string())
}

// --- statements and offsets ----------------------------------------------------

/// A statement with UTF-16 offsets: the TS `index`, `startOffset` (`start`)
/// and `endOffset` (`end`), and the trimmed text's range `[from, to)`, which
/// the wrapper slices for `sql`.
#[derive(Serialize, Debug, PartialEq)]
#[cfg_attr(
    feature = "ts",
    derive(ts_rs::TS),
    ts(export, rename = "WasmStatement")
)]
struct JsStatement {
    index: usize,
    start: usize,
    end: usize,
    from: usize,
    to: usize,
}

/// Converts statements (ascending, as `split_statements` gives them) to
/// UTF-16 in one pass over `sql`.
///
/// `Statement::end` is `sql.len()` for an unterminated last statement; the TS
/// `endOffset` is then the UTF-16 length minus one. That mapping is done here,
/// in UTF-16, because `len - 1` in bytes can land inside a multi-byte char.
fn js_statements<'a>(
    sql: &str,
    statements: impl IntoIterator<Item = &'a Statement>,
) -> Vec<JsStatement> {
    let mut cursor = Utf16Cursor::new(sql);
    statements
        .into_iter()
        .map(|s| {
            let start = cursor.to_utf16(s.start);
            let from = cursor.to_utf16(s.text.start);
            let to = cursor.to_utf16(s.text.end);
            let end = if s.end >= sql.len() {
                cursor.to_utf16(sql.len()).saturating_sub(1)
            } else {
                cursor.to_utf16(s.end)
            };
            JsStatement {
                index: s.index,
                start,
                end,
                from,
                to,
            }
        })
        .collect()
}

/// A JS number used as a UTF-16 offset. Offsets are integers in practice
/// (Monaco's); for any other number this keeps the TS comparison's answer: a
/// fraction rounds up (`x <= end` and `x >= start` against integers hold
/// exactly when they hold for `ceil(x)`), a negative number is before
/// everything, and NaN, which fails every TS comparison and so falls through
/// to the last statement, is past the end.
fn utf16_offset(offset: f64) -> usize {
    if offset.is_nan() {
        usize::MAX
    } else {
        // `as` saturates: +inf and anything past usize::MAX become usize::MAX.
        offset.ceil().max(0.0) as usize
    }
}

/// `splitSqlStatements`: `{ok: [{index, start, end, from, to}]}`.
#[wasm_bindgen]
pub fn split_statements(sql: &str, engine_id: &str) -> String {
    respond(engine(engine_id).map(|e| js_statements(sql, &scan::split_statements(sql, e))))
}

/// `getStatementAtOffset` for the UTF-16 `offset`: `{ok: {index, start, end,
/// from, to} | null}`.
#[wasm_bindgen]
pub fn statement_at(sql: &str, offset: f64, engine_id: &str) -> String {
    respond(engine(engine_id).map(|e| {
        let byte = offsets::utf16_to_byte(sql, utf16_offset(offset));
        scan::statement_at(sql, byte, e).and_then(|s| js_statements(sql, [&s]).pop())
    }))
}

// --- parameters -------------------------------------------------------------------

/// One `ParameterValue`, its value already in the wire format (`encodeParam`).
/// A missing `value` (JS `undefined`, dropped by `JSON.stringify`) is NULL,
/// as `undefined` was to the TS.
#[derive(Deserialize)]
#[cfg_attr(
    feature = "ts",
    derive(ts_rs::TS),
    ts(export, rename = "WireParameterValue")
)]
struct ParamIn {
    name: String,
    /// `encodeParam`'s output.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional, type = "unknown"))]
    value: serde_json::Value,
}

#[derive(Serialize)]
#[cfg_attr(
    feature = "ts",
    derive(ts_rs::TS),
    ts(export, rename = "SubstituteResult")
)]
#[serde(rename_all = "camelCase")]
struct SubstitutedOut {
    sql: String,
    /// In the wire format; the wrapper decodes them with `decodeCell`.
    #[cfg_attr(feature = "ts", ts(type = "Array<unknown>"))]
    bind_values: Vec<Value>,
}

/// `extractParameters`: `{ok: [name]}`.
#[wasm_bindgen]
pub fn extract_parameters(sql: &str) -> String {
    respond(Ok(params::extract_parameters(sql)))
}

/// `hasParameters`.
#[wasm_bindgen]
pub fn has_parameters(sql: &str) -> bool {
    params::has_parameters(sql)
}

/// `substituteParameters`. `values_json` is `[{name, value}]` with each value
/// in the wire format. `{ok: {sql, bindValues}}` with the bind values in the
/// wire format, or `{error}`, which the wrapper throws as a
/// `ParameterSubstitutionError`: a value that can't be substituted, a value
/// `Value::from_wire` can't decode, or malformed arguments.
#[wasm_bindgen]
pub fn substitute_parameters(
    sql: &str,
    values_json: &str,
    engine_id: &str,
    force_inline: bool,
) -> String {
    respond(substitute(sql, values_json, engine_id, force_inline))
}

fn substitute(
    sql: &str,
    values_json: &str,
    engine_id: &str,
    force_inline: bool,
) -> Result<SubstitutedOut, String> {
    let e = engine(engine_id)?;
    let values: Vec<ParamIn> = serde_json::from_str(values_json)
        .map_err(|err| format!("The parameter values couldn't be read: {err}"))?;
    let values = values
        .into_iter()
        .map(|p| {
            Value::from_wire(p.value)
                .map(|v| (p.name.clone(), v))
                .map_err(|err| format!("The value for {{{{{}}}}} couldn't be read: {err}", p.name))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let out = params::substitute(sql, &values, e, force_inline).map_err(|err| err.message)?;
    Ok(SubstitutedOut {
        sql: out.sql,
        bind_values: out.bind_values,
    })
}

// --- statement checks ----------------------------------------------------------------

/// `detectQueryType`: `{ok: "select" | "insert" | "update" | "delete" | "other"}`.
#[wasm_bindgen]
pub fn query_type(sql: &str, engine_id: &str) -> String {
    respond(engine(engine_id).map(|e| statements::query_type(sql, e)))
}

/// `isDestructiveStatement`: `{ok: DestructiveReason | null}`.
#[wasm_bindgen]
pub fn destructive_reason(sql: &str, engine_id: &str) -> String {
    respond(engine(engine_id).map(|e| statements::destructive_reason(sql, e)))
}

/// `extractTableFromSelect`: `{ok: {schema?, table} | null}`.
#[wasm_bindgen]
pub fn table_from_select(sql: &str, engine_id: &str) -> String {
    respond(engine(engine_id).map(|e| statements::table_from_select(sql, e)))
}

/// `hasRowLimit`: `{ok: boolean}`.
#[wasm_bindgen]
pub fn has_row_limit(sql: &str, engine_id: &str) -> String {
    respond(engine(engine_id).map(|e| scan::has_row_limit(sql, e)))
}

/// `countQuery`: `{ok: string}`.
#[wasm_bindgen]
pub fn count_query(sql: &str, engine_id: &str) -> String {
    respond(engine(engine_id).map(|e| scan::count_query(sql, e)))
}

/// `validateReadOnlyQuery`: `{ok: message | null}`.
#[wasm_bindgen]
pub fn read_only_error(sql: &str, engine_id: &str) -> String {
    respond(engine(engine_id).map(|e| read_only::read_only_error(sql, e)))
}

// --- CREATE TABLE ------------------------------------------------------------------------

/// `parseCreateTableSql`: `{ok: CreateTableDefinition | null}`, with
/// placeholder ids (`column-N`, `index-N`, `fk-N`) the wrapper replaces.
/// `null` also past the parser's step budget.
#[wasm_bindgen]
pub fn parse_create_table(sql: &str) -> String {
    respond(Ok(create_table::parse_create_table(sql)))
}

// --- AST ------------------------------------------------------------------------------------

/// `parseSql`: `{ok: ParsedQuery | null}`. `schema_json` is the tutorial
/// schema, `{table: [column]}`; `valid_tables_json` is `null` (every table)
/// or an array of names. The wrapper turns the TS `undefined` into the
/// tutorial's table names.
#[wasm_bindgen]
pub fn parse_builder_query(
    sql: &str,
    engine_id: &str,
    schema_json: &str,
    valid_tables_json: &str,
) -> String {
    respond(builder_query(
        sql,
        engine_id,
        schema_json,
        valid_tables_json,
    ))
}

fn builder_query(
    sql: &str,
    engine_id: &str,
    schema_json: &str,
    valid_tables_json: &str,
) -> Result<Option<ast::ParsedQuery>, String> {
    let e = engine(engine_id)?;
    let schema: TutorialSchema = serde_json::from_str(schema_json)
        .map_err(|err| format!("the tutorial schema couldn't be read: {err}"))?;
    let valid: Option<Vec<String>> = serde_json::from_str(valid_tables_json)
        .map_err(|err| format!("the valid table names couldn't be read: {err}"))?;
    Ok(ast::parse_builder_query(sql, e, &schema, valid.as_deref()))
}

#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, rename = "VisualResult"))]
#[serde(rename_all = "camelCase")]
struct VisualOut {
    visual: Option<ast::ParsedQueryVisual>,
    parse_error: Option<String>,
}

/// `parseQueryForVisualization`: `{ok: {visual, parseError}}`. `visual` is
/// the AST or `null`; `parseError` is sqlparser's message, with its column in
/// UTF-16 units, when the SQL doesn't parse.
#[wasm_bindgen]
pub fn parse_visual(sql: &str, engine_id: &str) -> String {
    respond(engine(engine_id).map(|e| match ast::parse_visual(sql, e) {
        Ok(visual) => VisualOut {
            visual,
            parse_error: None,
        },
        Err(message) => VisualOut {
            visual: None,
            parse_error: Some(offsets::rewrite_error_location(sql, &message)),
        },
    }))
}

/// `getParseError`: `{ok: message | null}`, the column in UTF-16 units.
#[wasm_bindgen]
pub fn parse_error(sql: &str, engine_id: &str) -> String {
    respond(engine(engine_id).map(|e| {
        ast::parse_error(sql, e).map(|message| offsets::rewrite_error_location(sql, &message))
    }))
}

/// The column references behind `resolveColumnSources`: `{ok: [ColumnRef |
/// null] | null}`, one entry per output column. The wrapper looks up the
/// tables and their primary keys (decision 9).
#[wasm_bindgen]
pub fn column_refs(sql: &str, engine_id: &str) -> String {
    respond(engine(engine_id).map(|e| ast::column_refs(sql, e)))
}

// --- test only ----------------------------------------------------------------------------

/// Traps on purpose, so vitest can check that `callWasm` recovers from a real
/// trap (`src/lib/sql/wasm-trap.test.ts`). Only in a module built with the
/// `test-trap` feature, which that test builds for itself; the app's module
/// never has it.
///
/// - `panic`: a Rust panic, which `panic = "abort"` makes a
///   `WebAssembly.RuntimeError` (unreachable).
/// - `stack`: deep recursion with small frames, which runs out of the
///   engine's native stack first (`RangeError` in V8) and leaves the module's
///   shadow stack pointer where it was.
/// - `shadow-stack`: recursion with 64 KiB frames, which runs past the
///   module's 2 MiB shadow stack (`RuntimeError`, memory access out of
///   bounds).
#[cfg(feature = "test-trap")]
#[doc(hidden)]
#[wasm_bindgen]
pub fn __test_trap(kind: &str) -> String {
    use std::hint::black_box;

    #[inline(never)]
    fn small(n: u64) -> u64 {
        let n = black_box(n);
        if n == 0 {
            return 0;
        }
        small(n - 1).wrapping_add(black_box(1))
    }

    #[inline(never)]
    fn big(n: u64) -> u64 {
        let frame = black_box([n as u8; 64 * 1024]);
        if n == 0 {
            return u64::from(frame[0]);
        }
        big(n - 1).wrapping_add(u64::from(frame[1]))
    }

    match kind {
        "panic" => panic!("seaquel-wasm test trap"),
        "stack" => small(black_box(u64::MAX)).to_string(),
        "shadow-stack" => big(black_box(1 << 20)).to_string(),
        other => error(&format!("unknown trap kind {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value as Json};

    fn parse(s: &str) -> Json {
        serde_json::from_str(s).expect("valid JSON")
    }

    #[test]
    fn version_is_the_crate_version() {
        assert_eq!(version(), "0.1.0");
    }

    #[test]
    fn unknown_engines_are_errors_not_panics() {
        let want = json!({ "error": "unknown engine: \"oracle\"" });
        for out in [
            split_statements("SELECT 1", "oracle"),
            statement_at("SELECT 1", 0.0, "oracle"),
            substitute_parameters("SELECT 1", "[]", "oracle", false),
            query_type("SELECT 1", "oracle"),
            destructive_reason("SELECT 1", "oracle"),
            table_from_select("SELECT 1", "oracle"),
            has_row_limit("SELECT 1", "oracle"),
            count_query("SELECT 1", "oracle"),
            read_only_error("SELECT 1", "oracle"),
            parse_builder_query("SELECT 1", "oracle", "{}", "null"),
            parse_visual("SELECT 1", "oracle"),
            parse_error("SELECT 1", "oracle"),
            column_refs("SELECT 1", "oracle"),
        ] {
            assert_eq!(parse(&out), want);
        }
    }

    #[test]
    fn malformed_json_arguments_are_errors() {
        for values in [
            "",
            "{",
            "null",
            "{}",
            "[1]",
            "[{\"value\": 1}]",
            "[{\"name\": \"p\", \"value\": {\"$sq\": \"bigint\"}}]",
            "[{\"name\": \"p\", \"value\": {\"$sq\": \"nope\", \"v\": 1}}]",
            "[{\"name\": \"p\", \"value\": {\"$sq\": \"bigint\", \"v\": \"x\"}}]",
        ] {
            let out = parse(&substitute_parameters(
                "SELECT {{p}}",
                values,
                "postgres",
                false,
            ));
            assert!(out["error"].is_string(), "{values}: {out}");
        }
        let out = parse(&substitute_parameters(
            "SELECT {{p}}",
            "[{\"name\": \"p\", \"value\": {\"$sq\": \"bigint\"}}]",
            "postgres",
            false,
        ));
        assert_eq!(
            out["error"],
            "The value for {{p}} couldn't be read: tagged value \"bigint\" has no \"v\""
        );
        for (schema, valid) in [
            ("", "null"),
            ("[]", "null"),
            ("{}", ""),
            ("{}", "{}"),
            ("{\"t\": 1}", "null"),
        ] {
            let out = parse(&parse_builder_query("SELECT 1", "postgres", schema, valid));
            assert!(out["error"].is_string(), "{schema} {valid}: {out}");
        }
    }

    #[test]
    fn substitution_round_trips_wire_values() {
        let out = parse(&substitute_parameters(
            "SELECT {{a}}, {{b}}, {{c}}, {{d}}, {{missing}}",
            r#"[{"name":"a","value":{"$sq":"bigint","v":"9007199254740993"}},
                {"name":"b","value":{"$sq":"decimal","v":"12.50"}},
                {"name":"c","value":{"$sq":"float","v":"NaN"}},
                {"name":"d"}]"#,
            "postgres",
            false,
        ));
        assert_eq!(
            out,
            json!({ "ok": {
                "sql": "SELECT $1, $2, $3, $4, $5",
                "bindValues": [
                    { "$sq": "bigint", "v": "9007199254740993" },
                    { "$sq": "decimal", "v": "12.50" },
                    { "$sq": "float", "v": "NaN" },
                    null,
                    null,
                ],
            }})
        );
        // A refused value is an error, with the TS message.
        let out = parse(&substitute_parameters(
            "SELECT {{p}}",
            r#"[{"name":"p","value":{"$sq":"decimal","v":"NaN"}}]"#,
            "mssql",
            false,
        ));
        assert_eq!(
            out["error"],
            "The value for {{p}} is not a finite decimal number"
        );
    }

    const MIXED: &str = "SELECT '東京' AS city;\nSELECT '😀' AS face;\nSELECT 3";

    #[test]
    fn statements_come_back_in_utf16() {
        let out = parse(&split_statements(MIXED, "postgres"));
        assert_eq!(
            out,
            json!({ "ok": [
                { "index": 0, "start": 0, "end": 19, "from": 0, "to": 19 },
                { "index": 1, "start": 20, "end": 40, "from": 21, "to": 40 },
                { "index": 2, "start": 41, "end": 49, "from": 42, "to": 50 },
            ]})
        );
        // The cursor at the start of statement 2 (UTF-16 21, byte 25).
        let at = parse(&statement_at(MIXED, 21.0, "postgres"));
        assert_eq!(at["ok"]["index"], 1);
    }

    #[test]
    fn unterminated_last_statement_ends_at_utf16_length_minus_one() {
        for (sql, end) in [
            ("SELECT 1", 7),
            ("SELECT '東京'", 10),
            ("SELECT 1 -- ✓", 12),
            ("SELECT '😀'", 10),
        ] {
            let out = parse(&split_statements(sql, "postgres"));
            assert_eq!(out["ok"][0]["end"], end, "{sql}");
        }
    }

    #[test]
    fn statement_at_takes_any_number() {
        let sql = "SELECT 1; SELECT 2";
        for (offset, index) in [
            (-5.0, 0),
            (f64::NEG_INFINITY, 0),
            (0.0, 0),
            (8.5, 1),
            (9.0, 1),
            (1e300, 1),
            (f64::INFINITY, 1),
            (f64::NAN, 1),
        ] {
            let out = parse(&statement_at(sql, offset, "postgres"));
            assert_eq!(out["ok"]["index"], index, "offset {offset}");
        }
        assert_eq!(
            parse(&statement_at("", 0.0, "postgres")),
            json!({ "ok": null })
        );
        assert_eq!(
            parse(&statement_at("-- only", 3.0, "mysql")),
            json!({ "ok": null })
        );
    }

    #[test]
    fn parse_errors_carry_utf16_columns() {
        let sql = "SELECT '😀😀' x y";
        let out = parse(&parse_error(sql, "postgres"));
        let message = out["ok"].as_str().expect("a parse error");
        assert!(message.ends_with("at Line: 1, Column: 17"), "{message}");
        let out = parse(&parse_visual(sql, "postgres"));
        assert_eq!(out["ok"]["visual"], Json::Null);
        assert_eq!(out["ok"]["parseError"], message);
        let out = parse(&parse_visual("SELECT 1", "postgres"));
        assert_eq!(out["ok"]["parseError"], Json::Null);
        assert_eq!(out["ok"]["visual"]["type"], "select");
    }

    #[test]
    fn every_export_is_total_on_odd_input() {
        let inputs = [
            "",
            "\u{FFFD}",
            "SELECT '\u{FFFD}",
            "東京😀",
            ";;;",
            "SELECT {{p}} FROM t WHERE x = '{{p}}' -- {{p}}",
            "CREATE TABLE t (a int",
            "((((((((((",
        ];
        for sql in inputs {
            for e in SqlEngine::ALL {
                let id = e.as_str();
                for out in [
                    split_statements(sql, id),
                    statement_at(sql, 3.0, id),
                    substitute_parameters(sql, "[{\"name\":\"p\",\"value\":\"x\"}]", id, false),
                    substitute_parameters(sql, "[{\"name\":\"p\",\"value\":-1}]", id, true),
                    query_type(sql, id),
                    destructive_reason(sql, id),
                    table_from_select(sql, id),
                    has_row_limit(sql, id),
                    count_query(sql, id),
                    read_only_error(sql, id),
                    parse_builder_query(sql, id, "{}", "null"),
                    parse_visual(sql, id),
                    parse_error(sql, id),
                    column_refs(sql, id),
                ] {
                    let out = parse(&out);
                    assert!(
                        out.get("ok").is_some() || out["error"].is_string(),
                        "{sql:?} [{id}]: {out}"
                    );
                }
            }
            parse(&extract_parameters(sql));
            parse(&parse_create_table(sql));
        }
    }
}
