//! Saved queries as the tools show and run them, moved from `seaquel-mcp`'s
//! `tools/saved.rs` so the assistant and MCP fill `{{parameters}}` the
//! same way:
//!
//! - the definitions are the saved query's own `parameters` when it has
//!   some, else one `text` parameter per `{{name}}` in the SQL
//!   (`param-dialog.svelte.ts` `getParameterDefinitions`), plus, unlike the
//!   dialog, a `text` parameter with no default for each `{{name}}` the
//!   stored definitions leave out (the dialog shows no field for it and runs
//!   it as NULL; here it needs a value like any other);
//! - each value is the dialog's text for it, the caller's value or else the
//!   definition's default, coerced by its type (`coerceValue` in
//!   `src/lib/sql/parameters.ts`);
//! - Core then fills them in with `seaquel_sql::params::substitute` for the
//!   connection's engine, without forcing inline values (`resolve-query.ts`).
//!
//! Unlike the dialog, which runs a parameter left empty as NULL, a
//! parameter with no value and no default is refused, and so is a value
//! for a name the query doesn't take.

use seaquel_sql::params::extract_parameters;
use seaquel_types::storage::{PersistedQueryParameter, PersistedSavedQuery};
use seaquel_types::Value;
use serde_json::{json, Map, Value as Json};

use super::ToolError;

pub const SAVED_QUERY_NOT_FOUND: &str = "SAVED_QUERY_NOT_FOUND";
pub const AMBIGUOUS_SAVED_QUERY: &str = "AMBIGUOUS_SAVED_QUERY";
pub const INVALID_PARAMETERS: &str = "INVALID_PARAMETERS";

/// A saved query's parameter definitions: the stored ones (as the editor's
/// dialog gets them), then a required `text` parameter for each `{{name}}`
/// in the SQL they don't cover. With none stored, that is the dialog's rule.
pub fn definitions(q: &PersistedSavedQuery) -> Vec<PersistedQueryParameter> {
    let mut defs: Vec<PersistedQueryParameter> = q
        .parameters
        .as_ref()
        .and_then(|raw| {
            serde_json::from_str::<Option<Vec<_>>>(raw.get())
                .ok()
                .flatten()
        })
        .unwrap_or_default();
    for name in extract_parameters(&q.query) {
        if !defs.iter().any(|d| d.name == name) {
            defs.push(PersistedQueryParameter {
                name,
                ty: "text".to_string(),
                default_value: None,
                description: None,
            });
        }
    }
    defs
}

pub fn describe(q: &PersistedSavedQuery, project: &str, connections: &[&str]) -> Json {
    let parameters: Vec<Json> = definitions(q)
        .into_iter()
        .map(|p| {
            let mut entry = json!({ "name": p.name, "type": p.ty });
            if let Some(d) = p.default_value {
                entry["default"] = json!(d);
            }
            if let Some(d) = p.description.filter(|d| !d.is_empty()) {
                entry["description"] = json!(d);
            }
            entry
        })
        .collect();
    let mut entry = json!({
        "id": q.id,
        "name": q.name,
        "project": project,
        "connections": connections,
        "parameters": parameters,
    });
    if let Some(d) = q.description.as_ref().filter(|d| !d.is_empty()) {
        entry["description"] = json!(d);
    }
    entry
}

