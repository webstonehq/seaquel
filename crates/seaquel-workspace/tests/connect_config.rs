//! Replays the frozen connect-config fixtures (`tests/fixtures/connect-config`,
//! recorded from the TypeScript; see its README) against
//! `seaquel_workspace::connections`.
//!
//! The expected result of a case is `case[case.gui]`: autoReconnect's config
//! and tunnel, the reconnect tab's rebuild, or `CREDENTIALS_REQUIRED` for the
//! `form` cases. The keys a case reads are always autoReconnect's.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use futures::executor::block_on;
use seaquel_types::storage::PersistedConnection;
use seaquel_types::ConnectConfig;
use seaquel_workspace::connections::{
    build_config, read_secrets, tunnel_config, ConfigError, Secrets, CREDENTIALS_REQUIRED,
};
use serde_json::{json, Map, Value};

const FILES: [&str; 9] = [
    "postgres", "mysql", "mariadb", "mssql", "sqlite", "duckdb", "ssh", "secrets", "shared",
];

struct Case {
    name: String,
    raw: Value,
    row: PersistedConnection,
    keychain: BTreeMap<String, String>,
    failing: HashSet<String>,
    tunnel_port: Option<u16>,
    gui: String,
}

fn cases() -> Vec<Case> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/connect-config");
    let mut out = Vec::new();
    for file in FILES {
        let text = std::fs::read_to_string(dir.join(format!("{file}.json"))).unwrap();
        let list: Vec<Value> = serde_json::from_str(&text).unwrap();
        for raw in list {
            let name = raw["name"].as_str().unwrap().to_string();
            let row: PersistedConnection = serde_json::from_value(raw["row"].clone())
                .unwrap_or_else(|e| panic!("{name}: row: {e}"));
            let keychain = raw["secrets"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
                .collect();
            let failing = raw
                .get("secretErrors")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(|k| k.as_str().unwrap().to_string()).collect())
                .unwrap_or_default();
            let tunnel_port = raw["tunnelPort"]
                .as_u64()
                .map(|p| u16::try_from(p).unwrap());
            let gui = raw["gui"].as_str().unwrap().to_string();
            out.push(Case {
                name,
                raw,
                row,
                keychain,
                failing,
                tunnel_port,
                gui,
            });
        }
    }
    out
}

/// `read_secrets` against the case's keychain. Returns the keys it read, in
/// order, and its result.
fn read(case: &Case) -> (Vec<String>, Result<Secrets, ConfigError>) {
    let mut keys = Vec::new();
    let result = block_on(read_secrets(&case.row, |key: String| {
        keys.push(key.clone());
        let answer = if case.failing.contains(&key) {
            Err("SECRET_STORE_ERROR".to_string())
        } else {
            Ok(case.keychain.get(&key).cloned())
        };
        std::future::ready(answer)
    }));
    (keys, result)
}

/// A `ConnectConfig` as it crosses IPC: absent fields left out, like the
/// recorder's `JSON.stringify`.
fn config_json(c: &ConnectConfig) -> Value {
    let mut m = Map::new();
    m.insert("driver".into(), json!(c.driver.as_str()));
    let mut put = |k: &str, v: Option<Value>| {
        if let Some(v) = v {
            m.insert(k.into(), v);
        }
    };
    put(
        "connection_string",
        c.connection_string.as_ref().map(|v| json!(v)),
    );
    put("host", c.host.as_ref().map(|v| json!(v)));
    put("port", c.port.map(|v| json!(v)));
    put("database", c.database.as_ref().map(|v| json!(v)));
    put("username", c.username.as_ref().map(|v| json!(v)));
    put("password", c.password.as_ref().map(|v| json!(v)));
    put("encrypt", c.encrypt.map(|v| json!(v)));
    put("trust_cert", c.trust_cert.map(|v| json!(v)));
    put("path", c.path.as_ref().map(|v| json!(v)));
    put("create_if_missing", c.create_if_missing.map(|v| json!(v)));
    Value::Object(m)
}

/// The secret values of a case long enough to search for in text.
fn secret_values(case: &Case) -> Vec<&str> {
    case.keychain
        .values()
        .map(String::as_str)
        .filter(|v| v.len() >= 4)
        .collect()
}

#[test]
fn there_are_107_cases_split_as_the_readme_says() {
    let cases = cases();
    assert_eq!(cases.len(), 107);
    let count = |gui: &str| cases.iter().filter(|c| c.gui == gui).count();
    assert_eq!(count("autoReconnect"), 71);
    assert_eq!(count("reconnectTab"), 24);
    assert_eq!(count("form"), 12);
}

