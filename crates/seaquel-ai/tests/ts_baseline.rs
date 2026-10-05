//! Task 1's TypeScript baseline replayed against the registry, the prompt
//! and the renderers: every case of `prompts.json`,
//! `mentions.json`, `tool-results.json` and `history.json` must equal its
//! recording with `changes.json`'s `expected` fields in their place.
//!
//! `turns.json`, `page.json`, `generate.json`, `models.json` and
//! `errors.json` need a turn, a store, the provider mock or the page's
//! wording: Core replays them and the page words the errors.
//! [`the_files_left_to_task_4_are_not_replayed_here`] keeps that
//! list honest.
//!
//! Tool calls go through a small stand-in for Core: the registry's
//! `prepare` and read-only check, the case's approval, then the case's
//! answers (query results, the schema, the saved queries, the EXPLAIN plan,
//! the page's answer to a client tool) through the renderers. What Core
//! asks the database is checked as the record's `fetches`/`calls`.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde_json::{json, Value as Json};
use sha2::{Digest, Sha256};

use seaquel_ai::limits::{QuerySpec, HISTORY_BYTES, SCHEMA_CONTEXT_BYTES};
use seaquel_ai::prompt::history::{history, HistoryRow, Part, Role};
use seaquel_ai::prompt::{
    mentions, schema_context, system, MentionDashboard, MentionQuery, SchemaContext,
};
use seaquel_ai::sharing::Sharing;
use seaquel_ai::tools::{
    prepare, read_only_check, read_only_sql, render, saved, Args, Gate, Profile, Tool, ToolError,
    ToolOutput,
};
use seaquel_sql::params::substitute;
use seaquel_sql::SqlEngine;
use seaquel_types::storage::PersistedSavedQuery;
use seaquel_types::{ExplainResult, SchemaTable, Value};

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ts-baseline");

fn read(file: &str) -> Json {
    let path = format!("{DIR}/{file}");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn cases(file: &str) -> Vec<Json> {
    read(&format!("{file}.json")).as_array().unwrap().clone()
}

fn changes() -> &'static serde_json::Map<String, Json> {
    static CHANGES: OnceLock<serde_json::Map<String, Json>> = OnceLock::new();
    CHANGES.get_or_init(|| read("changes.json").as_object().unwrap().clone())
}

fn inputs() -> &'static Json {
    static INPUTS: OnceLock<Json> = OnceLock::new();
    INPUTS.get_or_init(|| read("inputs.json"))
}

/// A case's field as Rust must produce it: `changes.json`'s `expected`
/// value when it lists one, else the recording's.
fn expected(file: &str, case: &Json, field: &str) -> Json {
    let key = format!("{file}/{}", case["name"].as_str().unwrap());
    if let Some(v) = changes()
        .get(&key)
        .and_then(|c| c["expected"].as_object())
        .and_then(|e| e.get(field))
    {
        return v.clone();
    }
    case[field].clone()
}

fn is_absent(v: &Json) -> bool {
    v.get("$absent").is_some()
}

/// A TypeScript `SchemaTable` as Rust reads it (`indexes` may be missing).
fn schema_table(mut t: Json) -> SchemaTable {
    if t.get("indexes").is_none() {
        t["indexes"] = json!([]);
    }
    serde_json::from_value(t).unwrap()
}

fn tables_of(v: &Json) -> Vec<SchemaTable> {
    match v {
        Json::String(name) => tables_of(&inputs()[name.as_str()]),
        Json::Array(items) => items.iter().cloned().map(schema_table).collect(),
        Json::Null => Vec::new(),
        other => panic!("not a schema: {other}"),
    }
}

fn schema() -> Vec<SchemaTable> {
    tables_of(&json!("SCHEMA"))
}

/// The README's `LARGE`: `t_0001` … `t_3000` in `big`, columns `c1` … `c6`
/// of type `integer`, `c<j>` nullable when `j` is even.
fn large(spec: &Json) -> Vec<SchemaTable> {
    let n = spec["tables"].as_u64().unwrap();
    let cols = spec["columns"].as_u64().unwrap();
    let schema = spec["schema"].as_str().unwrap();
    (1..=n)
        .map(|i| {
            let columns: Vec<Json> = (1..=cols)
                .map(|j| {
                    json!({"name": format!("c{j}"), "type": "integer", "nullable": j % 2 == 0,
                           "isPrimaryKey": false, "isForeignKey": false})
                })
                .collect();
            schema_table(
                json!({"name": format!("t_{i:04}"), "schema": schema, "type": "table",
                                "columns": columns, "indexes": []}),
            )
        })
        .collect()
}

