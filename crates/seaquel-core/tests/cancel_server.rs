//! A cancelled stream stops on the database, not only in Core: a
//! workspace's `cancel`, `disconnect` and `close_all`, and Core's own
//! `cancel_stream`, each end a running `pg_sleep(30)` / `SLEEP(30)` on the
//! server within a few seconds (the phase 5a probe found them still running
//! after every one of those).
//!
//! Live: set `SEAQUEL_TEST_POSTGRES`, `SEAQUEL_TEST_MYSQL` and
//! `SEAQUEL_TEST_MARIADB` as for the engine smoke tests; each is skipped
//! when unset unless `SEAQUEL_TEST_REQUIRE_ENGINES` is set.
#![cfg(all(feature = "workspace", feature = "storage"))]

use std::time::Duration;
use tokio::time::Instant;

use futures::StreamExt;
use seaquel_core::{ConnectRequest, ConnectionForm, Core, QueryOptions, StreamEvent};
use seaquel_engine::ConnectConfig;
use serde_json::json;

#[derive(Clone, Copy, Debug)]
enum Stop {
    WorkspaceCancel,
    Disconnect,
    CloseAll,
    CoreCancel,
}

const STOPS: [Stop; 4] = [
    Stop::WorkspaceCancel,
    Stop::Disconnect,
    Stop::CloseAll,
    Stop::CoreCancel,
];

struct Server {
    var: &'static str,
    ty: &'static str,
    sleep: fn(&str) -> String,
    /// A 30 s sleep taking its seconds as the one bound parameter.
    sleep_param: fn(&str) -> String,
    running: fn(&str) -> String,
}

const POSTGRES: Server = Server {
    var: "SEAQUEL_TEST_POSTGRES",
    ty: "postgres",
    sleep: |m| format!("SELECT pg_sleep(30) AS {m}"),
    sleep_param: |m| format!("SELECT pg_sleep($1) AS {m}"),
    running: |m| {
        format!(
            "SELECT count(*) FROM pg_stat_activity WHERE state = 'active' \
             AND pid <> pg_backend_pid() AND query LIKE '%{m}%'"
        )
    },
};

const MYSQL: Server = Server {
    var: "SEAQUEL_TEST_MYSQL",
    ty: "mysql",
    sleep: |m| format!("SELECT SLEEP(30) AS {m}"),
    sleep_param: |m| format!("SELECT SLEEP(?) AS {m}"),
    running: |m| {
        format!(
            "SELECT COUNT(*) FROM information_schema.PROCESSLIST WHERE COMMAND IN ('Query', 'Execute') \
             AND ID <> CONNECTION_ID() AND INFO LIKE '%{m}%'"
        )
    },
};

const MARIADB: Server = Server {
    var: "SEAQUEL_TEST_MARIADB",
    ..MYSQL
};

fn config(server: &Server) -> Option<ConnectConfig> {
    match std::env::var(server.var) {
        Ok(raw) => Some(serde_json::from_str(&raw).expect(server.var)),
        Err(_) if std::env::var_os("SEAQUEL_TEST_REQUIRE_ENGINES").is_some() => {
            panic!("{} is not set", server.var)
        }
        Err(_) => {
            eprintln!("skipping: {} is not set", server.var);
            None
        }
    }
}

fn marker(stop: Stop) -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    format!(
        "seaquel_cancel_{}_{}",
        format!("{stop:?}").to_lowercase(),
        &id[..8]
    )
}

/// How many statements holding `marker` run on the server, seen from
/// `observer` (a connection of Core's own).
async fn running(core: &Core, observer: &str, server: &Server, marker: &str) -> i64 {
    let r = core
        .query(observer, &(server.running)(marker), vec![])
        .await
        .expect("observer query");
    r.rows[0][0].as_i64().expect("count")
}

