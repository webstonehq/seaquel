//! A saved connection plus its keychain secrets, turned into the
//! `ConnectConfig` (and SSH `TunnelConfig`) the desktop app would connect
//! with. A port of the TypeScript in `connection-manager.svelte.ts`
//! (`initializePersistedConnections`, `autoReconnect`, `reconnect`,
//! `setupSshTunnel`), `connection-tabs.svelte.ts` (the reconnect tab's
//! prefill), `connection-string.ts` and `wire.ts` (`toRustConfig`).
//!
//! The frozen fixtures in `tests/fixtures/connect-config` are the spec; their
//! README says which of the app's two paths each case follows:
//!
//! - **autoReconnect**, the app's non-interactive path, for everything it
//!   can connect offline;
//! - **the reconnect tab's rebuild** (`getConnectionData` and
//!   `buildConnectionString`) where autoReconnect can't: a Postgres, MySQL,
//!   MariaDB or SQLite row with no stored string, and a row whose string the
//!   SSH rewrite can't parse (a key=value MSSQL string);
//! - `CREDENTIALS_REQUIRED` where the app would wait for the user to type
//!   something.
//!
//! Secrets always follow autoReconnect's rules ([`read_secrets`]), and there
//! is no retry with the tab's config after a failure: the caller connects
//! once with what [`build_config`] returns.
//!
//! The call order is [`read_secrets`], then [`tunnel_config`] (open the
//! tunnel it returns, if any), then [`build_config`] with the tunnel's local
//! port.
//!
//! Two deliberate differences from the TS as recorded, made on both sides in
//! phase 4, Task 3 (see the fixtures README's "Changes"): reinjection
//! percent-encodes the password, and MSSQL trusts any certificate only for
//! `disable`, `allow`, `prefer` or no mode.

use std::fmt;
use std::future::Future;

use seaquel_types::ssh::TunnelConfig;
use seaquel_types::storage::PersistedConnection;
use seaquel_types::{ConnectConfig, DriverType};
use serde_json::Value;

use crate::connection_string::{
    connection_data_string, js_number, parses_as_url, reinject_password, rewrite_host_port,
    username_from_url, FormData,
};

/// The app would need the user to type something: a password that isn't
/// saved, an SSH password or key file, or a missing field.
pub const CREDENTIALS_REQUIRED: &str = "CREDENTIALS_REQUIRED";

/// The row can't be turned into a config at all: an unknown type, a port out
/// of range, a string the SSH rewrite can't parse, or a tunnelled row built
/// without its tunnel's port.
pub const INVALID_CONNECTION: &str = "INVALID_CONNECTION";

/// Why a saved connection can't be connected. Messages name the connection
/// and never include a secret or the connection string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub code: &'static str,
    pub message: String,
}

impl ConfigError {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: INVALID_CONNECTION,
            message: message.into(),
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ConfigError {}

/// The keychain secrets a connection uses, as [`read_secrets`] returns them.
/// An empty secret is kept as `None`. `Debug` redacts the values.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secrets {
    /// `db:<id>`: the database password.
    pub db: Option<String>,
    /// `ssh:<id>`: the SSH password.
    pub ssh: Option<String>,
    /// `ssh-key:<id>`: the SSH key's passphrase.
    pub ssh_key: Option<String>,
    /// Reads that failed (a denied prompt, a locked keychain), with the
    /// store's error code. The TS treats those as no secret, and so does
    /// this; they are kept so a caller can refuse to connect without them
    /// (`Workspace::connect_saved` does, with `SECRET_UNREADABLE`).
    pub unreadable: Vec<UnreadableSecret>,
}

/// A keychain read that failed: its key and the store's error code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadableSecret {
    pub key: String,
    pub code: String,
}

impl fmt::Debug for Secrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted = |v: &Option<String>| v.as_ref().map(|_| "<redacted>");
        f.debug_struct("Secrets")
            .field("db", &redacted(&self.db))
            .field("ssh", &redacted(&self.ssh))
            .field("ssh_key", &redacted(&self.ssh_key))
            .field("unreadable", &self.unreadable)
            .finish()
    }
}

/// The keychain key of a connection's database password.
pub fn db_key(id: &str) -> String {
    format!("db:{id}")
}

