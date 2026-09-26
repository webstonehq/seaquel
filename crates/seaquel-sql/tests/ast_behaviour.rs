//! The AST helpers (Task 6) beyond the fixtures: every prefix of the AST
//! corpus (the editor calls these while someone types), `!=` kept as written
//! next to multi-byte text and comments, and the node-sql-parser quirks the
//! query builder relies on (decision 2).

use std::collections::HashMap;

use seaquel_sql::ast::{
    column_refs, parse_builder_query, parse_error, parse_visual, AggregateFunction, Connector,
    FilterOperator, ParsedQuery, TutorialSchema,
};
use seaquel_sql::SqlEngine;
use serde_json::{json, Value};

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(&path).expect("fixture")).expect("json")
}

fn tutorial_schema() -> TutorialSchema {
    serde_json::from_value(fixture("tutorial.json")["tutorialSchema"].clone()).expect("schema")
}

/// The AST corpus: every visual.json input and the hand-written AST cases in
/// bugfixes.json, with their engines.
fn corpus() -> Vec<(String, SqlEngine)> {
    let mut out = Vec::new();
    let visual = fixture("visual.json");
    let fixes = fixture("bugfixes.json");
    let inputs = visual["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .chain(fixes["cases"].as_array().expect("cases"))
        .map(|c| &c["input"]);
    for input in inputs {
        let sql = input["sql"].as_str().expect("sql").to_string();
        let engine = input["engine"]
            .as_str()
            .unwrap_or("postgres")
            .parse()
            .expect("engine");
        out.push((sql, engine));
    }
    out
}

#[test]
fn every_prefix_of_the_ast_corpus_is_total() {
    let schema = tutorial_schema();
    let mut prefixes = 0;
    for (sql, engine) in corpus() {
        let ends = sql
            .char_indices()
            .map(|(i, _)| i)
            .chain(std::iter::once(sql.len()));
        for end in ends {
            let p = &sql[..end];
            // The entry's engine, and PostgreSQL (the tutorial's).
            for e in [engine, SqlEngine::Postgres] {
                let visual = parse_visual(p, e);
                assert_eq!(visual.is_err(), parse_error(p, e).is_some(), "{e} {p:?}");
                if let Ok(Some(v)) = &visual {
                    serde_json::to_string(v).expect("serializes");
                }
                if let Some(refs) = column_refs(p, e) {
                    serde_json::to_string(&refs).expect("serializes");
                }
                if let Some(q) = parse_builder_query(p, e, &schema, None) {
                    serde_json::to_string(&q).expect("serializes");
                }
            }
            prefixes += 1;
        }
    }
    assert!(prefixes > 10_000, "{prefixes}");
}

#[test]
fn empty_input() {
    for sql in ["", "   ", "\n\t", "-- only a comment", "/* c */"] {
        for e in SqlEngine::ALL {
            assert_eq!(parse_visual(sql, e), Ok(None), "{e} {sql:?}");
            assert_eq!(parse_error(sql, e), None, "{e} {sql:?}");
            assert_eq!(column_refs(sql, e), None, "{e} {sql:?}");
        }
    }
    // The TS returns an empty state (with `ctes: []`) for blank input, and
    // `trim()` counts U+FEFF as blank.
    let blank = ParsedQuery {
        ctes: Some(vec![]),
        ..Default::default()
    };
    for sql in ["", "  \n", "\u{FEFF}", "\u{00A0}\u{2003}"] {
        assert_eq!(
            parse_builder_query(sql, SqlEngine::Postgres, &HashMap::new(), None),
            Some(blank.clone()),
            "{sql:?}"
        );
    }
    // A comment isn't blank to `trim()`, and has no SELECT: `null`.
    assert_eq!(
        parse_builder_query("-- x", SqlEngine::Postgres, &HashMap::new(), None),
        None
    );
}

fn where_expressions(sql: &str, e: SqlEngine) -> Vec<String> {
    fn walk(f: &Value, out: &mut Vec<String>) {
        out.push(f["expression"].as_str().expect("expression").to_string());
        for c in f["children"].as_array().into_iter().flatten() {
            walk(c, out);
        }
    }
    let v = serde_json::to_value(parse_visual(sql, e).expect("parses")).expect("json");
    let mut out = Vec::new();
    for f in v["filters"].as_array().expect("filters") {
        walk(f, &mut out);
    }
    out
}

#[test]
fn not_equal_is_kept_as_written() {
    // sqlparser reads `!=` and `<>` as one operator; the source text decides.
    // Positions are chars in sqlparser's spans, so multi-byte text before the
    // operator must not shift them.
    let sql = "SELECT '東京😀' AS c FROM t WHERE a != 1 AND b <> '😀' AND c/*<>*/!=/*<>*/2";
    assert_eq!(
        where_expressions(sql, SqlEngine::Postgres),
        [
            "a != 1 AND b <> '😀' AND c != 2",
            "a != 1 AND b <> '😀'",
            "a != 1",
            "b <> '😀'",
            "c != 2",
        ]
    );
    let crlf = "SELECT *\r\nFROM t\r\nWHERE '東京' <> x\r\n  AND y\r\n!=\r\n  3";
    assert_eq!(
        where_expressions(crlf, SqlEngine::Mssql),
        ["'東京' <> x AND y != 3", "'東京' <> x", "y != 3"]
    );
    // In a JOIN condition and a HAVING too.
    let v = parse_visual(
        "SELECT a FROM t JOIN u ON t.id != u.id GROUP BY a HAVING COUNT(*) <> 2",
        SqlEngine::Mysql,
    )
    .expect("parses")
    .expect("a statement");
    assert_eq!(v.joins[0].condition, "t.id != u.id");
    assert_eq!(v.having.expect("having").expression, "COUNT(*) <> 2");
}

#[test]
fn visual_prints_parameters_and_limits() {
    let v = |sql: &str, e| {
        serde_json::to_value(parse_visual(sql, e).expect("parses").expect("statement"))
            .expect("json")
    };
    // Fix 6: every placeholder form prints as written.
    assert_eq!(
        where_expressions("SELECT * FROM t WHERE a = ? AND b = ?", SqlEngine::Mysql)[0],
        "a = ? AND b = ?"
    );
    assert_eq!(
        where_expressions("SELECT * FROM t WHERE a = @p1", SqlEngine::Mssql)[0],
        "a = @p1"
    );
    // Fix 2: a LIMIT that isn't a number gives no node, not `count: NaN`.
    assert_eq!(
        v("SELECT * FROM t LIMIT $1", SqlEngine::Postgres)["limit"],
        Value::Null
    );
    assert_eq!(
        v("SELECT * FROM t LIMIT ALL", SqlEngine::Postgres)["limit"],
        Value::Null
    );
    // An OFFSET alone gives no LIMIT node either.
    assert_eq!(
        v("SELECT * FROM t OFFSET 5", SqlEngine::Postgres)["limit"],
        Value::Null
    );
    // Fix 7: TOP n PERCENT isn't a row count.
    assert_eq!(
        v("SELECT TOP 10 PERCENT a FROM t", SqlEngine::Mssql)["limit"],
        Value::Null
    );
    assert_eq!(
        v("SELECT TOP (7) a FROM t", SqlEngine::Mssql)["limit"],
        json!({ "count": 7 })
    );
    // Fix 5 on MariaDB too, and LIMIT … OFFSET keeps its order.
    assert_eq!(
        v("SELECT a FROM t LIMIT 3, 4", SqlEngine::Mariadb)["limit"],
        json!({ "count": 4, "offset": 3 })
    );
    assert_eq!(
        v("SELECT a FROM t LIMIT 4 OFFSET 3", SqlEngine::Mysql)["limit"],
        json!({ "count": 4, "offset": 3 })
    );
}

#[test]
fn visual_counts_aggregates_as_the_ts_did() {
    let p = |sql: &str| {
        parse_visual(sql, SqlEngine::Postgres)
            .expect("parses")
            .expect("statement")
            .projections
            .into_iter()
            .map(|p| (p.expression, p.aggregate_function))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        p("SELECT sum(a) + 1, SUM(a) OVER (), COALESCE(SUM(a), 0), CASE WHEN MAX(a) > 1 THEN 1 END FROM t"),
        [
            ("SUM(a) + 1".to_string(), Some("SUM".to_string())),
            ("SUM(a)".to_string(), Some("SUM".to_string())),
            // The TS never looked inside another function's arguments or a CASE.
            ("COALESCE(SUM(a), 0)".to_string(), None),
            ("CASE WHEN MAX(a) > 1 THEN 1 END".to_string(), None),
        ]
    );
}

fn builder(sql: &str) -> ParsedQuery {
    parse_builder_query(sql, SqlEngine::Postgres, &tutorial_schema(), None).expect("parses")
}

#[test]
fn builder_keeps_the_ts_quirks() {
    // The filter-connector shift: each filter carries the connector of the
    // AND/OR node it's the right side of, so the first keeps AND.
    let q = builder("SELECT * FROM products WHERE price > 1 OR stock > 2 AND name = 'x'");
    let conns: Vec<_> = q
        .filters
        .iter()
        .map(|f| (f.column.as_str(), f.connector))
        .collect();
    assert_eq!(
        conns,
        [
            ("products.price", Connector::And),
            ("products.stock", Connector::Or),
            ("products.name", Connector::And),
        ]
    );
    // Unqualified columns go to the table whose schema has them, else the
    // first table.
    let q = builder(
        "SELECT name, nosuchcolumn FROM orders JOIN customers ON orders.customer_id = customers.id",
    );
    assert_eq!(q.tables[0].table_name, "orders");
    assert_eq!(q.tables[0].selected_columns, ["nosuchcolumn"]);
    assert_eq!(q.tables[1].selected_columns, ["name"]);
    // A window SUM() counts as an aggregate.
    let q = builder("SELECT SUM(price) OVER (PARTITION BY category_id) FROM products");
    assert_eq!(q.column_aggregates.len(), 1);
    assert_eq!(q.column_aggregates[0].function, AggregateFunction::Sum);
    // IN lists, IS NULL and BETWEEN filters are dropped; a subquery IN is kept.
    let q = builder(
        "SELECT * FROM products WHERE id IN (1, 2) AND description IS NULL \
         AND price BETWEEN 1 AND 5 AND stock <> 50.0 AND id NOT IN (SELECT id FROM products)",
    );
    let f: Vec<_> = q
        .filters
        .iter()
        .map(|f| (f.operator, f.value.as_str(), f.subquery_index))
        .collect();
    assert_eq!(
        f,
        [
            // A decimal keeps its text, `<>` is `!=`.
            (FilterOperator::NotEq, "50.0", None),
            (FilterOperator::NotIn, "", Some(0)),
        ]
    );
    // MySQL's `LIMIT 5, 10`: the builder reads the first number (the offset).
    let q = parse_builder_query(
        "SELECT name FROM products LIMIT 5, 10",
        SqlEngine::Mysql,
        &tutorial_schema(),
        None,
    )
    .expect("parses");
    assert_eq!(serde_json::to_value(&q).expect("json")["limit"], json!(5));
}

#[test]
fn builder_enums_serialize_as_the_ts_unions() {
    let q = builder(
        "SELECT p.category_id, COUNT(*) AS n FROM products p LEFT JOIN categories c \
         ON c.id = p.category_id WHERE p.name NOT LIKE 'A%' GROUP BY p.category_id \
         HAVING AVG(p.price) >= 10 ORDER BY p.category_id DESC LIMIT 3",
    );
    let v = serde_json::to_value(&q).expect("json");
    assert_eq!(v["joins"][0]["joinType"], "LEFT");
    assert_eq!(v["filters"][0]["operator"], "NOT LIKE");
    assert_eq!(v["filters"][0]["connector"], "AND");
    assert_eq!(v["having"][0]["operator"], ">=");
    assert_eq!(v["having"][0]["aggregateFunction"], "AVG");
    assert_eq!(v["selectAggregates"][0]["function"], "COUNT");
    assert_eq!(v["orderBy"][0]["direction"], "DESC");
    assert_eq!(v["limit"], 3);
    let q = builder("SELECT * FROM (SELECT id FROM products) s");
    assert_eq!(
        serde_json::to_value(&q).expect("json")["subqueries"][0]["role"],
        "from"
    );
}

#[test]
fn builder_parses_each_engines_quoting() {
    // Fix 9: the builder parses in the connection's dialect.
    let schema = tutorial_schema();
    for (engine, sql) in [
        (
            SqlEngine::Mysql,
            "SELECT `customers`.`name` FROM `Sales`.`customers`",
        ),
        (
            SqlEngine::Mariadb,
            "SELECT `customers`.`name` FROM `Sales`.`customers`",
        ),
        (
            SqlEngine::Mssql,
            "SELECT [customers].[name] FROM [Sales].[customers]",
        ),
        (
            SqlEngine::Postgres,
            r#"SELECT "customers"."name" FROM "Sales"."customers""#,
        ),
        (
            SqlEngine::Sqlite,
            r#"SELECT "customers"."name" FROM "main"."customers""#,
        ),
        (
            SqlEngine::Duckdb,
            r#"SELECT "customers"."name" FROM "main"."customers""#,
        ),
    ] {
        let q = parse_builder_query(sql, engine, &schema, None).expect("parses");
        assert_eq!(q.tables[0].table_name, "customers", "{engine}");
        assert_eq!(q.tables[0].selected_columns, ["name"], "{engine}");
    }
    // PostgreSQL rejects backticks (decision 4).
    assert_eq!(
        parse_builder_query("SELECT `a` FROM `t`", SqlEngine::Postgres, &schema, None),
        None
    );
}
