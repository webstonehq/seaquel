//! Ports of the TypeScript's connection-string helpers: `buildConnectionString`
//! and `getConnectionData`'s rebuild rule (`utils/connection-string.ts`), and
//! the URL steps of `ConnectionManager` (`connection-manager.svelte.ts`):
//! password reinjection, the SSH host and port rewrite, and the
//! URL-username fallback.
//!
//! Each keeps the TypeScript's behaviour, quirks included (the fixtures
//! README lists them), except that reinjection percent-encodes the password
//! (phase 4, Task 3; the TS was changed to match).

use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use url::Url;

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

/// `databaseTypes` (`stores/connection-wizard.svelte.ts`): the first
/// protocol and the default port of a connection type.
fn database_type(ty: &str) -> Option<(&'static str, f64)> {
    Some(match ty {
        "postgres" => ("postgres", 5432.0),
        "mysql" => ("mysql", 3306.0),
        "mariadb" => ("mariadb", 3306.0),
        "sqlite" => ("sqlite", 0.0),
        "duckdb" => ("duckdb", 0.0),
        "mssql" => ("mssql", 1433.0),
        _ => return None,
    })
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

/// The reconnect tab's form fields `buildConnectionString` reads.
#[derive(Clone)]
pub struct FormData<'a> {
    pub ty: &'a str,
    pub host: &'a str,
    pub port: f64,
    pub database_name: &'a str,
    pub username: &'a str,
    pub password: &'a str,
    /// `""` counts as unset, as in the TS.
    pub ssl_mode: &'a str,
    /// The stored string, `""` when there is none.
    pub connection_string: &'a str,
}