/// The keychain key of a connection's SSH password.
pub fn ssh_key(id: &str) -> String {
    format!("ssh:{id}")
}

/// The keychain key of a connection's SSH key passphrase.
pub fn ssh_key_passphrase_key(id: &str) -> String {
    format!("ssh-key:{id}")
}

/// Read the secrets connecting `row` needs, the way autoReconnect does, and
/// give up where it does. `get` reads one keychain key and fails with the
/// store's error code; a failed read counts as no secret (and is listed in
/// [`Secrets::unreadable`]).
///
/// - SQLite and DuckDB read nothing.
/// - `db:<id>` is read only when `savePassword` is on. With it off, this
///   gives up (`CREDENTIALS_REQUIRED`) before reading anything else, even if
///   the stored string still carries a password. With it on and nothing
///   saved, the connection is taken to be passwordless.
/// - With an enabled SSH tunnel, `ssh:<id>` is read when `saveSshPassword`
///   is on and `ssh-key:<id>` when `saveSshKeyPassphrase` is on. Password
///   auth without an SSH password, and key auth without a key file, give up.
/// - Where the reconnect tab's rebuild applies, a row missing what the tab
///   requires (a name, a database, a host, the SSH host or user) gives up
///   too.
pub async fn read_secrets<F, Fut>(
    row: &PersistedConnection,
    mut get: F,
) -> Result<Secrets, ConfigError>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<Option<String>, String>>,
{
    let conn = Conn::new(row)?;
    let mut secrets = Secrets::default();
    if !conn.kind.is_file() {
        if row.save_password {
            let key = db_key(&row.id);
            secrets.db = read(&mut get, key, &mut secrets.unreadable).await;
        }
        // The password check comes before any SSH secret is read.
        conn.check_password(&secrets)?;
        if conn.ssh.is_some() {
            if row.save_ssh_password {
                let key = ssh_key(&row.id);
                secrets.ssh = read(&mut get, key, &mut secrets.unreadable).await;
            }
            if row.save_ssh_key_passphrase {
                let key = ssh_key_passphrase_key(&row.id);
                secrets.ssh_key = read(&mut get, key, &mut secrets.unreadable).await;
            }
        }
    }
    conn.check(&secrets)?;
    Ok(secrets)
}

async fn read<F, Fut>(
    get: &mut F,
    key: String,
    unreadable: &mut Vec<UnreadableSecret>,
) -> Option<String>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<Option<String>, String>>,
{
    match get(key.clone()).await {
        Ok(value) => value.filter(|v| !v.is_empty()),
        Err(code) => {
            unreadable.push(UnreadableSecret { key, code });
            None
        }
    }
}

/// The SSH tunnel to open before connecting `row`, or `None` when it has no
/// enabled tunnel (SQLite and DuckDB never do: autoReconnect ignores their
/// tunnel). `trust_host_key` is the fingerprint the user approved, or `None`
/// to accept only a host already in known_hosts.
///
/// It forwards to the row's host and port, whatever the stored string says
/// (quirk 7). On autoReconnect's path, the password and passphrase are sent
/// only when saved; on the reconnect tab's they are always sent, as `""`
/// when missing, and so is the key path (quirk 5).
///
/// Gives up like [`read_secrets`] when `secrets` lack something required.
pub fn tunnel_config(
    row: &PersistedConnection,
    secrets: &Secrets,
    trust_host_key: Option<String>,
) -> Result<Option<TunnelConfig>, ConfigError> {
    let conn = Conn::new(row)?;
    conn.check(secrets)?;
    let Some(ssh) = &conn.ssh else {
        return Ok(None);
    };
    let config = match conn.path {
        Path::AutoReconnect => TunnelConfig {
            ssh_host: ssh.host.clone(),
            ssh_port: conn.port(ssh.port, "SSH port")?,
            ssh_username: ssh.username.clone(),
            auth_method: ssh.auth_method.clone(),
            password: secrets.ssh.clone().filter(|s| !s.is_empty()),
            key_path: ssh.key_path.clone(),
            key_passphrase: secrets.ssh_key.clone().filter(|s| !s.is_empty()),
            remote_host: row.host.clone(),
            remote_port: conn.port(Some(row.port), "port")?,
            trust_host_key,
        },
        Path::ReconnectTab => {
            let form = conn.form(secrets);
            let tab = conn.tab_ssh(ssh);
            TunnelConfig {
                ssh_host: tab.host.to_string(),
                ssh_port: conn.port(Some(tab.port), "SSH port")?,
                ssh_username: tab.username.to_string(),
                auth_method: tab.auth_method.to_string(),
                password: Some(secrets.ssh.clone().unwrap_or_default()),
                key_path: Some(tab.key_path.to_string()),
                key_passphrase: Some(secrets.ssh_key.clone().unwrap_or_default()),
                remote_host: form.host.to_string(),
                remote_port: conn.port(Some(form.port), "port")?,
                trust_host_key,
            }
        }
    };
    Ok(Some(config))
}

