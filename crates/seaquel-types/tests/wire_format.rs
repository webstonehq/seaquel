//! Pins the JSON shape of every wire type. The TypeScript frontend (wire.ts,
//! both providers) depends on these exact shapes; a failure here means the
//! frontend breaks too.

use seaquel_types::{
    BatchStatement, ColumnCategory, ColumnTypeInfo, ConnectConfig, ConnectResult,
    CreateTableColumn, CreateTableDefinition, CreateTableForeignKey, CreateTableIndex,
    DatabaseOverview, DatabaseStatistics, DbError, DriverType, ExecuteResult, ExpectRows,
    ExplainPlanNode, ExplainResult, ForeignKeyRef, IndexUsageInfo, QueryResult, SchemaColumn,
    SchemaIndex, SchemaTable, StreamBatch, StreamEvent, TableKind, TableSizeInfo, Value,
};
use serde_json::{from_value, json, to_value};

#[test]
fn query_result_is_columnar() {
    let r = QueryResult {
        columns: vec!["a".into(), "b".into()],
        rows: vec![vec![Value::Int(1), Value::Text("x".into())]],
    };
    assert_eq!(
        to_value(&r).unwrap(),
        json!({ "columns": ["a", "b"], "rows": [[1, "x"]] })
    );
}

#[test]
fn stream_batch_keeps_snake_case_fields() {
    let b = StreamBatch {
        columns: None,
        rows: vec![],
        is_final: true,
    };
    assert_eq!(
        to_value(&b).unwrap(),
        json!({ "columns": null, "rows": [], "is_final": true })
    );
}

#[test]
fn execute_result_shape() {
    let r = ExecuteResult {
        rows_affected: 3,
        last_insert_id: None,
    };
    assert_eq!(
        to_value(&r).unwrap(),
        json!({ "rows_affected": 3, "last_insert_id": null })
    );
}

#[test]
fn connect_result_shape() {
    let r = ConnectResult {
        connection_id: "sqlite-1".into(),
    };
    assert_eq!(
        to_value(&r).unwrap(),
        json!({ "connection_id": "sqlite-1" })
    );
}

#[test]
fn db_error_shape_and_display() {
    let e = DbError::query_error("boom");
    assert_eq!(
        to_value(&e).unwrap(),
        json!({ "message": "Query failed: boom", "code": "QUERY_ERROR" })
    );
    assert_eq!(e.to_string(), "QUERY_ERROR: Query failed: boom");
}

#[test]
fn connect_config_accepts_every_field() {
    let c: ConnectConfig = from_value(json!({
        "driver": "mssql",
        "connection_string": "cs",
        "host": "db.local",
        "port": 1433,
        "database": "master",
        "username": "sa",
        "password": "pw",
        "encrypt": true,
        "trust_cert": false,
        "path": "/tmp/x.duckdb",
        "create_if_missing": true
    }))
    .unwrap();
    assert_eq!(c.driver, DriverType::Mssql);
    assert_eq!(c.connection_string.as_deref(), Some("cs"));
    assert_eq!(c.host.as_deref(), Some("db.local"));
    assert_eq!(c.port, Some(1433));
    assert_eq!(c.database.as_deref(), Some("master"));
    assert_eq!(c.username.as_deref(), Some("sa"));
    assert_eq!(c.password.as_deref(), Some("pw"));
    assert_eq!(c.encrypt, Some(true));
    assert_eq!(c.trust_cert, Some(false));
    assert_eq!(c.path.as_deref(), Some("/tmp/x.duckdb"));
    assert_eq!(c.create_if_missing, Some(true));
}

#[test]
fn connect_config_optional_fields_default_to_none() {
    let c: ConnectConfig = from_value(json!({ "driver": "duckdb" })).unwrap();
    assert_eq!(c.driver, DriverType::Duckdb);
    assert!(c.connection_string.is_none());
    assert!(c.path.is_none());
    assert!(c.create_if_missing.is_none());
}