fn sharing(schema: bool, data: bool) -> Sharing {
    Sharing { schema, data }
}

/// `{bytes, sha256, head, tail}` as the recorder digests a long context.
fn digest(s: &str) -> Json {
    let chars: Vec<char> = s.chars().collect();
    let head: String = chars.iter().take(200).collect();
    let tail: String = chars[chars.len().saturating_sub(200)..].iter().collect();
    let sha = Sha256::digest(s.as_bytes());
    json!({
        "bytes": s.len(),
        "sha256": sha.iter().map(|b| format!("{b:02x}")).collect::<String>(),
        "head": head,
        "tail": tail,
    })
}

// ── prompts.json ───────────────────────────────────────────────────────────

#[test]
fn prompts_and_schema_contexts_replay() {
    let all = cases("prompts");
    assert_eq!(all.len(), 28);
    for case in &all {
        let name = case["name"].as_str().unwrap();
        let input = &case["input"];
        if name.starts_with("system/") {
            let engine: SqlEngine = input["engine"].as_str().unwrap().parse().unwrap();
            // Schema sharing is the case's `schema`, data sharing off.
            let s = sharing(!input["schema"].is_null(), false);
            let context: Option<SchemaContext> = s
                .schema
                .then(|| schema_context(&tables_of(&input["schema"]), SCHEMA_CONTEXT_BYTES));
            let prompt = system(
                engine,
                context.as_ref(),
                s,
                true,
                input["dashboards"].as_bool().unwrap(),
            );
            assert_eq!(
                Json::String(prompt),
                expected("prompts", case, "prompt"),
                "{name}"
            );
        } else {
            let tables = match input.get("generate") {
                Some(spec) => large(spec),
                None => tables_of(&input["tables"]),
            };
            let context = schema_context(&tables, SCHEMA_CONTEXT_BYTES);
            let want = expected("prompts", case, "context");
            let got = if want.is_object() {
                digest(&context.text)
            } else {
                Json::String(context.text.clone())
            };
            assert_eq!(got, want, "{name}");
        }
    }
}

#[test]
fn the_large_schema_context_keeps_whole_tables_and_names_the_rest() {
    let tables = large(&inputs()["LARGE"]);
    let context = schema_context(&tables, SCHEMA_CONTEXT_BYTES);
    assert_eq!((context.kept, context.left), (1056, 1944));
    let (body, note) = context.text.rsplit_once("\n\n").unwrap();
    assert!(body.len() <= SCHEMA_CONTEXT_BYTES);
    assert!(body.ends_with("  c6 integer"), "whole tables only");
    assert_eq!(
        note,
        "(1944 more tables not shown; use list_tables and describe_table to see them.)"
    );
}

// ── mentions.json ──────────────────────────────────────────────────────────

fn saved_inputs() -> Vec<MentionQuery> {
    serde_json::from_value(inputs()["SAVED"].clone()).unwrap()
}

fn dashboard_inputs() -> Vec<MentionDashboard> {
    serde_json::from_value(inputs()["DASHBOARDS"].clone()).unwrap()
}

#[test]
fn mentions_replay() {
    let all = cases("mentions");
    assert_eq!(all.len(), 19);
    for case in &all {
        let input = &case["input"];
        let enriched = mentions(
            input["content"].as_str().unwrap(),
            input["sharing"]["schema"].as_bool().unwrap(),
            &tables_of(&input["tables"]),
            &saved_inputs(),
            &dashboard_inputs(),
        );
        assert_eq!(
            Json::String(enriched),
            expected("mentions", case, "enriched"),
            "{}",
            case["name"]
        );
    }
}

// ── tool-results.json ──────────────────────────────────────────────────────

const CONNECTION: &str = "Local";
const DEFAULT_ANSWER: &str = r#"{"columns":["n"],"rows":[[1]]}"#;

/// What Core asked the database, as the record writes it.
fn fetch(spec: QuerySpec) -> Json {
    json!({
        "maxRows": spec.max_rows,
        "maxBytes": spec.max_bytes,
        "timeoutMs": spec.timeout.as_millis() as u64,
    })
}