/// `buildConnectionString`: a URL from the form fields.
///
/// - SQLite and DuckDB: `sqlite://<path>`, `duckdb://<path or :memory:>`.
/// - Otherwise `<protocol>://[user[:password]@]host[:port]/database`, with
///   the user and password through `encodeURIComponent`, no credentials at
///   all when the username is empty (even with a password), and the port
///   left out when it is the type's default.
/// - `sslmode=` (Postgres) or `ssl-mode=` (MySQL and MariaDB, mapped to
///   `DISABLED`/`PREFERRED`/`REQUIRED`; `verify-ca` and `verify-full` pass
///   through unmapped) when a mode is set.
pub fn build_connection_string(data: &FormData<'_>) -> String {
    if data.ty == "sqlite" {
        return format!("sqlite://{}", data.database_name);
    }
    if data.ty == "duckdb" {
        let name = if data.database_name.is_empty() {
            ":memory:"
        } else {
            data.database_name
        };
        return format!("duckdb://{name}");
    }

    let credentials = if data.username.is_empty() {
        String::new()
    } else if data.password.is_empty() {
        format!("{}@", encode_uri_component(data.username))
    } else {
        format!(
            "{}:{}@",
            encode_uri_component(data.username),
            encode_uri_component(data.password)
        )
    };
    let selected = database_type(data.ty);
    let protocol = selected.map_or(data.ty, |(p, _)| p);
    let port = if selected.map(|(_, d)| d) == Some(data.port) {
        String::new()
    } else {
        format!(":{}", js_number(data.port))
    };
    let mut s = format!(
        "{protocol}://{credentials}{}{port}/{}",
        data.host, data.database_name
    );

    let is_mysql = data.ty == "mysql" || data.ty == "mariadb";
    if (data.ty == "postgres" || is_mysql) && !data.ssl_mode.is_empty() {
        let separator = if s.contains('?') { '&' } else { '?' };
        let (param, value) = if is_mysql {
            let mapped = match data.ssl_mode {
                "disable" => "DISABLED",
                "allow" | "prefer" => "PREFERRED",
                "require" => "REQUIRED",
                other => other,
            };
            ("ssl-mode", mapped)
        } else {
            ("sslmode", data.ssl_mode)
        };
        s.push_str(&format!("{separator}{param}={value}"));
    }
    s
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

/// `isTablePlusUrl`: a `+ssh` scheme, or a TablePlus-only query parameter.
fn is_table_plus_url(s: &str) -> bool {
    // `slice(0, indexOf(":"))`; the caller only asks about strings with a `:`.
    let scheme = s.find(':').map_or(s, |i| &s[..i]);
    if scheme.ends_with("+ssh") {
        return true;
    }
    let Some(q) = s.find('?') else {
        return false;
    };
    url::form_urlencoded::parse(&s.as_bytes()[q + 1..])
        .any(|(key, _)| TABLEPLUS_PARAMS.contains(&key.to_lowercase().as_str()))
}

/// The string `getConnectionData` hands `reconnect`: the stored one with
/// `postgresql://` read as `postgres://`, unless it doesn't split into
/// exactly three parts on `:` or is a TablePlus URL, in which case
/// [`build_connection_string`] rebuilds it from the fields (quirk 4: that
/// drops query parameters, a default port and IPv6 brackets).
pub fn connection_data_string(data: &FormData<'_>) -> String {
    let s = if data.connection_string.is_empty() {
        build_connection_string(data)
    } else {
        data.connection_string
            .replacen("postgresql://", "postgres://", 1)
    };
    if s.is_empty() || s.matches(':').count() != 2 || is_table_plus_url(&s) {
        build_connection_string(data)
    } else {
        s
    }
}

/// `new URL(s.replace("postgresql://", "postgres://"))`: the first
/// `postgresql://` anywhere, as JavaScript's `String.replace` does.
fn parse(s: &str) -> Option<Url> {
    Url::parse(&s.replacen("postgresql://", "postgres://", 1)).ok()
}

/// Whether [`rewrite_host_port`] can parse `s`. `setupSshTunnel` throws
/// `Invalid URL` on a string it can't (a key=value MSSQL string).
pub fn parses_as_url(s: &str) -> bool {
    parse(s).is_some()
}

/// `setupSshTunnel`'s rewrite: the URL's host becomes `127.0.0.1` and its
/// port the tunnel's, re-serialised (so `postgresql://` comes back as
/// `postgres://`). `None` where the TS throws `Invalid URL`. On a TablePlus
/// `+ssh` URL this rewrites the outer (SSH) authority (quirk 7).
pub fn rewrite_host_port(s: &str, port: u16) -> Option<String> {
    let mut url = parse(s)?;
    // The setters are no-ops where the URL can't have a host or port, as in
    // the TS.
    let _ = url.set_host(Some("127.0.0.1"));
    let _ = url.set_port(Some(port));
    Some(url.into())
}

/// `reconnect`'s password reinjection: the URL with `password` in its user
/// info, re-serialised. A string that isn't a URL is returned unchanged.
///
/// The password goes through `encodeURIComponent` first. The TS used to set
/// it raw, and the URL setter leaves `%`, `+`, `&`, `$` and `,` alone, so
/// `50%off` became an invalid escape (phase 4, Task 3 fixed both sides).
/// An empty username gives `postgres://:pw@host/…` (quirk 3).
pub fn reinject_password(s: &str, password: &str) -> String {
    match parse(s) {
        Some(mut url) => {
            // A no-op on a URL that can't have credentials (no host).
            let _ = url.set_password(Some(&encode_uri_component(password)));
            url.into()
        }
        None => s.to_string(),
    }
}

/// `initializePersistedConnections`' username fallback: the URL's user,
/// percent-decoded. `None` for SQLite and DuckDB strings, strings that
/// aren't URLs, URLs without a user, and a user that doesn't decode.
pub fn username_from_url(s: &str) -> Option<String> {
    let replaced = s.replacen("postgresql://", "postgres://", 1);
    if replaced.starts_with("sqlite") || replaced.starts_with("duckdb") {
        return None;
    }
    let url = Url::parse(&replaced).ok()?;
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
    fn reinjection_percent_encodes_the_password() {
        let s = "postgresql://alice@db.example.com:5432/app";
        assert_eq!(
            reinject_password(s, "50%off"),
            "postgres://alice:50%25off@db.example.com:5432/app"
        );
        assert_eq!(
            reinject_password(s, "a+b&c=d e"),
            "postgres://alice:a%2Bb%26c%3Dd%20e@db.example.com:5432/app"
        );
        // Round trip: the driver decodes what was encoded.
        let url = Url::parse(&reinject_password(s, "p@ss:w/rd#?%$,")).unwrap();
        assert_eq!(
            decode_uri_component(url.password().unwrap()).as_deref(),
            Some("p@ss:w/rd#?%$,")
        );
        assert_eq!(reinject_password("Server=x;", "pw"), "Server=x;");
    }

    #[test]
    fn js_number_prints_integers_without_a_fraction() {
        assert_eq!(js_number(5433.0), "5433");
        assert_eq!(js_number(0.5), "0.5");
    }
}
