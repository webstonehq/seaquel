//! Connection-string helpers for [`crate::connections`]' builder:
//! JavaScript's URI component encoding, the string built from a form's
//! fields, TablePlus URLs (`tLSMode`, `+ssh`), putting a password into a
//! string, and the URL-username fallback.
//!
//! Strings are only re-serialised through a URL parser when something in
//! them changes (a password, or the SSH host and port rewrite), and the
//! parser keeps the scheme (`postgresql://` stays), the port, the query and
//! IPv6 brackets.

use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use url::{Host, Url};

/// What `encodeURIComponent` leaves alone: `A–Z a–z 0–9 - _ . ! ~ * ' ( )`.
const URI_COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
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
pub fn encode_uri_component(s: &str) -> String {
    utf8_percent_encode(s, URI_COMPONENT).to_string()
}

/// JavaScript's `decodeURIComponent`: `None` where it throws (a `%` not
/// followed by two hex digits, or bytes that aren't UTF-8).
pub fn decode_uri_component(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let hex = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// A connection type's default port: Postgres 5432, MySQL and MariaDB 3306,
/// MSSQL 1433. File engines have none.
pub fn default_port(ty: &str) -> Option<u16> {
    match ty {
        "postgres" => Some(5432),
        "mysql" | "mariadb" => Some(3306),
        "mssql" => Some(1433),
        _ => None,
    }
}

/// A number as JavaScript's template literals print it, for the integral
/// ports it is used on (`5433`, not `5433.0`).
pub(crate) fn js_number(n: f64) -> String {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 1e21 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// The fields a Postgres, MySQL or MariaDB URL is built from.
pub struct UrlFields<'a> {
    /// `postgres`, `mysql` or `mariadb`: also the scheme.
    pub ty: &'a str,
    pub host: &'a str,
    /// The port; the type's default (or 0) is left out.
    pub port: u16,
    pub database_name: &'a str,
    pub username: &'a str,
    /// `None` leaves the driver's default.
    pub ssl_mode: Option<&'a str>,
}

/// The URL `buildConnectionString` writes, without a password (the builder
/// puts that in afterwards, so an empty username still gets it):
/// `<type>://[user@]host[:port]/database[?ssl]`.
///
/// - The user goes through `encodeURIComponent`.
/// - The port is left out when it is the type's default or 0.
/// - An IPv6 host gets its brackets.
/// - Postgres gets `sslmode=<mode>`. MySQL and MariaDB get `ssl-mode=`, with
///   `disable` as `DISABLED`, `allow` and `prefer` as `PREFERRED`, `require`
///   as `REQUIRED`, `verify-ca` as `VERIFY_CA` and `verify-full` as
///   `VERIFY_IDENTITY`; another value passes through. No mode, no parameter.
pub fn build_url(f: &UrlFields<'_>) -> String {
    let user = if f.username.is_empty() {
        String::new()
    } else {
        format!("{}@", encode_uri_component(f.username))
    };
    let host = if f.host.contains(':') && !f.host.starts_with('[') {
        format!("[{}]", f.host)
    } else {
        f.host.to_string()
    };
    let port = if f.port == 0 || Some(f.port) == default_port(f.ty) {
        String::new()
    } else {
        format!(":{}", f.port)
    };
    let mut s = format!("{}://{user}{host}{port}/{}", f.ty, f.database_name);
    if let Some(mode) = f.ssl_mode {
        s.push('?');
        if is_mysql(f.ty) {
            s.push_str("ssl-mode=");
            s.push_str(mysql_ssl_mode(mode));
        } else {
            s.push_str("sslmode=");
            s.push_str(mode);
        }
    }
    s
}

fn is_mysql(ty: &str) -> bool {
    ty == "mysql" || ty == "mariadb"
}

/// A form's SSL mode as MySQL's `ssl-mode` value.
fn mysql_ssl_mode(mode: &str) -> &str {
    match mode {
        "disable" => "DISABLED",
        "allow" | "prefer" => "PREFERRED",
        "require" => "REQUIRED",
        "verify-ca" => "VERIFY_CA",
        "verify-full" => "VERIFY_IDENTITY",
        other => other,
    }
}

/// Query parameters TablePlus adds that database drivers don't understand
/// (lower case).
const TABLEPLUS_PARAMS: [&str; 9] = [
    "statuscolor",
    "env",
    "name",
    "tlsmode",
    "useprivatekey",
    "safemodelevel",
    "advancedsafemodelevel",
    "driverversion",
    "lazyload",
];

/// Keys that already set the SSL mode, for Postgres and MySQL alike.
const SSL_MODE_KEYS: [&str; 3] = ["sslmode", "ssl-mode", "ssl_mode"];

/// `s` split at its query: `(before, Some(query), fragment)`.
fn split_query(s: &str) -> (&str, Option<&str>, &str) {
    let (body, fragment) = match s.find('#') {
        Some(i) => s.split_at(i),
        None => (s, ""),
    };
    match body.split_once('?') {
        Some((before, query)) => (before, Some(query), fragment),
        None => (body, None, fragment),
    }
}

fn join_query(before: &str, pairs: &[String], fragment: &str) -> String {
    if pairs.is_empty() {
        format!("{before}{fragment}")
    } else {
        format!("{before}?{}{fragment}", pairs.join("&"))
    }
}

fn key_of(pair: &str) -> String {
    pair.split('=').next().unwrap_or("").to_ascii_lowercase()
}

/// Settled choice B: when `s` sets no SSL mode, a TablePlus `tLSMode` of 0,
/// 1 or 2 is replaced, where it stands, by the type's SSL parameter: prefer,
/// disable or require (`PREFERRED`, `DISABLED`, `REQUIRED` for MySQL and
/// MariaDB). Anything else in the string is left as it is.
pub fn translate_tls_mode(s: &str, ty: &str) -> String {
    let (before, Some(query), fragment) = split_query(s) else {
        return s.to_string();
    };
    let pairs: Vec<&str> = query.split('&').collect();
    if pairs
        .iter()
        .any(|p| SSL_MODE_KEYS.contains(&key_of(p).as_str()))
    {
        return s.to_string();
    }
    let mut changed = false;
    let pairs: Vec<String> = pairs
        .into_iter()
        .map(|p| {
            if key_of(p) != "tlsmode" {
                return p.to_string();
            }
            let mode = match p.split_once('=').map(|(_, v)| v) {
                Some("0") => "prefer",
                Some("1") => "disable",
                Some("2") => "require",
                _ => return p.to_string(),
            };
            changed = true;
            if is_mysql(ty) {
                format!("ssl-mode={}", mysql_ssl_mode(mode))
            } else {
                format!("sslmode={mode}")
            }
        })
        .collect();
    if changed {
        join_query(before, &pairs, fragment)
    } else {
        s.to_string()
    }
}

/// `s` without TablePlus-only query parameters.
fn drop_tableplus_params(s: &str) -> String {
    let (before, Some(query), fragment) = split_query(s) else {
        return s.to_string();
    };
    let pairs: Vec<String> = query
        .split('&')
        .filter(|p| !p.is_empty() && !TABLEPLUS_PARAMS.contains(&key_of(p).as_str()))
        .map(str::to_string)
        .collect();
    join_query(before, &pairs, fragment)
}

/// The SSH part of a TablePlus `+ssh` URL.
#[derive(Clone, PartialEq, Eq)]
pub struct PlusSsh {
    pub host: String,
    /// `None` when the URL gives none (22).
    pub port: Option<u16>,
    pub username: String,
    /// The SSH password in the URL's SSH user info, decoded, if any.
    pub password: Option<String>,
    /// `usePrivateKey=true`: key authentication.
    pub use_private_key: bool,
}

impl std::fmt::Debug for PlusSsh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlusSsh")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("use_private_key", &self.use_private_key)
            .finish()
    }
}

