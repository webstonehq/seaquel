//! The prompt's pieces beyond the recorded cases: mentions under each
//! sharing combination, history rendered back into rounds, its budget's
//! whole rounds, and the dashboard-id line.

#![allow(clippy::disallowed_types, clippy::disallowed_methods)] // a timing test; native only

use serde_json::{json, Value as Json};

use seaquel_ai::limits::PART_RESULT_BYTES;
use seaquel_ai::prompt::history::{
    history, last_dashboard_id, push_user, with_dashboard_context, HistoryRow, Part, Role,
};
use seaquel_ai::prompt::mentions::MAX_MENTIONS;
use seaquel_ai::prompt::{mentions, schema_context, system, MentionDashboard, MentionQuery};
use seaquel_ai::sharing::Sharing;
use seaquel_ai::tools::ToolOutput;
use seaquel_ai::wire::{round_request, Message, Provider, ProviderKind, Round};
use seaquel_sql::SqlEngine;
use seaquel_types::SchemaTable;

fn users() -> SchemaTable {
    serde_json::from_value(json!({
        "name": "users", "schema": "public", "type": "table",
        "columns": [{"name": "id", "type": "integer", "nullable": false,
                     "isPrimaryKey": true, "isForeignKey": false}],
        "indexes": [],
    }))
    .unwrap()
}

fn query() -> MentionQuery {
    MentionQuery {
        name: "Monthly revenue".into(),
        query: "SELECT 1".into(),
    }
}

fn dashboard() -> MentionDashboard {
    serde_json::from_value(json!({"id": "dash-1", "name": "Sales", "widgets": [
        {"id": "w-1", "title": "Revenue", "widgetType": "kpi", "query": "SELECT sum(total) FROM orders"}
    ]}))
    .unwrap()
}

#[test]
fn mentions_follow_schema_sharing_whatever_data_sharing_is() {
    let text = r#"See @public.users and @"Monthly revenue" and @Sales"#;
    for data in [false, true] {
        // Data sharing has no say: the caller passes schema sharing only.
        let _ = data;
        let on = mentions(text, true, &[users()], &[query()], &[dashboard()]);
        assert!(
            on.starts_with(&format!("{text}\n\nReferenced context:\n")),
            "{on}"
        );
        assert!(on.contains("Table: public.users (table)"));
        assert!(on.contains("Saved query: Monthly revenue\n```sql\nSELECT 1\n```"));
        assert!(on.contains("    Query: SELECT sum(total) FROM orders"));
        let off = mentions(text, false, &[users()], &[query()], &[dashboard()]);
        assert_eq!(
            off, text,
            "no columns, SQL or widget queries without schema sharing"
        );
    }
}

