//! The web `ConnectPolicy` on `/rpc` (`db.connect` and `db.test`), with the
//! server's real Core (`web_core()`): no SQLite or DuckDB, no server files
//! or sockets in the config Core builds, and no SSH tunnels, refused before
//! one opens. This must hold in a unified build too (`cargo test
//! --workspace`), where Cargo turns on Core's `ssh` feature and compiles the
//! file engines in for the desktop, CLI and MCP crates.

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use seaquel_server::web_core;
use serde_json::{json, Value};

mod common;
use common::Env;

fn web_env() -> Env {
    Env::with_core(Arc::new(web_core()), 4, Arc::default())
}

fn form(fields: Value) -> Value {
    let mut form = json!({"name": "f", "port": 0, "username": "u", "databaseName": "app"});
    for (k, v) in fields.as_object().unwrap() {
        form[k] = v.clone();
    }
    form
}

/// `db.connect` and `db.test` of `target` both answer `code` with `status`,
/// within 5 s (so nothing was dialled).
async fn assert_refused(env: &Env, target: Value, code: &str, status: StatusCode) {
    for method in ["connect", "test"] {
        let params = json!({"target": target, "secrets": {"db": "pw", "ssh": "sp", "sshKey": "kp"},
                   "createIfMissing": true});
        let (got_status, body) =
            tokio::time::timeout(Duration::from_secs(5), env.db("u1", method, params))
                .await
                .unwrap_or_else(|_| panic!("{method} {target} took too long"));
        assert_eq!(body["code"], code, "{method} {target}: {body}");
        assert_eq!(got_status, status, "{method} {target}: {body}");
    }
}