/// A TablePlus `+ssh` URL (row 7b), split into its database URL and its SSH
/// part:
///
/// `postgres+ssh://deploy@bastion:22/alice:pw@db:5432/app?name=Prod&usePrivateKey=true`
/// is `postgres://alice:pw@db:5432/app` over `deploy@bastion:22` with key
/// authentication.
///
/// The database URL keeps its query, except that `tLSMode` is translated
/// ([`translate_tls_mode`]) and the other TablePlus-only parameters are
/// dropped. `None` when `s` isn't a `+ssh` URL, or its SSH part has no host
/// or a port that isn't a number.
pub fn split_plus_ssh(s: &str, ty: &str) -> Option<(String, PlusSsh)> {
    let (scheme, rest) = s.split_once("://")?;
    let db_scheme = scheme.strip_suffix("+ssh")?;
    let (ssh_authority, db_rest) = rest.split_once('/')?;
    let (userinfo, host_port) = match ssh_authority.rsplit_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, ssh_authority),
    };
    let (host, port) = if let Some(bracketed) = host_port.strip_prefix('[') {
        let (h, after) = bracketed.split_once(']')?;
        match after.strip_prefix(':') {
            Some(p) => (h, Some(p.parse::<u16>().ok()?)),
            None if after.is_empty() => (h, None),
            None => return None,
        }
    } else {
        match host_port.rsplit_once(':') {
            Some((h, p)) => (h, Some(p.parse::<u16>().ok()?)),
            None => (host_port, None),
        }
    };
    if host.is_empty() {
        return None;
    }
    let (username, password) = match userinfo {
        Some(u) => match u.split_once(':') {
            Some((user, pw)) => (
                decode_uri_component(user).unwrap_or_else(|| user.to_string()),
                Some(decode_uri_component(pw).unwrap_or_else(|| pw.to_string())),
            ),
            None => (
                decode_uri_component(u).unwrap_or_else(|| u.to_string()),
                None,
            ),
        },
        None => (String::new(), None),
    };
    let use_private_key = split_query(db_rest).1.is_some_and(|q| {
        q.split('&').any(|p| {
            key_of(p) == "useprivatekey"
                && p.split_once('=')
                    .is_some_and(|(_, v)| v.eq_ignore_ascii_case("true"))
        })
    });
    let db_url = format!("{db_scheme}://{db_rest}");
    let db_url = drop_tableplus_params(&translate_tls_mode(&db_url, ty));
    Some((
        db_url,
        PlusSsh {
            host: host.to_string(),
            port,
            username,
            password: password.filter(|p| !p.is_empty()),
            use_private_key,
        },
    ))
}