#[test]
fn mentions_scan_like_the_typescript_regex() {
    let m = |t: &str| mentions(t, true, &[users()], &[query()], &[dashboard()]);
    // `@"` without a closing quote falls back to `@\S+`.
    assert_eq!(m(r#"@"Monthly revenue"#), r#"@"Monthly revenue"#);
    // A non-breaking space ends a token, as `\s` does in JavaScript.
    assert!(m("@users\u{a0}now").contains("Table: public.users"));
    // U+0085 isn't JavaScript whitespace: it stays in the token.
    assert_eq!(m("@users\u{85}"), "@users\u{85}");
    assert_eq!(m("@ users"), "@ users");
    assert_eq!(m("@"), "@");
}

#[test]
fn the_system_prompt_names_only_the_tools_offered() {
    let ctx = schema_context(&[users()], 1024);
    let on = Sharing {
        schema: true,
        data: true,
    };
    let both = system(SqlEngine::Duckdb, Some(&ctx), on, true, false);
    assert!(both.contains("Table: public.users"));
    assert!(both.contains(
        "and list_saved_queries to see the project's saved queries. Use run_query to run"
    ));
    let generate = system(SqlEngine::Duckdb, Some(&ctx), on, false, false);
    assert!(generate.contains("Table: public.users"));
    assert!(!generate.contains("list_tables") && !generate.contains("run_query"));
    assert!(generate.starts_with("You are a helpful SQL assistant for a DuckDB database."));
    let off = Sharing {
        schema: false,
        data: false,
    };
    let none = system(SqlEngine::Mssql, None, off, true, false);
    assert_eq!(
        none,
        "You are a helpful SQL assistant for a SQL Server database. Always use SQL Server-compatible syntax.\n\nProvide clear, concise SQL queries and explanations. When writing SQL, wrap it in a markdown code block."
    );
}

#[test]
fn the_system_prompt_leaves_the_schema_out_without_schema_sharing() {
    let ctx = schema_context(&[users()], 1024);
    for tools in [false, true] {
        let data_only = Sharing {
            schema: false,
            data: true,
        };
        let prompt = system(SqlEngine::Postgres, Some(&ctx), data_only, tools, false);
        assert!(!prompt.contains("Database schema"), "{prompt}");
        assert!(!prompt.contains("users"), "{prompt}");
        assert_eq!(prompt.contains("Use run_query"), tools);
    }
}

#[test]
fn a_schema_context_cut_names_what_it_left_out() {
    let tables: Vec<SchemaTable> = (0..5)
        .map(|i| {
            let mut t = users();
            t.name = format!("t{i}");
            t
        })
        .collect();
    let one = schema_context(&tables[..1], 1024).text;
    let fit = one.len() + 2;
    let cut = schema_context(&tables, fit);
    assert_eq!((cut.kept, cut.left), (1, 4));
    assert_eq!(
        cut.text,
        format!(
            "{one}\n\n(4 more tables not shown; use list_tables and describe_table to see them.)"
        )
    );
    let none = schema_context(&tables, 5);
    assert_eq!((none.kept, none.left), (0, 5));
    assert_eq!(
        none.text,
        "Database schema:\n\n(5 more tables not shown; use list_tables and describe_table to see them.)"
    );
    assert_eq!(schema_context(&[], 1024).text, "");
}

// ── History ────────────────────────────────────────────────────────────────

fn user(id: &str, content: &str) -> HistoryRow {
    HistoryRow {
        id: id.into(),
        role: Role::User,
        content: content.into(),
        parts: None,
        dashboard_id: None,
    }
}

fn reply(id: &str, parts: Vec<Part>) -> HistoryRow {
    HistoryRow {
        id: id.into(),
        role: Role::Assistant,
        content: String::new(),
        parts: Some(parts),
        dashboard_id: None,
    }
}

fn text(round: u32, t: &str) -> Part {
    Part::Text {
        round,
        text: t.into(),
    }
}

fn tool(round: u32, id: &str, ok: bool, result: &str) -> Part {
    Part::Tool {
        round,
        call_id: id.into(),
        name: "run_query".into(),
        input: json!({"sql": "SELECT 1", "max_rows": 5}),
        ok,
        result: result.into(),
        result_bytes: None,
    }
}

fn bodies(messages: Vec<Message>) -> (Json, Json) {
    let body = |kind| {
        let p = Provider {
            kind,
            base_url: None,
            model: "m".into(),
        };
        let r = round_request(
            &p,
            None,
            &Round {
                system: "s".into(),
                messages: messages.clone(),
                tools: vec![],
            },
            false,
        );
        let v: Json = serde_json::from_slice(&r.body).unwrap();
        v["messages"].clone()
    };
    (
        body(ProviderKind::Anthropic),
        body(ProviderKind::OpenAiCompatible),
    )
}

#[test]
fn stored_parts_go_back_as_their_rounds() {
    let mut cut = tool(1, "c2", true, "");
    if let Part::Tool {
        result,
        result_bytes,
        ..
    } = &mut cut
    {
        *result = "x".repeat(10);
        *result_bytes = Some(20_000);
    }
    let rows = vec![
        user("u1", "How many?"),
        reply(
            "r1",
            vec![
                text(0, "Let me look. "),
                tool(0, "c1", false, "QUERY_ERROR: nope"),
                cut,
                text(2, "There are 3."),
            ],
        ),
        user("u2", "Thanks"),
    ];
    let h = history(&rows, 1 << 20);
    assert_eq!(h.kept.len(), 3);
    let (anthropic, openai) = bodies(h.messages);
    assert_eq!(
        anthropic,
        json!([
            {"role": "user", "content": "How many?"},
            {"role": "assistant", "content": [
                {"type": "text", "text": "Let me look. "},
                {"type": "tool_use", "id": "c1", "name": "run_query", "input": {"max_rows": 5, "sql": "SELECT 1"}},
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "c1", "content": "QUERY_ERROR: nope", "is_error": true},
            ]},
            {"role": "assistant", "content": [
                {"type": "tool_use", "id": "c2", "name": "run_query", "input": {"max_rows": 5, "sql": "SELECT 1"}},
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "c2", "content": "xxxxxxxxxx\n(cut at 16 KB of 20000 bytes)"},
            ]},
            {"role": "assistant", "content": "There are 3."},
            {"role": "user", "content": "Thanks"},
        ])
    );
    // OpenAI's first message is the system prompt.
    assert_eq!(
        openai[2]["tool_calls"][0]["function"]["arguments"],
        "{\"max_rows\":5,\"sql\":\"SELECT 1\"}"
    );
    assert_eq!(
        openai[3],
        json!({"role": "tool", "tool_call_id": "c1", "content": "Error: QUERY_ERROR: nope"})
    );
    assert_eq!(
        openai[4].get("content"),
        None,
        "a round with calls and no text"
    );
}

