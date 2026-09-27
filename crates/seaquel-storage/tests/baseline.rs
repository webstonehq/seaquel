//! The baseline against the frozen schema of every release (Task 2's
//! fixtures): each file opens, ends in the structure today's TypeScript
//! upgrade gave it, and a second open changes nothing.

mod common;

use common::*;
use seaquel_storage::{Storage, StorageOptions};
use sqlx::Connection;

/// Every release that wrote a metadata file, plus today's tree, with the
/// fixture that holds the structure today's upgrade path gives it.
const RELEASES: &[(&str, &str)] = &[
    (
        "v2026.4.5-beta.1",
        // Today's TypeScript can't upgrade a beta.1 file (it fails on
        // `ai_messages`), so the target is the file taken through v2026.4.5.
        "schemas/upgraded/v2026.4.5-beta.1-via-v2026.4.5.sql",
    ),
    ("v2026.4.5", "schemas/upgraded/v2026.4.5.sql"),
    ("v2026.4.8", "schemas/upgraded/v2026.4.8.sql"),
    ("v2026.9.1", "schemas/upgraded/v2026.9.1.sql"),
    ("v2026.9.2", "schemas/upgraded/v2026.9.2.sql"),
    ("current", "schemas/current.sql"),
];

/// Files from these releases on upgrade to exactly today's fresh schema.
const SAME_AS_CURRENT: &[&str] = &["v2026.4.8", "v2026.9.1", "v2026.9.2", "current"];

/// The version rows each upgraded file ends with.
fn expected_versions(release: &str) -> Vec<i64> {
    match release {
        "v2026.4.5-beta.1" => vec![1, 3, 4],
        _ => vec![4],
    }
}

#[tokio::test]
async fn fresh_file_is_todays_schema_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    let storage = Storage::open(&path, StorageOptions::default())
        .await
        .unwrap();
    storage.close().await;

    let expected = fixture_shape("schemas/current.sql").await;
    assert_same_shape(&schema_shape_of(&path).await, &expected, "fresh file");

    // A fresh file is made by the same statements, in the same order, so even
    // the stored `CREATE` text matches.
    let mut actual = raw_connect(&path).await;
    let reference_path = dir.path().join("reference.db");
    load_fixture(&reference_path, "schemas/current.sql").await;
    let mut reference = raw_connect(&reference_path).await;
    assert_eq!(
        schema_text(&mut actual).await,
        schema_text(&mut reference).await
    );
    assert_eq!(versions(&mut actual).await, vec![4]);
    actual.close().await.unwrap();
    reference.close().await.unwrap();
}

#[tokio::test]
async fn every_release_upgrades_to_todays_shape() {
    for (release, target) in RELEASES {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("seaquel.db");
        load_fixture(&path, &format!("schemas/{release}.sql")).await;

        let storage = Storage::open(&path, StorageOptions::default())
            .await
            .unwrap_or_else(|e| panic!("{release}: {e}"));
        storage.close().await;

        let actual = schema_shape_of(&path).await;
        let expected = fixture_shape(target).await;
        assert_same_shape(&actual, &expected, release);

        if SAME_AS_CURRENT.contains(release) {
            let current = fixture_shape("schemas/current.sql").await;
            assert_same_shape(&actual, &current, &format!("{release} against current.sql"));
        }

        let mut conn = raw_connect(&path).await;
        assert_eq!(
            versions(&mut conn).await,
            expected_versions(release),
            "{release}"
        );
        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name IN (?, ?) ORDER BY name",
        )
        .bind(SQLX_MIGRATIONS)
        .bind(DATA_STEPS)
        .fetch_all(&mut conn)
        .await
        .unwrap();
        assert_eq!(
            tables,
            [DATA_STEPS, SQLX_MIGRATIONS],
            "{release}: migrations and data steps ran"
        );
        conn.close().await.unwrap();
    }
}

#[tokio::test]
async fn opening_twice_changes_nothing() {
    for (release, _) in RELEASES {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("seaquel.db");
        load_fixture(&path, &format!("schemas/{release}.sql")).await;

        Storage::open(&path, StorageOptions::default())
            .await
            .unwrap()
            .close()
            .await;
        let first = snapshot(&path).await;
        Storage::open(&path, StorageOptions::default())
            .await
            .unwrap()
            .close()
            .await;
        assert_eq!(snapshot(&path).await, first, "{release}");
    }
}