/// Whether `s` is a TablePlus `+ssh` URL.
pub fn is_plus_ssh(s: &str) -> bool {
    s.split_once("://")
        .is_some_and(|(scheme, _)| scheme.ends_with("+ssh"))
}

/// Settled choice A: `s` with `password` as its password, replacing any it
/// has. In a URL it goes into the user info through `encodeURIComponent`
/// (an empty user gives `scheme://:pw@host`, row 3). In a `key=value;`
/// string the `Password=`/`Pwd=` pair is replaced, or one is added. A URL
/// that can't hold credentials (no host) comes back unchanged.
pub fn put_password(s: &str, password: &str) -> String {
    match Url::parse(s) {
        Ok(mut url) => {
            if url
                .set_password(Some(&encode_uri_component(password)))
                .is_ok()
            {
                url.into()
            } else {
                s.to_string()
            }
        }
        Err(_) => put_key_value_password(s, password),
    }
}

/// A `key=value;` string with its `Password=`/`Pwd=` pair replaced (or
/// added). A value holding `;`, `{` or `}` is braced, `}` doubled.
fn put_key_value_password(s: &str, password: &str) -> String {
    let value = if password.contains([';', '{', '}']) || password.trim() != password {
        format!("{{{}}}", password.replace('}', "}}"))
    } else {
        password.to_string()
    };
    let mut replaced = false;
    let mut parts: Vec<String> = s
        .split(';')
        .filter_map(|part| {
            let key = part
                .split('=')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            if key == "password" || key == "pwd" {
                if replaced {
                    return None;
                }
                replaced = true;
                let name = part.split('=').next().unwrap_or("Password");
                return Some(format!("{name}={value}"));
            }
            Some(part.to_string())
        })
        .collect();
    if !replaced {
        if parts.last().is_some_and(String::is_empty) {
            parts.pop();
        }
        parts.push(format!("Password={value}"));
        parts.push(String::new());
    }
    parts.join(";")
}

/// The passwords a connection string holds itself, for redacting errors:
/// a URL's user-info password (as written and decoded), both parts of a
/// TablePlus `+ssh` URL, or a key=value string's `Password=`/`Pwd=` value.
pub fn passwords_in(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    if is_plus_ssh(s) {
        let (_, rest) = s.split_once("://").unwrap_or(("", s));
        let (ssh_authority, db_rest) = rest.split_once('/').unwrap_or((rest, ""));
        let ssh_userinfo = ssh_authority.rsplit_once('@').map_or("", |(u, _)| u);
        if let Some((_, pw)) = ssh_userinfo.split_once(':') {
            out.push(pw.to_string());
        }
        out.extend(passwords_in(&format!("x://{db_rest}")));
    } else if let Ok(url) = Url::parse(s) {
        if let Some(pw) = url.password() {
            out.push(pw.to_string());
        }
    } else {
        out.extend(key_value_passwords(s));
    }
    let decoded: Vec<String> = out.iter().filter_map(|p| decode_uri_component(p)).collect();
    out.extend(decoded);
    out.retain(|p| !p.is_empty());
    out.sort();
    out.dedup();
    out
}

