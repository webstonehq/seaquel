//! JSON as JavaScript's `JSON.parse` and `JSON.stringify` see it, for the
//! dashboard files: object keys keep their order (array-index keys first,
//! ascending, as JavaScript objects order them), a repeated key keeps its
//! first place and its last value, and numbers print as `String(n)` does.

use std::fmt;

use std::collections::HashMap;

use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
use serde_json::value::RawValue;

#[derive(Clone, PartialEq)]
pub(crate) enum J {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

impl fmt::Debug for J {
    // Never the content: dashboards hold queries and names.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            J::Null => "J::Null",
            J::Bool(_) => "J::Bool",
            J::Num(_) => "J::Num",
            J::Str(_) => "J::Str",
            J::Arr(_) => "J::Arr",
            J::Obj(_) => "J::Obj",
        })
    }
}

/// An object's members as written, values still raw, in order.
struct Members<'a>(Vec<(String, &'a RawValue)>);

impl<'de: 'a, 'a> Deserialize<'de> for Members<'a> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V<'a>(std::marker::PhantomData<&'a ()>);
        impl<'de: 'a, 'a> Visitor<'de> for V<'a> {
            type Value = Members<'a>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Members<'a>, A::Error> {
                let mut out = Vec::new();
                while let Some(entry) = map.next_entry::<String, &'de RawValue>()? {
                    out.push(entry);
                }
                Ok(Members(out))
            }
        }
        d.deserialize_map(V(std::marker::PhantomData))
    }
}

/// `JSON.parse`, or `None` where it would throw (and where serde_json is
/// stricter: a lone surrogate escape, nesting past 128 levels).
///
/// Numbers are read from their text with Rust's correctly rounded parser,
/// as `JSON.parse` reads them (one past `f64` is infinite, which
/// `stringify` writes as `null`): serde_json's default reader can be one
/// ulp off, and its `float_roundtrip` feature would reach every crate
/// through feature unification (M7). Serde only checks the text and splits
/// arrays and objects into raw members.
pub(crate) fn parse(text: &str) -> Option<J> {
    let raw: &RawValue = serde_json::from_str(text).ok()?;
    from_raw(raw.get(), 0)
}

/// How deep a value may nest (R1): what serde_json allows by default.
/// Past it the value doesn't parse, so nothing recurses further (each
/// level is a stack frame here and in the writers) and the re-scan per
/// level stays bounded at 128 × the text.
pub(crate) const MAX_DEPTH: usize = 128;

fn from_raw(text: &str, depth: usize) -> Option<J> {
    if depth > MAX_DEPTH {
        return None;
    }
    let t = text.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\r'));
    match t.as_bytes().first()? {
        b'{' => {
            let members: Members<'_> = serde_json::from_str(t).ok()?;
            let mut out: Vec<(String, J)> = Vec::with_capacity(members.0.len());
            // A repeated key keeps its first place and its last value,
            // found through an index rather than a scan.
            let mut at: HashMap<String, usize> = HashMap::with_capacity(members.0.len());
            for (k, raw) in members.0 {
                let v = from_raw(raw.get(), depth + 1)?;
                match at.get(&k) {
                    Some(&i) => out[i].1 = v,
                    None => {
                        at.insert(k.clone(), out.len());
                        out.push((k, v));
                    }
                }
            }
            Some(J::Obj(out))
        }
        b'[' => {
            let items: Vec<&RawValue> = serde_json::from_str(t).ok()?;
            items
                .into_iter()
                .map(|r| from_raw(r.get(), depth + 1))
                .collect::<Option<Vec<J>>>()
                .map(J::Arr)
        }
        b'"' => serde_json::from_str::<String>(t).ok().map(J::Str),
        b't' => Some(J::Bool(true)),
        b'f' => Some(J::Bool(false)),
        b'n' => Some(J::Null),
        _ => t.parse::<f64>().ok().map(J::Num),
    }
}