/// The config to open `row` with on Core. `tunnel_port` is the local port of
/// the tunnel [`tunnel_config`] asked for; it is required when there is one
/// and ignored otherwise.
///
/// - **Postgres, MySQL, MariaDB** (MariaDB on the `mysql` driver): the
///   stored string, or the tab's rebuild when there is none. Through a
///   tunnel its host and port become `127.0.0.1:<tunnel_port>`. A saved
///   password is put into its user info, percent-encoded; without one the
///   stored string goes out verbatim (`postgresql://` included).
/// - **SQLite**: the stored string, or `sqlite://<databaseName>`;
///   `create_if_missing` off.
/// - **DuckDB**: the string without `duckdb://` or `duckdb:` (query
///   parameters kept), `:memory:` when that leaves nothing, and the database
///   name (or `:memory:`) when there is no string.
/// - **MSSQL**: the row's fields; the string is ignored. `encrypt` unless the
///   mode is `disable`; `trust_cert` only for `disable`, `allow`, `prefer`
///   or no mode.
///
/// Gives up like [`read_secrets`] when `secrets` lack something required.
pub fn build_config(
    row: &PersistedConnection,
    secrets: &Secrets,
    tunnel_port: Option<u16>,
) -> Result<ConnectConfig, ConfigError> {
    let conn = Conn::new(row)?;
    conn.check(secrets)?;
    let tunnel_port = match conn.ssh {
        Some(_) => Some(tunnel_port.ok_or_else(|| {
            ConfigError::invalid(format!(
                "Connection {:?} goes through an SSH tunnel; its local port is required",
                row.name
            ))
        })?),
        None => None,
    };
    let form = conn.form(secrets);
    let password = secrets.db.as_deref().filter(|p| !p.is_empty());
    // The string `reconnect` starts from.
    let string = match conn.path {
        Path::AutoReconnect => conn.connection_string.map(str::to_string),
        Path::ReconnectTab => Some(connection_data_string(&form)),
    };
    // `setupSshTunnel`'s rewrite. MSSQL runs it too (and discards the
    // result), so a string it can't parse fails there as well.
    let string = match (string, tunnel_port) {
        (Some(s), Some(port)) => Some(rewrite_host_port(&s, port).ok_or_else(|| {
            ConfigError::invalid(format!(
                "Connection {:?}: its connection string isn't a URL, so it can't go through \
                 the SSH tunnel",
                row.name
            ))
        })?),
        (s, _) => s,
    };

    let mut config = empty(conn.kind.driver());
    match conn.kind {
        Kind::Duckdb => {
            let path = match conn.connection_string {
                Some(s) => {
                    let rest = s.strip_prefix("duckdb://").unwrap_or(s);
                    let rest = rest.strip_prefix("duckdb:").unwrap_or(rest);
                    if rest.is_empty() {
                        ":memory:"
                    } else {
                        rest
                    }
                }
                None if row.database_name.is_empty() => ":memory:",
                None => &row.database_name,
            };
            config.path = Some(path.to_string());
        }
        Kind::Sqlite => {
            config.connection_string = string;
            config.create_if_missing = Some(false);
        }
        Kind::Mssql => {
            let (ssl_mode, host, port) = match conn.path {
                Path::AutoReconnect => (row.ssl_mode.as_deref(), row.host.as_str(), row.port),
                Path::ReconnectTab => (Some(form.ssl_mode), form.host, form.port),
            };
            config.host = Some(match tunnel_port {
                Some(_) => "127.0.0.1".to_string(),
                None => host.to_string(),
            });
            config.port = Some(match tunnel_port {
                Some(port) => port,
                None => conn.port(Some(port), "port")?,
            });
            config.database = Some(row.database_name.clone());
            config.username = Some(conn.username.clone());
            config.password = Some(password.unwrap_or_default().to_string());
            config.encrypt = Some(ssl_mode != Some("disable"));
            config.trust_cert = Some(mssql_trusts_any_cert(ssl_mode));
        }
        Kind::Postgres | Kind::Mysql | Kind::Mariadb => {
            config.connection_string = match (string, password) {
                (Some(s), Some(pw)) => Some(reinject_password(&s, pw)),
                (s, _) => s,
            };
        }
    }
    Ok(config)
}