#[test]
fn a_reply_without_parts_is_its_text() {
    let mut r = reply("r1", vec![]);
    r.parts = None;
    r.content = "Hello".into();
    let h = history(&[user("u1", "Hi"), r], 1 << 20);
    assert_eq!(
        h.messages,
        vec![
            Message::User("Hi".into()),
            Message::Assistant {
                text: "Hello".into(),
                tool_calls: vec![]
            }
        ]
    );
    assert_eq!(h.bytes, 7);
}

#[test]
fn the_budget_never_splits_a_round() {
    let big = "y".repeat(1000);
    let rows = vec![
        user("u1", "old"),
        reply("r1", vec![text(0, "a"), tool(0, "c1", true, "x")]),
        user("u2", "q"),
        reply(
            "r2",
            vec![
                tool(0, "c2", true, &big),
                tool(0, "c3", true, &big),
                tool(1, "c4", true, &big),
                text(2, "done"),
            ],
        ),
    ];
    // Room for the question, round 2 and round 1, not round 0's two calls.
    let budget = 1 + 4 + 2 * (1000 + 25);
    let h = history(&rows, budget);
    assert_eq!(
        h.kept
            .iter()
            .map(|k| (k.id.as_str(), k.rounds.clone()))
            .collect::<Vec<_>>(),
        [("u2", None), ("r2", Some(vec![1, 2]))]
    );
    // Every call in the messages has its result right after it.
    let mut open: Vec<String> = Vec::new();
    for m in &h.messages {
        match m {
            Message::Assistant { tool_calls, .. } => {
                assert!(open.is_empty());
                open = tool_calls.iter().map(|c| c.id.clone()).collect();
            }
            Message::ToolResults(results) => {
                assert_eq!(
                    results
                        .iter()
                        .map(|r| r.call_id.clone())
                        .collect::<Vec<_>>(),
                    open
                );
                open.clear();
            }
            Message::User(_) => assert!(open.is_empty()),
        }
    }
    assert!(open.is_empty());
    assert!(h.bytes <= budget);
    // Nothing fits: the turn goes whole.
    let h = history(&rows, 3);
    assert!(h.kept.is_empty() && h.messages.is_empty() && h.bytes == 0);
}

#[test]
fn a_tool_part_keeps_16_kb_of_its_result() {
    let long = format!("{}é", "a".repeat(PART_RESULT_BYTES - 1));
    let out = ToolOutput {
        text: long.clone(),
        is_error: false,
    };
    let p = Part::tool(3, "c", "run_query", json!({"sql": "SELECT 1"}), &out);
    let Part::Tool {
        round,
        result,
        result_bytes,
        ok,
        ..
    } = &p
    else {
        panic!()
    };
    assert_eq!((*round, *ok), (3, true));
    assert_eq!(
        result.len(),
        PART_RESULT_BYTES - 1,
        "cut on a character boundary"
    );
    assert_eq!(*result_bytes, Some(long.len()));
    let short = ToolOutput {
        text: "DENIED: User denied query execution".into(),
        is_error: true,
    };
    let p = Part::tool(0, "c", "run_query", json!({}), &short);
    assert_eq!(
        serde_json::to_value(&p).unwrap(),
        json!({"round": 0, "type": "tool", "callId": "c", "name": "run_query", "input": {},
               "ok": false, "result": "DENIED: User denied query execution"})
    );
}

#[test]
fn the_dashboard_line_goes_on_the_typed_message() {
    let line = "\n\n[Context: The active dashboard ID is \"dash-1\". Use this ID for any dashboard tool calls.]";
    let mut with_dash = reply("r1", vec![text(0, "Made it.")]);
    with_dash.dashboard_id = Some("dash-1".into());
    let rows = vec![
        user("u1", "Make one"),
        with_dash,
        user("u2", "Thanks"),
        reply("r2", vec![text(0, "Ok")]),
    ];
    assert_eq!(last_dashboard_id(&rows), Some("dash-1"));
    assert_eq!(last_dashboard_id(&rows[..1]), None);

    // A round in progress: the last message is a round's tool results.
    let mut messages = history(&rows, 1 << 20).messages;
    messages.push(Message::User("Add a widget".into()));
    messages.push(Message::Assistant {
        text: String::new(),
        tool_calls: vec![],
    });
    messages.push(Message::ToolResults(vec![]));
    with_dashboard_context(&mut messages, "dash-1");
    assert_eq!(messages[4], Message::User(format!("Add a widget{line}")));
    assert_eq!(
        messages[2],
        Message::User("Thanks".into()),
        "only the last typed one"
    );
}

