use seaquel_engine::{CastMap, Dialect, Driver, RowValues, SchemaColumn, SchemaIndex, Value};
use seaquel_engine_postgres::PostgresDialect;
use seaquel_engine_testkit::{
    config_from_env, run_introspection, run_smoke, scratch_name, IntrospectionSpec, SmokeSpec,
};
use seaquel_types::{ForeignKeyRef, TableKind};
use serde::Deserialize;

/// Set SEAQUEL_TEST_POSTGRES to a ConnectConfig JSON to run this, e.g.
/// {"driver":"postgres","connection_string":"postgres://postgres@127.0.0.1:5432/seaquel_test"}
#[tokio::test]
async fn smoke() {
    let Some(config) = config_from_env("SEAQUEL_TEST_POSTGRES") else {
        return;
    };
    run_smoke(&*seaquel_engine_postgres::engine(), &config, &SmokeSpec::DOLLAR).await;
}

/// Integers bind as INT8, not FLOAT8: 2^53 + 1 would round to 2^53 as an f64
/// and compare false.
#[tokio::test]
async fn integer_params_bind_exactly() {
    let Some(config) = config_from_env("SEAQUEL_TEST_POSTGRES") else {
        return;
    };
    let driver = seaquel_engine_postgres::engine().open(&config).await.expect("open");
    let r = driver
        .query(
            "SELECT $1::int8 = 9007199254740993 AS eq, $1::int8 AS n",
            vec![Value::Int(9_007_199_254_740_993)],
        )
        .await
        .expect("query");
    assert_eq!(r.rows[0][0], Value::Bool(true));
    assert_eq!(r.rows[0][1], Value::Int(9_007_199_254_740_993));
    assert_eq!(
        serde_json::to_value(&r.rows[0][1]).unwrap(),
        serde_json::json!({ "$sq": "bigint", "v": "9007199254740993" })
    );
    driver.close().await.expect("close");
}