/// Whether an MSSQL connection with this `sslMode` accepts any server
/// certificate: only for `disable`, `allow`, `prefer` and no mode (unset or
/// `""`). `require`, `verify-ca`, `verify-full` and anything else verify it.
/// The TS used to trust for everything but `require` (phase 4, Task 3 fixed
/// both sides).
pub fn mssql_trusts_any_cert(ssl_mode: Option<&str>) -> bool {
    matches!(ssl_mode, None | Some("" | "disable" | "allow" | "prefer"))
}

fn empty(driver: DriverType) -> ConnectConfig {
    ConnectConfig {
        driver,
        connection_string: None,
        host: None,
        port: None,
        database: None,
        username: None,
        password: None,
        encrypt: None,
        trust_cert: None,
        path: None,
        create_if_missing: None,
        restricted: None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Postgres,
    Mysql,
    Mariadb,
    Sqlite,
    Duckdb,
    Mssql,
}

impl Kind {
    fn is_file(self) -> bool {
        matches!(self, Kind::Sqlite | Kind::Duckdb)
    }

    fn driver(self) -> DriverType {
        match self {
            Kind::Postgres => DriverType::Postgres,
            Kind::Mysql | Kind::Mariadb => DriverType::Mysql,
            Kind::Sqlite => DriverType::Sqlite,
            Kind::Duckdb => DriverType::Duckdb,
            Kind::Mssql => DriverType::Mssql,
        }
    }
}

/// Which of the app's paths a row follows (module docs).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Path {
    AutoReconnect,
    ReconnectTab,
}

/// The row's `sshTunnel`, when enabled.
struct Ssh {
    host: String,
    port: Option<f64>,
    username: String,
    auth_method: String,
    key_path: Option<String>,
}

/// The reconnect tab's SSH fields (`connection-tabs.svelte.ts` `open`).
struct TabSsh<'a> {
    host: &'a str,
    port: f64,
    username: &'a str,
    auth_method: &'a str,
    key_path: &'a str,
}

/// What's missing when the app would wait for the user.
enum Missing {
    Password,
    SshPassword,
    SshKeyPath,
    Field(&'static str),
}

/// A row as the app holds it after loading: the URL-username fallback
/// applied, the tunnel parsed, and its path decided.
struct Conn<'a> {
    row: &'a PersistedConnection,
    kind: Kind,
    /// The stored string; `""` counts as none.
    connection_string: Option<&'a str>,
    username: String,
    /// Only for server engines: autoReconnect ignores a file engine's tunnel.
    ssh: Option<Ssh>,
    path: Path,
}