#[test]
fn the_dashboard_line_is_added_once() {
    let mut messages = vec![Message::User("Add a widget".into())];
    with_dashboard_context(&mut messages, "dash-1");
    let once = messages.clone();
    with_dashboard_context(&mut messages, "dash-1");
    assert_eq!(messages, once);
    // Another id replaces nothing: the line says the latest one only once.
    with_dashboard_context(&mut messages, "dash-2");
    let Message::User(text) = &messages[0] else {
        panic!()
    };
    assert_eq!(text.matches("[Context:").count(), 2);
    assert!(text.ends_with("\"dash-2\". Use this ID for any dashboard tool calls.]"));
}

/// Anthropic answers 400 to an assistant message with empty content, so a
/// reply stored with nothing in it (Stop or an error before any text) is
/// left out, and the questions on either side of it become one message.
#[test]
fn an_empty_reply_is_left_out_and_the_questions_joined() {
    let mut empty = reply("r1", vec![]);
    empty.parts = None;
    let only_blank_text = reply("r2", vec![text(0, "")]);
    let mut answer = reply("r3", vec![]);
    answer.parts = None;
    answer.content = "Answer.".into();
    let rows = vec![
        user("u1", "First"),
        empty,
        user("u2", "Second"),
        only_blank_text,
        user("u3", "Third"),
        answer,
    ];
    let h = history(&rows, 1 << 20);
    assert_eq!(h.kept.len(), 6, "the budget still counts every row");
    assert_eq!(
        h.messages,
        vec![
            Message::User("First\n\nSecond\n\nThird".into()),
            Message::Assistant {
                text: "Answer.".into(),
                tool_calls: vec![]
            },
        ]
    );
    // The turn's own message joins a history that ends with a question.
    let mut messages = history(&rows[..1], 1 << 20).messages;
    push_user(&mut messages, "Again".into());
    assert_eq!(messages, vec![Message::User("First\n\nAgain".into())]);
    let (anthropic, openai) = bodies(messages);
    for body in [anthropic, openai] {
        for pair in body.as_array().unwrap().windows(2) {
            assert!(
                !(pair[0]["role"] == "user" && pair[1]["role"] == "user"),
                "{body}"
            );
        }
        for m in body.as_array().unwrap() {
            assert!(!(m["role"] == "assistant" && m["content"] == ""), "{body}");
        }
    }
    let mut messages = vec![];
    push_user(&mut messages, "Hi".into());
    assert_eq!(messages, vec![Message::User("Hi".into())]);
}

#[test]
fn mentions_resolve_at_most_a_hundred_names() {
    let tables: Vec<SchemaTable> = (0..150)
        .map(|i| {
            let mut t = users();
            t.name = format!("t{i}");
            t
        })
        .collect();
    let text: String = (0..150).map(|i| format!("@t{i} ")).collect();
    let out = mentions(&text, true, &tables, &[], &[]);
    assert_eq!(MAX_MENTIONS, 100);
    assert_eq!(out.matches("\nTable: public.").count(), 100);
    assert!(out.contains("Table: public.t99 "));
    assert!(!out.contains("Table: public.t100 "));
}

#[test]
fn mentions_over_a_large_schema_and_message_are_fast() {
    let tables: Vec<SchemaTable> = (0..3000)
        .map(|i| {
            let mut t = users();
            t.name = format!("table_{i:04}");
            t
        })
        .collect();
    let queries: Vec<MentionQuery> = (0..3000)
        .map(|i| MentionQuery {
            name: format!("query {i}"),
            query: "SELECT 1".into(),
        })
        .collect();
    // 1 MiB of mentions, none of them known past the first few.
    let mut text = String::from("@table_0001 @public.table_0002 ");
    let mut i = 0;
    while text.len() < 1 << 20 {
        text.push_str(&format!("@nothing_{i} "));
        i += 1;
    }
    let start = std::time::Instant::now();
    let out = mentions(&text, true, &tables, &queries, &[]);
    let took = start.elapsed();
    assert_eq!(out.matches("\nTable: public.").count(), 2);
    assert!(took.as_millis() < 1000, "{took:?}");
}
