//! Removing passwords from connection strings before they're stored
//! (Decision 13.1 of the phase 3 plan). Passwords live in the keychain or
//! the web vault, never in `connections.connection_string`.

use url::Url;

/// `s` without its password, for `connections.connection_string`.
///
/// - **SQLite** strings (`sqlite:…`) are returned as they are.
/// - **key=value** strings (`Server=…;Password=…`, ADO.NET, ODBC, JDBC
///   properties, anything that isn't a URL with an authority) lose every `Password` and `Pwd` pair, whatever the case. A
///   value may be quoted with `'…'`, `"…"` or `{…}` (a doubled closing quote
///   is a literal one), and a `;` inside quotes doesn't end the pair. The
///   other pairs are kept byte for byte. The TypeScript kept these strings
///   whole, password included.
/// - **URLs** with a password in their user info, or a `password` or `pwd`
///   query parameter (any case; libpq and MySQL read both), lose them
///   through the WHATWG URL parser, as the TypeScript's
///   `stripPasswordFromConnectionString` did for the user info:
///   `postgresql://` is read as `postgres://`, the URL is re-serialised
///   (which percent-encodes, lowercases the scheme and so on), and
///   `postgres://` is written back as `postgresql://`. A URL without a
///   password is left exactly as it is, where the TypeScript normalised it
///   too. In a URL with an authority (`scheme://…`), the key=value pass
///   below runs only on what follows the authority (path and query), so a
///   `;` in the user info can't cut the URL apart. A password in a string the URL parser rejects
///   (`postgres://u:p#w@h/db`) stays.
///
/// The passes repeat until nothing changes (one removal can expose another,
/// or turn the rest into a URL), so the result is stable: stripping it
/// again changes nothing.
///
/// The `strip_connection_string_passwords` data step applies this to the
/// rows already stored.
pub fn strip_connection_string_password(s: &str) -> String {
    /// Far more rounds than any real string needs; each one that changes
    /// something removes at least one pair or URL part.
    const MAX_ROUNDS: usize = 8;
    let mut current = s.to_string();
    for _ in 0..MAX_ROUNDS {
        if current.starts_with("sqlite:") {
            break;
        }
        // A URL with an authority: the URL pass, then the key=value pass on
        // what follows the authority only (`/db;Password=x`), so a `;` in
        // the user info can't cut the URL apart.
        let next = if is_authority_url(&current) {
            let url = strip_url_password(&current).unwrap_or_else(|| current.clone());
            let (authority, rest) = url.split_at(authority_end(&url));
            format!("{authority}{}", strip_key_value_password(rest))
        } else {
            let kv = strip_key_value_password(&current);
            strip_url_password(&kv).unwrap_or(kv)
        };
        if next == current {
            break;
        }
        current = next;
    }
    current
}

/// Whether `s` parses as a URL with an authority (`scheme://…`), read the
/// way [`strip_url_password`] reads it.
fn is_authority_url(s: &str) -> bool {
    Url::parse(&s.replacen("postgresql://", "postgres://", 1)).is_ok_and(|u| u.has_authority())
}

/// Where the authority of an authority URL ends: the first `/`, `?` or `#`
/// (or `\\`, which WHATWG reads as `/` for special schemes) after `://`,
/// the same boundary the URL parser used.
fn authority_end(url: &str) -> usize {
    let start = url.find("://").map_or(0, |i| i + 3);
    url[start..]
        .find(['/', '?', '#', '\\'])
        .map_or(url.len(), |i| start + i)
}

/// Whether a key (a key=value pair's, or a URL query parameter's) names a
/// password.
fn is_password_key(key: &str) -> bool {
    let key = key.trim_matches(BLANKS);
    key.eq_ignore_ascii_case("password") || key.eq_ignore_ascii_case("pwd")
}

/// The URL handling, when `s` parses as a URL with a password in its user
/// info or its query.
fn strip_url_password(s: &str) -> Option<String> {
    let mut url = Url::parse(&s.replacen("postgresql://", "postgres://", 1)).ok()?;
    let mut changed = false;
    if url.password().is_some_and(|p| !p.is_empty()) {
        url.set_password(None).ok()?;
        changed = true;
    }
    if let Some(query) = url.query() {
        let params: Vec<&str> = query.split('&').collect();
        let is_password_param = |param: &&str| {
            url::form_urlencoded::parse(param.as_bytes())
                .next()
                .is_some_and(|(name, _)| is_password_key(&name))
        };
        if params.iter().any(is_password_param) {
            let kept: Vec<&str> = params
                .iter()
                .filter(|p| !is_password_param(p))
                .copied()
                .collect();
            let kept = kept.join("&");
            url.set_query((!kept.is_empty()).then_some(kept.as_str()));
            changed = true;
        }
    }
    changed.then(|| url.as_str().replacen("postgres://", "postgresql://", 1))
}

fn strip_key_value_password(s: &str) -> String {
    let pairs = split_pairs(s);
    if !pairs.iter().any(|p| is_password_pair(p)) {
        return s.to_string();
    }
    pairs
        .into_iter()
        .filter(|p| !is_password_pair(p))
        .collect::<Vec<_>>()
        .join(";")
}

/// Where the scanner is inside one `key=value` pair.
#[derive(Clone, Copy, PartialEq)]
enum State {
    /// Before the first `=`.
    Key,
    /// After `=`, skipping blanks before the value.
    BeforeValue,
    Unquoted,
    /// Inside a quoted value; the char closes it.
    Quoted(char),
    /// After a quoted value's closing quote.
    AfterQuote,
}

const BLANKS: [char; 4] = [' ', '\t', '\n', '\r'];

/// `s` split at every `;` that isn't inside a quoted value.
fn split_pairs(s: &str) -> Vec<&str> {
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let mut pairs = Vec::new();
    let (mut start, mut state, mut i) = (0, State::Key, 0);
    while i < chars.len() {
        let (at, c) = chars[i];
        match state {
            State::Quoted(close) if c == close => {
                if chars.get(i + 1).map(|&(_, n)| n) == Some(close) {
                    i += 2;
                    continue;
                }
                state = State::AfterQuote;
            }
            State::Quoted(_) => {}
            _ if c == ';' => {
                pairs.push(&s[start..at]);
                start = at + 1;
                state = State::Key;
            }
            State::Key if c == '=' => state = State::BeforeValue,
            State::BeforeValue if BLANKS.contains(&c) => {}
            State::BeforeValue => {
                state = match c {
                    '\'' => State::Quoted('\''),
                    '"' => State::Quoted('"'),
                    '{' => State::Quoted('}'),
                    _ => State::Unquoted,
                }
            }
            State::Key | State::Unquoted | State::AfterQuote => {}
        }
        i += 1;
    }
    pairs.push(&s[start..]);
    pairs
}

