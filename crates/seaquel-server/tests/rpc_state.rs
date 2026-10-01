//! Phase 5d-2 over `/rpc`: the `settings` and `ui` groups and the
//! `library` additions. Ownership (another user's ids are not found and
//! change nothing), the `ui` group's window id against `X-Seaquel-Origin`,
//! the web limits, API keys refused on the web, statuses, and
//! `storageChanged` on each of the user's `/rpc/stream` sockets and no one
//! else's.

use axum::http::StatusCode;
use serde_json::{json, Value as Json};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection};

mod common;
use common::{next, open_stream, quiet, Env};

/// One call of `group` as `user` from tab `origin`.
async fn call(
    env: &Env,
    user: &str,
    origin: Option<&str>,
    group: &str,
    method: &str,
    params: Json,
) -> (StatusCode, Json) {
    let inner = if params.is_null() {
        json!({"method": method})
    } else {
        json!({"method": method, "params": params})
    };
    env.rpc_from(
        user,
        origin.as_slice(),
        &json!({"method": group, "params": inner}),
    )
    .await
}

/// A call from the user's own tab `win-<user>` that must succeed; its
/// `{value, seq}`.
async fn ok(env: &Env, user: &str, group: &str, method: &str, params: Json) -> Json {
    let origin = format!("win-{user}");
    let (status, body) = call(env, user, Some(&origin), group, method, params).await;
    assert_eq!(status, StatusCode::OK, "{group}.{method}: {body}");
    assert_eq!(body["result"]["method"], method, "{body}");
    body["result"]["result"].clone()
}

fn id(v: &Json) -> String {
    v["value"]["id"].as_str().unwrap().to_string()
}

fn view_state(project: &str) -> Json {
    json!({"projectId": project, "queryTabs": [{"id": "t1", "name": "Q", "query": "SELECT 1"}],
        "schemaTabs": [], "explainTabs": [], "erdTabs": [], "tabOrder": ["t1"],
        "activeQueryTabId": "t1", "activeView": "query"})
}

/// Everything 5d-2 stores for `user`, without the `seq`s.
async fn everything(env: &Env, user: &str, project: &str, conn: &str, chat: &str) -> Json {
    let s = |m: &'static str, p: Json| async move {
        ok(env, user, "settings", m, p).await["value"].clone()
    };
    let l = |m: &'static str, p: Json| async move {
        ok(env, user, "library", m, p).await["value"].clone()
    };
    let chat_messages = {
        let origin = format!("win-{user}");
        let (_, body) = call(
            env,
            user,
            Some(&origin),
            "library",
            "chatMessagesList",
            json!({"chatId": chat}),
        )
        .await;
        // Alice's chat id in Bob's file: an empty list, not an error.
        body["result"]["result"]["value"].clone()
    };
    json!({
        "dashboards": l("dashboardsList", json!({"projectId": project})).await,
        "versions": l("dashboardVersionsList", json!({"projectId": project})).await,
        "workflows": l("workflowsList", json!({"projectId": project})).await,
        "chats": l("chatsList", json!({"connectionId": conn})).await,
        "messages": chat_messages,
        "sidebar": l("projectSidebarGet", json!({"projectId": project})).await,
        "ai": s("aiSettingsGet", Json::Null).await,
        "themes": s("themesGet", Json::Null).await,
        "onboarding": s("onboardingGet", Json::Null).await,
        "tutorial": s("tutorialList", Json::Null).await,
        "setting": s("settingGet", json!({"key": "editorKeybindingMode"})).await,
        "import": s("importStateGet", json!({"source": "tableplus"})).await,
    })
}

/// Window `win-<user>`'s active project and view state of `project`,
/// without the `seq`s.
async fn window(env: &Env, user: &str, project: &str) -> Json {
    let window = format!("win-{user}");
    json!({
        "active": ok(env, user, "ui", "windowGet", json!({"windowId": window})).await["value"],
        "state": ok(env, user, "ui", "windowStateLoad",
            json!({"windowId": window, "projectId": project})).await["value"],
    })
}

