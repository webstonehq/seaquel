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
}