/// Whether a pair's key, blanks trimmed, is `password` or `pwd` in any
/// (ASCII) case.
fn is_password_pair(pair: &str) -> bool {
    pair.split_once('=')
        .is_some_and(|(key, _)| is_password_key(key))
}

// ── Strings the old builder made (phase 5a Decision 12) ──

/// The fields a connection string can stand for, as a stored row holds them
/// (`connections.type`, `host`, `port`, `database_name`, `username`,
/// `ssl_mode`), read the way `connections::load_all` reads them.
#[derive(Clone, Copy)]
pub struct StringFields<'a> {
    pub ty: &'a str,
    pub host: &'a str,
    pub port: f64,
    pub database_name: &'a str,
    pub username: &'a str,
    pub ssl_mode: Option<&'a str>,
}

/// What `encodeURIComponent` leaves alone: `A–Z a–z 0–9 - _ . ! ~ * ' ( )`.
const URI_COMPONENT: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

/// JavaScript's `encodeURIComponent`.
fn encode_uri_component(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, URI_COMPONENT).to_string()
}

/// JavaScript's `decodeURIComponent`: `None` where it throws (a `%` not
/// followed by two hex digits, or bytes that aren't UTF-8).
fn decode_uri_component(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `String.prototype.trim`: ECMAScript WhiteSpace and LineTerminator, which
/// is Rust's `White_Space` minus U+0085 plus U+FEFF.
fn js_trim(s: &str) -> &str {
    s.trim_matches(|c: char| (c.is_whitespace() && c != '\u{0085}') || c == '\u{FEFF}')
}

/// JavaScript's `String(n)` for a number.
fn js_number(n: f64) -> String {
    if n.is_nan() {
        "NaN".into()
    } else if n.is_infinite() {
        if n > 0.0 { "Infinity" } else { "-Infinity" }.into()
    } else if n == 0.0 {
        "0".into()
    } else if n.abs() >= 1e21 || n.abs() < 1e-6 {
        // `1e+21`, `1.5e-7`: Rust's `{:e}` is the same shortest digits
        // without the `+`.
        let e = format!("{n:e}");
        match e.split_once('e') {
            Some((m, x)) if !x.starts_with('-') => format!("{m}e+{x}"),
            _ => e,
        }
    } else {
        // Shortest round-trip digits, like JS: 2^53 + 2 is
        // `9007199254740994`, and 123456789012345680000 keeps JS's zeros
        // where the exact integer would print other digits.
        n.to_string()
    }
}

/// Names `LEGACY_TYPES[type]` and `LEGACY_MYSQL_SSL[sslMode]` found on
/// `Object.prototype` in the TypeScript (both are plain object literals).
const OBJECT_PROTOTYPE: &[&str] = &[
    "__proto__",
    "__defineGetter__",
    "__defineSetter__",
    "__lookupGetter__",
    "__lookupSetter__",
    "constructor",
    "hasOwnProperty",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toLocaleString",
    "toString",
    "valueOf",
];

/// What the old `buildConnectionString` (before phase 5a) made of these
/// fields, without the password (stored strings had theirs stripped): a port
/// of `legacyBuiltString` in `src/lib/utils/connection-string-rules.ts`.
///
/// `None` only for a MySQL or MariaDB SSL mode naming an `Object.prototype`
/// member (`toString`, …): the TypeScript put that member's text in the
/// string, which differs between JavaScript engines, so no string can be
/// said to be the old builder's.
pub fn legacy_built_string(f: &StringFields) -> Option<String> {
    let database = f.database_name;
    match f.ty {
        "sqlite" => return Some(format!("sqlite://{database}")),
        "duckdb" => {
            let db = if database.is_empty() {
                ":memory:"
            } else {
                database
            };
            return Some(format!("duckdb://{db}"));
        }
        _ => {}
    }
    // A type naming an `Object.prototype` member found a function there,
    // with no `protocol` or `defaultPort`: the same as an unknown type.
    let (protocol, default_port) = match f.ty {
        "postgres" => ("postgres", Some(5432.0)),
        "mysql" => ("mysql", Some(3306.0)),
        "mariadb" => ("mariadb", Some(3306.0)),
        "mssql" => ("mssql", Some(1433.0)),
        other => (other, None),
    };
    let credentials = if f.username.is_empty() {
        String::new()
    } else {
        format!("{}@", encode_uri_component(f.username))
    };
    // `f.port !== known?.defaultPort`: NaN differs from everything.
    let port = if Some(f.port) == default_port {
        String::new()
    } else {
        format!(":{}", js_number(f.port))
    };
    let mut s = format!("{protocol}://{credentials}{}{port}/{database}", f.host);
    if let Some(ssl) = f.ssl_mode.filter(|m| !m.is_empty()) {
        match f.ty {
            "postgres" => s.push_str(&format!("?sslmode={ssl}")),
            "mysql" | "mariadb" => {
                let value = match ssl {
                    "disable" => "DISABLED",
                    "allow" | "prefer" => "PREFERRED",
                    "require" => "REQUIRED",
                    other if OBJECT_PROTOTYPE.contains(&other) => return None,
                    other => other,
                };
                s.push_str(&format!("?ssl-mode={value}"));
            }
            _ => {}
        }
    }
    Some(s)
}

/// `comparable` in the TypeScript: trimmed, `postgresql://` (any case) read
/// as `postgres://`, and, when it parses as a URL, without its password and
/// re-serialised by the WHATWG parser.
fn comparable(s: &str) -> String {
    let t = js_trim(s);
    let t = match t.get(..13) {
        Some(head) if head.eq_ignore_ascii_case("postgresql://") => {
            format!("postgres://{}", &t[13..])
        }
        _ => t.to_string(),
    };
    match Url::parse(&t) {
        Ok(mut url) => {
            // JS's setter does nothing where a URL can't hold a password.
            let _ = url.set_password(None);
            url.as_str().to_string()
        }
        Err(_) => t,
    }
}

/// Whether `s` is the string the old builder made from `fields`, so it says
/// nothing of its own: a port of `isLegacyBuiltString`. Such strings go
/// stale as soon as a field is edited, and Core connects with the string
/// when there is one, so the `drop_legacy_built_connection_strings` data
/// step clears them.
pub fn is_legacy_built_string(s: &str, fields: &StringFields) -> bool {
    if s.is_empty() {
        return false;
    }
    legacy_built_string(fields).is_some_and(|built| comparable(s) == comparable(&built))
}

/// The user the TypeScript's load read from a row's string when its
/// `username` was empty (`connection-manager.svelte.ts`), before it asked
/// [`is_legacy_built_string`]: the URL's user, percent-decoded, or `""` for
/// SQLite and DuckDB strings and anything that isn't a URL.
pub(crate) fn username_from_string(s: &str) -> String {
    let s = s.replacen("postgresql://", "postgres://", 1);
    if s.starts_with("sqlite") || s.starts_with("duckdb") {
        return String::new();
    }
    Url::parse(&s)
        .ok()
        .filter(|u| !u.username().is_empty())
        .and_then(|u| decode_uri_component(u.username()))
        .unwrap_or_default()
}

// ── Stripping secrets as the TypeScript does (phase 5a) ──

/// JavaScript's `\s` (ECMAScript WhiteSpace and LineTerminator).
fn js_space(c: char) -> bool {
    (c.is_whitespace() && c != '\u{0085}') || c == '\u{FEFF}'
}

/// `SECRET_KEY`: `/^(password|pwd|sslpassword)$/i`.
fn is_secret_key(key: &str) -> bool {
    ["password", "pwd", "sslpassword"]
        .iter()
        .any(|k| key.eq_ignore_ascii_case(k))
}

/// `DUCKDB_SECRET_KEY`: `/secret|password|pwd|token|key_id|access_key|session/i`.
/// Without the `u` flag, `i` folds ASCII only.
fn is_duckdb_secret_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "secret",
        "password",
        "pwd",
        "token",
        "key_id",
        "access_key",
        "session",
    ]
    .iter()
    .any(|k| key.contains(k))
}