/// Every row of every table in `user`'s `meta.db`, and its schema, read
/// beside the server's own connections: each row as its columns' SQL
/// literals, sorted.
async fn file_rows(env: &Env, user: &str) -> Vec<String> {
    let path = env.dir.path().join("users").join(user).join("meta.db");
    let mut conn = SqliteConnectOptions::new()
        .filename(&path)
        .read_only(true)
        .connect()
        .await
        .unwrap();
    let mut out: Vec<String> = sqlx::query_scalar(
        "SELECT type || ' ' || name || ' ' || coalesce(sql, '') FROM sqlite_master",
    )
    .fetch_all(&mut conn)
    .await
    .unwrap();
    let tables: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table'")
            .fetch_all(&mut conn)
            .await
            .unwrap();
    let quote = |name: &str| format!("\"{}\"", name.replace('"', "\"\""));
    for table in tables {
        let columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info(?) ORDER BY cid")
                .bind(&table)
                .fetch_all(&mut conn)
                .await
                .unwrap();
        let row = columns
            .iter()
            .map(|c| format!("quote({})", quote(c)))
            .collect::<Vec<_>>()
            .join(" || '|' || ");
        let rows: Vec<String> = sqlx::query_scalar(&format!("SELECT {row} FROM {}", quote(&table)))
            .fetch_all(&mut conn)
            .await
            .unwrap();
        out.extend(rows.into_iter().map(|r| format!("{table}: {r}")));
    }
    conn.close().await.unwrap();
    out.sort();
    out
}