/// The `Password=`/`Pwd=` values of a `key=value;` string, braced values
/// (`{a;b}}c}`) read to their closing brace.
fn key_value_passwords(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = s;
    while !rest.is_empty() {
        let (key, after) = match rest.split_once('=') {
            Some(kv) => kv,
            None => break,
        };
        let key = key.trim().to_ascii_lowercase();
        let after = after.trim_start();
        let (value, next) = if let Some(braced) = after.strip_prefix('{') {
            let mut value = String::new();
            let mut chars = braced.char_indices().peekable();
            let mut end = braced.len();
            while let Some((i, c)) = chars.next() {
                if c == '}' {
                    if chars.peek().map(|(_, c)| *c) == Some('}') {
                        value.push('}');
                        chars.next();
                        continue;
                    }
                    end = i + 1;
                    break;
                }
                value.push(c);
            }
            let tail = &braced[end..];
            (value, tail.split_once(';').map_or("", |(_, t)| t))
        } else {
            match after.split_once(';') {
                Some((v, t)) => (v.trim().to_string(), t),
                None => (after.trim().to_string(), ""),
            }
        };
        if key == "password" || key == "pwd" {
            out.push(value);
        }
        rest = next;
    }
    out
}

/// The host and port a URL string connects to: the host without IPv6
/// brackets, and the port or `default`. `None` when `s` isn't a URL or has
/// no host.
pub fn url_host_port(s: &str, default: u16) -> Option<(String, u16)> {
    let url = Url::parse(s).ok()?;
    let host = match url.host()? {
        Host::Domain("") => return None,
        Host::Domain(d) => d.to_string(),
        Host::Ipv4(a) => a.to_string(),
        Host::Ipv6(a) => a.to_string(),
    };
    Some((host, url.port().unwrap_or(default)))
}

/// The SSH tunnel's rewrite: the URL's host becomes `127.0.0.1` and its
/// port the tunnel's, everything else kept. `None` when `s` isn't a URL.
pub fn through_tunnel(s: &str, port: u16) -> Option<String> {
    let mut url = Url::parse(s).ok()?;
    url.set_host(Some("127.0.0.1")).ok()?;
    url.set_port(Some(port)).ok()?;
    Some(url.into())
}

/// Whether `s` parses as a URL, which the SSH rewrite needs.
pub fn parses_as_url(s: &str) -> bool {
    Url::parse(s).is_ok()
}