/// `LEFTOVER_SECRET`: `/(password|pwd)\s*=/i`, anywhere in `s`.
fn has_leftover_secret(s: &str) -> bool {
    // ASCII lowercasing keeps every byte offset.
    let lower = s.to_ascii_lowercase();
    ["password", "pwd"].iter().any(|word| {
        lower
            .match_indices(word)
            .any(|(at, _)| lower[at + word.len()..].chars().find(|c| !js_space(*c)) == Some('='))
    })
}

/// `/^(sqlite|duckdb):/i`
fn is_file_string(t: &str) -> bool {
    ["sqlite:", "duckdb:"].iter().any(|p| {
        t.get(..p.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(p))
    })
}

/// The pairs of a file string's query, and whether each can carry a
/// credential (`stripFileString`'s test).
fn file_string_pairs(t: &str) -> Option<(&str, Vec<(&str, bool)>)> {
    let q = t.find('?')?;
    let pairs = t[q + 1..]
        .split('&')
        .map(|pair| {
            let key = pair.split('=').next().unwrap_or("");
            let decoded = decode_uri_component(key).unwrap_or_else(|| key.to_string());
            (pair, is_duckdb_secret_key(&decoded))
        })
        .collect();
    Some((&t[..q], pairs))
}

/// `stripFileString`: the query options that can carry credentials go; the
/// rest stay as typed.
fn strip_file_string(t: &str) -> String {
    let Some((head, pairs)) = file_string_pairs(t) else {
        return t.to_string();
    };
    let kept: Vec<&str> = pairs
        .into_iter()
        .filter(|(_, secret)| !secret)
        .map(|(pair, _)| pair)
        .collect();
    if kept.is_empty() {
        head.to_string()
    } else {
        format!("{head}?{}", kept.join("&"))
    }
}

/// One `key=value` pair as `stripKeyValue` reads it: the trimmed key and
/// the value as written (quotes included).
struct Pair {
    key: String,
    value: String,
}

/// `stripKeyValue`'s scanner: the ADO style (`Server=h;Password=pw;`) when
/// the string holds a `;`, libpq's (`host=h password=pw`) otherwise. `None`
/// where it can't be read safely. Also returns whether it was the ADO style.
fn key_value_pairs(s: &str) -> Option<(bool, Vec<Pair>)> {
    let c: Vec<char> = s.chars().collect();
    let n = c.len();
    let semicolons = s.contains(';');
    let is_sep = |ch: char| js_space(ch) || (semicolons && ch == ';');
    let mut pairs = Vec::new();
    let mut i = 0;
    while i < n {
        while i < n && is_sep(c[i]) {
            i += 1;
        }
        if i >= n {
            break;
        }
        let eq = i + c[i..].iter().position(|&x| x == '=')?;
        let key_text: String = c[i..eq].iter().collect();
        let key = js_trim(&key_text).to_string();
        let bad_key = if semicolons {
            key.contains(';')
        } else {
            key.chars().any(js_space)
        };
        if key.is_empty() || bad_key {
            return None;
        }
        i = eq + 1;
        while i < n && c[i] == ' ' {
            i += 1;
        }
        let open = c.get(i).copied().filter(|o| matches!(o, '\'' | '"' | '{'));
        let value = if let Some(open) = open {
            let close = if open == '{' { '}' } else { open };
            let mut j = i + 1;
            let mut done = false;
            while j < n {
                if open == '\'' && !semicolons && c[j] == '\\' {
                    j += 2;
                    continue;
                }
                if c[j] == close {
                    if c.get(j + 1) == Some(&close) {
                        j += 2;
                        continue;
                    }
                    done = true;
                    break;
                }
                j += 1;
            }
            if !done {
                return None;
            }
            let value: String = c[i..=j].iter().collect();
            i = j + 1;
            value
        } else {
            let end = if semicolons {
                c[i..].iter().position(|&x| x == ';')
            } else {
                c[i..].iter().position(|&x| js_space(x))
            };
            let stop = end.map_or(n, |p| i + p);
            let value: String = c[i..stop].iter().collect();
            i = stop;
            js_trim(&value).to_string()
        };
        pairs.push(Pair { key, value });
    }
    Some((semicolons, pairs))
}

/// `stripKeyValue`: the string without its password pairs.
fn strip_key_value(s: &str) -> Option<String> {
    let (semicolons, pairs) = key_value_pairs(s)?;
    let parts: Vec<String> = pairs
        .iter()
        .filter(|p| !is_secret_key(&p.key))
        .map(|p| format!("{}={}", p.key, p.value))
        .collect();
    Some(if semicolons {
        parts.iter().map(|p| format!("{p};")).collect()
    } else {
        parts.join(" ")
    })
}

