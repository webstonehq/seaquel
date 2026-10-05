//! The layout of the JSON case fixtures in `tests/fixtures/` (and every
//! crate's): an object's keys one per line, and each case of a list of
//! objects or lists compact on a line of its own, with a trailing newline.
//! Used by the tests that write a fixture (`SEAQUEL_RECORD_CELLS`,
//! `SEAQUEL_RECORD_KINDS`), so a re-recording keeps that layout.

use serde_json::Value as Json;

/// `value` laid out one case per line.
pub fn one_case_per_line(value: &Json) -> String {
    fn compact(v: &Json) -> String {
        serde_json::to_string(v).unwrap()
    }
    fn is_case_list(v: &Json) -> bool {
        matches!(v, Json::Array(a)
            if !a.is_empty() && a.iter().all(|x| x.is_object() || x.is_array()))
    }
    fn cases(v: &Json) -> String {
        let items: Vec<String> = v.as_array().unwrap().iter().map(compact).collect();
        format!("[\n{}\n]", items.join(",\n"))
    }
    match value {
        v if is_case_list(v) => cases(v) + "\n",
        Json::Object(m) => {
            let lines: Vec<String> = m
                .iter()
                .map(|(k, v)| {
                    let key = serde_json::to_string(k).unwrap();
                    if is_case_list(v) {
                        format!("{key}:{}", cases(v))
                    } else {
                        format!("{key}:{}", compact(v))
                    }
                })
                .collect();
            format!("{{\n{}\n}}\n", lines.join(",\n"))
        }
        other => compact(other) + "\n",
    }
}
