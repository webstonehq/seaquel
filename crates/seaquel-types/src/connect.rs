//! What a GUI sends to connect or test: the connection form the user filled
//! in, and the secrets it supplies (Core's `Workspace::connect` and
//! `Workspace::test`, phase 5a).
//!
//! [`SuppliedSecrets`] carries passwords, so its `Debug` redacts them. A
//! [`ConnectionForm`] has no password field (the form's go in
//! [`SuppliedSecrets`], and a stray `password` key is dropped), but a pasted
//! connection string can hold one, so its `Debug` shows the string without
//! it.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The connection form (the GUI's `ConnectionFormData`) without its three
/// secrets. camelCase on the wire, with the field names the GUI uses; every
/// field may be left out. Numbers are JSON numbers, as the form holds them.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ConnectionForm {
    pub name: String,
    /// `postgres`, `mysql`, `mariadb`, `sqlite`, `duckdb` or `mssql`.
    #[serde(rename = "type")]
    pub ty: String,
    pub host: String,
    /// 0 means the engine's default port.
    pub port: f64,
    pub database_name: String,
    pub username: String,
    /// Absent (or `""`) is the wizard's "Default": the engine's own default.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub ssl_mode: Option<String>,
    /// A typed or pasted connection string; `""` builds one from the fields.
    pub connection_string: String,
    pub ssh_enabled: bool,
    pub ssh_host: String,
    /// 0 means 22.
    pub ssh_port: f64,
    pub ssh_username: String,
    /// `password` or `key`; `""` is `password`.
    pub ssh_auth_method: String,
    pub ssh_key_path: String,
    pub save_password: bool,
    pub save_ssh_password: bool,
    pub save_ssh_key_passphrase: bool,
}

impl fmt::Debug for ConnectionForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectionForm")
            .field("name", &self.name)
            .field("ty", &self.ty)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database_name", &self.database_name)
            .field("username", &self.username)
            .field("ssl_mode", &self.ssl_mode)
            .field(
                "connection_string",
                &match self.connection_string.as_str() {
                    "" => String::new(),
                    s => crate::connection_string_for_debug(s),
                },
            )
            .field("ssh_enabled", &self.ssh_enabled)
            .field("ssh_host", &self.ssh_host)
            .field("ssh_port", &self.ssh_port)
            .field("ssh_username", &self.ssh_username)
            .field("ssh_auth_method", &self.ssh_auth_method)
            .field("ssh_key_path", &self.ssh_key_path)
            .field("save_password", &self.save_password)
            .field("save_ssh_password", &self.save_ssh_password)
            .field("save_ssh_key_passphrase", &self.save_ssh_key_passphrase)
            .finish()
    }
}

/// Secrets the caller supplies with a connect or test: typed into the form,
/// or (on the web) decrypted from the vault. Each one, when present and not
/// empty, wins over anything a secret store holds. camelCase on the wire
/// (`db`, `ssh`, `sshKey`). `Debug` redacts the values.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SuppliedSecrets {
    /// The database password.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub db: Option<String>,
    /// The SSH password.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub ssh: Option<String>,
    /// The SSH key's passphrase.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub ssh_key: Option<String>,
}

impl SuppliedSecrets {
    /// No secrets.
    pub fn none() -> Self {
        Self::default()
    }

    /// Only a database password.
    pub fn db(password: impl Into<String>) -> Self {
        Self {
            db: Some(password.into()),
            ..Self::default()
        }
    }
}

impl fmt::Debug for SuppliedSecrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted = |v: &Option<String>| v.as_ref().map(|_| "<redacted>");
        f.debug_struct("SuppliedSecrets")
            .field("db", &redacted(&self.db))
            .field("ssh", &redacted(&self.ssh))
            .field("ssh_key", &redacted(&self.ssh_key))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supplied_secrets_debug_redacts_values() {
        let s = SuppliedSecrets {
            db: Some("hunter2-db".into()),
            ssh: Some("hunter2-ssh".into()),
            ssh_key: Some("hunter2-key".into()),
        };
        let debug = format!("{s:?} {s:#?}");
        assert!(!debug.contains("hunter2"), "{debug}");
        assert!(debug.contains("<redacted>"), "{debug}");
    }

    #[test]
    fn a_forms_debug_hides_the_password_in_its_string() {
        let form = ConnectionForm {
            ty: "postgres".into(),
            connection_string: "postgres://alice:hunter2@db.example.com/app".into(),
            ..ConnectionForm::default()
        };
        let debug = format!("{form:?} {form:#?}");
        assert!(!debug.contains("hunter2"), "{debug}");
        assert!(
            debug.contains("postgres://alice@db.example.com/app"),
            "{debug}"
        );
        let kv = ConnectionForm {
            connection_string: "Server=x;Password=hunter2;".into(),
            ..ConnectionForm::default()
        };
        assert!(!format!("{kv:?}").contains("hunter2"));
    }

    #[test]
    fn wire_names_are_camel_case() {
        let s: SuppliedSecrets =
            serde_json::from_str(r#"{"db":"a","ssh":"b","sshKey":"c"}"#).unwrap();
        assert_eq!(s.ssh_key.as_deref(), Some("c"));
        let form: ConnectionForm = serde_json::from_str(
            r#"{"type":"postgres","databaseName":"app","sshKeyPath":"/k","password":"dropped"}"#,
        )
        .unwrap();
        assert_eq!(form.ty, "postgres");
        assert_eq!(form.database_name, "app");
        assert_eq!(form.ssh_key_path, "/k");
        assert_eq!(form.ssl_mode, None);
        assert!(!serde_json::to_string(&form).unwrap().contains("dropped"));
    }
}