#[test]
fn driver_type_is_lowercase_on_the_wire() {
    for (wire, expected) in [
        ("postgres", DriverType::Postgres),
        ("mysql", DriverType::Mysql),
        ("sqlite", DriverType::Sqlite),
        ("mssql", DriverType::Mssql),
        ("duckdb", DriverType::Duckdb),
    ] {
        assert_eq!(from_value::<DriverType>(json!(wire)).unwrap(), expected);
        assert_eq!(expected.as_str(), wire);
    }
    assert!(from_value::<DriverType>(json!("Postgres")).is_err());
    assert!(from_value::<DriverType>(json!("mariadb")).is_err());
}

#[test]
fn batch_statement_params_default_to_empty() {
    let s: BatchStatement = from_value(json!({ "sql": "DELETE FROM t" })).unwrap();
    assert_eq!(s.sql, "DELETE FROM t");
    assert!(s.params.is_empty());
    assert_eq!(s.expect_rows, None);
}

#[test]
fn batch_statement_expect_rows_is_camel_case() {
    let s: BatchStatement = from_value(json!({
        "sql": "UPDATE t SET a = 1 WHERE id = 1",
        "params": [],
        "expectRows": { "min": 1 }
    }))
    .unwrap();
    assert_eq!(s.expect_rows, Some(ExpectRows { min: 1 }));
    assert!(s.check_affected(0, 1).is_ok());
    let err = s.check_affected(2, 0).unwrap_err();
    assert_eq!(err.code, "NO_ROWS_AFFECTED");
    assert!(err.message.contains("(index 2)"), "{}", err.message);
}

#[test]
fn stream_event_batch_is_flattened() {
    let ev = StreamEvent::Batch(StreamBatch {
        columns: Some(vec!["n".into()]),
        rows: vec![vec![Value::Int(1)]],
        is_final: false,
    });
    assert_eq!(
        to_value(&ev).unwrap(),
        json!({ "type": "batch", "columns": ["n"], "rows": [[1]], "is_final": false })
    );
}

#[test]
fn stream_event_done_and_error() {
    assert_eq!(
        to_value(StreamEvent::Done).unwrap(),
        json!({ "type": "done" })
    );
    assert_eq!(
        to_value(StreamEvent::from(DbError::connection_not_found("x"))).unwrap(),
        json!({
            "type": "error",
            "message": "Connection not found: x",
            "code": "CONNECTION_NOT_FOUND"
        })
    );
}

#[test]
fn engine_not_available_error() {
    let e = DbError::engine_not_available("mssql");
    assert_eq!(e.code, "ENGINE_NOT_AVAILABLE");
    assert!(e.message.contains("\"mssql\""), "{}", e.message);
}

#[test]
fn read_only_error_keeps_the_message_as_given() {
    let e = DbError::read_only("Only read-only SELECT queries are permitted");
    assert_eq!(e.code, "READ_ONLY");
    assert_eq!(e.message, "Only read-only SELECT queries are permitted");
}

// --- Dialect types (mirror src/lib/types/{schema,explain,statistics,create-table}.ts) ---

fn round_trip<T>(v: &T, expected: serde_json::Value)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    assert_eq!(to_value(v).unwrap(), expected);
    assert_eq!(&from_value::<T>(expected).unwrap(), v);
}

#[test]
fn schema_table_shape() {
    let t = SchemaTable {
        name: "users".into(),
        schema: "public".into(),
        kind: TableKind::MaterializedView,
        row_count: None,
        columns: vec![],
        indexes: vec![],
    };
    round_trip(
        &t,
        json!({ "name": "users", "schema": "public", "type": "materialized-view", "columns": [], "indexes": [] }),
    );
}

#[test]
fn table_kind_names() {
    assert_eq!(to_value(TableKind::Table).unwrap(), json!("table"));
    assert_eq!(to_value(TableKind::View).unwrap(), json!("view"));
    assert_eq!(
        to_value(TableKind::MaterializedView).unwrap(),
        json!("materialized-view")
    );
}