async fn wait_for(core: &Core, observer: &str, server: &Server, marker: &str, n: i64) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if running(core, observer, server, marker).await == n {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

async fn stops_on_the_server(server: &Server) {
    let Some(config) = config(server) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let core = seaquel_core::with_default_plugins()
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .build();
    let observer = core.connect(&config).await.expect("observer").connection_id;
    let conn_str = config.connection_string.clone().expect("connection_string");
    let form: ConnectionForm = serde_json::from_value(json!({
        "name": "live", "type": server.ty, "connectionString": conn_str,
    }))
    .unwrap();

    for (i, stop) in STOPS.into_iter().enumerate() {
        let marker = marker(stop);
        let ws = core
            .open_workspace(seaquel_core::WorkspaceSpec::new(
                dir.path().join(i.to_string()),
            ))
            .await
            .unwrap();
        let (id, stream) = match stop {
            Stop::CoreCancel => {
                let id = core.connect(&config).await.unwrap().connection_id;
                let stream = core.query_stream(
                    "s1".into(),
                    id.clone(),
                    (server.sleep)(&marker),
                    vec![],
                    QueryOptions::default(),
                );
                (id, stream)
            }
            _ => {
                let id = ws
                    .connect(&core, ConnectRequest::form(form.clone()))
                    .await
                    .unwrap();
                let stream = ws.query_stream(
                    &core,
                    "s1".into(),
                    id.clone(),
                    (server.sleep)(&marker),
                    vec![],
                    QueryOptions::default(),
                );
                (id, stream)
            }
        };
        let drive = stream.collect::<Vec<StreamEvent>>();
        let control = async {
            assert!(
                wait_for(&core, &observer, server, &marker, 1).await,
                "{stop:?}: the statement never started"
            );
            let started = Instant::now();
            match stop {
                Stop::WorkspaceCancel => ws.cancel(&core, "s1"),
                Stop::Disconnect => ws.disconnect(&core, &id).await.unwrap(),
                Stop::CloseAll => ws.close_all(&core).await,
                Stop::CoreCancel => core.cancel_stream("s1"),
            }
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "{stop:?} took {:?} to answer",
                started.elapsed()
            );
            started
        };
        let (events, started) = tokio::time::timeout(
            Duration::from_secs(20),
            futures::future::join(drive, control),
        )
        .await
        .unwrap_or_else(|_| panic!("{} {stop:?}: the stream never ended", server.var));
        assert!(
            wait_for(&core, &observer, server, &marker, 0).await,
            "{} {stop:?}: still running on the server {:?} after the stop",
            server.var,
            started.elapsed()
        );
        assert!(
            !events.iter().any(|e| matches!(e, StreamEvent::Done)),
            "{stop:?}: {events:?}"
        );
        // The connection (where it's still open) still works.
        if matches!(stop, Stop::WorkspaceCancel) {
            let r = ws.query(&core, &id, "SELECT 1", vec![]).await.unwrap();
            assert_eq!(r.rows.len(), 1);
            ws.disconnect(&core, &id).await.unwrap();
        }
        if matches!(stop, Stop::CoreCancel) {
            let r = core.query(&id, "SELECT 1", vec![]).await.unwrap();
            assert_eq!(r.rows.len(), 1);
            core.disconnect(&id).await.unwrap();
        }
    }
    core.disconnect(&observer).await.unwrap();
}

/// Statements whose text the server reports differently from what was
/// sent still stop on a cancel: a bound parameter (MySQL with binlog shows
/// it expanded), leading whitespace (MySQL and MariaDB strip it) and a
/// character outside the BMP (MariaDB shows `????`, and MySQL can't compare
/// it with its utf8mb3 `PROCESSLIST`).
/// A statement's SQL and parameters, for a marker.
type Case = fn(&Server, &str) -> (String, Vec<seaquel_core::Value>);