/// A query answer (`{columns, rows, truncated?}`, `{error}` or `{cancel}`)
/// through the assistant's renderer, as Core runs it: the driver returns at
/// most `max_rows` rows and marks the batch truncated past them. `None`
/// for a cancel: nothing follows it.
fn answer_rows(answer: &Json, max_rows: usize) -> Option<Result<Json, ToolError>> {
    if answer.get("cancel").is_some() {
        return None;
    }
    if let Some(e) = answer["error"].as_str() {
        let (code, message) = e.split_once(": ").unwrap();
        return Some(Err(ToolError::new(code, message)));
    }
    let mut rows = render::Rows::new(Profile::Assistant, max_rows);
    rows.set_columns(serde_json::from_value(answer["columns"].clone()).unwrap());
    let all = answer["rows"].as_array().unwrap();
    if all.len() > max_rows || answer["truncated"] == json!(true) {
        rows.mark_truncated();
    }
    for row in all.iter().take(max_rows) {
        let cells: Vec<Value> = row
            .as_array()
            .unwrap()
            .iter()
            .map(|c| Value::from_wire(c.clone()).unwrap())
            .collect();
        if !rows.push(&cells) {
            break;
        }
    }
    Some(Ok(rows.into_json()))
}

/// One tool case's world: the sharing flags Core read, whether the page
/// offered the client tools, the approval, what the database, storage and
/// page answer.
struct World<'a> {
    sharing: Sharing,
    client_tools: bool,
    approve: bool,
    answers: &'a Json,
    rust: &'a Json,
    page_answer: &'a Json,
}

struct Ran {
    output: Option<ToolOutput>,
    queries: Vec<Json>,
    forwarded: bool,
}

fn saved_queries() -> Vec<PersistedSavedQuery> {
    inputs()["SAVED"]
        .as_array()
        .unwrap()
        .iter()
        .map(|q| {
            let mut q = q.clone();
            q["projectId"] = json!("p1");
            q["createdAt"] = json!("2026-01-02T03:04:05.000Z");
            q["updatedAt"] = json!("2026-01-02T03:04:05.000Z");
            serde_json::from_value(q).unwrap()
        })
        .collect()
}