#[test]
fn schema_table_full_shape() {
    let t = SchemaTable {
        name: "orders".into(),
        schema: "public".into(),
        kind: TableKind::Table,
        row_count: Some(42),
        columns: vec![SchemaColumn {
            name: "user_id".into(),
            ty: "integer".into(),
            cast_type: Some("integer".into()),
            nullable: false,
            default_value: Some("0".into()),
            is_primary_key: false,
            is_foreign_key: true,
            foreign_key_ref: Some(ForeignKeyRef {
                referenced_schema: "public".into(),
                referenced_table: "users".into(),
                referenced_column: "id".into(),
            }),
            collation: Some("Latin1_General_BIN".into()),
            is_unique: false,
            in_unique_constraint: false,
        }],
        indexes: vec![SchemaIndex {
            name: "orders_user_id_idx".into(),
            columns: vec!["user_id".into()],
            unique: false,
            ty: "btree".into(),
        }],
    };
    round_trip(
        &t,
        json!({
            "name": "orders",
            "schema": "public",
            "type": "table",
            "rowCount": 42,
            "columns": [{
                "name": "user_id",
                "type": "integer",
                "castType": "integer",
                "nullable": false,
                "defaultValue": "0",
                "isPrimaryKey": false,
                "isForeignKey": true,
                "foreignKeyRef": {
                    "referencedSchema": "public",
                    "referencedTable": "users",
                    "referencedColumn": "id"
                },
                "collation": "Latin1_General_BIN"
            }],
            "indexes": [{ "name": "orders_user_id_idx", "columns": ["user_id"], "unique": false, "type": "btree" }]
        }),
    );
}

#[test]
fn schema_column_omits_absent_optionals() {
    let c = SchemaColumn {
        name: "id".into(),
        ty: "integer".into(),
        cast_type: None,
        nullable: false,
        default_value: None,
        is_primary_key: true,
        is_foreign_key: false,
        foreign_key_ref: None,
        collation: None,
        is_unique: false,
        in_unique_constraint: false,
    };
    round_trip(
        &c,
        json!({ "name": "id", "type": "integer", "nullable": false, "isPrimaryKey": true, "isForeignKey": false }),
    );
}

/// The UNIQUE flags are sent only when set.
#[test]
fn schema_column_unique_flags() {
    let c = SchemaColumn {
        name: "email".into(),
        ty: "VARCHAR".into(),
        cast_type: None,
        nullable: true,
        default_value: None,
        is_primary_key: false,
        is_foreign_key: false,
        foreign_key_ref: None,
        collation: None,
        is_unique: true,
        in_unique_constraint: true,
    };
    round_trip(
        &c,
        json!({ "name": "email", "type": "VARCHAR", "nullable": true, "isPrimaryKey": false, "isForeignKey": false, "isUnique": true, "inUniqueConstraint": true }),
    );
}

fn leaf_node(id: &str) -> ExplainPlanNode {
    ExplainPlanNode {
        id: id.into(),
        node_type: "Seq Scan".into(),
        relation_name: None,
        alias: None,
        startup_cost: None,
        total_cost: None,
        plan_rows: None,
        plan_width: None,
        actual_startup_time: None,
        actual_total_time: None,
        actual_rows: None,
        actual_loops: None,
        filter: None,
        index_name: None,
        index_cond: None,
        join_type: None,
        hash_cond: None,
        sort_key: None,
        children: vec![],
    }
}

#[test]
fn explain_plan_node_minimal_shape() {
    round_trip(
        &leaf_node("n1"),
        json!({ "id": "n1", "nodeType": "Seq Scan", "children": [] }),
    );
}