/// The values to substitute, in definition order, from the host's `params`.
pub fn parameter_values(
    defs: &[PersistedQueryParameter],
    mut given: Map<String, Json>,
) -> Result<Vec<(String, Value)>, ToolError> {
    let mut values = Vec::with_capacity(defs.len());
    let mut missing = Vec::new();
    let mut bad = Vec::new();
    for def in defs {
        let text = match given.remove(&def.name) {
            Some(Json::String(s)) => s,
            Some(Json::Number(n)) => js_number_text(n.as_f64().unwrap_or(f64::NAN)),
            Some(Json::Bool(b)) => b.to_string(),
            Some(Json::Null) => String::new(),
            Some(_) => {
                bad.push(def.name.clone());
                continue;
            }
            None => match &def.default_value {
                Some(d) => d.clone(),
                None => {
                    missing.push(def.name.clone());
                    continue;
                }
            },
        };
        values.push((def.name.clone(), coerce_value(&text, &def.ty)));
    }
    let mut unknown: Vec<String> = given.into_iter().map(|(k, _)| k).collect();
    unknown.sort();
    if missing.is_empty() && unknown.is_empty() && bad.is_empty() {
        return Ok(values);
    }
    let mut problems = Vec::new();
    if !missing.is_empty() {
        problems.push(format!("no value for {}", missing.join(", ")));
    }
    if !bad.is_empty() {
        problems.push(format!(
            "{} must be a string, number, boolean or null",
            bad.join(", ")
        ));
    }
    if !unknown.is_empty() {
        problems.push(format!("the query takes no {}", unknown.join(", ")));
    }
    let takes = if defs.is_empty() {
        "no parameters".to_string()
    } else {
        defs.iter()
            .map(|d| match &d.default_value {
                Some(v) => format!("{} ({}, default {v:?})", d.name, d.ty),
                None => format!("{} ({})", d.name, d.ty),
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    Err(ToolError::new(
        INVALID_PARAMETERS,
        format!("{}. The saved query takes {takes}.", problems.join("; ")),
    ))
}

/// JavaScript's `String(n)`.
fn js_number_text(f: f64) -> String {
    ryu_js::Buffer::new().format(f).to_string()
}

/// A JavaScript number as the Rust side receives it: `encodeParam`, JSON,
/// then `Value::from_wire` (so `2**60` arrives as the digits JavaScript
/// prints for it, and a non-finite number as a float tag).
fn js_number_value(f: f64) -> Value {
    if !f.is_finite() {
        return Value::Float(f);
    }
    serde_json::from_str::<Json>(&js_number_text(f))
        .ok()
        .and_then(|j| Value::from_wire(j).ok())
        .unwrap_or(Value::Float(f))
}

/// `coerceValue` in `src/lib/sql/parameters.ts`.
pub fn coerce_value(value: &str, ty: &str) -> Value {
    if value.is_empty() {
        return Value::Null;
    }
    match ty {
        "number" => {
            let n = js_parse_float(value);
            if n.is_nan() {
                Value::Null
            } else {
                js_number_value(n)
            }
        }
        "boolean" => Value::Bool(value.to_lowercase() == "true" || value == "1"),
        _ => Value::Text(value.to_string()),
    }
}

/// JavaScript's whitespace and line terminators, which `parseFloat` skips.
pub fn is_js_space(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}

/// JavaScript's `parseFloat`: the longest prefix (after whitespace) that is
/// a decimal literal or `Infinity`, else NaN.
fn js_parse_float(s: &str) -> f64 {
    let s = s.trim_start_matches(is_js_space);
    let b = s.as_bytes();
    let mut i = 0;
    if matches!(b.first(), Some(b'+' | b'-')) {
        i = 1;
    }
    if s[i..].starts_with("Infinity") {
        return if b.first() == Some(&b'-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }
    let digits = |from: usize| {
        let mut j = from;
        while b.get(j).is_some_and(u8::is_ascii_digit) {
            j += 1;
        }
        j
    };
    let int_end = digits(i);
    let mut end = int_end;
    let mut any = int_end > i;
    if b.get(end) == Some(&b'.') {
        let frac_end = digits(end + 1);
        if frac_end > end + 1 || any {
            any |= frac_end > end + 1;
            end = frac_end;
        }
    }
    if !any {
        return f64::NAN;
    }
    if matches!(b.get(end), Some(b'e' | b'E')) {
        let mut j = end + 1;
        if matches!(b.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        let exp_end = digits(j);
        if exp_end > j {
            end = exp_end;
        }
    }
    s[..end].parse().unwrap_or(f64::NAN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_float_takes_the_longest_numeric_prefix() {
        assert_eq!(js_parse_float("12"), 12.0);
        assert_eq!(js_parse_float("  -1.5e3xyz"), -1500.0);
        assert_eq!(js_parse_float("12abc"), 12.0);
        assert_eq!(js_parse_float(".5"), 0.5);
        assert_eq!(js_parse_float("5."), 5.0);
        assert_eq!(js_parse_float("1e"), 1.0);
        assert_eq!(js_parse_float("+Infinity"), f64::INFINITY);
        assert_eq!(js_parse_float("-Infinityx"), f64::NEG_INFINITY);
        assert!(js_parse_float("abc").is_nan());
        assert!(js_parse_float(".").is_nan());
        assert!(js_parse_float("-").is_nan());
        assert!(js_parse_float("0x10") == 0.0);
    }

    #[test]
    fn coerce_follows_the_editor() {
        assert_eq!(coerce_value("", "number"), Value::Null);
        assert_eq!(coerce_value("", "text"), Value::Null);
        assert_eq!(coerce_value("abc", "number"), Value::Null);
        assert_eq!(coerce_value("42", "number"), Value::Int(42));
        assert_eq!(coerce_value("-0", "number"), Value::Int(0));
        assert_eq!(coerce_value("1.25", "number"), Value::Float(1.25));
        assert_eq!(
            coerce_value("1152921504606846976", "number"),
            Value::Int(1_152_921_504_606_847_000)
        );
        assert_eq!(coerce_value("1e21", "number"), Value::Float(1e21));
        assert_eq!(
            coerce_value("Infinity", "number"),
            Value::Float(f64::INFINITY)
        );
        assert_eq!(coerce_value("TRUE", "boolean"), Value::Bool(true));
        assert_eq!(coerce_value("1", "boolean"), Value::Bool(true));
        assert_eq!(coerce_value("yes", "boolean"), Value::Bool(false));
        assert_eq!(
            coerce_value("2024-01-02", "date"),
            Value::Text("2024-01-02".into())
        );
        assert_eq!(coerce_value("x", "other"), Value::Text("x".into()));
    }

    fn def(name: &str, ty: &str, default: Option<&str>) -> PersistedQueryParameter {
        PersistedQueryParameter {
            name: name.into(),
            ty: ty.into(),
            default_value: default.map(Into::into),
            description: None,
        }
    }

    fn saved_query(sql: &str, parameters: Option<&str>) -> PersistedSavedQuery {
        let mut q = serde_json::json!({
            "id": "q", "name": "q", "query": sql, "projectId": "p",
            "createdAt": "2026-01-02T03:04:05.000Z", "updatedAt": "2026-01-02T03:04:05.000Z",
        });
        if let Some(p) = parameters {
            q["parameters"] = serde_json::from_str(p).unwrap();
        }
        serde_json::from_value(q).unwrap()
    }

    #[test]
    fn a_parameter_the_stored_definitions_miss_is_required() {
        let q = saved_query(
            "SELECT {{a}}, {{b}}, {{a}}",
            Some(r#"[{"name":"a","type":"number","defaultValue":"1"}]"#),
        );
        let defs = definitions(&q);
        assert_eq!(
            defs.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(defs[1].ty, "text");
        assert_eq!(defs[1].default_value, None);
        let err = parameter_values(&defs, Map::new()).unwrap_err();
        assert_eq!(err.code, INVALID_PARAMETERS);
        assert!(
            err.message.starts_with("no value for b."),
            "{}",
            err.message
        );
        let given = serde_json::json!({ "b": "x" });
        let values = parameter_values(&defs, given.as_object().unwrap().clone()).unwrap();
        assert_eq!(values[1], ("b".into(), Value::Text("x".into())));

        let q = saved_query("SELECT {{c}}", None);
        assert_eq!(definitions(&q)[0].name, "c");
    }

    #[test]
    fn values_come_from_params_then_defaults() {
        let defs = [def("a", "number", None), def("b", "text", Some("dflt"))];
        let given = serde_json::json!({ "a": 7 });
        let values = parameter_values(&defs, given.as_object().unwrap().clone()).unwrap();
        assert_eq!(
            values,
            vec![
                ("a".into(), Value::Int(7)),
                ("b".into(), Value::Text("dflt".into()))
            ]
        );
    }

    #[test]
    fn missing_unknown_and_bad_values_are_refused() {
        let defs = [def("a", "number", None)];
        let given = serde_json::json!({ "z": 1 });
        let err = parameter_values(&defs, given.as_object().unwrap().clone()).unwrap_err();
        assert_eq!(err.code, INVALID_PARAMETERS);
        assert!(err.message.contains("no value for a"), "{}", err.message);
        assert!(err.message.contains("takes no z"), "{}", err.message);
        let given = serde_json::json!({ "a": [1] });
        let err = parameter_values(&defs, given.as_object().unwrap().clone()).unwrap_err();
        assert!(err.message.contains("must be a string"), "{}", err.message);
    }
}