/// A TablePlus `+ssh` URL's path, `/dbuser:dbpass@dbhost/db`, split as
/// `/^\/([^@/:]*):[^/]*@([^@/]*)(\/.*)?$/` does: the user, the password,
/// the host and the rest.
fn ssh_path_parts(path: &str) -> Option<(&str, &str, &str, &str)> {
    let rest = path.strip_prefix('/')?;
    let (segment, tail) = match rest.find('/') {
        Some(at) => rest.split_at(at),
        None => (rest, ""),
    };
    let colon = segment.find([':', '@'])?;
    if segment.as_bytes()[colon] != b':' {
        return None;
    }
    let after = &segment[colon + 1..];
    let at = after.rfind('@')?;
    Some((&segment[..colon], &after[..at], &after[at + 1..], tail))
}

/// `stripSecrets` on a URL: `None` where the TypeScript stored `""`.
fn strip_url(t: &str) -> Option<String> {
    let mut url = Url::parse(t).ok()?;
    if url.fragment().is_some_and(|f| !f.is_empty()) {
        return None;
    }
    // JS's setter does nothing where a URL can't hold a password.
    let _ = url.set_password(None);
    if url.scheme().ends_with("+ssh") {
        if let Some((user, _, host, tail)) = ssh_path_parts(url.path()) {
            let path = format!("/{user}@{host}{tail}");
            url.set_path(&path);
        }
        let path = url.path();
        let after = &path[1.min(path.len())..];
        let colon_before_at = after
            .find('@')
            .is_some_and(|at| path.starts_with('/') && after[..at].contains(':'));
        if colon_before_at || path.matches('@').count() > 1 {
            return None;
        }
    }
    if let Some(query) = url.query() {
        let pairs: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();
        if pairs.iter().any(|(k, _)| is_secret_key(k)) {
            let kept: Vec<(String, String)> = pairs
                .into_iter()
                .filter(|(k, _)| !is_secret_key(k))
                .collect();
            if kept.is_empty() {
                url.set_query(None);
            } else {
                url.query_pairs_mut().clear().extend_pairs(kept);
            }
        }
    }
    Some(url.as_str().to_string())
}

/// `stripSecrets`: the string with no password in it, or `""` where it
/// can't be read safely.
fn strip_secrets(t: &str) -> String {
    if is_file_string(t) {
        strip_file_string(t)
    } else if !t.contains("://") {
        strip_key_value(t).unwrap_or_default()
    } else {
        strip_url(t).unwrap_or_default()
    }
}

/// The string as it may be stored: an exact port of the TypeScript's
/// `stripConnectionStringSecrets` (`src/lib/utils/connection-string-rules.ts`).
///
/// - `None` for `""` (the TypeScript's `undefined`).
/// - A URL loses its user-info password (and, for a TablePlus `+ssh` URL,
///   the database password in its path) and any `password`, `pwd` or
///   `sslpassword` query parameter. The query is re-encoded only when one
///   goes; the scheme is kept.
/// - A key=value string (ADO `Server=h;Password=pw;` or libpq
///   `host=h password=pw`) loses those pairs, quoted values included.
/// - File strings (SQLite, DuckDB) lose options that can carry credentials
///   (`s3_secret_access_key`, …) and keep the rest as typed.
/// - A string that can't be read safely (a URL the parser rejects, one with
///   a fragment, an unclosed quote) is `""`, and so is any result that still
///   has a `password=` or `pwd=` in it.
///
/// `connections::insert` and `connections::update` store strings through
/// it. `connections::save` and the `strip_connection_string_passwords` data
/// step keep the weaker [`strip_connection_string_password`], which the
/// phase 3 fixtures pin.
pub fn strip_connection_string_secrets(s: &str) -> Option<String> {
    if s.is_empty() {
        return None;
    }
    let stripped = strip_secrets(js_trim(s));
    Some(if has_leftover_secret(&stripped) {
        String::new()
    } else {
        stripped
    })
}

// ── Taking the secrets out (phase 5d Decision 12a) ──

/// The secrets a stored string holds, for the one-time upgrade that moves
/// them to the keychain and strips the string. Its `Debug` says which parts
/// are present, never a value.
pub struct SecretSplit {
    /// One database password the driver reads from a URL's user info (for
    /// a TablePlus `+ssh` URL, its path's), or a Postgres URL's `password`
    /// parameter: store it as `db:<id>`.
    pub db: Option<String>,
    /// One SSH password (a TablePlus `+ssh` URL's user info), cleanly
    /// separated: store it as `ssh:<id>`.
    pub ssh: Option<String>,
    /// A secret that can't be moved: any key=value password (ADO, libpq),
    /// a `pwd` or `sslpassword` parameter, a `password` parameter outside
    /// Postgres, DuckDB credentials, two different passwords of one kind, a
    /// password where the URL parser doesn't look, or a string the strip
    /// blanks. It's lost when the string is stripped,
    /// so the connection is listed in the one-time notice.
    pub unmovable: bool,
    /// What to store afterwards: always
    /// [`strip_connection_string_secrets`]'s result.
    pub stripped: String,
}

impl std::fmt::Debug for SecretSplit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretSplit")
            .field("db", &self.db.is_some())
            .field("ssh", &self.ssh.is_some())
            .field("unmovable", &self.unmovable)
            .finish_non_exhaustive()
    }
}

/// What [`find_secrets`] found.
#[derive(Default)]
struct Found {
    db: Vec<String>,
    ssh: Vec<String>,
    unmovable: bool,
}

impl Found {
    /// A decoded password, or an unmovable secret where it didn't decode.
    fn take(&mut self, ssh: bool, password: Option<String>) {
        match password {
            Some(pw) if ssh => self.ssh.push(pw),
            Some(pw) => self.db.push(pw),
            None => self.unmovable = true,
        }
    }
}

/// A query parameter strictly percent-decoded (`+` is a space); `None` for
/// a bad escape or bytes that aren't UTF-8.
fn form_decode_strict(s: &str) -> Option<String> {
    decode_uri_component(&s.replace('+', " "))
}

/// `…://user:pass@…` in text the URL parser can't use: a `:` before the
/// last `@` of what follows `://`.
fn looks_like_user_password(t: &str) -> bool {
    t.split_once("://")
        .is_some_and(|(_, rest)| rest.rfind('@').is_some_and(|at| rest[..at].contains(':')))
}

