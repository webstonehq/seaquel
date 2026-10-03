//! The tool registry: which tools each profile offers, their frozen
//! schemas, the assistant's argument parser and the renderers' budgets.

use serde_json::{json, Value as Json};

use seaquel_ai::limits::ASSISTANT_RESULT_BYTES;
use seaquel_ai::sharing::Sharing;
use seaquel_ai::tools::{
    definitions, parse, prepare, read_only_check, read_only_sql, render, Args, Gate, Profile, Tool,
    ToolError,
};
use seaquel_sql::SqlEngine;
use seaquel_types::{SchemaColumn, SchemaIndex, SchemaTable, Value};

const SCHEMAS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/tool-schemas.json"
);

fn on() -> Sharing {
    Sharing {
        schema: true,
        data: true,
    }
}

fn gate(sharing: Sharing) -> Gate<'static> {
    Gate {
        sharing,
        client_tools: true,
        connection_name: "Local",
    }
}

fn names(profile: Profile, sharing: Sharing, client: bool) -> Vec<&'static str> {
    definitions(profile, sharing, client)
        .iter()
        .map(|d| d.name)
        .collect()
}

#[test]
fn the_assistant_offers_tools_by_sharing_and_client_tools() {
    for schema in [false, true] {
        for data in [false, true] {
            for client in [false, true] {
                let mut want = Vec::new();
                if data {
                    want.extend(["run_query", "explain_query"]);
                }
                if schema {
                    want.extend([
                        "list_schemas",
                        "list_tables",
                        "describe_table",
                        "list_saved_queries",
                    ]);
                }
                if data {
                    want.push("run_saved_query");
                }
                if client {
                    want.extend([
                        "create_dashboard",
                        "add_widget",
                        "get_dashboard",
                        "update_widget",
                        "remove_widget",
                    ]);
                }
                assert_eq!(
                    names(Profile::Assistant, Sharing { schema, data }, client),
                    want,
                    "schema {schema}, data {data}, client {client}"
                );
            }
        }
    }
}

#[test]
fn mcp_always_lists_its_eight_tools() {
    let off = Sharing {
        schema: false,
        data: false,
    };
    let want = [
        "describe_table",
        "explain_query",
        "list_connections",
        "list_saved_queries",
        "list_schemas",
        "list_tables",
        "run_query",
        "run_saved_query",
    ];
    assert_eq!(names(Profile::Mcp, off, false), want);
    assert_eq!(names(Profile::Mcp, on(), true), want);
}

fn mentions_key(v: &Json, key: &str) -> bool {
    match v {
        Json::Object(map) => map.iter().any(|(k, v)| k == key || mentions_key(v, key)),
        Json::Array(items) => items.iter().any(|v| mentions_key(v, key)),
        _ => false,
    }
}

#[test]
fn the_assistant_profile_has_no_connection_argument() {
    for d in definitions(Profile::Assistant, on(), true) {
        assert!(
            !mentions_key(&d.input_schema, "connection"),
            "{}: {}",
            d.name,
            d.input_schema
        );
        assert!(
            !mentions_key(&d.input_schema, "$ref"),
            "{}: inlined",
            d.name
        );
    }
}

/// `tool-schemas.json`: MCP's tools as `tools/list` gives them (seaquel-mcp's
/// `tests/tool_schemas.rs` checks the server against the same file), and the
/// assistant's as a round sends them.
#[test]
fn the_schemas_equal_the_frozen_ones() {
    let frozen: Json = serde_json::from_str(&std::fs::read_to_string(SCHEMAS).unwrap()).unwrap();
    let mcp: Vec<Json> = definitions(Profile::Mcp, on(), false)
        .iter()
        .map(|d| d.mcp_json())
        .collect();
    // Byte for byte: both are serde_json's text of the same value.
    assert_eq!(
        Json::Array(mcp).to_string(),
        frozen["mcp"].to_string(),
        "MCP's schemas changed"
    );
    let assistant: Vec<Json> = definitions(Profile::Assistant, on(), true)
        .iter()
        .map(|d| {
            let spec = d.spec();
            json!({"name": spec.name, "description": spec.description, "input_schema": spec.input_schema})
        })
        .collect();
    assert_eq!(
        Json::Array(assistant).to_string(),
        frozen["assistant"].to_string(),
        "the assistant's schemas changed"
    );
}

