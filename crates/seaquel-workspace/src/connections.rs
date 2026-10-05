//! The connection builder: a saved connection or a filled-in form, plus the
//! secrets the caller supplies and the ones in the secret store, turned into
//! the `ConnectConfig` (and SSH `TunnelConfig`) Core connects with.
//!
//! Both targets go through one function ([`plan`]), so a saved row and a
//! form that describe the same connection get the same config. The v2
//! fixtures in `tests/fixtures/connect-config-v2` are the spec; their README
//! explains every rule. In short:
//!
//! - **Secrets.** Supplied secrets always win. A saved row reads the ones
//!   not supplied from the store under its save flags; a form reads nothing.
//! - **Giving up** (`CREDENTIALS_REQUIRED`): a saved row with no database
//!   password and `savePassword` off; SSH password auth without an SSH
//!   password; SSH key auth without a key file; an empty host or SSH host
//!   where it is used; an empty MSSQL username (SQL Server has no default
//!   login).
//! - **Postgres, MySQL, MariaDB:** the stored or typed string as it is
//!   (TablePlus `tLSMode` translated, `+ssh` URLs split), or one built from
//!   the fields when there is none; the password put into it, replacing any
//!   it has; through a tunnel, its host and port rewritten.
//! - **MSSQL:** the fields, never the string; through a tunnel,
//!   `tls_server_name` is the server's own name.
//! - **SQLite, DuckDB:** never tunnel and never read a secret.
//!
//! The call order is [`plan`], then [`Plan::tunnel`] (open the tunnel it
//! returns, if any), then [`Plan::config`] with the tunnel's local port.

use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;

use seaquel_types::connect::{ConnectionForm, SuppliedSecrets};
use seaquel_types::ssh::TunnelConfig;
use seaquel_types::storage::PersistedConnection;
use seaquel_types::{ConnectConfig, DriverType};
use serde_json::Value;

use crate::connection_string::{
    build_url, default_port, is_plus_ssh, js_number, parses_as_url, passwords_in, put_password,
    split_plus_ssh, through_tunnel, translate_tls_mode, url_host_port, username_from_url,
    UrlFields,
};

/// The user has to supply something first: a password that isn't saved, an
/// SSH password or key file, or a missing field.
pub const CREDENTIALS_REQUIRED: &str = "CREDENTIALS_REQUIRED";

/// The connection can't be turned into a config at all: an unknown type, a
/// port out of range, a string the SSH rewrite can't parse, or a tunnelled
/// connection built without its tunnel's port.
pub const INVALID_CONNECTION: &str = "INVALID_CONNECTION";

/// Why a connection can't be connected. Messages never include a secret or
/// the connection string.
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

/// The secrets a connection uses: supplied, or read from the store. An
/// empty secret is kept as `None`. `Debug` redacts the values.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secrets {
    /// The database password (`db:<id>`).
    pub db: Option<String>,
    /// The SSH password (`ssh:<id>`).
    pub ssh: Option<String>,
    /// The SSH key's passphrase (`ssh-key:<id>`).
    pub ssh_key: Option<String>,
    /// Store reads that failed (a denied prompt, a locked keychain), with the
    /// store's error code. They count as no secret here; Core refuses to
    /// connect without them (`SECRET_UNREADABLE`).
    pub unreadable: Vec<UnreadableSecret>,
}

/// A store read that failed: its key and the store's error code.
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

/// The store key of a connection's database password.
pub fn db_key(id: &str) -> String {
    format!("db:{id}")
}

/// The store key of a connection's SSH password.
pub fn ssh_key(id: &str) -> String {
    format!("ssh:{id}")
}

/// The store key of a connection's SSH key passphrase.
pub fn ssh_key_passphrase_key(id: &str) -> String {
    format!("ssh-key:{id}")
}

