//! What a web user may put in a connection config (Decision 11b).
//!
//! Next to [`crate::WEB_ENGINES`]: the server's engines connect over the
//! network only. A web user may not point a connection at a file or socket
//! on the server:
//!
//! - TLS certificate and key paths (Postgres `sslrootcert`/`sslcert`/
//!   `sslkey` and their spellings, MySQL `ssl-ca`/`ssl-cert`/`ssl-key`)
//!   would make the server read its own files, and the server's client key
//!   would authenticate the user's connection;
//! - a Unix socket (a Postgres host that is a path, `?host=/…`, MySQL
//!   `?socket=`) or a URL without a host (sqlx then tries the local socket
//!   directories) would reach a database on the server host, where peer
//!   authentication logs in as the server's own OS user.
//!
//! MSSQL takes host and port fields only, and has no certificate path.
//! Other drivers aren't checked here: Core refuses them (`WEB_ENGINES`).
//!
//! Keys are matched without regard to case, a little wider than sqlx (which
//! matches them exactly), so a new spelling in a later sqlx still fails
//! closed if it only differs in case.

use seaquel_types::{ConnectConfig, DbError, DriverType};
use url::Url;

/// The error code for a refused option.
pub const OPTION_NOT_ALLOWED: &str = "CONNECTION_OPTION_NOT_ALLOWED";

/// Postgres URL query keys that name a file on the server.
const POSTGRES_FILE_KEYS: &[&str] = &[
    "sslrootcert",
    "ssl-root-cert",
    "ssl-ca",
    "sslcert",
    "ssl-cert",
    "sslkey",
    "ssl-key",
    "passfile",
];

/// MySQL URL query keys that name a file or socket on the server.
const MYSQL_FILE_KEYS: &[&str] = &[
    "sslca", "ssl-ca", "sslcert", "ssl-cert", "sslkey", "ssl-key", "socket",
];

fn refused(message: String) -> DbError {
    DbError {
        message,
        code: OPTION_NOT_ALLOWED.to_string(),
    }
}

/// `Ok` if `config` only reaches databases over the network, else a
/// `CONNECTION_OPTION_NOT_ALLOWED` error that names the option.
pub fn check_connect_config(config: &ConnectConfig) -> Result<(), DbError> {
    let (name, file_keys): (&str, &[&str]) = match config.driver {
        DriverType::Postgres => ("PostgreSQL", POSTGRES_FILE_KEYS),
        DriverType::Mysql => ("MySQL", MYSQL_FILE_KEYS),
        _ => return Ok(()),
    };
    let Some(conn_str) = config.connection_string.as_deref() else {
        return Ok(()); // the driver refuses a missing string itself
    };
    // What sqlx parses. A string that isn't a URL fails in the driver.
    let Ok(url) = Url::parse(conn_str) else {
        return Ok(());
    };

    let host = url.host_str().unwrap_or("");
    let decoded_host = percent_decode(host);
    if host.is_empty() {
        return Err(refused(format!(
            "A {name} connection on the web app needs a host name. A connection without one \
             would use a local socket on the server."
        )));
    }
    if decoded_host.starts_with('/') {
        return Err(refused(format!(
            "A {name} connection on the web app can't use a Unix socket (\"{decoded_host}\"). \
             Connect to a host name instead."
        )));
    }

    for (key, value) in url.query_pairs() {
        let lower = key.to_ascii_lowercase();
        if file_keys.contains(&lower.as_str()) {
            return Err(refused(format!(
                "The \"{key}\" option isn't available in the web app: it names a file on the \
                 server. Use the desktop app for connections that need client certificates \
                 or a custom CA file."
            )));
        }
        if matches!(config.driver, DriverType::Postgres) && lower == "host" && !value.is_empty() {
            // `host=/path` is a socket; any other `host=` overrides the URL's
            // host, so check it the same way.
            if value.starts_with('/') {
                return Err(refused(format!(
                    "A {name} connection on the web app can't use a Unix socket (\"{value}\"). \
                     Connect to a host name instead."
                )));
            }
        }
    }
    Ok(())
}