/// A valid input for each assistant tool.
fn valid(tool: Tool) -> Json {
    match tool {
        Tool::RunQuery | Tool::ExplainQuery => json!({"sql": "SELECT 1"}),
        Tool::ListSchemas | Tool::ListSavedQueries => json!({}),
        Tool::ListTables => json!({"schema": "public"}),
        Tool::DescribeTable => json!({"table": "users"}),
        Tool::RunSavedQuery => json!({"saved_query": "q"}),
        Tool::CreateDashboard => json!({"name": "D"}),
        Tool::AddWidget => json!({"dashboard_id": "d", "title": "T", "x": 0, "y": 0,
                                  "width": 1, "height": 1, "widget_type": "text"}),
        Tool::GetDashboard => json!({"dashboard_id": "d"}),
        Tool::UpdateWidget => json!({"dashboard_id": "d", "widget_id": "w"}),
        Tool::RemoveWidget => json!({"dashboard_id": "d", "widget_id": "w"}),
        Tool::ListConnections => unreachable!(),
    }
}

fn refusal(name: &str, input: Json) -> String {
    prepare(name, &input, &gate(on())).unwrap_err().to_string()
}

#[test]
fn every_assistant_tool_parses_its_valid_input() {
    for tool in Tool::ASSISTANT {
        let call = prepare(tool.name(), &valid(tool), &gate(on()))
            .unwrap_or_else(|e| panic!("{}: {e}", tool.name()));
        assert_eq!(call.tool, tool);
        assert_eq!(call.connection, None);
    }
}

#[test]
fn every_assistant_tool_refuses_unknown_fields() {
    for tool in Tool::ASSISTANT {
        let mut input = valid(tool);
        input["zzz"] = json!(1);
        let text = refusal(tool.name(), input);
        assert!(
            text.starts_with("INVALID_ARGUMENT: unknown field `zzz`"),
            "{}: {text}",
            tool.name()
        );
    }
    // Nested configs too.
    let mut input = valid(Tool::AddWidget);
    input["text_config"] = json!({"content": "x", "font": "big"});
    assert_eq!(
        refusal("add_widget", input),
        "INVALID_ARGUMENT: text_config: unknown field `font`, expected `content`"
    );
}

#[test]
fn every_assistant_tool_refuses_wrong_types() {
    let cases = [
        (
            "run_query",
            json!({"sql": 42}),
            "sql: invalid type: integer `42`, expected a string",
        ),
        (
            "run_query",
            json!({"sql": "SELECT 1", "max_rows": "5"}),
            "max_rows: invalid type: string \"5\", expected u32",
        ),
        (
            "explain_query",
            json!({"sql": true}),
            "sql: invalid type: boolean `true`, expected a string",
        ),
        (
            "list_tables",
            json!({"schema": 1}),
            "schema: invalid type: integer `1`, expected a string",
        ),
        (
            "describe_table",
            json!({"table": ["t"]}),
            "table: invalid type: sequence, expected a string",
        ),
        (
            "run_saved_query",
            json!({"saved_query": "q", "params": []}),
            "params: invalid type: sequence, expected a map",
        ),
        (
            "create_dashboard",
            json!({"name": {}}),
            "name: invalid type: map, expected a string",
        ),
        (
            "get_dashboard",
            json!({"dashboard_id": 7}),
            "dashboard_id: invalid type: integer `7`, expected a string",
        ),
        (
            "remove_widget",
            json!({"dashboard_id": "d", "widget_id": 1.5}),
            "widget_id: invalid type: floating point `1.5`, expected a string",
        ),
        (
            "update_widget",
            json!({"dashboard_id": "d", "widget_id": "w", "y": "20"}),
            "y: invalid type: string \"20\", expected f64",
        ),
        (
            "add_widget",
            json!({"dashboard_id": "d", "title": "T", "x": 0, "y": 0, "width": 1,
                              "height": 1, "widget_type": "map"}),
            "widget_type: unknown variant `map`, expected one of `chart`, `kpi`, `text`",
        ),
        (
            "add_widget",
            json!({"dashboard_id": "d", "title": "T", "x": 0, "y": 0, "width": 1,
                              "height": 1, "widget_type": "chart",
                              "chart_config": {"yAxis": ["v", 3]}}),
            "chart_config.yAxis[1]: invalid type: integer `3`, expected a string",
        ),
    ];
    for (name, input, message) in cases {
        assert_eq!(
            refusal(name, input),
            format!("INVALID_ARGUMENT: {message}"),
            "{name}"
        );
    }
    assert_eq!(
        refusal("create_dashboard", json!({})),
        "INVALID_ARGUMENT: missing field `name`"
    );
}