impl<'a> Conn<'a> {
    fn new(row: &'a PersistedConnection) -> Result<Self, ConfigError> {
        let kind = match row.ty.as_str() {
            "postgres" => Kind::Postgres,
            "mysql" => Kind::Mysql,
            "mariadb" => Kind::Mariadb,
            "sqlite" => Kind::Sqlite,
            "duckdb" => Kind::Duckdb,
            "mssql" => Kind::Mssql,
            other => {
                return Err(ConfigError::invalid(format!(
                    "Connection {:?} has an unknown database type {other:?}",
                    row.name
                )))
            }
        };
        let connection_string = row.connection_string.as_deref().filter(|s| !s.is_empty());
        let username = match connection_string {
            Some(s) if row.username.is_empty() => username_from_url(s).unwrap_or_default(),
            _ => row.username.clone(),
        };
        let ssh = if kind.is_file() { None } else { parse_ssh(row) };
        let unparseable = || connection_string.is_some_and(|s| !parses_as_url(s));
        let path = match kind {
            Kind::Duckdb => Path::AutoReconnect,
            Kind::Sqlite if connection_string.is_none() => Path::ReconnectTab,
            Kind::Sqlite => Path::AutoReconnect,
            Kind::Mssql if ssh.is_some() && unparseable() => Path::ReconnectTab,
            Kind::Mssql => Path::AutoReconnect,
            _ if connection_string.is_none() || (ssh.is_some() && unparseable()) => {
                Path::ReconnectTab
            }
            _ => Path::AutoReconnect,
        };
        Ok(Self {
            row,
            kind,
            connection_string,
            username,
            ssh,
            path,
        })
    }

    /// autoReconnect's first give-up: no password and `savePassword` off.
    fn check_password(&self, secrets: &Secrets) -> Result<(), ConfigError> {
        let has_password = secrets.db.as_deref().is_some_and(|p| !p.is_empty());
        if !self.kind.is_file() && !has_password && !self.row.save_password {
            return Err(self.missing(Missing::Password, secrets));
        }
        Ok(())
    }

    /// Every give-up: autoReconnect's, then (on the tab's path) what
    /// `hasAllCredentials` requires before the tab connects on its own.
    fn check(&self, secrets: &Secrets) -> Result<(), ConfigError> {
        self.check_password(secrets)?;
        let has_ssh_password = secrets.ssh.as_deref().is_some_and(|p| !p.is_empty());
        if let Some(ssh) = &self.ssh {
            if ssh.auth_method == "password" && !has_ssh_password {
                return Err(self.missing(Missing::SshPassword, secrets));
            }
            if ssh.auth_method == "key" && ssh.key_path.as_deref().unwrap_or("").is_empty() {
                return Err(self.missing(Missing::SshKeyPath, secrets));
            }
        }
        if self.path == Path::ReconnectTab {
            let form = self.form(secrets);
            if self.row.name.trim().is_empty() {
                return Err(self.missing(Missing::Field("name"), secrets));
            }
            if form.database_name.trim().is_empty() {
                return Err(self.missing(Missing::Field("database"), secrets));
            }
            if !self.kind.is_file() && form.host.trim().is_empty() {
                return Err(self.missing(Missing::Field("host"), secrets));
            }
            if let Some(ssh) = &self.ssh {
                let tab = self.tab_ssh(ssh);
                if tab.host.trim().is_empty() {
                    return Err(self.missing(Missing::Field("SSH host"), secrets));
                }
                if tab.username.trim().is_empty() {
                    return Err(self.missing(Missing::Field("SSH username"), secrets));
                }
                if tab.auth_method == "password" && !has_ssh_password {
                    return Err(self.missing(Missing::SshPassword, secrets));
                }
                if tab.auth_method == "key" && tab.key_path.is_empty() {
                    return Err(self.missing(Missing::SshKeyPath, secrets));
                }
            }
        }
        Ok(())
    }

    fn missing(&self, what: Missing, secrets: &Secrets) -> ConfigError {
        let name = &self.row.name;
        let unreadable = |key: String| secrets.unreadable.iter().any(|u| u.key == key);
        let message = match what {
            Missing::Password => format!(
                "Connection {name:?} has no saved password. Open it in the Seaquel app, enter \
                 the password with \"Save password in keychain\" on, and connect once."
            ),
            Missing::SshPassword if unreadable(ssh_key(&self.row.id)) => format!(
                "Seaquel couldn't read the SSH password of connection {name:?} from the \
                 keychain. Allow access when the system asks, or open the connection in the \
                 Seaquel app and save its SSH password again."
            ),
            Missing::SshPassword => format!(
                "Connection {name:?} goes through an SSH tunnel whose password isn't saved. \
                 Open it in the Seaquel app, enter the SSH password with saving on, and connect \
                 once."
            ),
            Missing::SshKeyPath => format!(
                "Connection {name:?} uses SSH key authentication but has no key file. Open it in \
                 the Seaquel app, choose the key file, and connect once."
            ),
            Missing::Field(field) => format!(
                "Connection {name:?} has no {field}. Open it in the Seaquel app, fill it in, and \
                 connect once."
            ),
        };
        ConfigError {
            code: CREDENTIALS_REQUIRED,
            message,
        }
    }