/// `changes.json`'s `*` rule 1: a `run_query` input's `query` is sent as
/// `sql`.
fn renamed(tool: &str, input: &Json) -> Json {
    match input {
        Json::Object(map) if tool == "run_query" => Json::Object(
            map.iter()
                .map(|(k, v)| {
                    (
                        if k == "query" {
                            "sql".into()
                        } else {
                            k.clone()
                        },
                        v.clone(),
                    )
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// A stand-in for Core's `ai::tools::call` over the case's answers.
fn run_tool(name: &str, input: &Json, w: &World<'_>) -> Ran {
    let mut ran = Ran {
        output: None,
        queries: Vec::new(),
        forwarded: false,
    };
    let gate = Gate {
        sharing: w.sharing,
        client_tools: w.client_tools,
        connection_name: CONNECTION,
    };
    let result = (|| -> Option<Result<Json, ToolError>> {
        let call = match prepare(name, input, &gate) {
            Ok(call) => call,
            Err(e) => return Some(Err(e)),
        };
        if let Err(e) = read_only_check(&call, SqlEngine::Postgres) {
            return Some(Err(e));
        }
        let project = w.rust["project"].as_str().unwrap_or("Main");
        let mut query = |sql: &str, binds: Vec<Value>, max_rows: usize| {
            assert!(binds.is_empty(), "the cases bind nothing");
            let mut call = json!({"call": "runQuery", "sql": sql});
            for (k, v) in fetch(QuerySpec::new(max_rows)).as_object().unwrap() {
                call[k] = v.clone();
            }
            ran.queries.push(call);
            let default: Json = serde_json::from_str(DEFAULT_ANSWER).unwrap();
            answer_rows(w.answers.get(sql).unwrap_or(&default), max_rows)
        };
        match &call.args {
            Args::RunQuery { sql, .. } => {
                if !w.approve {
                    return Some(Err(ToolError::denied()));
                }
                query(sql, Vec::new(), call.max_rows())
            }
            Args::ExplainQuery { .. } => {
                if !w.approve {
                    return Some(Err(ToolError::denied()));
                }
                let plan: ExplainResult =
                    serde_json::from_value(w.rust["explain"].clone()).unwrap();
                Some(Ok(render::explain(Profile::Assistant, &plan)))
            }
            Args::ListSchemas => {
                let list: Vec<String> = serde_json::from_value(w.rust["schemas"].clone()).unwrap();
                Some(Ok(render::schemas(&list)))
            }
            Args::ListTables { schema: wanted } => Some(Ok(render::tables(
                Profile::Assistant,
                &schema(),
                wanted.as_deref(),
            ))),
            Args::DescribeTable {
                schema: wanted,
                table,
            } => {
                let tables = schema();
                Some(
                    render::find_table(&tables, wanted.as_deref(), table).map(|t| {
                        render::describe(Profile::Assistant, t, table, &t.columns, &t.indexes)
                    }),
                )
            }
            Args::ListSavedQueries { .. } => {
                let entries = saved_queries()
                    .iter()
                    .map(|q| saved::describe(q, project, &[CONNECTION]))
                    .collect();
                Some(Ok(render::saved_queries(Profile::Assistant, entries, 0)))
            }
            Args::RunSavedQuery {
                saved_query,
                params,
                ..
            } => {
                let all = saved_queries();
                let q = match render::find_saved_query(&all, saved_query, project, CONNECTION) {
                    Ok(q) => q,
                    Err(e) => return Some(Err(e)),
                };
                // Values, substitution, the read-only check of the SQL that
                // will run, then the approval card shows it.
                let values = match saved::parameter_values(&saved::definitions(q), params.clone()) {
                    Ok(v) => v,
                    Err(e) => return Some(Err(e)),
                };
                let s = substitute(&q.query, &values, SqlEngine::Postgres, false).unwrap();
                if let Err(e) = read_only_sql(&s.sql, SqlEngine::Postgres) {
                    return Some(Err(e));
                }
                if !w.approve {
                    return Some(Err(ToolError::denied()));
                }
                query(&s.sql, s.bind_values, call.max_rows())
            }
            Args::Client { .. } => {
                ran.forwarded = true;
                Some(Ok(Json::Null))
            }
            Args::ListConnections => unreachable!("not an assistant tool"),
        }
    })();
    ran.output = match result {
        None => None,
        Some(Err(e)) => Some(ToolOutput::error(&e)),
        Some(Ok(_)) if ran.forwarded => {
            let tool = Tool::find(Profile::Assistant, name).unwrap();
            Some(render::client_result(
                tool,
                w.page_answer.as_str().unwrap(),
                w.sharing.schema,
            ))
        }
        Some(Ok(v)) => Some(ToolOutput::ok(&v)),
    };
    ran
}

fn check_output(case: &Json, out: &Option<ToolOutput>) {
    let name = &case["name"];
    let want_result = expected("tool-results", case, "result");
    let want_error = expected("tool-results", case, "isError");
    match out {
        None => {
            assert!(is_absent(&want_result), "{name}: Rust sends no result");
            assert!(is_absent(&want_error), "{name}");
        }
        Some(out) => {
            assert_eq!(Json::String(out.text.clone()), want_result, "{name}");
            assert_eq!(Json::Bool(out.is_error), want_error, "{name}");
        }
    }
}

#[test]
fn tool_results_replay() {
    let all = cases("tool-results");
    assert_eq!(all.len(), 66);
    let empty = json!({});
    let mut replayed = 0;
    for case in &all {
        let name = case["name"].as_str().unwrap();
        let input = &case["input"];
        if let Some(rest) = name.strip_prefix("run-query/") {
            // `runAndFormat`: the run_query tool's result for an answer.
            let _ = rest;
            let mut fetches = Vec::new();
            let mut out = None;
            if input["abortBefore"] != json!(true) {
                fetches.push(fetch(QuerySpec::new(100)));
                out = answer_rows(&input["answer"], 100).map(|r| ToolOutput::from_result(&r));
            }
            assert_eq!(
                Json::Array(fetches),
                expected("tool-results", case, "fetches"),
                "{name}"
            );
            check_output(case, &out);
        } else if name == "tool/dashboard/unknown-direct" {
            // The page's handler reached directly; Rust never asks the page
            // about a tool it doesn't know.
            assert!(is_absent(&expected("tool-results", case, "result")));
            let gate = Gate {
                sharing: sharing(true, true),
                client_tools: true,
                connection_name: CONNECTION,
            };
            let err = prepare("pin_widget", &json!({}), &gate).unwrap_err();
            assert_eq!(
                err.to_string(),
                "INVALID_ARGUMENT: Unknown tool: pin_widget"
            );
        } else {
            let tool = input["tool"].as_str().unwrap();
            let params = &input["params"];
            let world = World {
                sharing: sharing(
                    params["shareSchema"].as_bool().unwrap_or(true),
                    params["shareData"].as_bool().unwrap_or(true),
                ),
                // The one case whose page offered no dashboard tools.
                client_tools: name != "tool/dashboard/not-available",
                approve: !name.ends_with("/denied"),
                answers: input.get("answers").unwrap_or(&empty),
                rust: input.get("rustInputs").unwrap_or(&empty),
                page_answer: &case["result"],
            };
            let ran = run_tool(tool, &renamed(tool, &input["input"]), &world);
            check_output(case, &ran.output);
            let calls = if Tool::find(Profile::Assistant, tool).is_some_and(Tool::is_client) {
                // The page's own calls, when the call reached it.
                if ran.forwarded {
                    case["calls"].clone()
                } else {
                    json!([])
                }
            } else {
                Json::Array(ran.queries)
            };
            assert_eq!(calls, expected("tool-results", case, "calls"), "{name}");
        }
        replayed += 1;
    }
    assert_eq!(replayed, 66);
}

// ── history.json ───────────────────────────────────────────────────────────

/// The README's `history.json` generator.
fn generated_rows(turns: &Json) -> Vec<HistoryRow> {
    let mut rows = Vec::new();
    for t in turns.as_array().unwrap() {
        let tag = t["tag"].as_str().unwrap();
        let rounds = t["rounds"].as_u64().unwrap() as u32;
        let result = "x".repeat(t["resultBytes"].as_u64().unwrap() as usize);
        rows.push(HistoryRow {
            id: format!("{tag}-user"),
            role: Role::User,
            content: t["user"].as_str().unwrap().to_string(),
            parts: None,
            dashboard_id: None,
        });
        let mut parts = Vec::new();
        let mut content = String::new();
        for r in 0..rounds {
            let text = format!("Round {r}. ");
            content.push_str(&text);
            parts.push(Part::Text { round: r, text });
            parts.push(Part::Tool {
                round: r,
                call_id: format!("{tag}_{r}"),
                name: "run_query".into(),
                input: json!({"sql": format!("SELECT {r}")}),
                ok: true,
                result: result.clone(),
                result_bytes: None,
            });
        }
        let final_text = t["finalText"].as_str().unwrap().to_string();
        content.push_str(&final_text);
        parts.push(Part::Text {
            round: rounds,
            text: final_text,
        });
        rows.push(HistoryRow {
            id: format!("{tag}-reply"),
            role: Role::Assistant,
            content,
            parts: Some(parts),
            dashboard_id: None,
        });
    }
    rows
}

#[test]
fn history_budget_replays() {
    let all = cases("history");
    assert_eq!(all.len(), 3);
    for case in &all {
        let rows = generated_rows(&case["input"]["generate"]);
        let h = history(&rows, HISTORY_BYTES);
        let kept: Vec<Json> = h
            .kept
            .iter()
            .map(|k| match &k.rounds {
                Some(r) => json!({"id": k.id, "rounds": r}),
                None => json!({"id": k.id}),
            })
            .collect();
        let name = &case["name"];
        assert_eq!(
            Json::Array(kept),
            expected("history", case, "kept"),
            "{name}"
        );
        assert_eq!(json!(h.bytes), expected("history", case, "bytes"), "{name}");
    }
}

// ── What is replayed where ─────────────────────────────────────────────────

#[test]
fn every_change_for_these_files_names_a_case() {
    let mut names: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for file in ["prompts", "mentions", "tool-results", "history"] {
        names.insert(
            file,
            cases(file)
                .iter()
                .map(|c| c["name"].as_str().unwrap().to_string())
                .collect(),
        );
    }
    for key in changes().keys() {
        let Some((file, case)) = key.split_once('/') else {
            assert_eq!(key, "*");
            continue;
        };
        if let Some(list) = names.get(file) {
            assert!(list.iter().any(|n| n == case), "{key} names no case");
        }
    }
}

#[test]
fn the_files_left_to_task_4_are_not_replayed_here() {
    // Core's turn, the page's store, the inline prompt and the model list
    // replay these against the mock provider; the page words the
    // errors.
    for (file, count) in [
        ("turns", 73),
        ("page", 21),
        ("generate", 21),
        ("models", 18),
        ("errors", 8),
    ] {
        assert_eq!(cases(file).len(), count, "{file}");
    }
}