/// What to connect.
#[derive(Debug, Clone, Copy)]
pub enum Target<'a> {
    /// A saved connection: its secrets come from the caller first, then the
    /// store under the row's save flags.
    Saved(&'a PersistedConnection),
    /// A filled-in form (add, the reconnect tab, test): only the caller's
    /// secrets, and nothing is read from the store.
    Form(&'a ConnectionForm),
}

/// Turn `target` into a [`Plan`]: pick its secrets, apply the give-up
/// rules, and work out the tunnel and the database address.
///
/// `get` reads one store key and fails with the store's error code; a failed
/// read counts as no secret (and is listed in [`Secrets::unreadable`]). It is
/// called only for a saved row, only for secrets `supplied` lacks, and only
/// under the row's flags, in this order: `db:<id>` (then the password
/// give-up, before anything else is read), `ssh:<id>`, `ssh-key:<id>`.
pub async fn plan<F, Fut>(
    target: Target<'_>,
    supplied: &SuppliedSecrets,
    mut get: F,
) -> Result<Plan, ConfigError>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<Option<String>, String>>,
{
    let mut d = Draft::new(target)?;

    // Row 7b: a TablePlus `+ssh` URL is its database URL plus an SSH part.
    // The connection's own enabled tunnel wins (choice C).
    let mut string = d.string.clone();
    let mut url_ssh_password = None;
    if d.kind.is_url() {
        if let Some(s) = string.as_deref().filter(|s| is_plus_ssh(s)) {
            let (db, ssh) = split_plus_ssh(s, &d.ty).ok_or_else(|| {
                ConfigError::invalid(format!(
                    "{} has a TablePlus SSH URL that can't be read",
                    d.subject()
                ))
            })?;
            string = Some(db);
            if d.ssh.is_none() {
                d.ssh = Some(Ssh {
                    host: ssh.host,
                    port: ssh.port.map_or(0.0, f64::from),
                    username: ssh.username,
                    auth_method: if ssh.use_private_key {
                        "key"
                    } else {
                        "password"
                    }
                    .into(),
                    key_path: None,
                });
                url_ssh_password = ssh.password;
            }
        }
    }
    // Row 11: the string's user, decoded, when the field is empty.
    if d.username.is_empty() {
        if let Some(user) = string.as_deref().and_then(username_from_url) {
            d.username = user;
        }
    }

    let secrets = d.secrets(supplied, &mut get, url_ssh_password).await?;
    d.check(&secrets)?;

    let mut remote = None;
    let endpoint = match d.kind {
        Kind::Duckdb => {
            let (path, config) = match &string {
                Some(s) => duckdb_path(s),
                None if d.database.is_empty() => (":memory:".to_string(), None),
                None => (d.database.clone(), None),
            };
            Endpoint::Duckdb { path, config }
        }
        Kind::Sqlite => Endpoint::Sqlite(
            string
                .clone()
                .unwrap_or_else(|| format!("sqlite://{}", d.database)),
        ),
        Kind::Mssql => {
            if d.host.trim().is_empty() {
                return Err(d.missing(Missing::Field("host"), &secrets));
            }
            // Row 3: SQL Server has no default login; an empty one fails
            // with 18456 "Login failed for user ''" (checked live).
            if d.username.trim().is_empty() {
                return Err(d.missing(Missing::Field("username"), &secrets));
            }
            let port = d.port_or_default(d.port, 1433, "port")?;
            remote = Some((d.host.clone(), port));
            Endpoint::Mssql {
                host: d.host.clone(),
                port,
                database: d.database.clone(),
                username: d.username.clone(),
                password: secrets.db.clone().unwrap_or_default(),
                ssl_mode: d.ssl_mode.clone(),
            }
        }
        Kind::Postgres | Kind::Mysql | Kind::Mariadb => {
            let default = default_port(&d.ty).unwrap_or(0);
            let s = match &string {
                // Row 4: as it is, `tLSMode` translated (settled B).
                Some(s) => translate_tls_mode(s, &d.ty),
                // Row 1: built from the fields.
                None => {
                    if d.host.trim().is_empty() {
                        return Err(d.missing(Missing::Field("host"), &secrets));
                    }
                    let port = d.port_or_default(d.port, 0, "port")?;
                    build_url(&UrlFields {
                        ty: &d.ty,
                        host: &d.host,
                        port,
                        database_name: &d.database,
                        username: &d.username,
                        ssl_mode: d.ssl_mode.as_deref(),
                    })
                }
            };
            // Settled A: the supplied or saved password replaces the string's.
            let s = match &secrets.db {
                Some(pw) => put_password(&s, pw),
                None => s,
            };
            if d.ssh.is_some() {
                if !parses_as_url(&s) {
                    return Err(ConfigError::invalid(format!(
                        "{}: its connection string isn't a URL, so it can't go through the SSH \
                         tunnel",
                        d.subject()
                    )));
                }
                // Row 7c: forward to where the string points.
                remote = Some(match url_host_port(&s, default) {
                    Some(hp) => hp,
                    None => (d.host.clone(), d.port_or_default(d.port, default, "port")?),
                });
            }
            Endpoint::Url(s)
        }
    };

    let tunnel = match (&d.ssh, remote) {
        (Some(ssh), Some((remote_host, remote_port))) => Some(TunnelConfig {
            ssh_host: ssh.host.clone(),
            ssh_port: d.port_or_default(ssh.port, 22, "SSH port")?,
            ssh_username: ssh.username.clone(),
            auth_method: ssh.auth_method.clone(),
            password: secrets.ssh.clone(),
            key_path: ssh.key_path.clone(),
            key_passphrase: secrets.ssh_key.clone(),
            remote_host,
            remote_port,
            trust_host_key: None,
        }),
        _ => None,
    };

    let string_passwords = d.string.as_deref().map(passwords_in).unwrap_or_default();
    Ok(Plan {
        string_passwords,
        name: d.name,
        driver: d.kind.driver(),
        sql_engine: d.kind.sql_engine(),
        secrets,
        tunnel,
        endpoint,
    })
}

/// What [`plan`] worked out: the secrets, the tunnel to open (if any) and
/// the database address. Its `Debug` shows no secret or string.
pub struct Plan {
    name: String,
    /// Passwords the stored or typed string holds itself, for redaction.
    string_passwords: Vec<String>,
    driver: DriverType,
    /// The database type's SQL rules: MariaDB's own, though it connects
    /// with the MySQL driver.
    sql_engine: seaquel_sql::SqlEngine,
    secrets: Secrets,
    tunnel: Option<TunnelConfig>,
    endpoint: Endpoint,
}

enum Endpoint {
    /// Postgres, MySQL, MariaDB: the string, password included.
    Url(String),
    Mssql {
        host: String,
        port: u16,
        database: String,
        username: String,
        password: String,
        ssl_mode: Option<String>,
    },
    Sqlite(String),
    Duckdb {
        path: String,
        config: Option<BTreeMap<String, String>>,
    },
}

impl Plan {
    /// The connection's name (a form's may be empty).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The engine whose quoting and statement rules apply to the connection,
    /// from its database type: `Mariadb` for MariaDB, which connects with
    /// the `mysql` driver but scans `/*M! … */` as code.
    pub fn sql_engine(&self) -> seaquel_sql::SqlEngine {
        self.sql_engine
    }

    /// The secrets it uses, and the store reads that failed.
    pub fn secrets(&self) -> &Secrets {
        &self.secrets
    }

    /// Every secret value an error could echo, for redacting it: the
    /// supplied or stored secrets, and any password the connection string
    /// holds itself (as written and decoded).
    pub fn secret_values(&self) -> Vec<String> {
        let mut out: Vec<String> = [&self.secrets.db, &self.secrets.ssh, &self.secrets.ssh_key]
            .into_iter()
            .flatten()
            .cloned()
            .collect();
        out.extend(self.string_passwords.iter().cloned());
        out
    }

    /// The SSH tunnel to open first, or `None`. `trust_host_key` is the
    /// fingerprint the user approved, or `None` to accept only a host
    /// already in known_hosts.
    pub fn tunnel(&self, trust_host_key: Option<String>) -> Option<TunnelConfig> {
        self.tunnel.clone().map(|mut t| {
            t.trust_host_key = trust_host_key;
            t
        })
    }

    /// The config to open on Core. `tunnel_port` is the local port of the
    /// tunnel [`Plan::tunnel`] asked for: required when there is one, ignored
    /// otherwise. `create_if_missing` applies to SQLite.
    pub fn config(
        &self,
        tunnel_port: Option<u16>,
        create_if_missing: bool,
    ) -> Result<ConnectConfig, ConfigError> {
        let tunnel_port = match (&self.tunnel, tunnel_port) {
            (Some(_), Some(port)) => Some(port),
            (Some(_), None) => {
                return Err(ConfigError::invalid(format!(
                    "Connection {:?} goes through an SSH tunnel; its local port is required",
                    self.name
                )))
            }
            (None, _) => None,
        };
        let mut config = empty(self.driver);
        match &self.endpoint {
            Endpoint::Url(s) => {
                config.connection_string = Some(match tunnel_port {
                    Some(port) => through_tunnel(s, port).ok_or_else(|| {
                        ConfigError::invalid(format!(
                            "Connection {:?}: its connection string can't go through the SSH \
                             tunnel",
                            self.name
                        ))
                    })?,
                    None => s.clone(),
                });
            }
            Endpoint::Mssql {
                host,
                port,
                database,
                username,
                password,
                ssl_mode,
            } => {
                match tunnel_port {
                    Some(local) => {
                        config.host = Some("127.0.0.1".to_string());
                        config.port = Some(local);
                        // Row 6: the certificate is the server's, not 127.0.0.1's.
                        config.tls_server_name = Some(host.clone());
                    }
                    None => {
                        config.host = Some(host.clone());
                        config.port = Some(*port);
                    }
                }
                config.database = Some(database.clone());
                config.username = Some(username.clone());
                config.password = Some(password.clone());
                let mode = ssl_mode.as_deref();
                config.encrypt = Some(mode != Some("disable"));
                config.trust_cert = Some(mssql_trusts_any_cert(mode));
            }
            Endpoint::Sqlite(s) => {
                config.connection_string = Some(s.clone());
                config.create_if_missing = Some(create_if_missing);
            }
            Endpoint::Duckdb { path, config: opts } => {
                config.path = Some(path.clone());
                config.duckdb_config = opts.clone();
            }
        }
        Ok(config)
    }
}

impl fmt::Debug for Plan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Plan")
            .field("name", &self.name)
            .field("driver", &self.driver)
            .field("secrets", &self.secrets)
            .field("tunnel", &self.tunnel)
            .finish_non_exhaustive()
    }
}

