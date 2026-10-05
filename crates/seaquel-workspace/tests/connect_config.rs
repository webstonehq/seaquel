//! Replays the v2 connect-config fixtures (`tests/fixtures/connect-config-v2`,
//! the spec; see its README) through the one builder,
//! `seaquel_workspace::connections::plan`, for both targets.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use futures::executor::block_on;
use seaquel_types::connect::{ConnectionForm, SuppliedSecrets};
use seaquel_types::storage::PersistedConnection;
use seaquel_types::ConnectConfig;
use seaquel_workspace::connections::{plan, ConfigError, Plan, Target, CREDENTIALS_REQUIRED};
use serde_json::{json, Map, Value};

const GROUPS: [&str; 9] = [
    "postgres", "mysql", "mariadb", "mssql", "sqlite", "duckdb", "ssh", "secrets", "shared",
];

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn read(dir: &str, file: &str) -> Vec<Value> {
    let path = fixtures().join(dir).join(format!("{file}.json"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

enum Input {
    Saved {
        row: PersistedConnection,
        store: BTreeMap<String, String>,
        failing: HashSet<String>,
    },
    Form(ConnectionForm),
}

struct Case {
    name: String,
    op: String,
    changed_by: Vec<String>,
    input: Input,
    supplied: SuppliedSecrets,
    create_if_missing: bool,
    tunnel_port: Option<u16>,
    expected: Value,
}

fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    for group in GROUPS {
        for raw in read("connect-config-v2", group) {
            let name = raw["name"].as_str().unwrap().to_string();
            let input = &raw["input"];
            let parsed = match raw["target"].as_str().unwrap() {
                "saved" => Input::Saved {
                    row: serde_json::from_value(input["row"].clone())
                        .unwrap_or_else(|e| panic!("{name}: row: {e}")),
                    store: input["secrets"]
                        .as_object()
                        .unwrap()
                        .iter()
                        .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
                        .collect(),
                    failing: input
                        .get("secretErrors")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().map(|k| k.as_str().unwrap().to_string()).collect())
                        .unwrap_or_default(),
                },
                "form" => Input::Form(
                    serde_json::from_value(input["form"].clone())
                        .unwrap_or_else(|e| panic!("{name}: form: {e}")),
                ),
                other => panic!("{name}: target {other}"),
            };
            out.push(Case {
                op: raw["op"].as_str().unwrap().to_string(),
                changed_by: serde_json::from_value(raw["changedBy"].clone()).unwrap(),
                input: parsed,
                supplied: serde_json::from_value(input["supplied"].clone())
                    .unwrap_or_else(|e| panic!("{name}: supplied: {e}")),
                create_if_missing: input["createIfMissing"].as_bool().unwrap(),
                tunnel_port: input["tunnelPort"]
                    .as_u64()
                    .map(|p| u16::try_from(p).unwrap()),
                expected: raw["expected"].clone(),
                name,
            });
        }
    }
    out
}

/// The builder on a case: the store keys it read, in order, and its plan.
fn run(case: &Case) -> (Vec<String>, Result<Plan, ConfigError>) {
    let mut keys = Vec::new();
    let result = match &case.input {
        Input::Saved {
            row,
            store,
            failing,
        } => block_on(plan(Target::Saved(row), &case.supplied, |key: String| {
            keys.push(key.clone());
            let answer = if failing.contains(&key) {
                Err("SECRET_STORE_ERROR".to_string())
            } else {
                Ok(store.get(&key).cloned())
            };
            std::future::ready(answer)
        })),
        Input::Form(form) => block_on(plan(Target::Form(form), &case.supplied, |key: String| {
            keys.push(key);
            std::future::ready(Ok(None))
        })),
    };
    (keys, result)
}

/// A `ConnectConfig` as JSON with absent fields left out, like the
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
    put("restricted", c.restricted.map(|v| json!(v)));
    put(
        "tls_server_name",
        c.tls_server_name.as_ref().map(|v| json!(v)),
    );
    put("duckdb_config", c.duckdb_config.as_ref().map(|v| json!(v)));
    Value::Object(m)
}

/// The case's output in the fixtures' `expected` shape.
fn output(case: &Case) -> Value {
    let (keys, result) = run(case);
    let mut out = Map::new();
    if let Input::Saved { .. } = case.input {
        out.insert("secretsRead".into(), json!(keys));
    } else {
        assert!(keys.is_empty(), "{}: a form read {keys:?}", case.name);
    }
    match result {
        Err(e) => {
            out.insert("error".into(), json!(e.code));
        }
        Ok(plan) => {
            if let Some(tunnel) = plan.tunnel(None) {
                out.insert("tunnel".into(), serde_json::to_value(tunnel).unwrap());
            }
            match plan.config(case.tunnel_port, case.create_if_missing) {
                Ok(config) => {
                    out.insert("config".into(), config_json(&config));
                }
                Err(e) => {
                    out.insert("error".into(), json!(e.code));
                }
            }
        }
    }
    Value::Object(out)
}