#[test]
fn explain_result_shape() {
    let root = ExplainPlanNode {
        id: "n0".into(),
        node_type: "Hash Join".into(),
        relation_name: Some("users".into()),
        alias: Some("u".into()),
        startup_cost: Some(0.5),
        total_cost: Some(12.25),
        plan_rows: Some(100.0),
        plan_width: Some(36),
        actual_startup_time: Some(0.01),
        actual_total_time: Some(1.5),
        actual_rows: Some(99.5),
        actual_loops: Some(1),
        filter: Some("(id > 1)".into()),
        index_name: Some("users_pkey".into()),
        index_cond: Some("(id = 1)".into()),
        join_type: Some("Inner".into()),
        hash_cond: Some("(a.id = b.id)".into()),
        sort_key: Some(vec!["id".into()]),
        children: vec![leaf_node("n1")],
    };
    let r = ExplainResult {
        plan: root,
        planning_time: 0.2,
        execution_time: Some(3.5),
        is_analyze: true,
    };
    round_trip(
        &r,
        json!({
            "plan": {
                "id": "n0",
                "nodeType": "Hash Join",
                "relationName": "users",
                "alias": "u",
                "startupCost": 0.5,
                "totalCost": 12.25,
                "planRows": 100.0,
                "planWidth": 36,
                "actualStartupTime": 0.01,
                "actualTotalTime": 1.5,
                "actualRows": 99.5,
                "actualLoops": 1,
                "filter": "(id > 1)",
                "indexName": "users_pkey",
                "indexCond": "(id = 1)",
                "joinType": "Inner",
                "hashCond": "(a.id = b.id)",
                "sortKey": ["id"],
                "children": [{ "id": "n1", "nodeType": "Seq Scan", "children": [] }]
            },
            "planningTime": 0.2,
            "executionTime": 3.5,
            "isAnalyze": true
        }),
    );
}

#[test]
fn explain_result_omits_execution_time_without_analyze() {
    let r = ExplainResult {
        plan: leaf_node("n0"),
        planning_time: 0.1,
        execution_time: None,
        is_analyze: false,
    };
    round_trip(
        &r,
        json!({
            "plan": { "id": "n0", "nodeType": "Seq Scan", "children": [] },
            "planningTime": 0.1,
            "isAnalyze": false
        }),
    );
}

#[test]
fn database_statistics_shape() {
    let s = DatabaseStatistics {
        overview: DatabaseOverview {
            database_name: "seaquel_test".into(),
            total_size: "8 MB".into(),
            total_size_bytes: Some(8_388_608),
            table_count: 3,
            index_count: 5,
            connection_count: None,
        },
        table_sizes: vec![TableSizeInfo {
            schema: "public".into(),
            name: "users".into(),
            row_count: 10,
            total_size: "16 kB".into(),
            total_size_bytes: 16_384,
            data_size: Some("8 kB".into()),
            index_size: None,
        }],
        index_usage: vec![IndexUsageInfo {
            schema: "public".into(),
            table: "users".into(),
            index_name: "users_pkey".into(),
            size: "16 kB".into(),
            scans: 0,
            rows_read: None,
            unused: true,
        }],
    };
    round_trip(
        &s,
        json!({
            "overview": {
                "databaseName": "seaquel_test",
                "totalSize": "8 MB",
                "totalSizeBytes": 8_388_608,
                "tableCount": 3,
                "indexCount": 5
            },
            "tableSizes": [{
                "schema": "public",
                "name": "users",
                "rowCount": 10,
                "totalSize": "16 kB",
                "totalSizeBytes": 16_384,
                "dataSize": "8 kB"
            }],
            "indexUsage": [{
                "schema": "public",
                "table": "users",
                "indexName": "users_pkey",
                "size": "16 kB",
                "scans": 0,
                "unused": true
            }]
        }),
    );
}