fn find_secrets(t: &str) -> Found {
    let mut found = Found::default();
    if is_file_string(t) {
        found.unmovable = file_string_pairs(t)
            .is_some_and(|(_, pairs)| pairs.iter().any(|(_, secret)| *secret))
            || has_leftover_secret(&strip_file_string(t));
        return found;
    }
    if !t.contains("://") {
        let Some((_, pairs)) = key_value_pairs(t) else {
            found.unmovable = has_leftover_secret(t);
            return found;
        };
        // Core's builder hands a key=value string to the driver whole, with
        // the keychain password put in as a `Password=` pair, so a stored
        // pair can't be moved by driver-read rules: it's lost on stripping.
        // An empty value (`Password=;`, `''`) holds nothing.
        found.unmovable = pairs.iter().any(|p| {
            is_secret_key(&p.key) && !matches!(p.value.as_str(), "" | "''" | "\"\"" | "{}")
        }) || strip_key_value(t).is_some_and(|r| has_leftover_secret(&r));
        return found;
    }
    let Some(url) = Url::parse(t)
        .ok()
        .filter(|u| u.fragment().is_none_or(str::is_empty))
    else {
        found.unmovable = has_leftover_secret(t) || looks_like_user_password(t);
        return found;
    };
    let ssh = url.scheme().ends_with("+ssh");
    let postgres = matches!(url.scheme(), "postgres" | "postgresql");
    if let Some(pw) = url.password().filter(|p| !p.is_empty()) {
        // A TablePlus `+ssh` URL's user info is the SSH login.
        found.take(ssh, decode_uri_component(pw));
    }
    // A TablePlus `+ssh` URL's path (`/dbuser:dbpass@dbhost/db`) becomes the
    // database URL, so its password is a user-info password too.
    if ssh {
        match strip_url(t) {
            // A path the strip refuses (`/dbu:p/w@h/db`: a `/` in the
            // password) holds a password it can't cut out.
            None => found.unmovable = true,
            Some(_) => {
                if let Some((_, pw, _, _)) = ssh_path_parts(url.path()) {
                    if !pw.is_empty() {
                        found.take(false, decode_uri_component(pw));
                    }
                }
            }
        }
    }
    if let Some(query) = url.query() {
        for param in query.split('&') {
            let (name, value) = param.split_once('=').unwrap_or((param, ""));
            let Some(name) = form_decode_strict(name) else {
                if url::form_urlencoded::parse(param.as_bytes()).any(|(k, _)| is_secret_key(&k)) {
                    found.unmovable = true;
                }
                continue;
            };
            if !is_secret_key(&name) {
                continue;
            }
            // Only Postgres reads a `password` query parameter as the
            // password; `pwd`, another case, `sslpassword`, or any other
            // engine's parameter is lost on stripping.
            if name == "password" && postgres {
                found.take(false, form_decode_strict(value));
            } else if !value.is_empty() {
                found.unmovable = true;
            }
        }
    }
    // A password where the URL parser doesn't look (`sqlserver://h;pwd=x`,
    // in the host): the strip's last guard blanks it.
    if strip_url(t).is_some_and(|r| has_leftover_secret(&r)) {
        found.unmovable = true;
    }
    found
}

/// One password of a kind: `None` for none, and for two different ones
/// (which also makes the split unmovable). Empty passwords aren't secrets;
/// the same one given twice is one.
fn one(mut passwords: Vec<String>, unmovable: &mut bool) -> Option<String> {
    passwords.retain(|p| !p.is_empty());
    passwords.sort();
    passwords.dedup();
    if passwords.len() > 1 {
        *unmovable = true;
        return None;
    }
    passwords.pop()
}