    /// The reconnect tab's form (`connection-tabs.svelte.ts` `open`, with
    /// the credentials loaded): an empty host becomes `localhost`, port 0
    /// becomes 5432, and an unset `sslMode` becomes `disable` (quirk 5).
    fn form(&self, secrets: &'a Secrets) -> FormData<'_> {
        let row = self.row;
        FormData {
            ty: &row.ty,
            host: if row.host.is_empty() {
                "localhost"
            } else {
                &row.host
            },
            port: truthy_or(row.port, 5432.0),
            database_name: &row.database_name,
            username: &self.username,
            password: secrets.db.as_deref().unwrap_or(""),
            ssl_mode: row
                .ssl_mode
                .as_deref()
                .filter(|m| !m.is_empty())
                .unwrap_or("disable"),
            connection_string: self.connection_string.unwrap_or(""),
        }
    }

    fn tab_ssh<'s>(&self, ssh: &'s Ssh) -> TabSsh<'s> {
        TabSsh {
            host: &ssh.host,
            port: truthy_or(ssh.port.unwrap_or(0.0), 22.0),
            username: &ssh.username,
            auth_method: if ssh.auth_method.is_empty() {
                "password"
            } else {
                &ssh.auth_method
            },
            key_path: ssh.key_path.as_deref().unwrap_or(""),
        }
    }

    /// A JSON number as a port, which Core's `u16` fields need.
    fn port(&self, n: Option<f64>, what: &str) -> Result<u16, ConfigError> {
        n.filter(|n| n.fract() == 0.0 && (0.0..=65535.0).contains(n))
            .map(|n| n as u16)
            .ok_or_else(|| {
                let shown = n.map_or_else(|| "none".to_string(), js_number);
                ConfigError::invalid(format!(
                    "Connection {:?} has an invalid {what}: {shown}",
                    self.row.name
                ))
            })
    }
}

/// JavaScript's `n || fallback` for a number.
fn truthy_or(n: f64, fallback: f64) -> f64 {
    if n == 0.0 || n.is_nan() {
        fallback
    } else {
        n
    }
}

/// JavaScript truthiness of a JSON value.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// The row's `sshTunnel` when it is an object whose `enabled` is truthy.
fn parse_ssh(row: &PersistedConnection) -> Option<Ssh> {
    let raw = row.ssh_tunnel.as_ref()?;
    let v: Value = serde_json::from_str(raw.get()).ok()?;
    let obj = v.as_object()?;
    if !obj.get("enabled").is_some_and(truthy) {
        return None;
    }
    let text = |k: &str| obj.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    Some(Ssh {
        host: text("host"),
        port: obj.get("port").and_then(Value::as_f64),
        username: text("username"),
        auth_method: text("authMethod"),
        key_path: obj
            .get("keyPath")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mssql_verifies_the_certificate_unless_the_mode_says_not_to() {
        for mode in [
            None,
            Some(""),
            Some("disable"),
            Some("allow"),
            Some("prefer"),
        ] {
            assert!(mssql_trusts_any_cert(mode), "{mode:?}");
        }
        for mode in [
            "require",
            "verify-ca",
            "verify-full",
            "VERIFY-FULL",
            "other",
        ] {
            assert!(!mssql_trusts_any_cert(Some(mode)), "{mode}");
        }
    }

    #[test]
    fn secrets_debug_redacts_values() {
        let s = Secrets {
            db: Some("hunter2-db".into()),
            ssh: Some("hunter2-ssh".into()),
            ssh_key: Some("hunter2-key".into()),
            unreadable: vec![UnreadableSecret {
                key: "ssh:x".into(),
                code: "SECRET_STORE_ERROR".into(),
            }],
        };
        let debug = format!("{s:?} {s:#?}");
        assert!(!debug.contains("hunter2"), "{debug}");
        assert!(debug.contains("<redacted>") && debug.contains("ssh:x"));
    }
}