#[tokio::test]
async fn another_users_ids_are_not_found() {
    let env = Env::new(4);
    // Alice's things, in a project of her own.
    ok(&env, "alice", "library", "projectEnsureDefault", Json::Null).await;
    let p = id(&ok(
        &env,
        "alice",
        "library",
        "projectCreate",
        json!({"project": {"name": "Mine"}}),
    )
    .await);
    let conn = id(&ok(
        &env,
        "alice",
        "library",
        "connectionCreate",
        json!({"connection": {"projectId": p, "name": "c", "type": "postgres",
            "host": "db.example.com", "port": 5432, "databaseName": "app", "username": "u"}}),
    )
    .await);
    let d = id(&ok(
        &env,
        "alice",
        "library",
        "dashboardCreate",
        json!({"dashboard": {"projectId": p, "name": "D", "widgets": [], "viewport": {}}}),
    )
    .await);
    let version = ok(
        &env,
        "alice",
        "library",
        "dashboardUpdate",
        json!({"id": d, "patch": {"name": "D2", "captureVersion": true}}),
    )
    .await["value"]["version"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let w = id(&ok(
        &env,
        "alice",
        "library",
        "workflowCreate",
        json!({"workflow": {"projectId": p, "workflow": {"name": "W"}}}),
    )
    .await);
    let chat = id(&ok(
        &env,
        "alice",
        "library",
        "chatCreate",
        json!({"chat": {"connectionId": conn, "title": "T"}}),
    )
    .await);
    ok(
        &env,
        "alice",
        "library",
        "chatMessagesPut",
        json!({"chatId": chat, "messages": [{"id": "m1", "role": "user", "content": "hi",
            "timestamp": "2026-01-01T00:00:00Z"}]}),
    )
    .await;
    ok(
        &env,
        "alice",
        "library",
        "projectSidebarSet",
        json!({"projectId": p, "connectionOrder": [conn]}),
    )
    .await;
    let provider = ok(
        &env,
        "alice",
        "settings",
        "aiProviderCreate",
        json!({"provider": {"name": "P", "type": "anthropic"}}),
    )
    .await["value"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let theme = ok(
        &env,
        "alice",
        "settings",
        "userThemeCreate",
        json!({"theme": {"name": "Mine"}}),
    )
    .await["value"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    ok(
        &env,
        "alice",
        "ui",
        "windowActivate",
        json!({"windowId": "win-alice", "projectId": p}),
    )
    .await;
    ok(
        &env,
        "alice",
        "ui",
        "windowStateSave",
        json!({"windowId": "win-alice", "projectId": p, "rev": 1, "state": view_state(&p)}),
    )
    .await;
    ok(&env, "bob", "library", "projectEnsureDefault", Json::Null).await;
    // The files bracket the reads too, so a read that wrote would show.
    let alice_file = file_rows(&env, "alice").await;
    let bob_file = file_rows(&env, "bob").await;
    for table in ["dashboards", "dashboard_versions", "ai_chats"] {
        let prefix = format!("{table}: ");
        assert!(alice_file.iter().any(|r| r.starts_with(&prefix)), "{table}");
    }
    let alice_before = everything(&env, "alice", &p, &conn, &chat).await;
    let alice_window = window(&env, "alice", &p).await;
    assert_eq!(alice_window["active"]["activeProjectId"], p.as_str());
    assert_eq!(alice_window["state"]["rev"], 1);
    let bob_before = everything(&env, "bob", &p, &conn, &chat).await;
    // Bob's reads naming her ids come back empty.
    assert_eq!(
        bob_before,
        json!({
            "dashboards": [],
            "versions": [],
            "workflows": [],
            "chats": [],
            "messages": {"messages": [], "storedBytes": 0, "full": false},
            "sidebar": [],
            "ai": bob_before["ai"],
            "themes": bob_before["themes"],
            "onboarding": bob_before["onboarding"],
            "tutorial": bob_before["tutorial"],
            "setting": bob_before["setting"],
            "import": bob_before["import"],
        })
    );
    assert!(
        !bob_before.to_string().contains(&provider) && !bob_before.to_string().contains(&theme),
        "{bob_before}"
    );

    for (group, method, params, code) in [
        (
            "library",
            "dashboardCreate",
            json!({"dashboard": {"projectId": p, "name": "x", "widgets": [], "viewport": {}}}),
            "PROJECT_NOT_FOUND",
        ),
        (
            "library",
            "dashboardUpdate",
            json!({"id": d, "patch": {"name": "x"}}),
            "DASHBOARD_NOT_FOUND",
        ),
        (
            "library",
            "dashboardRemove",
            json!({"id": d}),
            "DASHBOARD_NOT_FOUND",
        ),
        (
            "library",
            "dashboardVersionGet",
            json!({"dashboardId": d, "versionId": version}),
            "DASHBOARD_NOT_FOUND",
        ),
        (
            "library",
            "workflowGet",
            json!({"workflowId": w}),
            "WORKFLOW_NOT_FOUND",
        ),
        (
            "library",
            "workflowRename",
            json!({"workflowId": w, "name": "x"}),
            "WORKFLOW_NOT_FOUND",
        ),
        (
            "library",
            "workflowCreate",
            json!({"workflow": {"projectId": p, "workflow": {"name": "x"}}}),
            "PROJECT_NOT_FOUND",
        ),
        (
            "library",
            "workflowUpdate",
            json!({"id": w, "workflow": {"name": "x"}}),
            "WORKFLOW_NOT_FOUND",
        ),
        (
            "library",
            "workflowRemove",
            json!({"id": w}),
            "WORKFLOW_NOT_FOUND",
        ),
        (
            "library",
            "chatCreate",
            json!({"chat": {"connectionId": conn, "title": "x"}}),
            "CONNECTION_NOT_FOUND",
        ),
        (
            "library",
            "chatUpdate",
            json!({"id": chat, "patch": {"title": "x"}}),
            "CHAT_NOT_FOUND",
        ),
        (
            "library",
            "chatRemove",
            json!({"id": chat}),
            "CHAT_NOT_FOUND",
        ),
        (
            "library",
            "chatMessagesPut",
            json!({"chatId": chat, "messages": [{"id": "m1", "role": "user",
                "content": "x", "timestamp": "t"}]}),
            "CHAT_NOT_FOUND",
        ),
        (
            "library",
            "chatMessagesRemove",
            json!({"chatId": chat, "ids": ["m1"]}),
            "CHAT_NOT_FOUND",
        ),
        (
            "library",
            "projectSidebarSet",
            json!({"projectId": p, "connectionOrder": []}),
            "PROJECT_NOT_FOUND",
        ),
        (
            "settings",
            "aiProviderUpdate",
            json!({"id": provider, "patch": {"name": "x"}}),
            "AI_PROVIDER_NOT_FOUND",
        ),
        (
            "settings",
            "aiProviderRemove",
            json!({"id": provider}),
            "AI_PROVIDER_NOT_FOUND",
        ),
        (
            "settings",
            "userThemeUpdate",
            json!({"id": theme, "theme": {"name": "x"}}),
            "THEME_NOT_FOUND",
        ),
        (
            "settings",
            "userThemeRemove",
            json!({"id": theme}),
            "THEME_NOT_FOUND",
        ),
        (
            "ui",
            "windowActivate",
            json!({"windowId": "win-bob", "projectId": p}),
            "PROJECT_NOT_FOUND",
        ),
        (
            "ui",
            "windowStateLoad",
            json!({"windowId": "win-bob", "projectId": p}),
            "PROJECT_NOT_FOUND",
        ),
        (
            "ui",
            "windowStateSave",
            json!({"windowId": "win-bob", "projectId": p, "rev": 9, "state": view_state(&p)}),
            "PROJECT_NOT_FOUND",
        ),
    ] {
        let (status, body) = call(&env, "bob", Some("win-bob"), group, method, params).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{group}.{method}: {body}");
        assert_eq!(body["code"], code, "{group}.{method}: {body}");
    }
    // Bob naming Alice's window from his own tab is refused, and from a tab
    // claiming her window's id he reaches only his own file.
    let (status, body) = call(
        &env,
        "bob",
        Some("win-bob"),
        "ui",
        "windowGet",
        json!({"windowId": "win-alice"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let got = ok(
        &env,
        "bob",
        "ui",
        "windowGet",
        json!({"windowId": "win-bob"}),
    )
    .await;
    assert!(got["value"]["activeProjectId"].is_null(), "{got}");
    let (status, body) = call(
        &env,
        "bob",
        Some("win-alice"),
        "ui",
        "windowGet",
        json!({"windowId": "win-alice"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["result"]["result"]["value"]["activeProjectId"].is_null(),
        "{body}"
    );
    // Reads naming her ids see nothing of hers.
    let (_, body) = call(
        &env,
        "bob",
        Some("win-bob"),
        "library",
        "chatMessagesList",
        json!({"chatId": chat}),
    )
    .await;
    assert_eq!(
        body["result"]["result"]["value"],
        json!({"messages": [], "storedBytes": 0, "full": false}),
        "{body}"
    );

    assert_eq!(
        everything(&env, "alice", &p, &conn, &chat).await,
        alice_before
    );
    assert_eq!(window(&env, "alice", &p).await, alice_window);
    assert_eq!(everything(&env, "bob", &p, &conn, &chat).await, bob_before);
    assert_eq!(file_rows(&env, "alice").await, alice_file);
    assert_eq!(file_rows(&env, "bob").await, bob_file);

    // From a tab claiming her window's id, Bob's view-state calls on his
    // own project land in his own file only.
    let mine = "default-seaquel";
    for (method, params) in [
        (
            "windowActivate",
            json!({"windowId": "win-alice", "projectId": mine}),
        ),
        (
            "windowStateLoad",
            json!({"windowId": "win-alice", "projectId": mine}),
        ),
        (
            "windowStateSave",
            json!({"windowId": "win-alice", "projectId": mine, "rev": 5,
                "state": view_state(mine)}),
        ),
    ] {
        let (status, body) = call(&env, "bob", Some("win-alice"), "ui", method, params).await;
        assert_eq!(status, StatusCode::OK, "{method}: {body}");
    }
    let (_, body) = call(
        &env,
        "bob",
        Some("win-alice"),
        "ui",
        "windowGet",
        json!({"windowId": "win-alice"}),
    )
    .await;
    assert_eq!(
        body["result"]["result"]["value"]["activeProjectId"], mine,
        "{body}"
    );
    assert_eq!(window(&env, "alice", &p).await, alice_window);
    assert_eq!(file_rows(&env, "alice").await, alice_file);
    assert_ne!(file_rows(&env, "bob").await, bob_file);
}

#[tokio::test]
async fn a_window_id_that_isnt_the_origin_is_refused_over_http() {
    let env = Env::new(4);
    ok(&env, "alice", "library", "projectEnsureDefault", Json::Null).await;
    let p = "default-seaquel";
    for (origins, method, params) in [
        (vec!["tab-2"], "windowGet", json!({"windowId": "tab-1"})),
        (vec![], "windowGet", json!({"windowId": "tab-1"})),
        // A bad or repeated header is dropped, so the call has no origin.
        (vec!["tab 1"], "windowGet", json!({"windowId": "tab 1"})),
        (
            vec!["tab-1", "tab-1"],
            "windowGet",
            json!({"windowId": "tab-1"}),
        ),
        (
            vec!["tab-2"],
            "windowActivate",
            json!({"windowId": "tab-1", "projectId": p}),
        ),
        (
            vec!["tab-2"],
            "windowStateLoad",
            json!({"windowId": "tab-1", "projectId": p}),
        ),
        (
            vec!["tab-2"],
            "windowStateSave",
            json!({"windowId": "tab-1", "projectId": p, "rev": 1, "state": view_state(p)}),
        ),
    ] {
        let body = json!({"method": "ui", "params": {"method": method, "params": params}});
        let (status, body) = env.rpc_from("alice", &origins, &body).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{method} {origins:?}: {body}"
        );
        assert_eq!(body["code"], "INVALID_ARGUMENT", "{body}");
    }
    // Nothing was stored for tab-1: its first load finds no row of its own.
    let (status, body) = call(
        &env,
        "alice",
        Some("tab-1"),
        "ui",
        "windowStateLoad",
        json!({"windowId": "tab-1", "projectId": p}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["result"]["result"]["value"]["copiedFrom"], "empty");
    // With the header naming it, the save lands.
    let (status, body) = call(
        &env,
        "alice",
        Some("tab-1"),
        "ui",
        "windowStateSave",
        json!({"windowId": "tab-1", "projectId": p, "rev": 1, "state": view_state(p)}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["result"]["result"]["value"],
        json!({"stale": false, "rev": 1})
    );
}

#[tokio::test]
async fn a_state_write_reaches_every_socket_of_that_user_and_none_of_another() {
    let env = Env::new(4);
    let addr = env.serve().await;
    ok(&env, "alice", "library", "projectEnsureDefault", Json::Null).await;
    ok(&env, "bob", "library", "projectEnsureDefault", Json::Null).await;
    let mut alice_1 = open_stream(addr, "alice").await;
    let mut alice_2 = open_stream(addr, "alice").await;
    let mut bob = open_stream(addr, "bob").await;

    let d = ok(
        &env,
        "alice",
        "library",
        "dashboardCreate",
        json!({"dashboard": {"projectId": "default-seaquel", "name": "canary-name",
            "widgets": [{"sql": "canary-widget"}], "viewport": {}}}),
    )
    .await;
    let theme = ok(
        &env,
        "alice",
        "settings",
        "userThemeCreate",
        json!({"theme": {"name": "canary-theme"}}),
    )
    .await;
    let saved = ok(
        &env,
        "alice",
        "ui",
        "windowStateSave",
        json!({"windowId": "win-alice", "projectId": "default-seaquel", "rev": 1,
            "state": view_state("default-seaquel")}),
    )
    .await;
    for ws in [&mut alice_1, &mut alice_2] {
        assert_eq!(
            next(ws).await,
            json!({"type": "storageChanged", "kind": "dashboard", "scope": "default-seaquel",
                   "ids": [id(&d)], "origin": "win-alice", "seq": d["seq"]})
        );
        let event = next(ws).await;
        assert_eq!(event["kind"], "theme", "{event}");
        assert_eq!(event["ids"], json!([theme["value"]["id"]]), "{event}");
        assert_eq!(event["seq"], theme["seq"], "{event}");
        assert_eq!(
            next(ws).await,
            json!({"type": "storageChanged", "kind": "projectState",
                   "scope": "default-seaquel", "ids": ["win-alice"], "origin": "win-alice",
                   "seq": saved["seq"]})
        );
        quiet(ws, 150).await;
    }
    quiet(&mut bob, 150).await;

    // A refused write sends nothing: a stale view-state save and a theme
    // that doesn't exist.
    ok(
        &env,
        "alice",
        "ui",
        "windowStateSave",
        json!({"windowId": "win-alice", "projectId": "default-seaquel", "rev": 1,
            "state": view_state("default-seaquel")}),
    )
    .await;
    let (status, _) = call(
        &env,
        "alice",
        Some("win-alice"),
        "settings",
        "userThemeRemove",
        json!({"id": "theme-nope"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    quiet(&mut alice_1, 150).await;

    // Bob's setting reaches only Bob.
    ok(
        &env,
        "bob",
        "settings",
        "settingSet",
        json!({"key": "editorKeybindingMode", "value": "emacs"}),
    )
    .await;
    let event = next(&mut bob).await;
    assert_eq!(event["kind"], "setting");
    assert_eq!(event["ids"], json!(["editorKeybindingMode"]));
    assert!(!event.to_string().contains("emacs"), "{event}");
    quiet(&mut alice_1, 150).await;
}

#[tokio::test]
async fn the_web_state_limits_apply() {
    let env = Env::new(4);
    ok(&env, "alice", "library", "projectEnsureDefault", Json::Null).await;
    let p = "default-seaquel";
    let long_name = "n".repeat(1025);
    let big_setting = format!("{{\"a\":\"{}\"}}", "x".repeat(256 * 1024));
    let mut many_tabs = view_state(p);
    many_tabs["queryTabs"] = (0..501)
        .map(|i| json!({"id": format!("t{i}"), "name": "Q", "query": ""}))
        .collect();
    let mut big_tab = view_state(p);
    big_tab["queryTabs"][0]["query"] = json!("x".repeat(2 * 1024 * 1024 + 1));
    for (group, method, params, needle) in [
        (
            "library",
            "dashboardCreate",
            json!({"dashboard": {"projectId": p, "name": long_name, "widgets": [],
                "viewport": {}}}),
            "",
        ),
        (
            "library",
            "dashboardCreate",
            json!({"dashboard": {"projectId": p, "name": "big",
                "widgets": ["x".repeat(4 * 1024 * 1024)], "viewport": {}}}),
            "max_dashboard_bytes",
        ),
        (
            "library",
            "workflowCreate",
            json!({"workflow": {"projectId": p, "workflow": {"name": "big",
                "rows": "x".repeat(16 * 1024 * 1024)}}}),
            "max_workflow_bytes",
        ),
        (
            "settings",
            "settingSet",
            json!({"key": "license_nudge", "value": big_setting}),
            "",
        ),
        (
            "settings",
            "aiProviderCreate",
            json!({"provider": {"name": long_name, "type": "anthropic"}}),
            "",
        ),
        (
            "settings",
            "userThemeCreate",
            json!({"theme": {"name": "big", "css": "x".repeat(256 * 1024)}}),
            "",
        ),
        (
            "ui",
            "windowStateSave",
            json!({"windowId": "win-alice", "projectId": p, "rev": 1, "state": many_tabs}),
            "max_tabs",
        ),
        (
            "ui",
            "windowStateSave",
            json!({"windowId": "win-alice", "projectId": p, "rev": 1, "state": big_tab}),
            "max_tab_text_bytes",
        ),
    ] {
        let (status, body) = call(&env, "alice", Some("win-alice"), group, method, params).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{group}.{method}: {body}");
        assert_eq!(body["code"], "INVALID_ARGUMENT", "{body}");
        assert!(
            body["message"].as_str().unwrap().contains(needle),
            "{group}.{method}: {body}"
        );
    }
    // A chat past its message size.
    let conn = ok(
        &env,
        "alice",
        "library",
        "connectionCreate",
        json!({"connection": {"projectId": p, "name": "c", "type": "postgres",
            "host": "db.example.com", "port": 5432, "databaseName": "app", "username": "u"}}),
    )
    .await;
    let chat = ok(
        &env,
        "alice",
        "library",
        "chatCreate",
        json!({"chat": {"connectionId": id(&conn), "title": "T"}}),
    )
    .await;
    let (status, body) = call(
        &env,
        "alice",
        Some("win-alice"),
        "library",
        "chatMessagesPut",
        json!({"chatId": id(&chat), "messages": [{"id": "m1", "role": "user",
            "content": "x".repeat(1024 * 1024 + 1), "timestamp": "t"}]}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    // Nothing was stored by any refusal.
    assert_eq!(
        ok(
            &env,
            "alice",
            "library",
            "dashboardsList",
            json!({"projectId": p})
        )
        .await["value"],
        json!([])
    );
    assert_eq!(
        ok(
            &env,
            "alice",
            "library",
            "workflowsList",
            json!({"projectId": p})
        )
        .await["value"],
        json!([])
    );
}

#[tokio::test]
async fn an_api_key_over_web_is_not_supported() {
    let env = Env::new(4);
    for (method, params) in [
        (
            "aiProviderCreate",
            json!({"provider": {"name": "P", "type": "anthropic"}, "apiKey": "canary-key"}),
        ),
        (
            "aiProviderUpdate",
            json!({"id": "p1", "patch": {}, "apiKey": "canary-key"}),
        ),
        (
            "aiProviderUpdate",
            json!({"id": "p1", "patch": {}, "apiKey": null}),
        ),
    ] {
        let (status, body) =
            call(&env, "alice", Some("win-alice"), "settings", method, params).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{method}: {body}");
        assert_eq!(body["code"], "NOT_SUPPORTED", "{body}");
        assert!(!body.to_string().contains("canary"), "{body}");
    }
    // Nothing was stored; without a key the provider is saved.
    let ai = ok(&env, "alice", "settings", "aiSettingsGet", Json::Null).await;
    assert_eq!(ai["value"]["providers"], json!([]));
    ok(
        &env,
        "alice",
        "settings",
        "aiProviderCreate",
        json!({"provider": {"name": "P", "type": "anthropic"}}),
    )
    .await;
}

#[tokio::test]
async fn a_retired_storage_method_is_a_400() {
    let env = Env::new(4);
    for body in [
        json!({"method": "storage", "params": {"method": "appStateSet",
            "params": {"key": "k", "value": "v"}}}),
        json!({"method": "storage", "params": {"method": "projectStateLoad",
            "params": {"projectId": "p"}}}),
        json!({"method": "storage", "params": {"method": "themesLoadUserThemes"}}),
    ] {
        let (status, res) = env.rpc("alice", &body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {res}");
        assert_eq!(res["code"], "INVALID_ARGUMENT");
    }
    // A bad body opens nothing.
    assert!(!env.dir.path().join("users/alice").exists());
}