#[test]
fn max_rows_out_of_range_is_refused() {
    for name in ["run_query", "run_saved_query"] {
        let base = if name == "run_query" {
            json!({"sql": "SELECT 1"})
        } else {
            json!({"saved_query": "q"})
        };
        for (n, ok) in [(0, false), (1, true), (1000, true), (1001, false)] {
            let mut input = base.clone();
            input["max_rows"] = json!(n);
            match prepare(name, &input, &gate(on())) {
                Ok(call) => {
                    assert!(ok, "{name} {n}");
                    assert_eq!(call.max_rows(), n as usize);
                }
                Err(e) => {
                    assert!(!ok, "{name} {n}");
                    assert_eq!(
                        e.to_string(),
                        format!("INVALID_ARGUMENT: max_rows must be between 1 and 1000, got {n}")
                    );
                }
            }
        }
        let call = prepare(name, &base, &gate(on())).unwrap();
        assert_eq!(call.max_rows(), 100);
        let mut input = base.clone();
        input["max_rows"] = json!(-1);
        assert!(refusal(name, input).starts_with("INVALID_ARGUMENT: max_rows: invalid value"));
    }
}

/// Decision 24: keys are visited in sorted order, so of two bad fields the
/// sorted-first is reported, whatever order the model wrote them in. This
/// fails if anything turns on serde_json's `preserve_order`.
#[test]
fn argument_errors_name_the_sorted_first_field() {
    let input: Json = serde_json::from_str(
        r#"{"y": "1", "x": "2", "widget_type": "gauge", "height": "3", "dashboard_id": "d",
            "widget_id": "w"}"#,
    )
    .unwrap();
    assert_eq!(
        refusal("update_widget", input),
        "INVALID_ARGUMENT: height: invalid type: string \"3\", expected f64"
    );
    // Missing fields come after every present one, in declaration order.
    let input: Json =
        serde_json::from_str(r#"{"widget_type": "kpi", "kpi_config": {"valueColumn": 1}}"#)
            .unwrap();
    assert_eq!(
        refusal("add_widget", input),
        "INVALID_ARGUMENT: kpi_config.valueColumn: invalid type: integer `1`, expected a string"
    );
    let input: Json = serde_json::from_str(r#"{"widget_type": "kpi", "y": 0}"#).unwrap();
    assert_eq!(
        refusal("add_widget", input),
        "INVALID_ARGUMENT: missing field `dashboard_id`"
    );
}

#[test]
fn sharing_is_checked_before_the_arguments() {
    let data_off = Sharing {
        schema: true,
        data: false,
    };
    let schema_off = Sharing {
        schema: false,
        data: true,
    };
    for name in ["run_query", "explain_query", "run_saved_query"] {
        let e = prepare(name, &json!({"bogus": 1}), &gate(data_off)).unwrap_err();
        assert_eq!(e.code, "DATA_SHARING_OFF", "{name}");
        assert_eq!(e, ToolError::data_sharing_off("Local"));
    }
    for name in [
        "list_schemas",
        "list_tables",
        "describe_table",
        "list_saved_queries",
    ] {
        let e = prepare(name, &json!({"bogus": 1}), &gate(schema_off)).unwrap_err();
        assert_eq!(e.code, "SCHEMA_SHARING_OFF", "{name}");
        assert_eq!(e, ToolError::schema_sharing_off("Local"));
    }
    // The dashboard tools need neither.
    let off = Sharing {
        schema: false,
        data: false,
    };
    assert!(prepare("get_dashboard", &json!({"dashboard_id": "d"}), &gate(off)).is_ok());
}

#[test]
fn client_tools_are_unknown_unless_offered_and_mcp_tools_unknown_to_the_assistant() {
    let mut g = gate(on());
    g.client_tools = false;
    assert_eq!(
        prepare("get_dashboard", &json!({"dashboard_id": "d"}), &g)
            .unwrap_err()
            .to_string(),
        "INVALID_ARGUMENT: Unknown tool: get_dashboard"
    );
    assert_eq!(
        refusal("list_connections", json!({})),
        "INVALID_ARGUMENT: Unknown tool: list_connections"
    );
    assert_eq!(
        parse(Profile::Mcp, "get_dashboard", &json!({"dashboard_id": "d"}))
            .unwrap_err()
            .to_string(),
        "INVALID_ARGUMENT: Unknown tool: get_dashboard"
    );
}

#[test]
fn a_client_call_carries_the_input_and_the_widget_query() {
    let input = json!({"dashboard_id": "d", "widget_id": "w", "query": "SELECT 2"});
    let call = prepare("update_widget", &input, &gate(on())).unwrap();
    assert_eq!(
        call.args,
        Args::Client {
            input: input.clone(),
            query: Some("SELECT 2".into())
        }
    );
}

#[test]
fn mcp_arguments_parse_as_rmcp_did() {
    // Unknown fields ignored; `connection` kept.
    let call = parse(
        Profile::Mcp,
        "run_query",
        &json!({"connection": "lite", "sql": "SELECT 1", "extra": true}),
    )
    .unwrap();
    assert_eq!(call.connection.as_deref(), Some("lite"));
    assert_eq!(
        call.args,
        Args::RunQuery {
            sql: "SELECT 1".into(),
            max_rows: None
        }
    );
    let call = parse(Profile::Mcp, "list_connections", &json!({})).unwrap();
    assert_eq!((call.tool, call.connection), (Tool::ListConnections, None));
    let e = parse(Profile::Mcp, "run_query", &json!({"connection": "lite"})).unwrap_err();
    assert_eq!(e.code, "INVALID_ARGUMENT");
    assert!(e.message.contains("sql"), "{}", e.message);
}

#[test]
fn the_read_only_check_covers_queries_and_widget_queries() {
    let g = gate(on());
    let check = |name: &str, input: Json| {
        read_only_check(&prepare(name, &input, &g).unwrap(), SqlEngine::Postgres)
    };
    assert!(check("run_query", json!({"sql": "SELECT 1"})).is_ok());
    assert_eq!(
        check("run_query", json!({"sql": "DELETE FROM t"})).unwrap_err(),
        ToolError::read_only()
    );
    assert!(check("explain_query", json!({"sql": "SELECT 1; DROP TABLE t"})).is_err());
    let widget = |query: &str| json!({"dashboard_id": "d", "widget_id": "w", "query": query});
    assert!(
        check("update_widget", widget("  ")).is_ok(),
        "a blank query is a text widget's"
    );
    assert!(check("update_widget", widget("SELECT 1")).is_ok());
    assert!(check("update_widget", widget("DROP TABLE users")).is_err());
    assert!(check("get_dashboard", json!({"dashboard_id": "d"})).is_ok());
    // A saved query's SQL is checked when Core runs it.
    assert!(check("run_saved_query", json!({"saved_query": "q"})).is_ok());
}

// ── Renderers ──────────────────────────────────────────────────────────────

fn column(name: &str) -> SchemaColumn {
    serde_json::from_value(json!({"name": name, "type": "text", "nullable": true,
                                  "isPrimaryKey": false, "isForeignKey": false}))
    .unwrap()
}

fn table(schema: &str, name: &str, columns: Vec<SchemaColumn>) -> SchemaTable {
    SchemaTable {
        name: name.into(),
        schema: schema.into(),
        kind: seaquel_types::TableKind::Table,
        row_count: None,
        columns,
        indexes: Vec::new(),
    }
}

#[test]
fn query_results_word_their_budget_per_profile() {
    let wide = Value::Text("w".repeat(60_000));
    for (profile, words) in [(Profile::Assistant, "256 KB"), (Profile::Mcp, "4 MB")] {
        let mut rows = render::Rows::new(profile, 1000);
        rows.set_columns(vec!["w".into()]);
        while rows.push(std::slice::from_ref(&wide)) {}
        let out = rows.into_json();
        let message = out["message"].as_str().unwrap();
        assert!(
            message.contains(&format!("past the {words} limit")),
            "{message}"
        );
        assert!(out.to_string().len() <= profile.result_bytes() + 512);
        assert_eq!(out["truncated"], true);
    }
    // The fetch budget's note keeps MB (it is 8 MB in both profiles).
    let mut rows = render::Rows::new(Profile::Assistant, 10);
    rows.set_columns(vec!["n".into()]);
    rows.mark_truncated();
    rows.push(&[Value::Int(1)]);
    assert!(rows.into_json()["message"]
        .as_str()
        .unwrap()
        .contains("the 8 MB limit on what a query may read"));
}

#[test]
fn the_assistant_cuts_long_listings_at_whole_items() {
    // 4,000 tables of about 100 bytes each: past 256 KB.
    let many: Vec<SchemaTable> = (0..4000)
        .map(|i| {
            table(
                "public",
                &format!("table_with_a_long_name_{i:05}_{}", "x".repeat(40)),
                vec![],
            )
        })
        .collect();
    let out = render::tables(Profile::Assistant, &many, None);
    let text = out.to_string();
    assert!(text.len() <= ASSISTANT_RESULT_BYTES, "{}", text.len());
    assert_eq!(out["truncated"], true);
    let shown = out["tables"].as_array().unwrap().len();
    assert!(shown > 1000 && shown < 4000, "{shown}");
    assert_eq!(
        out["message"],
        format!("Only the first {shown} of 4000 tables are shown: the next would take the result past the 256 KB limit. Pass `schema` to list one schema's tables.")
    );
    // MCP lists them all.
    let out = render::tables(Profile::Mcp, &many, None);
    assert_eq!(out["tables"].as_array().unwrap().len(), 4000);
    assert!(out.get("truncated").is_none());
    // Few tables: no note.
    let out = render::tables(Profile::Assistant, &many[..3], None);
    assert!(out.get("truncated").is_none() && out.get("message").is_none());

    // A table of 5,000 wide columns, then indexes.
    let cols: Vec<SchemaColumn> = (0..5000)
        .map(|i| column(&format!("column_{i:05}_{}", "y".repeat(60))))
        .collect();
    let t = table("public", "wide", cols.clone());
    let indexes: Vec<SchemaIndex> = vec![SchemaIndex {
        name: "i".into(),
        columns: vec!["a".into()],
        unique: false,
        ty: "btree".into(),
    }];
    let out = render::describe(Profile::Assistant, &t, "wide", &cols, &indexes);
    let text = out.to_string();
    assert!(text.len() <= ASSISTANT_RESULT_BYTES, "{}", text.len());
    assert_eq!(out["truncated"], true);
    let shown = out["columns"].as_array().unwrap().len();
    assert!(shown < 5000);
    assert_eq!(out["indexes"], json!([]), "columns come first");
    assert_eq!(
        out["message"],
        format!("Only the first {shown} of the table's 5001 columns, indexes and foreign keys are shown: the next would take the result past the 256 KB limit.")
    );
    let out = render::describe(Profile::Mcp, &t, "wide", &cols, &indexes);
    assert_eq!(out["columns"].as_array().unwrap().len(), 5000);
    assert_eq!(out["indexes"].as_array().unwrap().len(), 1);

    // Saved queries.
    let entries: Vec<Json> = (0..3000)
        .map(|i| {
            json!({"id": format!("q{i}"), "name": "n".repeat(100), "project": "Main",
                        "connections": ["Local"], "parameters": []})
        })
        .collect();
    let out = render::saved_queries(Profile::Assistant, entries.clone(), 0);
    let text = out.to_string();
    assert!(text.len() <= ASSISTANT_RESULT_BYTES, "{}", text.len());
    let shown = out["savedQueries"].as_array().unwrap().len();
    assert_eq!(
        out["message"],
        format!("Only the first {shown} of 3000 saved queries are shown: the next would take the result past the 256 KB limit.")
    );
    let out = render::saved_queries(Profile::Mcp, entries, 2);
    assert_eq!(out["savedQueries"].as_array().unwrap().len(), 3000);
    assert!(out["message"]
        .as_str()
        .unwrap()
        .starts_with("2 saved queries are not listed"));
}

const DASHBOARD: &str = r#"{"id":"dash-1","name":"Sales","widgets":[{"id":"w-1","title":"Revenue","widgetType":"kpi","query":"SELECT sum(total) FROM orders"},{"id":"w-2","title":"Notes","widgetType":"text","query":""}]}"#;

#[test]
fn get_dashboard_keeps_widget_queries_only_with_schema_sharing() {
    let shared = render::client_result(Tool::GetDashboard, DASHBOARD, true);
    assert_eq!(shared.text, DASHBOARD);
    assert!(!shared.is_error);
    let stripped = render::client_result(Tool::GetDashboard, DASHBOARD, false);
    assert!(!stripped.is_error);
    assert!(!stripped.text.contains("SELECT"), "{}", stripped.text);
    assert!(!stripped.text.contains("\"query\""), "{}", stripped.text);
    let v: Json = serde_json::from_str(&stripped.text).unwrap();
    assert_eq!(v["widgets"][0]["title"], "Revenue");
    // Other tools' answers aren't touched.
    let other = render::client_result(Tool::AddWidget, r#"{"widget_id":"w-new"}"#, false);
    assert_eq!(other.text, r#"{"widget_id":"w-new"}"#);
}

#[test]
fn a_page_error_is_an_error_result_without_a_prefix() {
    let out = render::client_result(
        Tool::GetDashboard,
        r#"{"error":"Dashboard not found"}"#,
        false,
    );
    assert!(out.is_error);
    assert_eq!(out.text, r#"{"error":"Dashboard not found"}"#);
    let out = render::client_result(Tool::RemoveWidget, r#"{"success":true}"#, true);
    assert!(!out.is_error);
}

#[test]
fn a_saved_query_s_sql_gets_the_read_only_check() {
    assert!(read_only_sql("SELECT 1", SqlEngine::Postgres).is_ok());
    assert_eq!(
        read_only_sql("DELETE FROM t WHERE id = 1", SqlEngine::Postgres).unwrap_err(),
        ToolError::read_only()
    );
}

#[test]
fn rows_stop_at_the_first_row_that_doesn_t_fit() {
    let mut rows = render::Rows::new(Profile::Assistant, 1000);
    rows.set_columns(vec!["s".into()]);
    assert!(rows.push(&[Value::Text("a".repeat(60_000))]));
    while rows.push(&[Value::Text("b".repeat(60_000))]) {}
    // A smaller row can't fill the gap: the rows shown are a prefix.
    assert!(!rows.push(&[Value::Text("c".into())]));
    let out = rows.into_json();
    assert!(!out.to_string().contains("\"c\""));
}

#[test]
fn a_get_dashboard_answer_that_doesn_t_parse_is_an_error_without_schema_sharing() {
    // Not JSON at all.
    let out = render::client_result(Tool::GetDashboard, "SELECT secret FROM t", false);
    assert!(out.is_error);
    assert!(out.text.starts_with("INVALID_ARGUMENT: "), "{}", out.text);
    assert!(!out.text.contains("secret"));
    // Nested past serde_json's recursion limit.
    let deep = format!(
        r#"{{"id":"d","widgets":[{{"query":"SELECT secret","x":{}1{}}}]}}"#,
        "[".repeat(200),
        "]".repeat(200)
    );
    let out = render::client_result(Tool::GetDashboard, &deep, false);
    assert!(out.is_error && !out.text.contains("secret"), "{}", out.text);
    // With schema sharing nothing is stripped, so the text goes as it is.
    let out = render::client_result(Tool::GetDashboard, "not json", true);
    assert!(!out.is_error);
    assert_eq!(out.text, "not json");
    let out = render::client_result(Tool::AddWidget, "not json", false);
    assert_eq!((out.text.as_str(), out.is_error), ("not json", false));
}

#[test]
fn client_results_are_cut_at_256_kb() {
    let big = format!(
        r#"{{"id":"d","name":"{}é","widgets":[]}}"#,
        "n".repeat(300_000)
    );
    for (tool, share) in [
        (Tool::GetDashboard, false),
        (Tool::GetDashboard, true),
        (Tool::CreateDashboard, true),
    ] {
        let out = render::client_result(tool, &big, share);
        assert!(
            out.text.len() <= ASSISTANT_RESULT_BYTES,
            "{}",
            out.text.len()
        );
        assert!(
            out.text
                .ends_with(&format!("\n(cut at 256 KB of {} bytes)", big.len())),
            "{tool:?}"
        );
        assert!(!out.is_error);
    }
    let small = r#"{"widget_id":"w"}"#;
    assert_eq!(
        render::client_result(Tool::AddWidget, small, true).text,
        small
    );
    // Multi-byte text is cut on a character boundary.
    let wide = "é".repeat(200_000);
    let out = render::client_result(Tool::RemoveWidget, &wide, true);
    assert!(out.text.len() <= ASSISTANT_RESULT_BYTES);
    assert!(out.text.starts_with("éé"));
}