/// Each literal decodes to the expected `Value`, and binding that value back
/// as `$1` compares equal to the literal: nothing is lost either way.
#[tokio::test]
async fn values_round_trip() {
    let Some(config) = config_from_env("SEAQUEL_TEST_POSTGRES") else {
        return;
    };
    let dec = |s: &str| Value::Decimal(s.into());
    let cases: Vec<(&str, Value)> = vec![
        ("9007199254740993::int8", Value::Int(9_007_199_254_740_993)),
        ("'-9223372036854775808'::int8", Value::Int(i64::MIN)),
        ("7::int2", Value::Int(7)),
        ("4294967295::oid", Value::Int(4_294_967_295)),
        ("'12.50'::numeric", dec("12.50")),
        ("'NaN'::numeric", dec("NaN")),
        ("'Infinity'::numeric", dec("Infinity")),
        ("'-Infinity'::numeric", dec("-Infinity")),
        (
            "'123456789012345678901234567890.123'::numeric",
            dec("123456789012345678901234567890.123"),
        ),
        ("'0.000'::numeric", dec("0.000")),
        ("'-0.0012'::numeric", dec("-0.0012")),
        ("'10000'::numeric", dec("10000")),
        ("'100000000.00000001'::numeric", dec("100000000.00000001")),
        ("1e-20::numeric", dec("0.00000000000000000001")),
        (r"'\x00ff10'::bytea", Value::Bytes(vec![0x00, 0xff, 0x10])),
        (r#"'{"a":1}'::jsonb"#, Value::Json(serde_json::json!({ "a": 1 }))),
        ("ARRAY[1,2]", Value::Array(vec![Value::Int(1), Value::Int(2)])),
        // Integer arrays bind as INT4[] when every element fits, else INT8[].
        ("ARRAY[1,NULL]::int4[]", Value::Array(vec![Value::Int(1), Value::Null])),
        (
            "ARRAY[9007199254740993,NULL]::int8[]",
            Value::Array(vec![Value::Int(9_007_199_254_740_993), Value::Null]),
        ),
        ("ARRAY['a',NULL]::text[]", Value::Array(vec![Value::from("a"), Value::Null])),
        (
            "ARRAY['12.50',NULL,'NaN']::numeric[]",
            Value::Array(vec![dec("12.50"), Value::Null, dec("NaN")]),
        ),
        ("'null'::jsonb", Value::Json(serde_json::Value::Null)),
        (
            r#"ARRAY['{"x":1}']::jsonb[]"#,
            Value::Array(vec![Value::Json(serde_json::json!({ "x": 1 }))]),
        ),
        (r"ARRAY['\x01']::bytea[]", Value::Array(vec![Value::Bytes(vec![1])])),
        ("ARRAY[1.5]::float8[]", Value::Array(vec![Value::Float(1.5)])),
        ("ARRAY[true,false]", Value::Array(vec![Value::Bool(true), Value::Bool(false)])),
        ("'1.5'::float8", Value::Float(1.5)),
        ("'inf'::float8", Value::Float(f64::INFINITY)),
    ];
    let driver = seaquel_engine_postgres::engine().open(&config).await.expect("open");
    for (literal, expected) in cases {
        let got = driver
            .query(&format!("SELECT {literal} AS v"), vec![])
            .await
            .unwrap_or_else(|e| panic!("SELECT {literal}: {e:?}"));
        let got = got.rows[0][0].clone();
        assert_eq!(got, expected, "decoding {literal}");
        let eq = driver
            .query(&format!("SELECT $1 = {literal} AS eq"), vec![got])
            .await
            .unwrap_or_else(|e| panic!("binding {literal}: {e:?}"));
        assert_eq!(eq.rows[0][0], Value::Bool(true), "binding {literal} back");
    }

    // `json` has no `=`: compare what it binds back as (JSONB).
    let json = driver.query("SELECT '[1,2]'::json AS v", vec![]).await.expect("json");
    let json = json.rows[0][0].clone();
    assert_eq!(json, Value::Json(serde_json::json!([1, 2])));
    let eq = driver.query("SELECT $1 = '[1,2]'::jsonb AS eq", vec![json]).await.expect("bind json");
    assert_eq!(eq.rows[0][0], Value::Bool(true));

    // NaN isn't equal to itself in Rust, so float NaN is checked by hand.
    let nan = driver.query("SELECT 'nan'::float8 AS v", vec![]).await.expect("nan");
    let nan = nan.rows[0][0].clone();
    assert!(matches!(nan, Value::Float(f) if f.is_nan()), "{nan:?}");
    assert_eq!(
        serde_json::to_value(&nan).unwrap(),
        serde_json::json!({ "$sq": "float", "v": "NaN" })
    );
    let eq = driver.query("SELECT $1 = 'nan'::float8 AS eq", vec![nan]).await.expect("bind nan");
    assert_eq!(eq.rows[0][0], Value::Bool(true));

    // Decimals from the client bind exactly, NaN included.
    for (text, literal) in [("NaN", "'NaN'::numeric"), ("1.10", "1.1"), ("-1e3", "-1000")] {
        let eq = driver
            .query(&format!("SELECT $1 = {literal} AS eq"), vec![dec(text)])
            .await
            .unwrap_or_else(|e| panic!("binding decimal {text}: {e:?}"));
        assert_eq!(eq.rows[0][0], Value::Bool(true), "decimal {text}");
    }

    // An INT4[] parameter still matches a BIGINT through `= ANY`.
    let any = driver
        .query(
            "SELECT 5::int8 = ANY($1) AS hit",
            vec![Value::Array(vec![Value::Int(4), Value::Int(5)])],
        )
        .await
        .expect("= ANY");
    assert_eq!(any.rows[0][0], Value::Bool(true));

    // Arrays whose element type has no `Value` of its own decode to the
    // same text as the scalar column. They bind back as TEXT[], so they're
    // checked on decoding only.
    let uuid = "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11";
    let text_arrays: Vec<(&str, Value)> = vec![
        (
            "ARRAY['2024-01-02 03:04:05.5'::timestamp, NULL]",
            Value::Array(vec![Value::from("2024-01-02 3:04:05.5"), Value::Null]),
        ),
        (
            "ARRAY['2024-01-02 03:04:05+00'::timestamptz]",
            Value::Array(vec![Value::from("2024-01-02 3:04:05.0 +00:00:00")]),
        ),
        ("ARRAY['03:04:05'::time]", Value::Array(vec![Value::from("3:04:05.0")])),
        ("ARRAY['1 day'::interval]", Value::Array(vec![Value::from("1 days")])),
        ("ARRAY['10.0.0.1/8'::inet]", Value::Array(vec![Value::from("10.0.0.1/8")])),
        ("ARRAY['2024-01-02'::date]", Value::Array(vec![Value::from("2024-01-02")])),
        ("'a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11'::uuid", Value::from(uuid)),
        (
            "ARRAY['a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11'::uuid]",
            Value::Array(vec![Value::from(uuid)]),
        ),
        // No decoder for the element type, multi-dimensional, not 1-based:
        // marked text, never binary garbage or a NULL-looking cell.
        ("ARRAY[point(1,2)]", Value::from("<unsupported: POINT[]>")),
        ("ARRAY[[1,2],[3,4]]", Value::from("<unsupported: INT4[]>")),
        ("'[0:1]={1,2}'::int[]", Value::from("<unsupported: INT4[]>")),
    ];
    for (literal, expected) in text_arrays {
        let got = driver
            .query(&format!("SELECT {literal} AS v"), vec![])
            .await
            .unwrap_or_else(|e| panic!("SELECT {literal}: {e:?}"));
        assert_eq!(got.rows[0][0], expected, "decoding {literal}");
    }

    // An empty array binds as TEXT[] and casts to the type it's used as;
    // a small-integer array (INT4[]) casts to INT8[].
    let r = driver
        .query(
            "SELECT cardinality($1::int[]) AS n, $1 = '{}'::text[] AS eq, $2::int8[] = ARRAY[1,2]::int8[] AS wide",
            vec![Value::Array(vec![]), Value::Array(vec![Value::Int(1), Value::Int(2)])],
        )
        .await
        .expect("empty array");
    assert_eq!(r.rows[0], vec![Value::Int(0), Value::Bool(true), Value::Bool(true)]);

    // Mixed arrays are rejected before reaching the server.
    let mixed = driver
        .query("SELECT $1 AS a", vec![Value::Array(vec![Value::Int(1), Value::from("x")])])
        .await
        .expect_err("mixed array");
    assert_eq!(mixed.code, "QUERY_ERROR");
    let nested = driver
        .query("SELECT $1 AS a", vec![Value::Array(vec![Value::Array(vec![Value::Int(1)])])])
        .await
        .expect_err("nested array");
    assert_eq!(nested.code, "QUERY_ERROR");
    assert!(nested.message.ends_with("nested arrays can't be bound"), "{}", nested.message);

    // information_schema domains decode like their base types.
    let r = driver
        .query(
            "SELECT 'YES'::information_schema.yes_or_no AS yn, \
                    'abc'::information_schema.sql_identifier AS si, \
                    'xyz'::information_schema.character_data AS cd, \
                    42::information_schema.cardinal_number AS cn",
            vec![],
        )
        .await
        .expect("domains");
    assert_eq!(
        r.rows[0],
        vec![Value::from("YES"), Value::from("abc"), Value::from("xyz"), Value::Int(42)]
    );

    driver.close().await.expect("close");
}

/// `bugfixes.json`: the scratch schema and the fix 1 / fix 4 expectations.
#[derive(Deserialize)]
struct Bugfixes {
    scratch: Scratch,
    cases: Vec<BugfixCase>,
}

#[derive(Deserialize)]
struct Scratch {
    schema: String,
    setup: Vec<String>,
    teardown: Vec<String>,
}

#[derive(Deserialize)]
struct BugfixCase {
    kind: String,
    input: serde_json::Value,
    output: serde_json::Value,
}

/// Introspection against the `bugfixes.json` scratch schema, renamed to a
/// unique schema so parallel runs can't collide. Covers bug fixes 1 (tables
/// named `order items` and `my-table` load), 4 (index columns from the
/// catalog) and 5 (statistics succeed with `order items` present).
#[tokio::test]
async fn introspection() {
    let Some(config) = config_from_env("SEAQUEL_TEST_POSTGRES") else {
        return;
    };
    let schema = scratch_name("seaquel_introspect_");
    let text = include_str!("fixtures/bugfixes.json");
    let recorded: Bugfixes = serde_json::from_str(text).expect("bugfixes.json");
    let fixtures: Bugfixes = serde_json::from_str(&text.replace(&recorded.scratch.schema, &schema))
        .expect("bugfixes.json");

    let mut columns: Vec<(String, Vec<SchemaColumn>)> = Vec::new();
    let mut indexes: Vec<(String, Vec<SchemaIndex>)> = Vec::new();
    for case in fixtures.cases {
        let table = case.input["table"].as_str().unwrap_or_default().to_string();
        match case.kind.as_str() {
            "columns" => columns.push((table, serde_json::from_value(case.output).expect("columns"))),
            "indexes" => indexes.push((table, serde_json::from_value(case.output).expect("indexes"))),
            _ => {}
        }
    }
    assert_eq!(
        (columns.len(), indexes.len()),
        (2, 3),
        "bugfixes.json columns/indexes cases"
    );

    // A composite primary key, and a foreign key into another schema.
    let col = |name: &str, ty: &str, nullable: bool, default: Option<&str>, pk: bool| SchemaColumn {
        name: name.into(),
        ty: ty.into(),
        cast_type: Some(ty.into()),
        nullable,
        default_value: default.map(Into::into),
        is_primary_key: pk,
        is_foreign_key: false,
        foreign_key_ref: None,
    };
    columns.push((
        "region_stock".into(),
        vec![
            col("region", "text", false, None, true),
            col("sku", "character varying", false, None, true),
            col("qty", "integer", false, Some("0"), false),
            SchemaColumn {
                is_foreign_key: true,
                foreign_key_ref: Some(ForeignKeyRef {
                    referenced_schema: "public".into(),
                    referenced_table: "users".into(),
                    referenced_column: "id".into(),
                }),
                ..col("user_id", "integer", true, None, false)
            },
            col("attrs", "jsonb", true, None, false),
        ],
    ));

    // An index on the materialized view: the catalog query isn't limited to
    // plain tables.
    let mut setup = fixtures.scratch.setup;
    setup.push(format!(
        "CREATE UNIQUE INDEX customer_counts_kind_key ON {schema}.customer_counts (kind)"
    ));
    indexes.push((
        "customer_counts".into(),
        vec![SchemaIndex {
            name: "customer_counts_kind_key".into(),
            columns: vec!["kind".into()],
            unique: true,
            ty: "btree".into(),
        }],
    ));

    // Plain key columns come back as attribute names, unquoted, like
    // `SchemaColumn::name`; only expression keys are printed SQL.
    setup.push(format!(
        "CREATE TABLE {schema}.quoted_cols (id integer PRIMARY KEY, \"Mixed Case\" text)"
    ));
    setup.push(format!(
        "CREATE INDEX quoted_cols_mixed_idx ON {schema}.quoted_cols (\"Mixed Case\", lower(\"Mixed Case\"))"
    ));
    indexes.push((
        "quoted_cols".into(),
        vec![
            SchemaIndex {
                name: "quoted_cols_mixed_idx".into(),
                columns: vec!["Mixed Case".into(), "lower(\"Mixed Case\")".into()],
                unique: false,
                ty: "btree".into(),
            },
            SchemaIndex {
                name: "quoted_cols_pkey".into(),
                columns: vec!["id".into()],
                unique: true,
                ty: "btree".into(),
            },
        ],
    ));

    // Scratch schemas of earlier runs killed before their teardown.
    let stale_cleanup = vec![format!(
        "DO $$ DECLARE s name; BEGIN \
           FOR s IN SELECT nspname FROM pg_namespace \
             WHERE nspname LIKE 'seaquel\\_introspect\\_%' AND nspname <> '{schema}' \
           LOOP EXECUTE format('DROP SCHEMA %I CASCADE', s); END LOOP; \
         END $$"
    )];

    let spec = IntrospectionSpec {
        schema: schema.clone(),
        setup,
        teardown: fixtures.scratch.teardown,
        stale_cleanup,
        tables: vec![
            ("customer_counts".into(), TableKind::MaterializedView),
            ("customer_names".into(), TableKind::View),
            ("customers".into(), TableKind::Table),
            ("events".into(), TableKind::Table),
            ("my-table".into(), TableKind::Table),
            ("order items".into(), TableKind::Table),
            ("quoted_cols".into(), TableKind::Table),
            ("region_stock".into(), TableKind::Table),
        ],
        columns,
        indexes,
        stats_table: "order items".into(),
        usage_index: "order items_lower_name_idx".into(),
        explain_sql: format!("SELECT name FROM {schema}.\"order items\" WHERE customer_id = $1;"),
        explain_params: vec![Value::Int(1)],
        explain_relation: "order items".into(),
    };
    run_introspection(&*seaquel_engine_postgres::engine(), &config, &spec).await;
}

/// Bug fix 6: rows keyed by uuid, date, timestamp and time can be edited and
/// deleted, and typed columns can be set to NULL. The UI sends these values
/// as text (as they decode) with a cast map like `castMapForColumns` builds,
/// so key placeholders need the cast (`uuid = text` doesn't exist), and a
/// NULL must bind as a type that casts to any column type (JSONB doesn't).
#[tokio::test]
async fn crud_on_typed_keys_and_columns() {
    let Some(config) = config_from_env("SEAQUEL_TEST_POSTGRES") else {
        return;
    };
    let driver = seaquel_engine_postgres::engine().open(&config).await.expect("open");
    let schema = scratch_name("seaquel_crud_");
    let d = PostgresDialect;

    // Scratch schemas of earlier runs killed before their teardown.
    driver
        .execute(
            &format!(
                "DO $$ DECLARE s name; BEGIN \
                   FOR s IN SELECT nspname FROM pg_namespace \
                     WHERE nspname LIKE 'seaquel\\_crud\\_%' AND nspname <> '{schema}' \
                   LOOP EXECUTE format('DROP SCHEMA %I CASCADE', s); END LOOP; \
                 END $$"
            ),
            vec![],
        )
        .await
        .expect("stale cleanup");
    for sql in [
        format!("CREATE SCHEMA {schema}"),
        format!(
            "CREATE TABLE {schema}.typed (id uuid PRIMARY KEY, day date, at timestamp, \
             addr inet, note text DEFAULT 'dflt')"
        ),
        format!(
            "INSERT INTO {schema}.typed VALUES ('a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11', \
             '2024-01-02', '2024-01-02 03:04:05.5', '10.0.0.1', 'n')"
        ),
        format!(
            "CREATE TABLE {schema}.keyed (day date, at timestamp, t time, n int, \
             PRIMARY KEY (day, at, t))"
        ),
        format!("INSERT INTO {schema}.keyed VALUES ('2024-01-02', '2024-01-02 03:04:05.5', '03:04:05', 1)"),
        // 24:00 and 00:00 are different TIME values.
        format!("CREATE TABLE {schema}.times (tk time PRIMARY KEY, n int)"),
        format!("INSERT INTO {schema}.times VALUES ('00:00', 1), ('24:00', 1)"),
    ]
    .into_iter()
    .chain(KEY_CASES.iter().flat_map(|(name, ty, literal, _)| {
        [
            format!("CREATE TABLE {schema}.k_{name} (k_{name} {ty} PRIMARY KEY, n int)"),
            format!("INSERT INTO {schema}.k_{name} VALUES ({literal}, 1)"),
        ]
    })) {
        driver.execute(&sql, vec![]).await.unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    }

    let result = run_typed_crud(&*driver, &d, &schema).await;
    driver
        .execute(&format!("DROP SCHEMA {schema} CASCADE"), vec![])
        .await
        .expect("teardown");
    driver.close().await.expect("close");
    result.unwrap_or_else(|e| panic!("{e}"));
}

/// Single-column keys for [`crud_on_typed_keys_and_columns`]: name, column
/// type, literal, and the cast `castMapForColumns` builds for it (from
/// `castType` since fix 7). Each row is
/// read back as the UI holds it, then updated and deleted by that key.
const KEY_CASES: &[(&str, &str, &str, &str)] = &[
    ("tz", "timestamptz", "'2024-07-02 03:04:05.5+05:30'", "timestamp with time zone"),
    ("iv", "interval", "'1 year 2 mons 3 days 04:05:06.789'", "interval"),
    ("ivneg", "interval", "'-1 day -02:00:00'", "interval"),
    ("tt", "timetz", "'12:34:56.5-08'", "time with time zone"),
    ("tt24", "timetz", "'24:00:00+05:30'", "time with time zone"),
    // `castType` (fix 7) is `"bit"`; CAST(… AS bit) would be bit(1).
    ("bits", "bit(4)", "B'1010'", "\"bit\""),
    // `character` decodes blank-padded; CAST(… AS character) would be char(1).
    ("chr", "char(3)", "'ab'", "bpchar"),
    ("oid", "oid", "12345", "oid"),
    ("dinf", "date", "'infinity'", "date"),
    ("dninf", "date", "'-infinity'", "date"),
    ("tsinf", "timestamp", "'infinity'", "timestamp without time zone"),
    ("tzninf", "timestamptz", "'-infinity'", "timestamp with time zone"),
    ("dbc", "date", "'4713-01-01 BC'", "date"),
    ("dbig", "date", "'10000-01-01'", "date"),
    ("tsbc", "timestamp", "'0044-03-15 12:00:00.5 BC'", "timestamp without time zone"),
    ("tsbig", "timestamp", "'10000-01-01 01:02:03'", "timestamp without time zone"),
    ("tzbc", "timestamptz", "'0044-03-15 12:00:00+00 BC'", "timestamp with time zone"),
];

/// The body of [`crud_on_typed_keys_and_columns`]; errors instead of
/// panicking so the scratch schema is always dropped.
async fn run_typed_crud(driver: &dyn Driver, d: &PostgresDialect, schema: &str) -> Result<(), String> {
    // What `castMapForColumns` builds: text columns get no cast.
    let casts: CastMap = [
        ("id", "uuid"),
        ("day", "date"),
        ("at", "timestamp without time zone"),
        ("t", "time without time zone"),
        ("addr", "inet"),
        ("tk", "time without time zone"),
        ("n", "integer"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .chain(KEY_CASES.iter().map(|(name, _, _, cast)| (format!("k_{name}"), cast.to_string())))
    .collect();

    // The row as the UI holds it: the decoded cells, dates and uuids as text.
    let read_row = |table: String| async move {
        let r = driver
            .query(&format!("SELECT * FROM {schema}.{table} ORDER BY 1 DESC"), vec![])
            .await
            .map_err(|e| format!("reading {table}: {e:?}"))?;
        let row = r.rows.first().ok_or(format!("{table} has no rows"))?;
        Ok::<RowValues, String>(r.columns.iter().cloned().zip(row.iter().cloned()).collect())
    };
    let run = |sql: seaquel_engine::SqlWithBindings, what: &'static str| async move {
        let binds = sql.bind_values.unwrap_or_default();
        let r = driver
            .execute(&sql.sql, binds)
            .await
            .map_err(|e| format!("{what}: {}: {e:?}", sql.sql))?;
        if r.rows_affected == 1 {
            Ok(())
        } else {
            Err(format!("{what}: {} affected {} rows", sql.sql, r.rows_affected))
        }
    };
    let pk = vec!["id".to_string()];

    // Edit typed columns of a uuid-keyed row, values sent as text.
    let row = read_row("typed".into()).await?;
    for (column, value) in [("day", "2025-05-06"), ("at", "2025-05-06 07:08:09"), ("addr", "10.1.2.3")] {
        run(
            d.build_update(schema, "typed", column, Value::from(value), &pk, &row, Some(&casts)),
            "update by uuid key",
        )
        .await?;
    }
    let r = driver
        .query(&format!("SELECT day::text, at::text, host(addr) FROM {schema}.typed"), vec![])
        .await
        .map_err(|e| format!("{e:?}"))?;
    let expected = vec![
        Value::from("2025-05-06"),
        Value::from("2025-05-06 07:08:09"),
        Value::from("10.1.2.3"),
    ];
    if r.rows[0] != expected {
        return Err(format!("after update: {:?}", r.rows[0]));
    }

    // Set NULL on each typed column.
    for column in ["day", "at", "addr"] {
        run(
            d.build_update(schema, "typed", column, Value::Null, &pk, &row, Some(&casts)),
            "set NULL",
        )
        .await?;
    }
    let r = driver
        .query(
            &format!("SELECT day IS NULL AND at IS NULL AND addr IS NULL FROM {schema}.typed"),
            vec![],
        )
        .await
        .map_err(|e| format!("{e:?}"))?;
    if r.rows[0][0] != Value::Bool(true) {
        return Err("typed columns aren't NULL".into());
    }

    run(
        d.build_set_default(schema, "typed", "note", &pk, &row, Some(&casts)),
        "set default by uuid key",
    )
    .await?;

    // A composite date/timestamp/time key, from the decoded row.
    let keyed = read_row("keyed".into()).await?;
    let keys: Vec<String> = ["day", "at", "t"].iter().map(|k| k.to_string()).collect();
    run(
        d.build_update(schema, "keyed", "n", Value::Int(2), &keys, &keyed, Some(&casts)),
        "update by date/timestamp/time key",
    )
    .await?;
    run(d.build_delete(schema, "keyed", &keys, &keyed, Some(&casts)), "delete by date/timestamp/time key")
        .await?;

    run(d.build_delete(schema, "typed", &pk, &row, Some(&casts)), "delete by uuid key").await?;

    // 24:00 must not match 00:00. ORDER BY 1 DESC reads the 24:00 row.
    let times = read_row("times".into()).await?;
    run(
        d.build_update(schema, "times", "n", Value::Int(2), &["tk".to_string()], &times, Some(&casts)),
        "update by a 24:00 time key",
    )
    .await?;
    let r = driver
        .query(&format!("SELECT tk::text, n FROM {schema}.times ORDER BY tk"), vec![])
        .await
        .map_err(|e| format!("{e:?}"))?;
    let expected = vec![
        vec![Value::from("00:00:00"), Value::Int(1)],
        vec![Value::from("24:00:00"), Value::Int(2)],
    ];
    if r.rows != expected {
        return Err(format!("24:00 update changed {:?}", r.rows));
    }

    for (name, ty, literal, _) in KEY_CASES {
        let table = format!("k_{name}");
        let keys = vec![table.clone()];
        let row = read_row(table.clone()).await?;
        let what = format!("{ty} key {literal} (decoded {:?})", row[0].1);
        let update = d.build_update(schema, &table, "n", Value::Int(2), &keys, &row, Some(&casts));
        let r = driver
            .execute(&update.sql, update.bind_values.unwrap_or_default())
            .await
            .map_err(|e| format!("update by {what}: {e:?}"))?;
        if r.rows_affected != 1 {
            return Err(format!("update by {what} affected {} rows", r.rows_affected));
        }
        let delete = d.build_delete(schema, &table, &keys, &row, Some(&casts));
        let r = driver
            .execute(&delete.sql, delete.bind_values.unwrap_or_default())
            .await
            .map_err(|e| format!("delete by {what}: {e:?}"))?;
        if r.rows_affected != 1 {
            return Err(format!("delete by {what} affected {} rows", r.rows_affected));
        }
    }
    let r = driver
        .query(
            &format!("SELECT (SELECT count(*) FROM {schema}.typed) + (SELECT count(*) FROM {schema}.keyed)"),
            vec![],
        )
        .await
        .map_err(|e| format!("{e:?}"))?;
    if r.rows[0][0] != Value::Int(0) {
        return Err(format!("rows left after delete: {:?}", r.rows[0][0]));
    }
    Ok(())
}

/// Values the `time` crate can't hold decode to text Postgres parses back:
/// ±infinity, 24:00, years before 1 AD (`… BC`) and after 9999. sqlx's own
/// decoders panic on ±infinity and far dates, and wrap 24:00 to 00:00.
#[tokio::test]
async fn temporal_edge_values() {
    let Some(config) = config_from_env("SEAQUEL_TEST_POSTGRES") else {
        return;
    };
    let driver = seaquel_engine_postgres::engine().open(&config).await.expect("open");
    let cases: &[(&str, &str, &str)] = &[
        ("'infinity'::date", "infinity", "date"),
        ("'-infinity'::date", "-infinity", "date"),
        ("'infinity'::timestamp", "infinity", "timestamp"),
        ("'-infinity'::timestamp", "-infinity", "timestamp"),
        ("'infinity'::timestamptz", "infinity", "timestamptz"),
        ("'-infinity'::timestamptz", "-infinity", "timestamptz"),
        ("'24:00:00'::time", "24:00:00", "time"),
        ("'24:00:00+05:30'::timetz", "24:00:00+05:30:00", "timetz"),
        ("'4713-01-01 BC'::date", "4713-01-01 BC", "date"),
        ("'0001-01-01 BC'::date", "0001-01-01 BC", "date"),
        ("'10000-01-01'::date", "10000-01-01", "date"),
        ("'5874897-12-31'::date", "5874897-12-31", "date"),
        ("'0044-03-15 12:00:00.5 BC'::timestamp", "0044-03-15 12:00:00.5 BC", "timestamp"),
        ("'4713-01-01 00:00 BC'::timestamp", "4713-01-01 00:00:00 BC", "timestamp"),
        ("'294276-12-31 23:59:59.999999'::timestamp", "294276-12-31 23:59:59.999999", "timestamp"),
        ("'0044-03-15 12:00:00+00 BC'::timestamptz", "0044-03-15 12:00:00+00 BC", "timestamptz"),
        ("'10000-01-01 01:02:03+00'::timestamptz", "10000-01-01 01:02:03+00", "timestamptz"),
    ];
    for (literal, expected, ty) in cases {
        let got = driver
            .query(&format!("SELECT {literal} AS v, ARRAY[{literal}, NULL] AS a"), vec![])
            .await
            .unwrap_or_else(|e| panic!("SELECT {literal}: {e:?}"));
        assert_eq!(got.rows[0][0], Value::from(*expected), "decoding {literal}");
        assert_eq!(
            got.rows[0][1],
            Value::Array(vec![Value::from(*expected), Value::Null]),
            "decoding ARRAY[{literal}]"
        );
        let eq = driver
            .query(&format!("SELECT CAST($1 AS {ty}) = {literal} AS eq"), vec![Value::from(*expected)])
            .await
            .unwrap_or_else(|e| panic!("casting {expected} back: {e:?}"));
        assert_eq!(eq.rows[0][0], Value::Bool(true), "{expected} casts back to {literal}");
    }
    driver.close().await.expect("close");
}

/// Bug fix 7: "Set NULL" and edits on enum, array, json and domain columns,
/// with the cast map the frontend builds from `castType` (verbatim). The enum
/// types live in the scratch schema, which isn't on the search_path, so the
/// cast must be schema-qualified (and quoted where needed). Character and bit
/// columns cast without their length, so a too-long value errors instead of
/// being truncated; a `bit(16)` key compares as `"bit"` and uses its index.
#[tokio::test]
async fn crud_with_catalog_cast_types() {
    let Some(config) = config_from_env("SEAQUEL_TEST_POSTGRES") else {
        return;
    };
    let driver = seaquel_engine_postgres::engine().open(&config).await.expect("open");
    let schema = scratch_name("seaquel_casts_");

    // Scratch schemas of earlier runs killed before their teardown.
    driver
        .execute(
            &format!(
                "DO $$ DECLARE s name; BEGIN \
                   FOR s IN SELECT nspname FROM pg_namespace \
                     WHERE nspname LIKE 'seaquel\\_casts\\_%' AND nspname <> '{schema}' \
                   LOOP EXECUTE format('DROP SCHEMA %I CASCADE', s); END LOOP; \
                   FOR s IN SELECT typname FROM pg_type \
                     WHERE typnamespace = 'public'::regnamespace \
                       AND typname LIKE 'seaquel\\_casts\\_%\\_pe' AND typname <> '{schema}_pe' \
                   LOOP EXECUTE format('DROP TYPE public.%I CASCADE', s); END LOOP; \
                 END $$"
            ),
            vec![],
        )
        .await
        .expect("stale cleanup");
    for sql in [
        format!("CREATE SCHEMA {schema}"),
        format!("CREATE TYPE {schema}.mood AS ENUM ('sad', 'happy')"),
        format!("CREATE TYPE {schema}.\"Weird Mood\" AS ENUM ('a', 'b')"),
        format!("CREATE DOMAIN {schema}.posint AS integer CHECK (VALUE > 0)"),
        format!("CREATE DOMAIN {schema}.short AS varchar(3)"),
        // Two domain levels over varchar(3), and an array of the domain.
        format!("CREATE DOMAIN {schema}.shorter AS {schema}.short"),
        // A user type in public (like an extension's), which is on the search_path.
        format!("CREATE TYPE public.{schema}_pe AS ENUM ('x', 'y')"),
        format!(
            "CREATE TABLE {schema}.typed (id int PRIMARY KEY, m {schema}.mood, \
             w {schema}.\"Weird Mood\", ia int[], ta text[], ma {schema}.mood[], j jsonb, \
             js json, p {schema}.posint, s {schema}.short, v varchar(5), c char(3), \
             n numeric(10,2), s2 {schema}.shorter, sa {schema}.short[], pe public.{schema}_pe)"
        ),
        format!(
            "INSERT INTO {schema}.typed VALUES (1, 'sad', 'a', '{{1}}', '{{x}}', '{{sad}}', \
             '{{}}', '[]', 1, 'abc', 'hello', 'abc', 1.5, 'abc', '{{abc}}', 'x')"
        ),
        format!("CREATE TABLE {schema}.bits (k bit(16) PRIMARY KEY, n int)"),
        format!("INSERT INTO {schema}.bits SELECT g::bit(16), 1 FROM generate_series(1, 5000) g"),
        format!("ANALYZE {schema}.bits"),
        format!("CREATE TABLE {schema}.chars (k char(3) PRIMARY KEY, n int)"),
        format!("INSERT INTO {schema}.chars VALUES ('ab', 1)"),
    ] {
        driver.execute(&sql, vec![]).await.unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    }

    let result = run_catalog_cast_crud(&*driver, &config, &schema).await;
    for sql in [format!("DROP SCHEMA {schema} CASCADE"), format!("DROP TYPE public.{schema}_pe")] {
        driver.execute(&sql, vec![]).await.expect("teardown");
    }
    driver.close().await.expect("close");
    result.unwrap_or_else(|e| panic!("{e}"));
}

/// The body of [`crud_with_catalog_cast_types`]; errors instead of panicking
/// so the scratch schema is always dropped.
async fn run_catalog_cast_crud(
    driver: &dyn Driver,
    config: &seaquel_types::ConnectConfig,
    schema: &str,
) -> Result<(), String> {
    let d = PostgresDialect;
    // `castMapForColumns` with `castType`: every column, cast verbatim.
    let cast_map = |table: &'static str| async move {
        let (columns, _) = driver
            .table_metadata(schema, table)
            .await
            .map_err(|e| format!("table_metadata {table}: {e:?}"))?;
        Ok::<CastMap, String>(
            columns
                .into_iter()
                .filter_map(|c| c.cast_type.map(|t| (c.name, t)))
                .collect(),
        )
    };
    let read_row = |table: &'static str| async move {
        let r = driver
            .query(&format!("SELECT * FROM {schema}.{table} ORDER BY 1 LIMIT 1"), vec![])
            .await
            .map_err(|e| format!("reading {table}: {e:?}"))?;
        let row = r.rows.first().ok_or(format!("{table} has no rows"))?;
        Ok::<RowValues, String>(r.columns.iter().cloned().zip(row.iter().cloned()).collect())
    };
    let run = |sql: seaquel_engine::SqlWithBindings, what: String| async move {
        let r = driver
            .execute(&sql.sql, sql.bind_values.unwrap_or_default())
            .await
            .map_err(|e| format!("{what}: {}: {e:?}", sql.sql))?;
        if r.rows_affected == 1 {
            Ok(())
        } else {
            Err(format!("{what}: {} affected {} rows", sql.sql, r.rows_affected))
        }
    };

    let casts = cast_map("typed").await?;
    let expected: CastMap = [
        ("id", "integer".to_string()),
        ("m", format!("{schema}.mood")),
        ("w", format!("{schema}.\"Weird Mood\"")),
        ("pe", format!("public.{schema}_pe")),
        // Through the whole domain chain, and through arrays of domains.
        ("s2", "character varying".into()),
        ("sa", "character varying[]".into()),
        ("ia", "integer[]".into()),
        ("ta", "text[]".into()),
        ("ma", format!("{schema}.mood[]")),
        ("j", "jsonb".into()),
        ("js", "json".into()),
        ("p", format!("{schema}.posint")),
        // A domain over a length-limited type casts to its unbounded base.
        ("s", "character varying".into()),
        ("v", "character varying".into()),
        ("c", "bpchar".into()),
        ("n", "numeric(10,2)".into()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    if casts != expected {
        return Err(format!("castType of typed: {casts:?}"));
    }

    let pk = vec!["id".to_string()];
    let row = read_row("typed").await?;
    let columns = ["m", "w", "ia", "ta", "ma", "j", "js", "p", "s", "v", "c", "n", "s2", "sa", "pe"];
    for column in columns {
        let sql = d.build_update(schema, "typed", column, Value::Null, &pk, &row, Some(&casts));
        run(sql, format!("set {column} NULL")).await?;
    }
    let r = driver
        .query(
            &format!(
                "SELECT num_nulls({}) FROM {schema}.typed",
                columns.join(", ")
            ),
            vec![],
        )
        .await
        .map_err(|e| format!("{e:?}"))?;
    if r.rows[0][0] != Value::Int(columns.len() as i64) {
        return Err(format!("NULL columns after set NULL: {:?}", r.rows[0][0]));
    }

    // Values as the UI sends them: text, JSON objects and exact decimals.
    let edits = [
        ("m", Value::from("happy"), "happy"),
        ("w", Value::from("b"), "b"),
        ("ia", Value::from("{1,2}"), "{1,2}"),
        ("ta", Value::from("{a,\"b c\"}"), "{a,\"b c\"}"),
        ("ma", Value::from("{sad,happy}"), "{sad,happy}"),
        ("j", Value::Json(serde_json::json!({ "a": 1 })), "{\"a\": 1}"),
        ("js", Value::from("{\"b\": 2}"), "{\"b\": 2}"),
        ("p", Value::from("5"), "5"),
        ("s", Value::from("xyz"), "xyz"),
        ("v", Value::from("world"), "world"),
        ("c", Value::from("xy"), "xy"),
        ("n", Value::Decimal("12.345".into()), "12.35"),
        ("s2", Value::from("xyz"), "xyz"),
        ("sa", Value::from("{ab,xyz}"), "{ab,xyz}"),
        ("pe", Value::from("y"), "y"),
    ];
    for (column, value, _) in &edits {
        let sql = d.build_update(schema, "typed", column, value.clone(), &pk, &row, Some(&casts));
        run(sql, format!("set {column}")).await?;
    }
    let r = driver
        .query(
            &format!(
                "SELECT {} FROM {schema}.typed",
                columns.map(|c| format!("{c}::text")).join(", ")
            ),
            vec![],
        )
        .await
        .map_err(|e| format!("{e:?}"))?;
    let got: Vec<Value> = r.rows[0].clone();
    let want: Vec<Value> = edits.iter().map(|(_, _, text)| Value::from(*text)).collect();
    if got != want {
        return Err(format!("after edits: {got:?}"));
    }

    // Too long for the column: an error, not a silently truncated value.
    for (column, value) in [("v", "toolong"), ("s", "abcd"), ("c", "abcd"), ("s2", "abcd"), ("sa", "{abcd}")] {
        let sql = d.build_update(schema, "typed", column, Value::from(value), &pk, &row, Some(&casts));
        if driver.execute(&sql.sql, sql.bind_values.unwrap_or_default()).await.is_ok() {
            return Err(format!("{column} = {value:?} was accepted: {}", sql.sql));
        }
    }
    // A domain's CHECK still applies.
    let sql = d.build_update(schema, "typed", "p", Value::from("0"), &pk, &row, Some(&casts));
    if driver.execute(&sql.sql, sql.bind_values.unwrap_or_default()).await.is_ok() {
        return Err(format!("posint = 0 was accepted: {}", sql.sql));
    }

    // castType doesn't depend on the loading connection's search_path: with
    // the scratch schema on it, user types still come back schema-qualified.
    let mut on_path = config.clone();
    let url = on_path.connection_string.clone().unwrap_or_default();
    let sep = if url.contains('?') { '&' } else { '?' };
    on_path.connection_string = Some(format!("{url}{sep}options=-c%20search_path%3D{schema}%2Cpublic"));
    let other = seaquel_engine_postgres::engine()
        .open(&on_path)
        .await
        .map_err(|e| format!("open with search_path: {e:?}"))?;
    let checked = async {
        let r = other
            .query("SHOW search_path", vec![])
            .await
            .map_err(|e| format!("{e:?}"))?;
        if r.rows[0][0] != Value::from(format!("{schema},public")) {
            return Err(format!("search_path not set: {:?}", r.rows[0][0]));
        }
        let (columns, _) = other
            .table_metadata(schema, "typed")
            .await
            .map_err(|e| format!("table_metadata on the search_path: {e:?}"))?;
        let on_path: CastMap = columns
            .into_iter()
            .filter_map(|c| c.cast_type.map(|t| (c.name, t)))
            .collect();
        if on_path != expected {
            return Err(format!("castType with {schema} on the search_path: {on_path:?}"));
        }
        Ok(())
    }
    .await;
    other.close().await.map_err(|e| format!("{e:?}"))?;
    checked?;

    // A bit(16) key compares as "bit": one row, through the primary-key index.
    let casts = cast_map("bits").await?;
    if casts.get("k").map(String::as_str) != Some("\"bit\"") {
        return Err(format!("castType of bits: {casts:?}"));
    }
    let keys = vec!["k".to_string()];
    let row = read_row("bits").await?;
    let update = d.build_update(schema, "bits", "n", Value::Int(2), &keys, &row, Some(&casts));
    let plan = driver
        .query(&format!("EXPLAIN {}", update.sql), update.bind_values.clone().unwrap_or_default())
        .await
        .map_err(|e| format!("EXPLAIN {}: {e:?}", update.sql))?;
    let plan = format!("{:?}", plan.rows);
    if !plan.contains("Index Scan") && !plan.contains("Index Only Scan") {
        return Err(format!("bit(16) key doesn't use the index: {plan}"));
    }
    run(update, "update by a bit(16) key".into()).await?;
    // A key edit to the wrong length errors (bit(16) would pad it silently).
    let sql = d.build_update(schema, "bits", "k", Value::from("101"), &keys, &row, Some(&casts));
    if driver.execute(&sql.sql, sql.bind_values.unwrap_or_default()).await.is_ok() {
        return Err(format!("a 3-bit key was accepted: {}", sql.sql));
    }

    // A char(3) key decodes blank-padded and still matches.
    let casts = cast_map("chars").await?;
    let row = read_row("chars").await?;
    run(
        d.build_update(schema, "chars", "n", Value::Int(2), &keys, &row, Some(&casts)),
        format!("update by a char(3) key {:?}", row[0].1),
    )
    .await?;
    run(d.build_delete(schema, "chars", &keys, &row, Some(&casts)), "delete by a char(3) key".into()).await?;
    Ok(())
}