#[test]
fn there_are_160_cases_split_as_the_readme_says() {
    let cases = cases();
    assert_eq!(cases.len(), 160);
    let forms = cases
        .iter()
        .filter(|c| matches!(c.input, Input::Form(_)))
        .count();
    assert_eq!(forms, 53);
    assert_eq!(
        cases.iter().filter(|c| !c.changed_by.is_empty()).count(),
        68
    );
    let tests = cases.iter().filter(|c| c.op == "test").count();
    assert!(tests > 0 && cases.iter().all(|c| c.op == "test" || c.op == "connect"));
}

/// Every case, through the one builder, gives exactly its `expected`.
#[test]
fn every_case_builds_what_v2_expects() {
    let mut failures = Vec::new();
    for case in cases() {
        let got = output(&case);
        if got != case.expected {
            failures.push(format!(
                "{}:\n   got      {got}\n   expected {}",
                case.name, case.expected
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} case(s) differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// A `CREDENTIALS_REQUIRED` for a saved row names it.
#[test]
fn a_saved_give_up_names_the_connection() {
    for case in cases() {
        let Input::Saved { row, .. } = &case.input else {
            continue;
        };
        if let Err(e) = run(&case).1 {
            assert_eq!(e.code, CREDENTIALS_REQUIRED, "{}", case.name);
            assert!(
                e.message.contains(&row.name),
                "{}: {}",
                case.name,
                e.message
            );
        }
    }
}

fn case(name: &str) -> Case {
    cases().into_iter().find(|c| c.name == name).unwrap()
}

/// A supplied password wins with `savePassword` off, and the store isn't
/// read for it.
#[test]
fn a_supplied_password_wins_with_save_password_off() {
    let mut c = case("pg/string-with-password-not-saved");
    c.supplied = SuppliedSecrets::db("typed-pw");
    let (keys, result) = run(&c);
    assert!(keys.is_empty(), "{keys:?}");
    let config = result.unwrap().config(None, false).unwrap();
    assert_eq!(
        config.connection_string.as_deref(),
        Some("postgresql://alice:typed-pw@db.example.com:5432/app")
    );

    // With the flag on, the supplied one still wins and the store isn't read.
    let mut c = case("pg/stored-string-default-port");
    c.supplied = SuppliedSecrets::db("typed-pw");
    let (keys, result) = run(&c);
    assert!(keys.is_empty(), "{keys:?}");
    let config = result.unwrap().config(None, false).unwrap();
    assert!(config
        .connection_string
        .unwrap()
        .contains("alice:typed-pw@"));
}

/// Supplied SSH secrets win too; only what's missing is read.
#[test]
fn supplied_ssh_secrets_are_not_read_from_the_store() {
    let mut c = case("secrets/all-flags-all-secrets");
    c.supplied = SuppliedSecrets {
        ssh: Some("typed-ssh".into()),
        ..SuppliedSecrets::none()
    };
    let (keys, result) = run(&c);
    assert_eq!(
        keys,
        [
            "db:conn-secrets-all-flags-all-secrets",
            "ssh-key:conn-secrets-all-flags-all-secrets"
        ]
    );
    let tunnel = result.unwrap().tunnel(None).unwrap();
    assert_eq!(tunnel.password.as_deref(), Some("typed-ssh"));
    assert_eq!(tunnel.key_passphrase.as_deref(), Some("key-pass"));
}

#[test]
fn a_trusted_fingerprint_goes_on_the_tunnel() {
    let (_, result) = run(&case("ssh/pg-password-auth"));
    let tunnel = result.unwrap().tunnel(Some("SHA256:abc".into())).unwrap();
    assert_eq!(tunnel.trust_host_key.as_deref(), Some("SHA256:abc"));
}

#[test]
fn a_tunnelled_connection_needs_its_port() {
    let (_, result) = run(&case("ssh/pg-password-auth"));
    let err = result.unwrap().config(None, false).unwrap_err();
    assert_eq!(err.code, "INVALID_CONNECTION");
}

#[test]
fn an_unknown_engine_is_refused() {
    let mut c = case("pg/stored-string-default-port");
    let Input::Saved { row, .. } = &mut c.input else {
        unreachable!()
    };
    row.ty = "oracle".into();
    let err = run(&c).1.unwrap_err();
    assert_eq!(err.code, "INVALID_CONNECTION");
    assert!(err.message.contains("oracle"), "{}", err.message);
}

/// A key=value string can't go through a tunnel: refused before any tunnel
/// opens.
#[test]
fn a_key_value_string_over_ssh_is_invalid() {
    let mut c = case("ssh/pg-password-auth");
    let Input::Saved { row, .. } = &mut c.input else {
        unreachable!()
    };
    row.connection_string = Some("host=db.internal user=alice dbname=app".into());
    let err = run(&c).1.unwrap_err();
    assert_eq!(err.code, "INVALID_CONNECTION");
}

/// A `+ssh` URL on a connection with no tunnel of its own uses the URL's
/// SSH part (choice C's suggestion).
#[test]
fn a_plus_ssh_url_without_a_tunnel_uses_its_own_ssh_part() {
    let form: ConnectionForm = serde_json::from_value(json!({
        "name": "Prod", "type": "postgres",
        "connectionString": "postgres+ssh://deploy@bastion.example.com:2200/alice@db.internal:5433/app?name=Prod&usePrivateKey=false",
    }))
    .unwrap();
    let supplied = SuppliedSecrets {
        db: Some("pw".into()),
        ssh: Some("ssh-pw".into()),
        ssh_key: None,
    };
    let p = block_on(plan(Target::Form(&form), &supplied, |_: String| {
        std::future::ready(Ok(None))
    }))
    .unwrap();
    let tunnel = p.tunnel(None).unwrap();
    assert_eq!(
        (
            tunnel.ssh_host.as_str(),
            tunnel.ssh_port,
            tunnel.ssh_username.as_str()
        ),
        ("bastion.example.com", 2200, "deploy")
    );
    assert_eq!(tunnel.auth_method, "password");
    assert_eq!(
        (tunnel.remote_host.as_str(), tunnel.remote_port),
        ("db.internal", 5433)
    );
    assert_eq!(
        p.config(Some(50000), false)
            .unwrap()
            .connection_string
            .as_deref(),
        Some("postgres://alice:pw@127.0.0.1:50000/app")
    );

    // usePrivateKey=true without a key file gives up.
    let mut form = form;
    form.connection_string = form
        .connection_string
        .replace("usePrivateKey=false", "usePrivateKey=true");
    let err = block_on(plan(Target::Form(&form), &supplied, |_: String| {
        std::future::ready(Ok(None))
    }))
    .unwrap_err();
    assert_eq!(err.code, CREDENTIALS_REQUIRED);
}

/// No secret value in any error, plan or config `Debug`.
#[test]
fn secrets_never_reach_debug_or_errors() {
    let mut failures = Vec::new();
    for case in cases() {
        let mut values: Vec<String> = [
            &case.supplied.db,
            &case.supplied.ssh,
            &case.supplied.ssh_key,
        ]
        .into_iter()
        .flatten()
        .cloned()
        .collect();
        if let Input::Saved { store, .. } = &case.input {
            values.extend(store.values().cloned());
        }
        // A secret that is also a plain field (`postgres` as user and
        // password) can't be told apart.
        let fields = match &case.input {
            Input::Saved { row, .. } => serde_json::to_string(row).unwrap(),
            Input::Form(form) => serde_json::to_string(form).unwrap(),
        };
        values.retain(|v| v.len() >= 4 && !fields.contains(v.as_str()));
        let (_, result) = run(&case);
        let mut texts = vec![format!("{result:?}")];
        if let Ok(plan) = &result {
            texts.push(format!("{:?}", plan.tunnel(None)));
            texts.push(format!("{:?}", plan.config(case.tunnel_port, false)));
            texts.push(format!("{:?}", plan.config(None, false)));
        }
        for text in texts {
            for v in &values {
                if text.contains(v.as_str()) {
                    failures.push(format!("{}: {v:?} in {text}", case.name));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A password inside the connection string itself, with nothing supplied,
/// is among the values errors are redacted of.
#[test]
fn a_strings_own_password_is_redacted_too() {
    let form: ConnectionForm = serde_json::from_value(json!({
        "name": "Paste", "type": "postgres",
        "connectionString": "postgres://alice:In%40String@db.example.com/app",
    }))
    .unwrap();
    let p = block_on(plan(
        Target::Form(&form),
        &SuppliedSecrets::none(),
        |_: String| std::future::ready(Ok(None)),
    ))
    .unwrap();
    let values = p.secret_values();
    assert!(values.iter().any(|v| v == "In%40String"), "{values:?}");
    assert!(values.iter().any(|v| v == "In@String"), "{values:?}");
    assert!(!format!("{p:?} {form:?}").contains("String"));
}