#[tokio::test]
async fn sqlite_and_duckdb_are_refused() {
    let env = web_env();
    let root = env.dir.path();
    // A real file where auth.db would be, so a refusal isn't just a
    // missing file.
    std::fs::write(root.join("auth.db"), b"").unwrap();
    let auth_db = root.join("auth.db").display().to_string();
    let meta_db = root
        .join("users/someone-else/meta.db")
        .display()
        .to_string();
    let new_db = root.join("new.db").display().to_string();
    let duck = root.join("x.duckdb").display().to_string();

    for fields in [
        json!({"type": "sqlite", "databaseName": auth_db}),
        json!({"type": "sqlite", "connectionString": format!("sqlite:{meta_db}")}),
        json!({"type": "sqlite", "databaseName": ":memory:"}),
        json!({"type": "sqlite", "databaseName": new_db}),
        json!({"type": "duckdb", "databaseName": ""}),
        json!({"type": "duckdb", "databaseName": duck}),
        json!({"type": "duckdb", "connectionString": format!("duckdb://{duck}?access_mode=read_write")}),
    ] {
        let target = json!({"type": "form", "form": form(fields)});
        assert_refused(
            &env,
            target,
            "ENGINE_NOT_AVAILABLE",
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
    assert!(!root.join("new.db").exists());
    assert!(!root.join("x.duckdb").exists());
}

/// Server files and sockets, however they get into the URL Core builds: a
/// typed string, or a form field the builder puts in the URL unescaped.
#[tokio::test]
async fn server_files_and_sockets_are_refused() {
    let env = web_env();
    let host = "db.example.invalid";
    for fields in [
        // Typed strings.
        json!({"type": "postgres", "connectionString": format!("postgres://u@{host}/app?sslkey=/data/auth.db")}),
        json!({"type": "postgres", "connectionString": format!("postgresql://u@{host}/app?sslrootcert=/etc/passwd")}),
        json!({"type": "postgres", "connectionString": format!("postgres://u@{host}/app?host=/var/run/postgresql")}),
        json!({"type": "postgres", "connectionString": format!("postgres://u@{host}/app?passfile=/root/.pgpass")}),
        json!({"type": "postgres", "connectionString": "postgres:///app"}),
        json!({"type": "postgres", "connectionString": "host=/var/run/postgresql dbname=app"}),
        json!({"type": "mysql", "connectionString": format!("mysql://root@{host}/app?ssl-ca=/data/auth.db")}),
        json!({"type": "mysql", "connectionString": format!("mysql://root@{host}/app?socket=/run/mysqld/mysqld.sock")}),
        json!({"type": "mariadb", "connectionString": format!("mariadb://root@{host}/app?sslkey=/srv/k")}),
        // Built from the fields.
        json!({"type": "postgres", "host": "/var/run/postgresql"}),
        json!({"type": "postgres", "host": format!("{host}/app?sslkey=/srv/client.key#")}),
        json!({"type": "postgres", "host": host, "databaseName": "app?sslcert=/srv/c.crt"}),
        json!({"type": "postgres", "host": host, "sslMode": "require&sslrootcert=/etc/passwd"}),
        json!({"type": "mysql", "host": host, "sslMode": "REQUIRED&ssl-ca=/data/auth.db"}),
        json!({"type": "mariadb", "host": host, "databaseName": "app?socket=/tmp/mysql.sock"}),
    ] {
        let target = json!({"type": "form", "form": form(fields)});
        assert_refused(
            &env,
            target,
            "CONNECTION_OPTION_NOT_ALLOWED",
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
}

/// A saved row goes through the same check.
#[tokio::test]
async fn a_saved_row_is_checked_too() {
    let env = web_env();
    let p = save_project(&env).await;
    let c1 = save(
        &env,
        json!({"projectId": p, "name": "Saved", "type": "postgres",
               "host": "db.example.invalid", "port": 5432, "databaseName": "app",
               "username": "u", "labelIds": [], "savePassword": false,
               "connectionString": "postgres://u@db.example.invalid/app?sslkey=/data/auth.db"}),
    )
    .await;
    assert_refused(
        &env,
        json!({"type": "saved", "id": c1}),
        "CONNECTION_OPTION_NOT_ALLOWED",
        StatusCode::BAD_REQUEST,
    )
    .await;
}

/// SSH is refused before a tunnel opens: the bastion is unroutable (a dial
/// would hang past the timeout), and the key file isn't read.
#[tokio::test]
async fn ssh_is_refused_before_any_tunnel() {
    let env = web_env();
    let ssh = |auth: &str| {
        json!({"sshEnabled": true, "sshHost": "10.255.255.1", "sshPort": 22,
               "sshUsername": "root", "sshAuthMethod": auth,
               "sshKeyPath": "/etc/ssh/ssh_host_ed25519_key"})
    };
    for (ty, auth) in [
        ("postgres", "password"),
        ("postgres", "key"),
        ("mysql", "key"),
        ("mssql", "password"),
    ] {
        let mut fields = ssh(auth);
        fields["type"] = json!(ty);
        fields["host"] = json!("10.0.0.5");
        let target = json!({"type": "form", "form": form(fields)});
        assert_refused(&env, target, "NOT_SUPPORTED", StatusCode::NOT_IMPLEMENTED).await;
    }

    let p = save_project(&env).await;
    let c2 = save(
        &env,
        json!({"projectId": p, "name": "Tunnelled", "type": "postgres",
               "host": "10.0.0.5", "port": 5432, "databaseName": "app", "username": "u",
               "labelIds": [], "savePassword": false,
               "sshTunnel": {"enabled": true, "host": "10.255.255.1", "port": 22,
                             "username": "root", "authMethod": "key",
                             "keyPath": "/etc/ssh/ssh_host_ed25519_key"}}),
    )
    .await;
    assert_refused(
        &env,
        json!({"type": "saved", "id": c2}),
        "NOT_SUPPORTED",
        StatusCode::NOT_IMPLEMENTED,
    )
    .await;
}

/// The default project's id, through the library.
async fn save_project(env: &Env) -> String {
    let (status, body) = env
        .rpc(
            "u1",
            &json!({"method": "library", "params": {"method": "projectEnsureDefault"}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, body) = env
        .rpc(
            "u1",
            &json!({"method": "library", "params": {"method": "projectsList"}}),
        )
        .await;
    body["result"]["result"]["value"][0]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Save `connection` (a draft) through the library; its id.
async fn save(env: &Env, connection: Value) -> String {
    let (status, body) = env
        .rpc(
            "u1",
            &json!({"method": "library", "params": {"method": "connectionCreate", "params":
                {"connection": connection}}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["result"]["result"]["value"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// A form the policy allows reaches the driver: it fails there (nothing
/// listens on port 1), not on the policy.
#[tokio::test]
async fn an_allowed_form_reaches_the_driver() {
    let env = web_env();
    for fields in [
        json!({"type": "mssql", "host": "127.0.0.1", "port": 1}),
        json!({"type": "postgres", "host": "db.example.invalid", "port": 5432}),
    ] {
        let target = json!({"type": "form", "form": form(fields)});
        for method in ["connect", "test"] {
            let (status, body) = tokio::time::timeout(
                Duration::from_secs(10),
                env.db(
                    "u1",
                    method,
                    json!({"target": target, "secrets": {"db": "pw"}}),
                ),
            )
            .await
            .unwrap_or_else(|_| panic!("{method} {target} took too long"));
            assert_eq!(
                body["code"], "CONNECTION_ERROR",
                "{method} {target}: {body}"
            );
            assert_eq!(status, StatusCode::BAD_GATEWAY, "{method} {target}: {body}");
        }
    }
}