/// Whether an MSSQL connection with this SSL mode accepts any server
/// certificate: only for `disable`, `allow`, `prefer` and no mode (unset or
/// `""`). `require`, `verify-ca`, `verify-full` and anything else verify it.
pub fn mssql_trusts_any_cert(ssl_mode: Option<&str>) -> bool {
    matches!(ssl_mode, None | Some("" | "disable" | "allow" | "prefer"))
}

/// A DuckDB string as a path and its options (row 10): without `duckdb://`
/// or `duckdb:`, the query parsed out, and `:memory:` when nothing is left.
fn duckdb_path(s: &str) -> (String, Option<BTreeMap<String, String>>) {
    let rest = s.strip_prefix("duckdb://").unwrap_or(s);
    let rest = rest.strip_prefix("duckdb:").unwrap_or(rest);
    let (path, query) = match rest.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (rest, None),
    };
    let config: Option<BTreeMap<String, String>> = query
        .map(|q| {
            url::form_urlencoded::parse(q.as_bytes())
                .filter(|(k, _)| !k.is_empty())
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect()
        })
        .filter(|m: &BTreeMap<String, String>| !m.is_empty());
    let path = if path.is_empty() { ":memory:" } else { path };
    (path.to_string(), config)
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
        tls_server_name: None,
        duckdb_config: None,
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

    /// Connects with a URL string (sqlx).
    fn is_url(self) -> bool {
        matches!(self, Kind::Postgres | Kind::Mysql | Kind::Mariadb)
    }

    fn sql_engine(self) -> seaquel_sql::SqlEngine {
        use seaquel_sql::SqlEngine;
        match self {
            Kind::Postgres => SqlEngine::Postgres,
            Kind::Mysql => SqlEngine::Mysql,
            Kind::Mariadb => SqlEngine::Mariadb,
            Kind::Sqlite => SqlEngine::Sqlite,
            Kind::Duckdb => SqlEngine::Duckdb,
            Kind::Mssql => SqlEngine::Mssql,
        }
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

/// An enabled SSH tunnel.
struct Ssh {
    host: String,
    /// 0 means 22.
    port: f64,
    username: String,
    /// `""` became `password`.
    auth_method: String,
    /// `None` when empty.
    key_path: Option<String>,
}

/// What's missing when the user has to supply something.
enum Missing {
    Password,
    SshPassword,
    SshKeyPath,
    Field(&'static str),
}

/// The intermediate form both targets map to: a saved row's fields (or the
/// form's), its tunnel when enabled, and its save flags.
struct Draft {
    /// The row's id; `None` for a form.
    saved_id: Option<String>,
    name: String,
    ty: String,
    kind: Kind,
    host: String,
    port: f64,
    database: String,
    username: String,
    /// `None` for unset or `""`: the engine's default.
    ssl_mode: Option<String>,
    /// `None` for unset or `""`.
    string: Option<String>,
    /// Only for server engines: file engines never tunnel (row 7d).
    ssh: Option<Ssh>,
    save_password: bool,
    save_ssh_password: bool,
    save_ssh_key_passphrase: bool,
}

fn non_empty(s: Option<&str>) -> Option<String> {
    s.filter(|s| !s.is_empty()).map(str::to_string)
}

impl Draft {
    fn new(target: Target<'_>) -> Result<Self, ConfigError> {
        let mut d = match target {
            Target::Saved(row) => Self {
                saved_id: Some(row.id.clone()),
                name: row.name.clone(),
                ty: row.ty.clone(),
                kind: Kind::Postgres,
                host: row.host.clone(),
                port: row.port,
                database: row.database_name.clone(),
                username: row.username.clone(),
                ssl_mode: non_empty(row.ssl_mode.as_deref()),
                string: non_empty(row.connection_string.as_deref()),
                ssh: parse_ssh(row),
                save_password: row.save_password,
                save_ssh_password: row.save_ssh_password,
                save_ssh_key_passphrase: row.save_ssh_key_passphrase,
            },
            Target::Form(form) => Self {
                saved_id: None,
                name: form.name.clone(),
                ty: form.ty.clone(),
                kind: Kind::Postgres,
                host: form.host.clone(),
                port: form.port,
                database: form.database_name.clone(),
                username: form.username.clone(),
                ssl_mode: non_empty(form.ssl_mode.as_deref()),
                string: non_empty(Some(&form.connection_string)),
                ssh: form.ssh_enabled.then(|| Ssh {
                    host: form.ssh_host.clone(),
                    port: form.ssh_port,
                    username: form.ssh_username.clone(),
                    auth_method: form.ssh_auth_method.clone(),
                    key_path: non_empty(Some(&form.ssh_key_path)),
                }),
                save_password: form.save_password,
                save_ssh_password: form.save_ssh_password,
                save_ssh_key_passphrase: form.save_ssh_key_passphrase,
            },
        };
        d.kind = match d.ty.as_str() {
            "postgres" => Kind::Postgres,
            "mysql" => Kind::Mysql,
            "mariadb" => Kind::Mariadb,
            "sqlite" => Kind::Sqlite,
            "duckdb" => Kind::Duckdb,
            "mssql" => Kind::Mssql,
            other => {
                return Err(ConfigError::invalid(format!(
                    "{} has an unknown database type {other:?}",
                    d.subject()
                )))
            }
        };
        if d.kind.is_file() {
            d.ssh = None;
        }
        if let Some(ssh) = &mut d.ssh {
            if ssh.auth_method.is_empty() {
                ssh.auth_method = "password".into();
            }
        }
        Ok(d)
    }

    /// `Connection "name"`, or `The connection` for a form without a name.
    fn subject(&self) -> String {
        if self.saved_id.is_none() && self.name.trim().is_empty() {
            "The connection".to_string()
        } else {
            format!("Connection {:?}", self.name)
        }
    }

    /// Supplied secrets first; for a saved row, the rest from the store
    /// under its flags. Gives up when a saved row has no database password
    /// and `savePassword` is off, before any SSH secret is read.
    async fn secrets<F, Fut>(
        &self,
        supplied: &SuppliedSecrets,
        get: &mut F,
        url_ssh_password: Option<String>,
    ) -> Result<Secrets, ConfigError>
    where
        F: FnMut(String) -> Fut,
        Fut: Future<Output = Result<Option<String>, String>>,
    {
        let mut secrets = Secrets::default();
        if self.kind.is_file() {
            return Ok(secrets);
        }
        let saved = self.saved_id.as_deref();
        secrets.db = match non_empty(supplied.db.as_deref()) {
            Some(pw) => Some(pw),
            None => match saved {
                Some(id) if self.save_password => {
                    read(get, db_key(id), &mut secrets.unreadable).await
                }
                _ => None,
            },
        };
        if saved.is_some() && secrets.db.is_none() && !self.save_password {
            return Err(self.missing(Missing::Password, &secrets));
        }
        if self.ssh.is_some() {
            secrets.ssh = match non_empty(supplied.ssh.as_deref()) {
                Some(pw) => Some(pw),
                None => match saved {
                    Some(id) if self.save_ssh_password => {
                        read(get, ssh_key(id), &mut secrets.unreadable).await
                    }
                    _ => None,
                },
            };
            secrets.ssh_key = match non_empty(supplied.ssh_key.as_deref()) {
                Some(pw) => Some(pw),
                None => match saved {
                    Some(id) if self.save_ssh_key_passphrase => {
                        read(get, ssh_key_passphrase_key(id), &mut secrets.unreadable).await
                    }
                    _ => None,
                },
            };
            if secrets.ssh.is_none() {
                secrets.ssh = url_ssh_password;
            }
        }
        Ok(secrets)
    }

    /// The tunnel's give-ups: password auth without a password, key auth
    /// without a key file, no SSH host or user.
    fn check(&self, secrets: &Secrets) -> Result<(), ConfigError> {
        let Some(ssh) = &self.ssh else {
            return Ok(());
        };
        if ssh.auth_method == "password" && secrets.ssh.is_none() {
            return Err(self.missing(Missing::SshPassword, secrets));
        }
        if ssh.auth_method == "key" && ssh.key_path.is_none() {
            return Err(self.missing(Missing::SshKeyPath, secrets));
        }
        if ssh.host.trim().is_empty() {
            return Err(self.missing(Missing::Field("SSH host"), secrets));
        }
        if ssh.username.trim().is_empty() {
            return Err(self.missing(Missing::Field("SSH username"), secrets));
        }
        Ok(())
    }

    fn missing(&self, what: Missing, secrets: &Secrets) -> ConfigError {
        let message = match &self.saved_id {
            Some(id) => {
                let name = &self.name;
                let unreadable = |key: String| {
                    secrets
                        .unreadable
                        .iter()
                        .find(|u| u.key == key)
                        .map(|u| u.code.as_str())
                };
                match what {
                    Missing::Password => format!(
                        "Connection {name:?} has no saved password. Open it in the Seaquel app, \
                         enter the password with \"Save password in keychain\" on, and connect \
                         once."
                    ),
                    Missing::SshPassword if unreadable(ssh_key(id)) == Some("NO_SECRET_STORE") => {
                        format!(
                            "Connection {name:?} needs its saved SSH password, but this \
                             workspace has no secret store to read it from. Enter the SSH \
                             password to connect."
                        )
                    }
                    Missing::SshPassword if unreadable(ssh_key(id)).is_some() => format!(
                        "Seaquel couldn't read the SSH password of connection {name:?} from the \
                         keychain. Allow access when the system asks, or open the connection in \
                         the Seaquel app and save its SSH password again."
                    ),
                    Missing::SshPassword => format!(
                        "Connection {name:?} goes through an SSH tunnel whose password isn't \
                         saved. Open it in the Seaquel app, enter the SSH password with saving \
                         on, and connect once."
                    ),
                    Missing::SshKeyPath => format!(
                        "Connection {name:?} uses SSH key authentication but has no key file. \
                         Open it in the Seaquel app, choose the key file, and connect once."
                    ),
                    Missing::Field(field) => format!(
                        "Connection {name:?} has no {field}. Open it in the Seaquel app, fill it \
                         in, and connect once."
                    ),
                }
            }
            None => match what {
                Missing::Password => "Enter the password.".to_string(),
                Missing::SshPassword => {
                    "Enter the SSH password to connect through the SSH tunnel.".to_string()
                }
                Missing::SshKeyPath => {
                    "Choose the SSH key file to connect through the SSH tunnel.".to_string()
                }
                Missing::Field(field) => format!("Enter the {field} to connect."),
            },
        };
        ConfigError {
            code: CREDENTIALS_REQUIRED,
            message,
        }
    }

    /// A JSON number as a `u16` port, with 0 as `default`.
    fn port_or_default(&self, n: f64, default: u16, what: &str) -> Result<u16, ConfigError> {
        if n.fract() == 0.0 && (0.0..=65535.0).contains(&n) {
            let port = n as u16;
            return Ok(if port == 0 { default } else { port });
        }
        Err(ConfigError::invalid(format!(
            "{} has an invalid {what}: {}",
            self.subject(),
            js_number(n)
        )))
    }
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
        port: obj.get("port").and_then(Value::as_f64).unwrap_or(0.0),
        username: text("username"),
        auth_method: text("authMethod"),
        key_path: non_empty(obj.get("keyPath").and_then(Value::as_str)),
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

    #[test]
    fn duckdb_options_come_out_of_the_path() {
        let (path, config) = duckdb_path("duckdb:///x.duckdb?access_mode=read_only&threads=2");
        assert_eq!(path, "/x.duckdb");
        let config = config.unwrap();
        assert_eq!(config["access_mode"], "read_only");
        assert_eq!(config["threads"], "2");
        assert_eq!(duckdb_path("duckdb://").0, ":memory:");
        assert_eq!(duckdb_path("duckdb://?threads=1").0, ":memory:");
        assert_eq!(
            duckdb_path("duckdb:rel.duckdb"),
            ("rel.duckdb".into(), None)
        );
    }
}