#[test]
fn statistics_optionals_when_present() {
    let o = DatabaseOverview {
        database_name: "db".into(),
        total_size: "1 GB".into(),
        total_size_bytes: None,
        table_count: 0,
        index_count: 0,
        connection_count: Some(7),
    };
    round_trip(
        &o,
        json!({ "databaseName": "db", "totalSize": "1 GB", "tableCount": 0, "indexCount": 0, "connectionCount": 7 }),
    );
    let t = TableSizeInfo {
        schema: "s".into(),
        name: "t".into(),
        row_count: 1,
        total_size: "1 B".into(),
        total_size_bytes: 1,
        data_size: None,
        index_size: Some("0 B".into()),
    };
    round_trip(
        &t,
        json!({ "schema": "s", "name": "t", "rowCount": 1, "totalSize": "1 B", "totalSizeBytes": 1, "indexSize": "0 B" }),
    );
    let i = IndexUsageInfo {
        schema: "s".into(),
        table: "t".into(),
        index_name: "i".into(),
        size: "1 B".into(),
        scans: 3,
        rows_read: Some(9),
        unused: false,
    };
    round_trip(
        &i,
        json!({ "schema": "s", "table": "t", "indexName": "i", "size": "1 B", "scans": 3, "rowsRead": 9, "unused": false }),
    );
}

#[test]
fn column_type_info_shape() {
    let t = ColumnTypeInfo {
        name: "NUMERIC".into(),
        category: ColumnCategory::Numeric,
        has_length: None,
        has_precision: Some(true),
    };
    round_trip(
        &t,
        json!({ "name": "NUMERIC", "category": "Numeric", "hasPrecision": true }),
    );
}

#[test]
fn column_category_names() {
    let all = [
        (ColumnCategory::String, "String"),
        (ColumnCategory::Numeric, "Numeric"),
        (ColumnCategory::DateTime, "Date/Time"),
        (ColumnCategory::Boolean, "Boolean"),
        (ColumnCategory::Json, "JSON"),
        (ColumnCategory::Binary, "Binary"),
        (ColumnCategory::Uuid, "UUID"),
        (ColumnCategory::Network, "Network"),
        (ColumnCategory::Other, "Other"),
    ];
    for (c, name) in all {
        assert_eq!(to_value(c).unwrap(), json!(name));
        assert_eq!(from_value::<ColumnCategory>(json!(name)).unwrap(), c);
    }
}

#[test]
fn create_table_definition_shape() {
    let d = CreateTableDefinition {
        table_name: "orders".into(),
        schema_name: "public".into(),
        columns: vec![
            CreateTableColumn {
                id: "c1".into(),
                name: "code".into(),
                ty: "VARCHAR".into(),
                length: Some("255".into()),
                precision: None,
                nullable: true,
                default_value: "".into(),
                is_primary_key: false,
                is_unique: true,
                collation: Some("Latin1_General_CS_AS".into()),
                in_unique_constraint: false,
            },
            CreateTableColumn {
                id: "c2".into(),
                name: "amount".into(),
                ty: "DECIMAL".into(),
                length: None,
                precision: Some("10,2".into()),
                nullable: false,
                default_value: "0".into(),
                is_primary_key: false,
                is_unique: false,
                collation: None,
                in_unique_constraint: false,
            },
        ],
        indexes: vec![CreateTableIndex {
            id: "i1".into(),
            name: "orders_code_idx".into(),
            columns: vec!["code".into()],
            unique: true,
            ty: "btree".into(),
        }],
        foreign_keys: vec![CreateTableForeignKey {
            id: "f1".into(),
            column: "user_id".into(),
            referenced_schema: "public".into(),
            referenced_table: "users".into(),
            referenced_column: "id".into(),
        }],
    };
    round_trip(
        &d,
        json!({
            "tableName": "orders",
            "schemaName": "public",
            "columns": [
                {
                    "id": "c1", "name": "code", "type": "VARCHAR", "length": "255",
                    "nullable": true, "defaultValue": "", "isPrimaryKey": false, "isUnique": true,
                    "collation": "Latin1_General_CS_AS"
                },
                {
                    "id": "c2", "name": "amount", "type": "DECIMAL", "precision": "10,2",
                    "nullable": false, "defaultValue": "0", "isPrimaryKey": false, "isUnique": false
                }
            ],
            "indexes": [{ "id": "i1", "name": "orders_code_idx", "columns": ["code"], "unique": true, "type": "btree" }],
            "foreignKeys": [{
                "id": "f1", "column": "user_id", "referencedSchema": "public",
                "referencedTable": "users", "referencedColumn": "id"
            }]
        }),
    );
}