/// The beta.1 file with data (`upgrades/v2026.4.5-beta.1-data.json`): the
/// TypeScript fails on it directly, and the Rust baseline has to give what
/// the TypeScript gave after v2026.4.5 had run first, rows included.
#[tokio::test]
async fn beta1_file_with_data_upgrades_and_keeps_its_data() {
    let data: serde_json::Value =
        serde_json::from_str(&fixture("upgrades/v2026.4.5-beta.1-data.json")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    load_fixture(&path, data["base"].as_str().unwrap()).await;
    for sql in data["seedSql"].as_array().unwrap() {
        exec_file(&path, sql.as_str().unwrap()).await;
    }

    // The fixture records that today's TypeScript failed here.
    assert_eq!(data["direct"]["error"], "no such table: ai_messages");
    let via = &data["viaV2026_4_5"];
    assert!(via["error"].is_null() && via["viaError"].is_null());

    Storage::open(&path, StorageOptions::default())
        .await
        .unwrap()
        .close()
        .await;

    let expected = fixture_shape("schemas/upgraded/v2026.4.5-beta.1-via-v2026.4.5.sql").await;
    assert_same_shape(&schema_shape_of(&path).await, &expected, "beta.1 with data");

    let mut conn = raw_connect(&path).await;
    let tables = via["rowsAfter"].as_object().unwrap();
    assert!(tables.contains_key("dashboards") && tables.contains_key("schema_version"));
    for (table, want) in tables {
        let columns: Vec<String> = want["columns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap().to_string())
            .collect();
        // Column order is part of the structure, and upgraded files keep
        // their own.
        let actual_columns: Vec<String> =
            sqlx::query_scalar(&format!("SELECT name FROM pragma_table_info('{table}')"))
                .fetch_all(&mut conn)
                .await
                .unwrap();
        if table != "schema_version" {
            assert_eq!(actual_columns, columns, "{table} columns");
        }

        let got = rows(&mut conn, table, &columns).await;
        let want_rows = want["rows"].as_array().unwrap();
        let want_types = want["types"].as_array().unwrap();
        assert_eq!(got.len(), want_rows.len(), "{table} row count");
        for (i, (values, types)) in got.iter().enumerate() {
            assert_eq!(values, &want_rows[i], "{table} row {i}");
            assert_eq!(types, &want_types[i], "{table} row {i} types");
        }
    }
    conn.close().await.unwrap();
}

/// The comparison isn't blind: it reports exactly the differences the
/// fixtures README records between older upgraded files and a fresh one, and
/// none for v2026.4.8 on.
#[tokio::test]
async fn the_comparison_sees_known_differences() {
    let current = fixture_shape("schemas/current.sql").await;
    let lines = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();

    // v2026.4.5: only `project_state.connection_order`'s position.
    let (upgraded, fresh) = shape_diff(
        &fixture_shape("schemas/upgraded/v2026.4.5.sql").await,
        &current,
    );
    assert_eq!(
        upgraded,
        lines(&[
            r#"project_state column 15 starred_shared_query_ids type=TEXT notnull=1 default=Some("'[]'") pk=0 hidden=0"#,
            r#"project_state column 16 starred_shared_dashboard_ids type=TEXT notnull=1 default=Some("'[]'") pk=0 hidden=0"#,
            r#"project_state column 17 pane_layout type=TEXT notnull=0 default=None pk=0 hidden=0"#,
            r#"project_state column 18 connection_order type=TEXT notnull=1 default=Some("'[]'") pk=0 hidden=0"#,
        ])
    );
    assert_eq!(
        fresh,
        lines(&[
            r#"project_state column 15 connection_order type=TEXT notnull=1 default=Some("'[]'") pk=0 hidden=0"#,
            r#"project_state column 16 starred_shared_query_ids type=TEXT notnull=1 default=Some("'[]'") pk=0 hidden=0"#,
            r#"project_state column 17 starred_shared_dashboard_ids type=TEXT notnull=1 default=Some("'[]'") pk=0 hidden=0"#,
            r#"project_state column 18 pane_layout type=TEXT notnull=0 default=None pk=0 hidden=0"#,
        ])
    );

    // beta.1 through v2026.4.5: `project_id` is nullable, last and without a
    // foreign key on saved_queries and dashboards; the rest is column order
    // in those two tables and project_state.
    let (upgraded, fresh) = shape_diff(
        &fixture_shape("schemas/upgraded/v2026.4.5-beta.1-via-v2026.4.5.sql").await,
        &current,
    );
    for line in [
        "saved_queries column 12 project_id type=TEXT notnull=0 default=None pk=0 hidden=0",
        "dashboards column 10 project_id type=TEXT notnull=0 default=None pk=0 hidden=0",
    ] {
        assert!(upgraded.contains(&line.to_string()), "{line}");
    }
    for line in [
        "saved_queries column 01 project_id type=TEXT notnull=1 default=None pk=0 hidden=0",
        "dashboards column 01 project_id type=TEXT notnull=1 default=None pk=0 hidden=0",
        r#"saved_queries fk 0.0 project_id -> projects(Some("id")) on_update=NO ACTION on_delete=CASCADE match=NONE"#,
        r#"dashboards fk 0.0 project_id -> projects(Some("id")) on_update=NO ACTION on_delete=CASCADE match=NONE"#,
    ] {
        assert!(fresh.contains(&line.to_string()), "{line}");
    }
    for line in upgraded.iter().chain(&fresh) {
        assert!(
            line.starts_with("saved_queries column ")
                || line.starts_with("dashboards column ")
                || line.starts_with("project_state column ")
                || line.starts_with("saved_queries fk ")
                || line.starts_with("dashboards fk "),
            "unexpected difference: {line}"
        );
    }

    // beta.1 upgraded directly by today's TypeScript: half done. Tables
    // (with their CHECKs) are missing, and connection_id is still there.
    let (upgraded, fresh) = shape_diff(
        &fixture_shape("schemas/upgraded/v2026.4.5-beta.1.sql").await,
        &current,
    );
    for line in ["table ai_messages", "table query_versions", "query_versions check CHECK ((snapshot IS NOT NULL AND diff IS NULL) OR (snapshot IS NULL AND diff IS NOT NULL))", "vault_state check CHECK (id = 1)"] {
        assert!(fresh.contains(&line.to_string()), "{line}");
    }
    assert!(upgraded.contains(
        &"saved_queries column 01 connection_id type=TEXT notnull=1 default=None pk=0 hidden=0"
            .to_string()
    ));

    // CHECK clauses are part of every shape.
    assert!(current.contains(&"theme_preferences check CHECK (id = 1)".to_string()));

    for same in [
        "schemas/upgraded/v2026.4.8.sql",
        "schemas/upgraded/v2026.9.1.sql",
        "schemas/upgraded/v2026.9.2.sql",
    ] {
        assert_eq!(fixture_shape(same).await, current, "{same}");
    }
}

/// `schema::is_current`, which a read-only open relies on, says true exactly
/// when the baseline would change nothing, on every release's file.
#[tokio::test]
async fn is_current_is_true_exactly_when_the_baseline_changes_nothing() {
    let mut seen = Vec::new();
    for (release, _) in RELEASES {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("seaquel.db");
        load_fixture(&path, &format!("schemas/{release}.sql")).await;
        let before = snapshot(&path).await;

        let mut conn = raw_connect(&path).await;
        let current = seaquel_storage::schema::is_current(&mut conn)
            .await
            .unwrap();
        let mut tx = conn.begin().await.unwrap();
        seaquel_storage::schema::baseline(&mut tx).await.unwrap();
        tx.commit().await.unwrap();
        let after_baseline = seaquel_storage::schema::is_current(&mut conn)
            .await
            .unwrap();
        conn.close().await.unwrap();

        let changed = snapshot(&path).await != before;
        assert_eq!(current, !changed, "{release}");
        assert!(after_baseline, "{release}");
        seen.push(current);
    }
    // Both answers are covered.
    assert!(seen.contains(&true) && seen.contains(&false), "{seen:?}");
}
