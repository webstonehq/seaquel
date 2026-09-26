//! Canonical JSON for air-gap bundles, byte for byte what `canonical.ts`
//! (shared verbatim with seaquel-app) produces:
//!
//! - object keys sorted the way JavaScript's default `sort()` orders them
//!   (by UTF-16 code units);
//! - no whitespace, no trailing newline;
//! - strings with `\"`, `\\`, `\b`, `\f`, `\n`, `\r`, `\t` and `\u00XX` for
//!   the other control characters below 0x20, everything else as is;
//! - numbers must be integers (JavaScript numbers, so `f64`), written as
//!   `Number.prototype.toString` writes them (`ryu-js`);
//! - arrays in order, empty arrays and `null`s written out.

use sha2::{Digest, Sha256};

use crate::server::js::cmp_utf16;

/// A JSON value as the canonicaliser sees it. Numbers are JavaScript
/// numbers.
#[derive(Debug, Clone, PartialEq)]
pub enum CanonicalValue {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<CanonicalValue>),
    Object(Vec<(String, CanonicalValue)>),
}

impl From<&serde_json::Value> for CanonicalValue {
    /// Numbers become `f64`, as `JSON.parse` would make them.
    fn from(v: &serde_json::Value) -> Self {
        use serde_json::Value as J;
        match v {
            J::Null => Self::Null,
            J::Bool(b) => Self::Bool(*b),
            J::Number(n) => Self::Number(n.as_f64().unwrap_or(f64::NAN)),
            J::String(s) => Self::String(s.clone()),
            J::Array(a) => Self::Array(a.iter().map(Self::from).collect()),
            J::Object(o) => {
                Self::Object(o.iter().map(|(k, v)| (k.clone(), Self::from(v))).collect())
            }
        }
    }
}

/// A number `canonical.ts` refused (`canonical: non-integer number …`).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("canonical: non-integer number {0}")]
pub struct NonIntegerNumber(pub f64);

/// The canonical UTF-8 bytes of `value`.
pub fn canonicalize(value: &CanonicalValue) -> Result<Vec<u8>, NonIntegerNumber> {
    let mut out = String::new();
    encode(value, &mut out)?;
    Ok(out.into_bytes())
}

fn encode(value: &CanonicalValue, out: &mut String) -> Result<(), NonIntegerNumber> {
    match value {
        CanonicalValue::Null => out.push_str("null"),
        CanonicalValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        CanonicalValue::Number(n) => {
            if !n.is_finite() || n.fract() != 0.0 {
                return Err(NonIntegerNumber(*n));
            }
            out.push_str(ryu_js::Buffer::new().format(*n));
        }
        CanonicalValue::String(s) => escape_string(s, out),
        CanonicalValue::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                encode(item, out)?;
            }
            out.push(']');
        }
        CanonicalValue::Object(entries) => {
            // A later duplicate key wins, as when JSON.parse built the object.
            let mut keys: Vec<&(String, CanonicalValue)> = Vec::with_capacity(entries.len());
            for entry in entries.iter().rev() {
                if !keys.iter().any(|(k, _)| *k == entry.0) {
                    keys.push(entry);
                }
            }
            keys.sort_by(|a, b| cmp_utf16(&a.0, &b.0));
            out.push('{');
            for (i, (k, v)) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                escape_string(k, out);
                out.push(':');
                encode(v, out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

fn escape_string(input: &str, out: &mut String) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push('"');
    for ch in input.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => {
                let b = c as u8;
                out.push_str("\\u00");
                out.push(HEX[(b >> 4) as usize] as char);
                out.push(HEX[(b & 0xf) as usize] as char);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Lowercase hex of `bytes`.
pub fn bytes_to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

/// Lowercase hex of the first 16 bytes of `sha256(pubkey)`: 32 characters.
pub fn fingerprint_pubkey(pubkey: &[u8]) -> String {
    bytes_to_hex(&Sha256::digest(pubkey)[..16])
}

/// Lowercase hex of `sha256(bytes)`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    bytes_to_hex(&Sha256::digest(bytes))
}