/// The username fallback (row 11): the URL's user, percent-decoded. `None`
/// for SQLite and DuckDB strings, strings that aren't URLs, URLs without a
/// user, and a user that doesn't decode.
pub fn username_from_url(s: &str) -> Option<String> {
    if s.starts_with("sqlite") || s.starts_with("duckdb") {
        return None;
    }
    let url = Url::parse(s).ok()?;
    if url.username().is_empty() {
        return None;
    }
    decode_uri_component(url.username())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_uri_component_matches_javascript() {
        assert_eq!(
            encode_uri_component("a+b&c=d e%f/g?h#i@j:k$l,m;"),
            "a%2Bb%26c%3Dd%20e%25f%2Fg%3Fh%23i%40j%3Ak%24l%2Cm%3B"
        );
        assert_eq!(encode_uri_component("-_.!~*'()"), "-_.!~*'()");
        assert_eq!(
            encode_uri_component("pässwörd✓"),
            "p%C3%A4ssw%C3%B6rd%E2%9C%93"
        );
    }

    #[test]
    fn decode_uri_component_fails_where_javascript_throws() {
        assert_eq!(
            decode_uri_component("j%C3%BCrgen").as_deref(),
            Some("jürgen")
        );
        assert_eq!(decode_uri_component("a+b").as_deref(), Some("a+b"));
        assert_eq!(decode_uri_component("50%off"), None);
        assert_eq!(decode_uri_component("%"), None);
        assert_eq!(decode_uri_component("%C3"), None);
    }

    #[test]
    fn the_password_is_percent_encoded_and_the_scheme_kept() {
        let s = "postgresql://alice@db.example.com:5432/app";
        assert_eq!(
            put_password(s, "50%off"),
            "postgresql://alice:50%25off@db.example.com:5432/app"
        );
        assert_eq!(
            put_password("postgresql://alice:old@db:5432/app", "new"),
            "postgresql://alice:new@db:5432/app"
        );
        assert_eq!(
            put_password("postgres://db.example.com/app", "pw"),
            "postgres://:pw@db.example.com/app"
        );
        // Round trip: the driver decodes what was encoded.
        let url = Url::parse(&put_password(s, "p@ss:w/rd#?%$,")).unwrap();
        assert_eq!(
            decode_uri_component(url.password().unwrap()).as_deref(),
            Some("p@ss:w/rd#?%$,")
        );
    }

    #[test]
    fn a_key_value_string_gets_its_password_pair_replaced_or_added() {
        assert_eq!(
            put_password("Server=x;User Id=sa;Password=old;", "new"),
            "Server=x;User Id=sa;Password=new;"
        );
        assert_eq!(put_password("Server=x;pwd=old", "new"), "Server=x;pwd=new");
        assert_eq!(
            put_password("Server=x;User Id=sa;", "a;b}"),
            "Server=x;User Id=sa;Password={a;b}}};"
        );
    }

    #[test]
    fn tls_mode_is_translated_in_place_unless_a_mode_is_set() {
        assert_eq!(
            translate_tls_mode("postgres://h/app?a=1&tLSMode=2&b=2", "postgres"),
            "postgres://h/app?a=1&sslmode=require&b=2"
        );
        assert_eq!(
            translate_tls_mode("mysql://h/app?tLSMode=0", "mysql"),
            "mysql://h/app?ssl-mode=PREFERRED"
        );
        assert_eq!(
            translate_tls_mode("postgres://h/app?sslmode=disable&tLSMode=2", "postgres"),
            "postgres://h/app?sslmode=disable&tLSMode=2"
        );
        assert_eq!(
            translate_tls_mode("postgres://h/app?tLSMode=7", "postgres"),
            "postgres://h/app?tLSMode=7"
        );
    }

    #[test]
    fn a_plus_ssh_url_splits_into_its_parts() {
        let (db, ssh) = split_plus_ssh(
            "postgres+ssh://deploy:s%40sh@bastion.example.com:2200/alice@db:5432/app?name=P&usePrivateKey=true&application_name=x&tLSMode=2",
            "postgres",
        )
        .unwrap();
        assert_eq!(
            db,
            "postgres://alice@db:5432/app?application_name=x&sslmode=require"
        );
        assert_eq!(ssh.host, "bastion.example.com");
        assert_eq!(ssh.port, Some(2200));
        assert_eq!(ssh.username, "deploy");
        assert_eq!(ssh.password.as_deref(), Some("s@sh"));
        assert!(ssh.use_private_key);
        assert!(!format!("{ssh:?}").contains("s@sh"));
        assert!(split_plus_ssh("postgres://a@b/c", "postgres").is_none());
    }

    #[test]
    fn built_urls_follow_decision_6() {
        let f = |ty, port, mode| {
            build_url(&UrlFields {
                ty,
                host: "h",
                port,
                database_name: "d",
                username: "u",
                ssl_mode: mode,
            })
        };
        assert_eq!(f("postgres", 0, None), "postgres://u@h/d");
        assert_eq!(
            f("postgres", 5432, Some("require")),
            "postgres://u@h/d?sslmode=require"
        );
        assert_eq!(
            f("mysql", 3307, Some("verify-ca")),
            "mysql://u@h:3307/d?ssl-mode=VERIFY_CA"
        );
        assert_eq!(
            f("mariadb", 3306, Some("verify-full")),
            "mariadb://u@h/d?ssl-mode=VERIFY_IDENTITY"
        );
        let v6 = build_url(&UrlFields {
            ty: "postgres",
            host: "::1",
            port: 5433,
            database_name: "d",
            username: "",
            ssl_mode: None,
        });
        assert_eq!(v6, "postgres://[::1]:5433/d");
        assert_eq!(url_host_port(&v6, 5432), Some(("::1".into(), 5433)));
    }

    #[test]
    fn the_tunnel_rewrite_keeps_the_rest() {
        assert_eq!(
            through_tunnel("postgresql://a:b@[::1]:6000/app?x=1", 50001).as_deref(),
            Some("postgresql://a:b@127.0.0.1:50001/app?x=1")
        );
        assert_eq!(through_tunnel("Server=x;", 1), None);
    }

    #[test]
    fn passwords_in_a_string_are_found() {
        assert_eq!(passwords_in("postgres://a:p%40ss@h/db"), ["p%40ss", "p@ss"]);
        assert_eq!(passwords_in("Server=x;Pwd={a;b}}};User=u"), ["a;b}"]);
        assert_eq!(
            passwords_in("postgres+ssh://u:sshpw@b:22/a:dbpw@h:5432/app"),
            ["dbpw", "sshpw"]
        );
        assert!(passwords_in("postgres://a@h/db").is_empty());
    }

    #[test]
    fn js_number_prints_integers_without_a_fraction() {
        assert_eq!(js_number(5433.0), "5433");
        assert_eq!(js_number(0.5), "0.5");
    }
}