fn percent_decode(s: &str) -> String {
    fn hex(b: u8) -> Option<u8> {
        (b as char).to_digit(16).map(|d| d as u8)
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(driver: DriverType, conn_str: &str) -> ConnectConfig {
        serde_json::from_value(serde_json::json!({
            "driver": driver.as_str(),
            "connection_string": conn_str,
        }))
        .unwrap()
    }

    fn code(driver: DriverType, conn_str: &str) -> Option<String> {
        check_connect_config(&config(driver, conn_str))
            .err()
            .map(|e| e.code)
    }

    #[test]
    fn network_urls_pass() {
        for s in [
            "postgres://u:p@db.example.com:5432/app?sslmode=require",
            "postgresql://u@127.0.0.1/app?application_name=x&options=-c%20search_path%3Dfoo",
            "postgres://u@db/app?host=other-host",
        ] {
            assert_eq!(code(DriverType::Postgres, s), None, "{s}");
        }
        for s in [
            "mysql://root@db:3306/app?ssl-mode=REQUIRED",
            "mysql://root@127.0.0.1/app?charset=utf8mb4",
        ] {
            assert_eq!(code(DriverType::Mysql, s), None, "{s}");
        }
    }

    #[test]
    fn postgres_file_and_socket_options_are_refused() {
        for s in [
            "postgres://u@db/app?sslrootcert=/data/auth.db",
            "postgres://u@db/app?ssl-root-cert=/etc/ssl/ca.pem",
            "postgres://u@db/app?ssl-ca=/etc/ssl/ca.pem",
            "postgres://u@db/app?sslcert=/srv/client.crt",
            "postgres://u@db/app?ssl-cert=/srv/client.crt",
            "postgres://u@db/app?sslkey=/srv/client.key",
            "postgres://u@db/app?SSLKEY=/srv/client.key",
            "postgres://u@db/app?ssl%6Bey=/srv/client.key",
            "postgres://u@db/app?passfile=/root/.pgpass",
            "postgres://u@db/app?host=/var/run/postgresql",
            "postgres://%2Fvar%2Frun%2Fpostgresql/app",
            "postgres:///app",
        ] {
            assert_eq!(
                code(DriverType::Postgres, s).as_deref(),
                Some(OPTION_NOT_ALLOWED),
                "{s}"
            );
        }
    }

    #[test]
    fn mysql_file_and_socket_options_are_refused() {
        for s in [
            "mysql://root@db/app?ssl-ca=/etc/ssl/ca.pem",
            "mysql://root@db/app?sslca=/etc/ssl/ca.pem",
            "mysql://root@db/app?ssl-cert=/srv/c.crt",
            "mysql://root@db/app?sslcert=/srv/c.crt",
            "mysql://root@db/app?ssl-key=/srv/c.key",
            "mysql://root@db/app?sslkey=/srv/c.key",
            "mysql://root@db/app?socket=/var/run/mysqld/mysqld.sock",
            "mysql:///app",
        ] {
            assert_eq!(
                code(DriverType::Mysql, s).as_deref(),
                Some(OPTION_NOT_ALLOWED),
                "{s}"
            );
        }
    }

    #[test]
    fn the_message_names_the_option() {
        let err = check_connect_config(&config(
            DriverType::Postgres,
            "postgres://u@db/app?sslkey=/srv/client.key",
        ))
        .unwrap_err();
        assert!(err.message.contains("\"sslkey\""), "{}", err.message);
        assert!(err.message.contains("web app"), "{}", err.message);
    }

    #[test]
    fn other_drivers_and_unparsable_strings_are_left_to_the_driver() {
        let mssql: ConnectConfig = serde_json::from_value(serde_json::json!({
            "driver": "mssql", "host": "db", "port": 1433,
        }))
        .unwrap();
        assert!(check_connect_config(&mssql).is_ok());
        assert_eq!(code(DriverType::Postgres, "not a url"), None);
        // Credentials with no host don't parse (sqlx's parser is the same
        // `url` crate), so the driver refuses it.
        assert!(Url::parse("postgres://u@/app").is_err());
        assert_eq!(code(DriverType::Sqlite, "sqlite:/data/auth.db"), None);
    }
}