impl J {
    pub(crate) fn get(&self, key: &str) -> Option<&J> {
        match self {
            J::Obj(o) => o.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// JavaScript's truthiness.
    pub(crate) fn truthy(&self) -> bool {
        match self {
            J::Null => false,
            J::Bool(b) => *b,
            J::Num(n) => *n != 0.0 && !n.is_nan(),
            J::Str(s) => !s.is_empty(),
            J::Arr(_) | J::Obj(_) => true,
        }
    }

    /// `JSON.stringify(v)`.
    pub(crate) fn compact(&self) -> String {
        let mut out = String::new();
        write(self, None, 0, &mut out);
        out
    }

    /// `JSON.stringify(v, null, 2)`.
    pub(crate) fn pretty(&self) -> String {
        let mut out = String::new();
        write(self, Some(2), 0, &mut out);
        out
    }
}

/// A key JavaScript treats as an array index (`"0"`, `"17"`, below
/// 2^32 - 1), which objects list first.
fn index_key(k: &str) -> Option<u32> {
    if k.is_empty() || (k.len() > 1 && k.starts_with('0')) || !k.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    k.parse::<u32>().ok().filter(|n| *n < u32::MAX)
}

fn ordered(o: &[(String, J)]) -> Vec<&(String, J)> {
    let mut idx: Vec<&(String, J)> = o.iter().filter(|(k, _)| index_key(k).is_some()).collect();
    idx.sort_by_key(|(k, _)| index_key(k));
    idx.extend(o.iter().filter(|(k, _)| index_key(k).is_none()));
    idx
}

pub(crate) fn js_number(n: f64) -> String {
    if !n.is_finite() {
        return "null".to_string();
    }
    ryu_js::Buffer::new().format(n).to_string()
}

fn write_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn newline(indent: Option<usize>, depth: usize, out: &mut String) {
    if let Some(n) = indent {
        out.push('\n');
        out.push_str(&" ".repeat(n * depth));
    }
}

fn write(v: &J, indent: Option<usize>, depth: usize, out: &mut String) {
    match v {
        J::Null => out.push_str("null"),
        J::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        J::Num(n) => out.push_str(&js_number(*n)),
        J::Str(s) => write_str(s, out),
        J::Arr(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline(indent, depth + 1, out);
                write(item, indent, depth + 1, out);
            }
            newline(indent, depth, out);
            out.push(']');
        }
        J::Obj(o) => {
            if o.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (i, (k, val)) in ordered(o).into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline(indent, depth + 1, out);
                write_str(k, out);
                out.push(':');
                if indent.is_some() {
                    out.push(' ');
                }
                write(val, indent, depth + 1, out);
            }
            newline(indent, depth, out);
            out.push('}');
        }
    }
}

/// `stripWidgetRuntimeState` (`const {result, isLoading, error,
/// lastRefreshed, ...rest} = widget`), or `None` where destructuring throws
/// (`null`). A string or an array spreads into index keys, a number or a
/// boolean into nothing, as JavaScript's object rest does. (A character
/// past the BMP spreads into two UTF-16 halves in JavaScript; here each
/// half is U+FFFD.)
pub(crate) fn strip_widget(w: &J) -> Option<J> {
    const RUN_STATE: [&str; 4] = ["result", "isLoading", "error", "lastRefreshed"];
    Some(match w {
        J::Null => return None,
        J::Obj(o) => J::Obj(
            o.iter()
                .filter(|(k, _)| !RUN_STATE.contains(&k.as_str()))
                .cloned()
                .collect(),
        ),
        J::Arr(items) => J::Obj(
            items
                .iter()
                .enumerate()
                .map(|(i, v)| (i.to_string(), v.clone()))
                .collect(),
        ),
        J::Str(s) => {
            let mut o = Vec::new();
            for c in s.chars() {
                if c.len_utf16() == 1 {
                    o.push((o.len().to_string(), J::Str(c.to_string())));
                } else {
                    for _ in 0..2 {
                        o.push((o.len().to_string(), J::Str('\u{fffd}'.to_string())));
                    }
                }
            }
            J::Obj(o)
        }
        J::Bool(_) | J::Num(_) => J::Obj(Vec::new()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stringify_matches_javascript() {
        let v = parse(
            r#"{"b":1,"a":[1.0,2.5,1e21,-0.0,1e-7],"2":true,"1":null,"b":3,"s":"q\"\n\u0001é"}"#,
        )
        .unwrap();
        assert_eq!(
            v.compact(),
            r#"{"1":null,"2":true,"b":3,"a":[1,2.5,1e+21,0,1e-7],"s":"q\"\n\u0001é"}"#
        );
        assert_eq!(
            parse(r#"{"a":[],"b":{},"c":[1,{"d":2}]}"#).unwrap().pretty(),
            "{\n  \"a\": [],\n  \"b\": {},\n  \"c\": [\n    1,\n    {\n      \"d\": 2\n    }\n  ]\n}"
        );
    }
}