/// The secret-selection rules on their own: which keychain entries are read,
/// in which order, and when connecting gives up.
#[test]
fn reads_the_keys_autoreconnect_reads_and_gives_up_where_it_does() {
    let mut failures = Vec::new();
    for case in cases() {
        let (keys, result) = read(&case);
        let expected: Vec<String> =
            serde_json::from_value(case.raw["autoReconnect"]["secretsRead"].clone()).unwrap();
        if keys != expected {
            failures.push(format!(
                "{}: read {keys:?}, expected {expected:?}",
                case.name
            ));
        }
        match (&result, case.gui.as_str()) {
            (Err(e), "form") if e.code == CREDENTIALS_REQUIRED => {
                if !e.message.contains(&case.row.name) {
                    failures.push(format!(
                        "{}: message doesn't name it: {}",
                        case.name, e.message
                    ));
                }
            }
            (Ok(_), "autoReconnect" | "reconnectTab") => {}
            (r, gui) => failures.push(format!("{}: gui {gui}, got {r:?}", case.name)),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_failed_read_is_no_secret_but_is_remembered() {
    let case = cases()
        .into_iter()
        .find(|c| c.name == "secrets/pg-read-error")
        .unwrap();
    let (_, result) = read(&case);
    let secrets = result.unwrap();
    assert_eq!(secrets.db, None);
    assert_eq!(secrets.unreadable.len(), 1);
    assert_eq!(secrets.unreadable[0].key, "db:conn-secrets-pg-read-error");
    assert_eq!(secrets.unreadable[0].code, "SECRET_STORE_ERROR");
}

#[test]
fn builds_the_config_and_tunnel_the_app_connects_with() {
    let mut failures = Vec::new();
    for case in cases() {
        if case.gui == "form" {
            continue;
        }
        let expected = &case.raw[&case.gui];
        let secrets = read(&case).1.unwrap();

        let tunnel = tunnel_config(&case.row, &secrets, None)
            .map(|t| t.map(|t| serde_json::to_value(t).unwrap()));
        let want_tunnel = expected.get("tunnel").cloned();
        match tunnel {
            Ok(t) if t == want_tunnel => {}
            other => failures.push(format!(
                "{}: tunnel {other:?}\n   expected {want_tunnel:?}",
                case.name
            )),
        }

        let config = build_config(&case.row, &secrets, case.tunnel_port).map(|c| config_json(&c));
        match config {
            Ok(c) if c == expected["config"] => {}
            other => failures.push(format!(
                "{}: config {other:?}\n   expected {}",
                case.name, expected["config"]
            )),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// `build_config` applies the give-up rules itself, so a caller that skips
/// `read_secrets` can't connect a `form` case either.
#[test]
fn build_config_refuses_the_form_cases_too() {
    let mut failures = Vec::new();
    for case in cases().into_iter().filter(|c| c.gui == "form") {
        let keys: Vec<String> =
            serde_json::from_value(case.raw["autoReconnect"]["secretsRead"].clone()).unwrap();
        let get = |prefix: &str| {
            keys.iter()
                .find(|k| k.starts_with(prefix))
                .filter(|k| !case.failing.contains(*k))
                .and_then(|k| case.keychain.get(k).cloned())
        };
        let secrets = Secrets {
            db: get("db:"),
            ssh: get("ssh:"),
            ssh_key: get("ssh-key:"),
            ..Secrets::default()
        };
        for result in [
            build_config(&case.row, &secrets, case.tunnel_port).map(|_| ()),
            tunnel_config(&case.row, &secrets, None).map(|_| ()),
        ] {
            match result {
                Err(e) if e.code == CREDENTIALS_REQUIRED => {}
                other => failures.push(format!("{}: {other:?}", case.name)),
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_trusted_fingerprint_goes_on_the_tunnel() {
    let case = cases()
        .into_iter()
        .find(|c| c.name == "ssh/pg-password-auth")
        .unwrap();
    let secrets = read(&case).1.unwrap();
    let tunnel = tunnel_config(&case.row, &secrets, Some("SHA256:abc".into()))
        .unwrap()
        .unwrap();
    assert_eq!(tunnel.trust_host_key.as_deref(), Some("SHA256:abc"));
}

#[test]
fn a_tunnelled_row_needs_its_port() {
    let case = cases()
        .into_iter()
        .find(|c| c.name == "ssh/pg-password-auth")
        .unwrap();
    let secrets = read(&case).1.unwrap();
    let err = build_config(&case.row, &secrets, None).unwrap_err();
    assert_eq!(err.code, "INVALID_CONNECTION");
}

#[test]
fn an_unknown_engine_is_refused() {
    let mut row = cases().remove(0).row;
    row.ty = "oracle".into();
    let err = build_config(&row, &Secrets::default(), None).unwrap_err();
    assert_eq!(err.code, "INVALID_CONNECTION");
    assert!(err.message.contains("oracle"), "{}", err.message);
}

/// No secret value in `Secrets`' `Debug`, or in any error or its `Debug`.
#[test]
fn secrets_never_reach_debug_or_errors() {
    let mut failures = Vec::new();
    for case in cases() {
        let values = secret_values(&case);
        let (_, result) = read(&case);
        let mut texts = vec![format!("{result:?}")];
        if let Ok(secrets) = &result {
            texts.push(format!("{secrets:?} {secrets:#?}"));
        }
        let all = Secrets {
            db: case.keychain.values().next().cloned(),
            ssh: case.keychain.values().nth(1).cloned(),
            ssh_key: case.keychain.values().nth(2).cloned(),
            ..Secrets::default()
        };
        texts.push(format!("{all:?}"));
        for port in [None, case.tunnel_port] {
            if let Err(e) = build_config(&case.row, &all, port) {
                texts.push(format!("{e} {e:?}"));
            }
        }
        for text in texts {
            for v in &values {
                if text.contains(v) {
                    failures.push(format!("{}: {v:?} in {text}", case.name));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