/// Takes the secrets out of a stored string, for the upgrade that moves
/// them to the keychain (phase 5d Decision 12a): `None` when it holds none,
/// so the row is left as it is. Otherwise the database and SSH passwords to
/// store, whether something else is lost by stripping, and the stripped
/// string, which is always what the app stores
/// ([`strip_connection_string_secrets`]).
pub fn split_connection_string_secret(s: &str) -> Option<SecretSplit> {
    let t = js_trim(s);
    if t.is_empty() {
        return None;
    }
    let found = find_secrets(t);
    let mut unmovable = found.unmovable;
    let db = one(found.db, &mut unmovable);
    let ssh = one(found.ssh, &mut unmovable);
    if db.is_none() && ssh.is_none() && !unmovable {
        return None;
    }
    Some(SecretSplit {
        db,
        ssh,
        unmovable,
        stripped: strip_connection_string_secrets(s).unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_split_outside_quotes_only() {
        assert_eq!(
            split_pairs("a=1;b='x;y';c=\"p\"\"q;r\";d={s;t}}u};e"),
            vec!["a=1", "b='x;y'", "c=\"p\"\"q;r\"", "d={s;t}}u}", "e"]
        );
        assert_eq!(split_pairs(""), vec![""]);
        assert_eq!(split_pairs(";"), vec!["", ""]);
        // A quote inside an unquoted value is a plain character.
        assert_eq!(split_pairs("a=it's;b=2"), vec!["a=it's", "b=2"]);
    }

    #[test]
    fn only_password_and_pwd_keys_match() {
        for p in ["Password=x", " pwd =x", "PWD=", "PassWord='a;b'"] {
            assert!(is_password_pair(p), "{p}");
        }
        for p in [
            "Password",
            "PasswordHint=x",
            "User Password=x",
            "x=Password",
            "",
        ] {
            assert!(!is_password_pair(p), "{p}");
        }
    }

    // ── is_legacy_built_string: the cases of
    // `src/lib/utils/connection-string-rules.test.ts`, plus the edges of
    // the port (JS number and trim rules, prototype names). ──

    fn pg() -> StringFields<'static> {
        StringFields {
            ty: "postgres",
            host: "prod.example.com",
            port: 5432.0,
            database_name: "app",
            username: "alice",
            ssl_mode: Some("disable"),
        }
    }

    fn file(ty: &'static str, database_name: &'static str) -> StringFields<'static> {
        StringFields {
            ty,
            host: "",
            port: 0.0,
            database_name,
            username: "",
            ssl_mode: None,
        }
    }

    #[test]
    fn the_old_builders_strings_are_legacy() {
        let cases: Vec<(&str, StringFields)> = vec![
            // As stored: password stripped, `postgres` turned into `postgresql`.
            (
                "postgresql://alice@prod.example.com/app?sslmode=disable",
                pg(),
            ),
            (
                "postgres://alice:secret@prod.example.com/app?sslmode=disable",
                pg(),
            ),
            (
                "postgresql://alice@prod.example.com:6543/app?sslmode=require",
                StringFields {
                    port: 6543.0,
                    ssl_mode: Some("require"),
                    ..pg()
                },
            ),
            (
                "mysql://root@db:3307/shop?ssl-mode=DISABLED",
                StringFields {
                    ty: "mysql",
                    host: "db",
                    port: 3307.0,
                    database_name: "shop",
                    username: "root",
                    ssl_mode: Some("disable"),
                },
            ),
            (
                "mssql://sa@sql/app",
                StringFields {
                    ty: "mssql",
                    host: "sql",
                    port: 1433.0,
                    database_name: "app",
                    username: "sa",
                    ssl_mode: Some("disable"),
                },
            ),
            ("sqlite:///data/a.db", file("sqlite", "/data/a.db")),
            ("duckdb://:memory:", file("duckdb", "")),
            (
                "postgresql://al%40ice@prod.example.com/app?sslmode=disable",
                StringFields {
                    username: "al@ice",
                    ..pg()
                },
            ),
            // The port's edges: JS `${port}` and the old builder's own quirks.
            (
                "mariadb://u@h/d?ssl-mode=PREFERRED",
                StringFields {
                    ty: "mariadb",
                    host: "h",
                    port: 3306.0,
                    database_name: "d",
                    username: "u",
                    ssl_mode: Some("prefer"),
                },
            ),
            (
                "mysql://u@h/d?ssl-mode=VERIFY_CA",
                StringFields {
                    ty: "mysql",
                    host: "h",
                    port: 3306.0,
                    database_name: "d",
                    username: "u",
                    ssl_mode: Some("VERIFY_CA"),
                },
            ),
            (
                "oracle://u@h:1521/d",
                StringFields {
                    ty: "oracle",
                    host: "h",
                    port: 1521.0,
                    database_name: "d",
                    username: "u",
                    ssl_mode: Some("require"),
                },
            ),
            (
                "mssql://h:1433.5/d",
                StringFields {
                    ty: "mssql",
                    host: "h",
                    port: 1433.5,
                    database_name: "d",
                    username: "",
                    ssl_mode: None,
                },
            ),
            // Blanks JS's trim removes around the stored string.
            (
                "\u{FEFF} mssql://sa@sql/app\n",
                StringFields {
                    ty: "mssql",
                    host: "sql",
                    port: 1433.0,
                    database_name: "app",
                    username: "sa",
                    ssl_mode: None,
                },
            ),
            // The scheme compares in any case.
            (
                "PostgreSQL://alice@prod.example.com/app?sslmode=disable",
                pg(),
            ),
        ];
        for (s, fields) in cases {
            assert!(is_legacy_built_string(s, &fields), "{s:?} should be legacy");
        }
    }

    #[test]
    fn other_strings_are_not_legacy() {
        let cases: Vec<(&str, StringFields)> = vec![
            // Stale: a field was edited after the string was saved.
            (
                "postgresql://alice@prod.example.com/app?sslmode=disable",
                StringFields {
                    host: "staging",
                    ..pg()
                },
            ),
            // Hand-typed, with something of its own.
            (
                "postgresql://alice@prod.example.com/app?application_name=seaquel",
                pg(),
            ),
            ("sqlite:///data/a.db?mode=ro", file("sqlite", "/data/a.db")),
            ("", pg()),
            // A key=value string is never what the old builder made.
            (
                "Server=sql;Database=app;User Id=sa",
                StringFields {
                    ty: "mssql",
                    host: "sql",
                    port: 1433.0,
                    database_name: "app",
                    username: "sa",
                    ssl_mode: None,
                },
            ),
            // U+0085 isn't blank to JS's trim.
            (
                "mssql://sa@sql/app\u{0085}",
                StringFields {
                    ty: "mssql",
                    host: "sql",
                    port: 1433.0,
                    database_name: "app",
                    username: "sa",
                    ssl_mode: None,
                },
            ),
            // An SSL mode that names an Object.prototype member: JS read the
            // member, whose text depends on the engine, so the port never
            // calls such a string legacy and keeps it.
            (
                "mysql://u@h/d?ssl-mode=toString",
                StringFields {
                    ty: "mysql",
                    host: "h",
                    port: 3306.0,
                    database_name: "d",
                    username: "u",
                    ssl_mode: Some("toString"),
                },
            ),
        ];
        for (s, fields) in cases {
            assert!(
                !is_legacy_built_string(s, &fields),
                "{s:?} shouldn't be legacy"
            );
        }
    }

    #[test]
    fn legacy_strings_are_built_as_the_old_builder_did() {
        let f = |ty, port, ssl_mode| StringFields {
            ty,
            host: "h",
            port,
            database_name: "d",
            username: "a b/é",
            ssl_mode,
        };
        let cases = [
            (f("postgres", 5432.0, None), "postgres://a%20b%2F%C3%A9@h/d"),
            (
                f("postgres", 5433.0, Some("")),
                "postgres://a%20b%2F%C3%A9@h:5433/d",
            ),
            (
                f("mysql", 3306.0, Some("allow")),
                "mysql://a%20b%2F%C3%A9@h/d?ssl-mode=PREFERRED",
            ),
            (
                f("mssql", 1433.0, Some("require")),
                "mssql://a%20b%2F%C3%A9@h/d",
            ),
            (f("x", f64::NAN, None), "x://a%20b%2F%C3%A9@h:NaN/d"),
            (f("x", -0.0, None), "x://a%20b%2F%C3%A9@h:0/d"),
            (f("x", 1e21, None), "x://a%20b%2F%C3%A9@h:1e+21/d"),
            (f("x", 1.5e-7, None), "x://a%20b%2F%C3%A9@h:1.5e-7/d"),
            (
                f("x", 9007199254740994.0, None),
                "x://a%20b%2F%C3%A9@h:9007199254740994/d",
            ),
            (
                f("x", 123456789012345680000.0, None),
                "x://a%20b%2F%C3%A9@h:123456789012345680000/d",
            ),
            (f("x", 2.5, None), "x://a%20b%2F%C3%A9@h:2.5/d"),
            (
                f("x", f64::INFINITY, None),
                "x://a%20b%2F%C3%A9@h:Infinity/d",
            ),
            (
                f("constructor", 1.0, None),
                "constructor://a%20b%2F%C3%A9@h:1/d",
            ),
            (file("duckdb", "/x.duckdb"), "duckdb:///x.duckdb"),
            (file("sqlite", ""), "sqlite://"),
        ];
        for (fields, want) in cases {
            assert_eq!(legacy_built_string(&fields).as_deref(), Some(want));
        }
    }

    #[test]
    fn the_username_fallback_reads_the_urls_user() {
        let cases = [
            ("postgresql://al%40ice@h/d", "al@ice"),
            ("mysql://root:pw@h/d", "root"),
            ("mysql://h/d", ""),
            // A bad escape: decodeURIComponent threw, and the TS kept "".
            ("mysql://a%zz@h/d", ""),
            ("sqlite:///u@x.db", ""),
            ("duckdbx://u@h", ""),
            ("Server=h;User Id=u", ""),
            // Only the first `postgresql://` is replaced, and the prefix
            // check is on the replaced text.
            ("xpostgresql://u@h", "u"),
        ];
        for (s, want) in cases {
            assert_eq!(username_from_string(s), want, "{s:?}");
        }
    }

    /// `stripConnectionStringSecrets` on the TypeScript's own cases
    /// (`connection-string-rules.test.ts`), the review's four (libpq
    /// `password=`, DuckDB `s3_secret_access_key`, a `+ssh` path password,
    /// a `#` fragment password) and edges. Every expected value was produced
    /// by running the TypeScript on the input.
    const STRIP_PARITY: &[(&str, Option<&str>)] = &[
        ("postgres://alice:secret@h/app?sslmode=disable", Some("postgres://alice@h/app?sslmode=disable")),
        ("postgresql://alice@h/app?options=-c%20x", Some("postgresql://alice@h/app?options=-c%20x")),
        ("postgres://alice@h/app?password=secret&sslmode=require", Some("postgres://alice@h/app?sslmode=require")),
        ("postgresql+ssh://sshu:SSHPW@bastion/dbu:DBPW@dbhost/db", Some("postgresql+ssh://sshu@bastion/dbu@dbhost/db")),
        ("Server=sql;Database=app;User Id=sa;Password=pw;", Some("Server=sql;Database=app;User Id=sa;")),
        ("Server=sql;PWD={p;w}}d};Database=app", Some("Server=sql;Database=app;")),
        ("Server=sql;Password=\"a;\"\"b\";Database=app", Some("Server=sql;Database=app;")),
        ("host=db password='a b\\'c' dbname=app", Some("host=db dbname=app")),
        ("host=db PASSWORD=secret dbname=app", Some("host=db dbname=app")),
        ("sqlite:///a.db?mode=ro", Some("sqlite:///a.db?mode=ro")),
        ("postgresql+ssh://s@b/dbu:p@x@h/db", Some("postgresql+ssh://s@b/dbu@h/db")),
        ("duckdb:///a.duckdb?threads=4&s3_secret_access_key=abc&S3_ACCESS_KEY_ID=k&s3_session_token=t&access_mode=READ_ONLY", Some("duckdb:///a.duckdb?threads=4&access_mode=READ_ONLY")),
        ("duckdb:///a.duckdb?s3_secret_access_key=abc", Some("duckdb:///a.duckdb")),
        ("postgres://alice:12#34@h/app", Some("")),
        ("postgres://alice:secret@h:notaport/app", Some("")),
        ("mysql://u:p?w@h/db", Some("")),
        ("Server=sql;Password='abc", Some("")),
        ("postgresql+ssh://s@b/dbu:p/w@h/db", Some("")),
        ("sqlserver://h;password=secret", Some("")),
        ("sqlserver://h;PWD = secret;database=app", Some("")),
        ("", None),
        ("host=db password=hunter2 dbname=app", Some("host=db dbname=app")),
        ("host=db user=me password=x sslpassword=y", Some("host=db user=me")),
        ("duckdb:///x.duckdb?s3_secret_access_key=abc", Some("duckdb:///x.duckdb")),
        ("DuckDB:///x.duckdb?S3_Secret_Access_Key=abc&threads=2", Some("DuckDB:///x.duckdb?threads=2")),
        ("postgresql+ssh://deploy@bastion/alice:pw@db/app", Some("postgresql+ssh://deploy@bastion/alice@db/app")),
        ("postgres://u:p@h/app#frag", Some("")),
        ("postgres://u@h/app#pw=1", Some("")),
        ("  postgres://u:p@h/app  ", Some("postgres://u@h/app")),
        ("\u{feff}host=db password=x", Some("host=db")),
        ("postgres://U:P@H:5432/App?SSLMode=require&Password=a+b%20c", Some("postgres://U@H:5432/App?SSLMode=require")),
        ("postgres://u@h/?a=1&password=&b=x y", Some("postgres://u@h/?a=1&b=x+y")),
        ("mysql://u:p%40ss@h/db", Some("mysql://u@h/db")),
        ("mysql://u:p%zz@h/db", Some("mysql://u@h/db")),
        ("Server=h;Password=;Database=d", Some("Server=h;Database=d;")),
        ("Server=h;Password = 'x''y';", Some("Server=h;")),
        ("Server=h; pwd=abc ;x=1", Some("Server=h;x=1;")),
        ("a=1 b=2", Some("a=1 b=2")),
        ("a=1 b", Some("")),
        ("=x", Some("")),
        ("Server=h;;;Password=p;;", Some("Server=h;")),
        ("host=a password='x\\\\y' port=5", Some("host=a port=5")),
        ("host=a password=\"q r\" port=5", Some("host=a port=5")),
        ("host=a password={q} port=5", Some("host=a port=5")),
        ("sqlite:x.db;Password=y", Some("")),
        ("sqlite:/a.db?key=1&cache=shared", Some("sqlite:/a.db?key=1&cache=shared")),
        ("duckdb:///a?%zz=1&token=t", Some("duckdb:///a?%zz=1")),
        ("duckdb:///a?", Some("duckdb:///a?")),
        ("postgresql+ssh://s:sp@b/dbu:dp@h/db", Some("postgresql+ssh://s@b/dbu@h/db")),
        ("postgresql+ssh://s@b/dbu@h/db", Some("postgresql+ssh://s@b/dbu@h/db")),
        ("postgresql+ssh://s@b/db", Some("postgresql+ssh://s@b/db")),
        ("postgres://h/db;Password=x", Some("")),
        ("mssql://sa:pw@h:1433/db?encrypt=true", Some("mssql://sa@h:1433/db?encrypt=true")),
        ("postgres://u:p@h/app?pwd=p", Some("postgres://u@h/app")),
        ("postgres://u:p@h/app?pwd=q", Some("postgres://u@h/app")),
        ("Password=only", Some("")),
        ("  ", Some("")),
        ("http://h/#", Some("http://h/#")),
        ("postgres://\u{fc}:\u{f6}@h/d", Some("postgres://%C3%BC@h/d")),
        ("x://h/path?sslpassword=k", Some("x://h/path")),
        ("host=db password=a\\b", Some("host=db")),
        ("Data Source=h;Password={a;b};Pwd=c", Some("Data Source=h;")),
        ("Server=h;PassWord=a;PWD=a", Some("Server=h;")),
    ];

    #[test]
    fn strips_secrets_exactly_as_the_typescript() {
        for (input, want) in STRIP_PARITY {
            assert_eq!(
                strip_connection_string_secrets(input).as_deref(),
                *want,
                "{input:?}"
            );
        }
    }

    /// (db, ssh, unmovable, stripped), or `None`.
    fn split(s: &str) -> Option<(Option<String>, Option<String>, bool, String)> {
        split_connection_string_secret(s).map(|x| (x.db, x.ssh, x.unmovable, x.stripped))
    }

    fn some(v: Option<&str>) -> Option<String> {
        v.map(Into::into)
    }

    #[test]
    fn driver_read_passwords_split_out() {
        // (input, db, ssh, stripped)
        let cases = [
            (
                "postgres://alice:secret@h/app",
                Some("secret"),
                None,
                "postgres://alice@h/app",
            ),
            (
                "mysql://u:p%40ss@h/db",
                Some("p@ss"),
                None,
                "mysql://u@h/db",
            ),
            (
                "mssql://sa:pw@h:1433/db?encrypt=true",
                Some("pw"),
                None,
                "mssql://sa@h:1433/db?encrypt=true",
            ),
            // A Postgres URL's `password` parameter, decoded.
            (
                "postgres://u@h/app?password=a+b%20c&x=1",
                Some("a b c"),
                None,
                "postgres://u@h/app?x=1",
            ),
            (
                "postgresql://u@h/app?password=x",
                Some("x"),
                None,
                "postgresql://u@h/app",
            ),
            // The same password twice is one password.
            (
                "postgres://u:p@h/app?password=p",
                Some("p"),
                None,
                "postgres://u@h/app",
            ),
            // TablePlus: the database password in the path (the database
            // URL's user info), the SSH one in the user info.
            (
                "postgresql+ssh://deploy@bastion/alice:pw@db/app",
                Some("pw"),
                None,
                "postgresql+ssh://deploy@bastion/alice@db/app",
            ),
            (
                "postgresql+ssh://s@b/dbu:p@x@h/db",
                Some("p@x"),
                None,
                "postgresql+ssh://s@b/dbu@h/db",
            ),
            (
                "postgresql+ssh://sshu:SSH%20PW@bastion/dbu:DBPW@dbhost/db",
                Some("DBPW"),
                Some("SSH PW"),
                "postgresql+ssh://sshu@bastion/dbu@dbhost/db",
            ),
            (
                "mysql+ssh://sshu:sp@bastion/dbu@dbhost/db",
                None,
                Some("sp"),
                "mysql+ssh://sshu@bastion/dbu@dbhost/db",
            ),
        ];
        for (input, db, ssh, stripped) in cases {
            assert_eq!(
                split(input),
                Some((some(db), some(ssh), false, stripped.to_string())),
                "{input:?}"
            );
            assert_eq!(
                strip_connection_string_secrets(input).as_deref(),
                Some(stripped)
            );
        }
    }

    #[test]
    fn secrets_the_driver_doesnt_read_are_unmovable() {
        // (input, db still moved, ssh still moved)
        let cases = [
            // Key=value strings: ADO (MSSQL) and libpq.
            ("Server=sql;Database=app;Password=pw;", None, None),
            ("Server=sql;PWD={p;w}}d};Database=app", None, None),
            ("Server=h;PassWord=a;PWD=a", None, None),
            ("host=db password=hunter2 dbname=app", None, None),
            ("host=db password='a b\\'c' dbname=app", None, None),
            ("host=db user=me password=x sslpassword=y", None, None),
            // Parameters no driver reads as the password.
            ("postgres://u:p@h/app?pwd=p", Some("p"), None),
            ("postgres://u@h/app?Password=x", None, None),
            ("mysql://u@h/db?password=x", None, None),
            ("mssql://h/db?password=x", None, None),
            ("x://h/path?sslpassword=k", None, None),
            ("postgres://u:p@h/app?sslpassword=k", Some("p"), None),
            // Not a password.
            ("duckdb:///x.duckdb?s3_secret_access_key=abc", None, None),
            ("sqlite:x.db;Password=y", None, None),
            // Two different passwords.
            ("postgres://u:p@h/app?password=q", None, None),
            // What the strip blanks.
            ("postgres://alice:12#34@h/app", None, None),
            ("postgres://alice:secret@h:notaport/app", None, None),
            ("Server=sql;Password='abc", None, None),
            ("postgresql+ssh://s:sp@b/dbu:p/w@h/db", None, Some("sp")),
            ("sqlserver://h;password=secret", None, None),
            ("sqlserver://h;PWD = secret;database=app", None, None),
            ("a=password=x", None, None),
            ("mysql://u:p%zz@h/db", None, None),
        ];
        for (input, db, ssh) in cases {
            let (got_db, got_ssh, unmovable, stripped) =
                split(input).unwrap_or_else(|| panic!("{input:?}: no secret found"));
            assert_eq!(
                (got_db, got_ssh, unmovable),
                (some(db), some(ssh), true),
                "{input:?}"
            );
            assert_eq!(
                Some(stripped),
                strip_connection_string_secrets(input),
                "{input:?}"
            );
        }
    }

    #[test]
    fn strings_without_a_secret_are_left_alone() {
        for input in [
            "",
            "  ",
            "postgres://u@h/app?sslmode=require",
            "postgres://U@H/app",
            "Server=h;Password=;Database=d",
            "postgres://u@h/?a=1&password=&b=x y",
            "mysql://u@h/?pwd=&b=1",
            "Server=h;Pwd='';Database=d",
            "sqlite:///a.db?mode=ro",
            "duckdb:///a.duckdb?threads=4",
            "Server=sql;Database='abc",
            "postgres://u@h/app#frag",
            "a=1 b",
            "postgresql+ssh://s@b/dbu@h/db",
        ] {
            assert!(split(input).is_none(), "{input:?}");
        }
    }

    /// Whatever the split finds, what it leaves to store is the strip's.
    #[test]
    fn the_split_always_stores_what_the_strip_stores() {
        for (input, want) in STRIP_PARITY {
            if let Some(split) = split_connection_string_secret(input) {
                assert_eq!(Some(split.stripped.as_str()), *want, "{input:?}");
            }
        }
    }

    #[test]
    fn a_split_debug_shows_no_value() {
        let s = split_connection_string_secret(
            "postgresql+ssh://s:hunter1@b/u:hunter2@h/db?sslpassword=hunter3",
        )
        .unwrap();
        let shown = format!("{s:?}");
        assert_eq!(
            shown,
            "SecretSplit { db: true, ssh: true, unmovable: true, .. }"
        );
        assert!(!shown.contains("hunter"));
    }
}