async fn awkward_statements_stop_on_the_server(server: &Server) {
    let Some(config) = config(server) else {
        return;
    };
    let core = seaquel_core::with_default_plugins()
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .build();
    let observer = core.connect(&config).await.expect("observer").connection_id;
    let cases: [(&str, Case); 5] = [
        ("trailing semicolon", |s, m| {
            (format!("{};", (s.sleep)(m)), vec![])
        }),
        ("trailing semicolon and whitespace", |s, m| {
            (format!("{} ;  \n", (s.sleep)(m)), vec![])
        }),
        ("bound parameter", |s, m| {
            ((s.sleep_param)(m), vec![seaquel_core::Value::Int(30)])
        }),
        ("leading whitespace", |s, m| {
            (format!("\n\n  \t {}", (s.sleep)(m)), vec![])
        }),
        ("emoji", |s, m| {
            (
                (s.sleep)(m).replacen("SELECT ", "SELECT '\u{1F600} ok' AS e, ", 1),
                vec![],
            )
        }),
    ];
    for (name, case) in cases {
        let marker = format!(
            "seaquel_awk_{}",
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        );
        let (sql, params) = case(server, &marker);
        let id = core.connect(&config).await.unwrap().connection_id;
        let stream = core.query_stream(
            "s1".into(),
            id.clone(),
            sql,
            params,
            QueryOptions::default(),
        );
        let control = async {
            assert!(
                wait_for(&core, &observer, server, &marker, 1).await,
                "{} {name}: the statement never started",
                server.var
            );
            core.cancel_stream("s1");
        };
        tokio::time::timeout(
            Duration::from_secs(20),
            futures::future::join(stream.collect::<Vec<_>>(), control),
        )
        .await
        .unwrap_or_else(|_| panic!("{} {name}: the stream never ended", server.var));
        assert!(
            wait_for(&core, &observer, server, &marker, 0).await,
            "{} {name}: still running on the server after the cancel",
            server.var
        );
        core.disconnect(&id).await.unwrap();
    }
    core.disconnect(&observer).await.unwrap();
}

#[tokio::test]
async fn postgres_awkward_statements_stop() {
    awkward_statements_stop_on_the_server(&POSTGRES).await;
}

#[tokio::test]
async fn mysql_awkward_statements_stop() {
    awkward_statements_stop_on_the_server(&MYSQL).await;
}

#[tokio::test]
async fn mariadb_awkward_statements_stop() {
    awkward_statements_stop_on_the_server(&MARIADB).await;
}

#[tokio::test]
async fn postgres_stops_on_the_server() {
    stops_on_the_server(&POSTGRES).await;
}

#[tokio::test]
async fn mysql_stops_on_the_server() {
    stops_on_the_server(&MYSQL).await;
}

#[tokio::test]
async fn mariadb_stops_on_the_server() {
    stops_on_the_server(&MARIADB).await;
}

/// SQL Server has no cancel of its own here: the driver closes its one
/// session when a call is dropped mid-statement. Closing it is enough: the
/// server sees the socket go and ends the batch (checked here for Core's
/// cancel and disconnect).
#[tokio::test]
async fn mssql_stops_when_its_session_closes() {
    let server = Server {
        var: "SEAQUEL_TEST_MSSQL",
        ty: "mssql",
        sleep: |m| format!("WAITFOR DELAY '00:00:30'; SELECT 1 AS {m}"),
        sleep_param: |m| format!("WAITFOR DELAY @P1; SELECT 1 AS {m}"),
        running: |m| {
            format!(
                "SELECT COUNT(*) FROM sys.dm_exec_requests r \
                 CROSS APPLY sys.dm_exec_sql_text(r.sql_handle) t \
                 WHERE r.session_id <> @@SPID AND t.text LIKE '%{m}%'"
            )
        },
    };
    let Some(config) = config(&server) else {
        return;
    };
    let core = seaquel_core::with_default_plugins()
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .build();
    let observer = core.connect(&config).await.expect("observer").connection_id;
    for stop in [Stop::CoreCancel, Stop::Disconnect] {
        let marker = marker(stop);
        let id = core.connect(&config).await.unwrap().connection_id;
        let stream = core.query_stream(
            "s1".into(),
            id.clone(),
            (server.sleep)(&marker),
            vec![],
            QueryOptions::default(),
        );
        let control = async {
            assert!(
                wait_for(&core, &observer, &server, &marker, 1).await,
                "{stop:?}: the statement never started"
            );
            match stop {
                Stop::Disconnect => core.disconnect(&id).await.unwrap(),
                _ => core.cancel_stream("s1"),
            }
        };
        tokio::time::timeout(
            Duration::from_secs(20),
            futures::future::join(stream.collect::<Vec<_>>(), control),
        )
        .await
        .expect("the stream never ended");
        assert!(
            wait_for(&core, &observer, &server, &marker, 0).await,
            "{stop:?}: still running on the server"
        );
        if matches!(stop, Stop::CoreCancel) {
            core.disconnect(&id).await.unwrap();
        }
    }
    core.disconnect(&observer).await.unwrap();
}
