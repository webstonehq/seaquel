//! Core runs a turn (phase 6 Task 4): the loop, approvals and client
//! tools, keys, the chat writes, timeouts and cancels, against the mock
//! provider. The fixture replays are in `ai_replay.rs`.

#![cfg(all(feature = "ai", feature = "ai-native"))]
// Native-only tests: the shared helpers and the timeout test read the
// system clock.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

mod ai_support;
mod common;

use std::sync::Arc;
use std::time::Duration;

use ai_support::*;
use futures::StreamExt;
use seaquel_ai::testing::scripts::{anthropic_max_tokens, anthropic_text, anthropic_tools, joined};
use seaquel_ai::testing::{Reply, SseEnd, TEST_KEY};
use seaquel_core::ai::{
    AiDecision, AiEgress, AiEvent, AiLimits, AiStop, Approval, ApprovalDecision, ClientResult,
};
use seaquel_core::{StateLimits, StoredKind, WorkspaceEvent};
use serde_json::json;

fn allow() -> Answer {
    Answer::Decide(AiDecision::Approval(ApprovalDecision::Allow))
}

/// A world with the chat `chat-1` on `conn-1`, open, and data sharing on.
async fn chat_world(setup: Setup) -> (World, String) {
    let w = world(setup).await;
    sqlx::query("UPDATE connections SET ai_share_data = 1")
        .execute(w.ws.storage().pool())
        .await
        .unwrap();
    let id = w.connect("conn-1").await;
    w.chat("chat-1", "conn-1").await;
    (w, id)
}

fn reply_sse(events: Vec<String>) -> Reply {
    sse(&joined(&events), SseEnd::Finish)
}

fn query_call(id: &str, sql: &str) -> (String, String, String) {
    (
        id.to_string(),
        "run_query".to_string(),
        json!({ "sql": sql }).to_string(),
    )
}

fn tools(text: &str, calls: &[(String, String, String)]) -> Reply {
    let calls: Vec<(&str, &str, &str)> = calls
        .iter()
        .map(|(a, b, c)| (a.as_str(), b.as_str(), c.as_str()))
        .collect();
    reply_sse(anthropic_tools(text, &calls))
}

// ── A plain turn ──

#[tokio::test]
async fn a_turn_streams_stores_two_rows_and_ends_done() {
    common::capture_logs();
    let (w, id) = chat_world(Setup::default()).await;
    w.mock
        .reply(reply_sse(anthropic_text("Hello from the model.")));
    let mut changes = w.ws.events();
    let events = run_turn(&w, params("s1", &id, "Say hello please"), |_| {
        Answer::Nothing
    })
    .await;

    assert!(
        matches!(&events[0], AiEvent::Started { provider_kind, model }
        if provider_kind == "anthropic" && model == "model-1")
    );
    assert_eq!(text_of(&events), "Hello from the model.");
    let Some(AiEvent::Done {
        messages,
        seq,
        stop,
    }) = terminal(&events)
    else {
        panic!("{events:?}")
    };
    assert_eq!(*stop, AiStop::End);
    assert_eq!(messages.len(), 2);
    assert_eq!(
        (messages[0].id.as_str(), messages[0].role.as_str()),
        ("s1-u", "user")
    );
    assert_eq!(messages[0].content, "Say hello please");
    assert_eq!(
        (messages[1].id.as_str(), messages[1].role.as_str()),
        ("s1-a", "assistant")
    );
    assert_eq!(messages[1].content, "Hello from the model.");
    assert_eq!(messages[1].parts, None, "no tool call: no parts");
    // Exactly what is stored, in order.
    assert_eq!(&w.messages("chat-1").await, messages);
    // Two writes, a `chatMessages` event each, the reply's sequence on
    // `done`; the reply's write also moved the chat, so a `chat` event
    // follows it.
    let mut seqs = Vec::new();
    let mut kinds = Vec::new();
    for e in drain(&mut changes) {
        let WorkspaceEvent::StorageChanged(c) = e else {
            continue;
        };
        assert_eq!(c.origin.as_deref(), Some("win-test"));
        kinds.push(c.kind);
        match c.kind {
            StoredKind::ChatMessages => {
                assert_eq!(c.scope.as_deref(), Some("chat-1"));
                seqs.push(c.seq);
            }
            StoredKind::Chat => {
                assert_eq!(c.scope.as_deref(), Some("conn-1"));
                assert_eq!(c.ids, Some(vec!["chat-1".to_string()]));
            }
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(
        kinds,
        [
            StoredKind::ChatMessages,
            StoredKind::ChatMessages,
            StoredKind::Chat
        ]
    );
    assert_eq!(&seqs[1], seq);
    assert!(events.last().unwrap().is_terminal());
    assert_eq!(events.iter().filter(|e| e.is_terminal()).count(), 1);
    // The chat moves up the list.
    let chat = seaquel_core::storage::ai_chats::get(w.ws.storage(), "chat-1")
        .await
        .unwrap()
        .unwrap();
    assert_ne!(chat.updated_at, T0);
}

/// The user's message is stored before the first round, while the model
/// still streams.
#[tokio::test]
async fn the_user_message_is_stored_before_the_first_round() {
    let (w, id) = chat_world(Setup::default()).await;
    w.mock.reply(Reply::Hang);
    let mut stream = w.ws.ai_chat(
        &w.core,
        params("s1", &id, "A question first"),
        seaquel_core::WriteOrigin::none(),
    );
    let started = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(started, AiEvent::Started { .. }));
    // The round is in flight; give it a moment to reach the mock.
    for _ in 0..100 {
        if !w.mock.requests().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let rows = w.messages("chat-1").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].content, "A question first");
    drop(stream);
}

// ── Tools ──

#[tokio::test]
async fn every_call_of_a_round_runs_in_order_and_parts_are_stored() {
    let (w, id) = chat_world(Setup::default()).await;
    w.db.answers.lock().unwrap().insert(
        "SELECT 2 AS n".into(),
        json!({"columns": ["n"], "rows": [[2]]}),
    );
    w.mock
        .reply(tools(
            "Two. ",
            &[
                query_call("c1", "SELECT 1 AS n"),
                query_call("c2", "SELECT 2 AS n"),
            ],
        ))
        .reply(tools("Three. ", &[query_call("c3", "SELECT 3 AS n")]))
        .reply(reply_sse(anthropic_text("Done.")));
    let mut p = params("s1", &id, "Run three");
    p.approval = Approval::AllowAll;
    let events = run_turn(&w, p, |_| Answer::Nothing).await;
    let calls: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            AiEvent::ToolCall { call_id, .. } => Some(call_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(calls, ["c1", "c2", "c3"]);
    let dones: Vec<(&str, bool, Option<u64>)> = events
        .iter()
        .filter_map(|e| match e {
            AiEvent::ToolDone {
                call_id, ok, rows, ..
            } => Some((call_id.as_str(), *ok, *rows)),
            _ => None,
        })
        .collect();
    assert_eq!(
        dones,
        [
            ("c1", true, Some(1)),
            ("c2", true, Some(1)),
            ("c3", true, Some(1))
        ]
    );
    let sqls: Vec<String> = w.db.ran().into_iter().map(|r| r.sql).collect();
    assert_eq!(sqls, ["SELECT 1 AS n", "SELECT 2 AS n", "SELECT 3 AS n"]);
    // Each query: 100 rows, 8 MiB, 60 s.
    for r in w.db.ran() {
        assert_eq!(
            (r.max_rows, r.max_bytes, r.timeout_ms),
            (Some(100), Some(8 * 1024 * 1024), Some(60_000))
        );
    }
    // The second request carries both calls and both results, in order.
    let second = &w.http.sent()[1].body["messages"];
    let ids: Vec<&str> = second[2]["content"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["tool_use_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["c1", "c2"]);
    let rows = w.messages("chat-1").await;
    let parts = rows[1].parts.as_ref().unwrap().as_array().unwrap();
    let shape: Vec<(u64, &str)> = parts
        .iter()
        .map(|p| (p["round"].as_u64().unwrap(), p["type"].as_str().unwrap()))
        .collect();
    assert_eq!(
        shape,
        [
            (0, "text"),
            (0, "tool"),
            (0, "tool"),
            (1, "text"),
            (1, "tool"),
            (2, "text")
        ]
    );
    assert_eq!(rows[1].content, "Two. Three. Done.");
}

#[tokio::test]
async fn the_twenty_first_call_ends_the_turn_before_it_runs() {
    let (w, id) = chat_world(Setup::default()).await;
    // 20 calls in one round, then a 21st in the next.
    let twenty: Vec<_> = (1..=20)
        .map(|i| query_call(&format!("c{i}"), &format!("SELECT {i} AS n")))
        .collect();
    w.mock
        .reply(tools("", &twenty))
        .reply(tools("", &[query_call("c21", "SELECT 21 AS n")]));
    let mut p = params("s1", &id, "Loop forever");
    p.approval = Approval::AllowAll;
    let events = run_turn(&w, p, |_| Answer::Nothing).await;
    assert_eq!(w.db.ran().len(), 20);
    let Some(AiEvent::Error { code, messages, .. }) = terminal(&events) else {
        panic!("{events:?}")
    };
    assert_eq!(code, "TOOL_LIMIT");
    let reply = &messages.as_ref().unwrap()[1];
    let parts = reply.parts.as_ref().unwrap().as_array().unwrap();
    assert_eq!(parts.iter().filter(|p| p["type"] == "tool").count(), 20);
    assert_eq!(&w.messages("chat-1").await[1], reply);
}

// ── Approvals ──

#[tokio::test]
async fn approvals_allow_deny_and_allow_all_mid_turn() {
    let (w, id) = chat_world(Setup::default()).await;
    w.mock
        .reply(tools("", &[query_call("c1", "SELECT 1 AS n")]))
        .reply(tools("", &[query_call("c2", "SELECT 2 AS n")]))
        .reply(tools("", &[query_call("c3", "SELECT 3 AS n")]))
        .reply(tools("", &[query_call("c4", "SELECT 4 AS n")]))
        .reply(reply_sse(anthropic_text("End.")));
    let mut asked = Vec::new();
    let events = run_turn(&w, params("s1", &id, "Four queries"), |e| match e {
        AiEvent::ApprovalRequired { call_id, sql } => {
            asked.push(sql.clone());
            match call_id.as_str() {
                "c1" => allow(),
                "c2" => Answer::Decide(AiDecision::Approval(ApprovalDecision::Deny)),
                _ => Answer::Decide(AiDecision::Approval(ApprovalDecision::AllowAll)),
            }
        }
        _ => Answer::Nothing,
    })
    .await;
    // c3's "allow all" covers c4.
    assert_eq!(asked, ["SELECT 1 AS n", "SELECT 2 AS n", "SELECT 3 AS n"]);
    let sqls: Vec<String> = w.db.ran().into_iter().map(|r| r.sql).collect();
    assert_eq!(sqls, ["SELECT 1 AS n", "SELECT 3 AS n", "SELECT 4 AS n"]);
    let denied = events.iter().find_map(|e| match e {
        AiEvent::ToolDone { call_id, code, .. } if call_id == "c2" => code.clone(),
        _ => None,
    });
    assert_eq!(denied.as_deref(), Some("DENIED"));
    let third = &w.http.sent()[2].body["messages"];
    let result = &third.as_array().unwrap().last().unwrap()["content"][0];
    assert_eq!(result["content"], "DENIED: User denied query execution");
    assert_eq!(result["is_error"], true);
    assert!(matches!(terminal(&events), Some(AiEvent::Done { .. })));
}

#[tokio::test]
async fn respond_reaches_only_this_workspaces_turn_and_only_once() {
    let (w, id) = chat_world(Setup::default()).await;
    let other = w
        .core
        .open_workspace(seaquel_core::WorkspaceSpec::new(w.dir.path().join("other")))
        .await
        .unwrap();
    w.mock
        .reply(tools("", &[query_call("c1", "SELECT 1 AS n")]))
        .reply(reply_sse(anthropic_text("End.")));
    let ws = w.ws.clone();
    let events = run_turn(&w, params("s1", &id, "One query"), |e| match e {
        AiEvent::ApprovalRequired { call_id, .. } => {
            // Another workspace can't answer it.
            let err = other
                .ai_respond("s1", call_id, AiDecision::Approval(ApprovalDecision::Allow))
                .unwrap_err();
            assert_eq!(err.code, "NOT_FOUND");
            // An unknown call in this turn isn't found either.
            let err = ws
                .ai_respond("s1", "nope", AiDecision::Approval(ApprovalDecision::Allow))
                .unwrap_err();
            assert_eq!(err.code, "NOT_FOUND");
            ws.ai_respond("s1", call_id, AiDecision::Approval(ApprovalDecision::Allow))
                .unwrap();
            // A second answer is ignored.
            Answer::Decide(AiDecision::Approval(ApprovalDecision::Allow))
        }
        _ => Answer::Nothing,
    })
    .await;
    assert_eq!(w.db.ran().len(), 1, "the query ran once");
    assert!(matches!(terminal(&events), Some(AiEvent::Done { .. })));
    // After the turn, its stream is unknown.
    let err =
        w.ws.ai_respond("s1", "c1", AiDecision::Approval(ApprovalDecision::Allow))
            .unwrap_err();
    assert_eq!(err.code, "NOT_FOUND");
    assert_eq!(w.ws.ai_waiter_count(), 0);
}

#[tokio::test]
async fn a_cancel_while_waiting_stores_what_streamed_and_leaves_no_waiter() {
    let (w, id) = chat_world(Setup::default()).await;
    w.mock
        .reply(tools("Checking. ", &[query_call("c1", "SELECT 1 AS n")]));
    let ws = w.ws.clone();
    let events = run_turn(&w, params("s1", &id, "Count something"), |e| match e {
        AiEvent::ApprovalRequired { .. } => {
            assert_eq!(ws.ai_waiter_count(), 1);
            Answer::Cancel
        }
        _ => Answer::Nothing,
    })
    .await;
    assert!(terminal(&events).is_none(), "nothing follows a cancel");
    assert_eq!(w.ws.ai_waiter_count(), 0);
    assert_eq!(w.ws.ai_turn_count(), 0);
    assert!(w.db.ran().is_empty());
    let rows = w.messages("chat-1").await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].content, "Checking. ");
    assert_eq!(
        rows[1].parts, None,
        "a call Stop left unanswered isn't stored"
    );
}

// ── Client tools ──

fn create_dashboard(id: &str) -> (String, String, String) {
    (
        id.to_string(),
        "create_dashboard".to_string(),
        json!({"name": "Sales"}).to_string(),
    )
}

#[tokio::test]
async fn client_tools_are_answered_by_the_page() {
    let (w, id) = chat_world(Setup::default()).await;
    w.mock
        .reply(tools("", &[create_dashboard("c1")]))
        .reply(tools("", &[create_dashboard("c2")]))
        .reply(reply_sse(anthropic_text("Made.")));
    let mut p = params("s1", &id, "Make a dashboard");
    p.client_tools = true;
    let mut seen = Vec::new();
    let events = run_turn(&w, p, |e| match e {
        AiEvent::ClientTool {
            call_id,
            name,
            input,
        } => {
            seen.push((call_id.clone(), name.clone(), input.clone()));
            Answer::Decide(AiDecision::Client(if call_id == "c1" {
                ClientResult {
                    result: json!({"dashboard_id": "dash-9"}).to_string(),
                    is_error: false,
                }
            } else {
                ClientResult {
                    result: "The page refused it".into(),
                    is_error: true,
                }
            }))
        }
        _ => Answer::Nothing,
    })
    .await;
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].2, json!({"name": "Sales"}));
    let sent = w.http.sent();
    let r1 = &sent[1].body["messages"].as_array().unwrap().last().unwrap()["content"][0];
    assert_eq!(r1["content"], r#"{"dashboard_id":"dash-9"}"#);
    assert!(r1.get("is_error").is_none());
    let r2 = &sent[2].body["messages"].as_array().unwrap().last().unwrap()["content"][0];
    assert_eq!(r2["content"], "The page refused it");
    assert_eq!(r2["is_error"], true);
    let rows = w.messages("chat-1").await;
    assert_eq!(rows[1].dashboard_id.as_deref(), Some("dash-9"));
    assert!(matches!(terminal(&events), Some(AiEvent::Done { .. })));
}

#[tokio::test]
async fn a_client_tool_never_answered_ends_with_the_cancel() {
    let (w, id) = chat_world(Setup::default()).await;
    w.mock.reply(tools("Making. ", &[create_dashboard("c1")]));
    let mut p = params("s1", &id, "Make a dashboard");
    p.client_tools = true;
    let events = run_turn(&w, p, |e| match e {
        AiEvent::ClientTool { .. } => Answer::Cancel,
        _ => Answer::Nothing,
    })
    .await;
    assert!(terminal(&events).is_none());
    assert_eq!(w.ws.ai_waiter_count(), 0);
    assert_eq!(w.messages("chat-1").await[1].content, "Making. ");
}

// ── Sharing, connections ──

#[tokio::test]
async fn a_turn_on_another_saved_connections_connection_is_refused() {
    let mut setup = Setup::default();
    setup.conns.push(Conn::new("conn-2", "postgres"));
    let (w, _) = chat_world(setup).await;
    let other = w.connect("conn-2").await;
    let events = run_turn(&w, params("s1", &other, "Hello there"), |_| Answer::Nothing).await;
    assert!(
        matches!(terminal(&events), Some(AiEvent::Error { code, messages: None, .. }) if code == "CONNECTION_MISMATCH"),
        "{events:?}"
    );
    assert!(w.messages("chat-1").await.is_empty());
    assert!(w.http.sent().is_empty());
}

#[tokio::test]
async fn a_reconnect_mid_turn_fails_the_next_tool_call_only() {
    let (w, id) = chat_world(Setup::default()).await;
    w.mock
        .reply(tools("", &[query_call("c1", "SELECT 1 AS n")]))
        .reply(reply_sse(anthropic_text("It went away.")));
    let ws = w.ws.clone();
    let core = &w.core;
    let mut p = params("s1", &id, "Query once");
    p.approval = Approval::Ask;
    let mut stream = ws.ai_chat(core, p, seaquel_core::WriteOrigin::none());
    let mut events = Vec::new();
    while let Some(e) = stream.next().await {
        if let AiEvent::ApprovalRequired { call_id, .. } = &e {
            ws.disconnect(core, &id).await.unwrap();
            ws.ai_respond("s1", call_id, AiDecision::Approval(ApprovalDecision::Allow))
                .unwrap();
        }
        events.push(e);
    }
    let code = events.iter().find_map(|e| match e {
        AiEvent::ToolDone { code, .. } => code.clone(),
        _ => None,
    });
    assert_eq!(code.as_deref(), Some("CONNECTION_NOT_FOUND"));
    assert!(matches!(terminal(&events), Some(AiEvent::Done { .. })));
}

// ── Refusals, in order, with nothing stored ──

async fn refused(setup: Setup, p: impl FnOnce(&str) -> seaquel_core::ai::ChatParams) -> String {
    let (w, id) = chat_world(setup).await;
    let events = run_turn(&w, p(&id), |_| Answer::Nothing).await;
    assert!(w.messages("chat-1").await.is_empty(), "nothing stored");
    assert!(w.http.sent().is_empty(), "no model call");
    match &events[..] {
        [AiEvent::Error {
            code,
            messages: None,
            seq: None,
            ..
        }] => code.clone(),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn core_refusals_store_nothing_and_call_nothing() {
    let p = |id: &str| params("s1", id, "Anything at all");
    // No provider configured at all, and one the connection names that's gone.
    let s = Setup {
        providers: vec![],
        ..Setup::default()
    };
    assert_eq!(refused(s, p).await, "NO_PROVIDER");
    // No key: in the keychain, or supplied.
    let s = Setup {
        key: Some(None),
        ..Setup::default()
    };
    assert_eq!(refused(s, p).await, "NO_API_KEY");
    let s = Setup {
        key: None,
        ..Setup::default()
    };
    assert_eq!(refused(s, p).await, "NO_API_KEY");
    // No model.
    let mut s = Setup::default();
    s.conns[0].model = None;
    assert_eq!(refused(s, p).await, "NO_MODEL");
    // The assistant turned off.
    let s = Setup {
        enabled: false,
        ..Setup::default()
    };
    assert_eq!(refused(s, p).await, "AI_DISABLED");
    // No client or no egress policy, and egress off.
    let s = Setup {
        http: false,
        ..Setup::default()
    };
    assert_eq!(refused(s, p).await, "NOT_SUPPORTED");
    let s = Setup {
        egress: None,
        ..Setup::default()
    };
    assert_eq!(refused(s, p).await, "NOT_SUPPORTED");
    let s = Setup {
        egress: Some(AiEgress::Off),
        ..Setup::default()
    };
    assert_eq!(refused(s, p).await, "AI_EGRESS_BLOCKED");
    // The message past the web's cap.
    let s = Setup {
        limits: AiLimits {
            max_message_bytes: Some(8),
            ..AiLimits::default()
        },
        ..Setup::default()
    };
    // Its own code (F1/F2/F5 review P1), worded by the page.
    assert_eq!(refused(s, p).await, "MESSAGE_TOO_LONG");
    // A chat that can't take the turn.
    let s = Setup {
        state_limits: StateLimits {
            max_messages_per_chat: Some(1),
            ..common::web_state_limits()
        },
        ..Setup::default()
    };
    assert_eq!(refused(s, p).await, "CHAT_FULL");
    // The user's message and the reply under one id.
    assert_eq!(
        refused(Setup::default(), |id| {
            let mut p = params("s1", id, "Anything at all");
            p.assistant_message_id = p.user_message.id.clone();
            p
        })
        .await,
        "INVALID_ARGUMENT"
    );
    // An unknown chat.
    assert_eq!(
        refused(Setup::default(), |id| {
            let mut p = params("s1", id, "Anything at all");
            p.chat_id = "nope".into();
            p
        })
        .await,
        "CHAT_NOT_FOUND"
    );
}

#[tokio::test]
async fn a_supplied_key_wins_over_the_keychain() {
    let s = Setup {
        key: Some(Some("the-keychain-key".into())),
        ..Setup::default()
    };
    let (w, id) = chat_world(s).await;
    w.mock.reply(reply_sse(anthropic_text("Hi.")));
    let mut p = params("s1", &id, "Hello there");
    p.api_key = Some(TEST_KEY.into());
    p.provider_id = Some("prov-1".into());
    run_turn(&w, p, |_| Answer::Nothing).await;
    assert_eq!(w.mock.requests()[0].header("x-api-key"), Some(TEST_KEY));
    // Without one, the keychain's.
    w.mock.reply(reply_sse(anthropic_text("Hi.")));
    run_turn(&w, params("s2", &id, "Hello again"), |_| Answer::Nothing).await;
    assert_eq!(
        w.mock.requests()[1].header("x-api-key"),
        Some("the-keychain-key")
    );
}

#[tokio::test]
async fn turns_past_the_cap_are_too_many_requests() {
    let s = Setup {
        limits: AiLimits {
            max_turns_in_flight: Some(1),
            ..AiLimits::default()
        },
        ..Setup::default()
    };
    let (w, id) = chat_world(s).await;
    w.mock.reply(Reply::Hang);
    let mut first = w.ws.ai_chat(
        &w.core,
        params("s1", &id, "First turn"),
        seaquel_core::WriteOrigin::none(),
    );
    assert!(matches!(first.next().await, Some(AiEvent::Started { .. })));
    let second = run_turn(&w, params("s2", &id, "Second turn"), |_| Answer::Nothing).await;
    assert!(
        matches!(&second[..], [AiEvent::Error { code, .. }] if code == "TOO_MANY_REQUESTS"),
        "{second:?}"
    );
    drop(first);
    assert_eq!(w.ws.ai_turn_count(), 0);
}

// ── Endings ──

#[tokio::test]
async fn max_tokens_is_stored_and_done_says_so() {
    let (w, id) = chat_world(Setup::default()).await;
    w.mock.reply(reply_sse(anthropic_max_tokens("A cut answ")));
    let events = run_turn(&w, params("s1", &id, "Write a lot"), |_| Answer::Nothing).await;
    assert!(matches!(
        terminal(&events),
        Some(AiEvent::Done {
            stop: AiStop::MaxTokens,
            ..
        })
    ));
    assert_eq!(w.messages("chat-1").await[1].content, "A cut answ");
}

/// A provider that streams `delta` again and again until the client goes
/// (probe F2: a misbehaving provider), after the start of a text block.
fn endless_text(delta: &str) -> Reply {
    use seaquel_ai::testing::scripts::anthropic_event;
    let start = anthropic_text("")[..2].to_vec();
    Reply::Sse {
        events: start,
        piece: 1 << 16,
        gap: Duration::ZERO,
        end: SseEnd::Repeat {
            event: anthropic_event(
                "content_block_delta",
                json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":delta}}),
            ),
            every: Duration::from_millis(1),
        },
    }
}

/// The reply as stored when it was cut at `cap` bytes: the text up to a
/// character boundary, then the note, within `cap`.
fn assert_cut(stored: &str, streamed: &str, cap: usize) {
    use seaquel_core::ai::REPLY_CUT_NOTE;
    assert!(stored.len() <= cap, "{} > {cap}", stored.len());
    let text = stored
        .strip_suffix(REPLY_CUT_NOTE)
        .unwrap_or_else(|| panic!("no note at the end of the stored reply"));
    // Cut close to the limit (one character of slack), never past it.
    assert!(
        text.len() + REPLY_CUT_NOTE.len() + 4 > cap,
        "{}",
        text.len()
    );
    // The page saw exactly the text that was kept.
    assert_eq!(text, streamed);
}

/// Probe F2: a reply past the message limit (`max_message_bytes`, the
/// web's 1 MiB) is cut on a character boundary, stored with the note, and
/// the turn ends `tooLong`; Core stops reading the provider's stream.
#[tokio::test]
async fn a_reply_past_the_message_limit_is_cut_stored_and_ends_too_long() {
    const CAP: usize = 4_000;
    let s = Setup {
        state_limits: StateLimits {
            max_message_bytes: Some(CAP),
            ..common::web_state_limits()
        },
        ..Setup::default()
    };
    let (w, id) = chat_world(s).await;
    // Three bytes, then two: a cut on a byte count would split one.
    w.mock.reply(endless_text("ab\u{20ac}\u{e9}cd"));
    let events = run_turn(&w, params("s1", &id, "Write forever"), |_| Answer::Nothing).await;
    let Some(AiEvent::Done { messages, stop, .. }) = terminal(&events) else {
        panic!("{:?}", terminal(&events))
    };
    assert_eq!(*stop, AiStop::TooLong);
    let stored = w.messages("chat-1").await;
    assert_eq!(&stored, messages);
    assert_cut(&stored[1].content, &text_of(&events), CAP);
    assert!(w.mock.client_gone(Duration::from_secs(5)).await);
}

/// F1/F2/F5 review P3: a round that calls a tool and then says more than
/// the reply cap ends `tooLong` at the cut: the call never runs, no tool
/// event reaches the page, and the stored reply has no tool part.
#[tokio::test]
async fn a_tool_call_before_text_over_the_cap_runs_nothing() {
    use seaquel_ai::testing::scripts::anthropic_event;
    const CAP: usize = 4_000;
    let s = Setup {
        state_limits: StateLimits {
            max_message_bytes: Some(CAP),
            ..common::web_state_limits()
        },
        ..Setup::default()
    };
    let (w, id) = chat_world(s).await;
    let input = json!({"sql": "SELECT 1"}).to_string();
    let mut events = vec![
        anthropic_event(
            "message_start",
            json!({"type":"message_start","message":{"id":"msg_p3","type":"message","role":"assistant",
                   "content":[],"model":"m","stop_reason":null,"stop_sequence":null,
                   "usage":{"input_tokens":1,"output_tokens":1}}}),
        ),
        anthropic_event(
            "content_block_start",
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call-p3","name":"run_query","input":{}}}),
        ),
        anthropic_event(
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":input}}),
        ),
        anthropic_event(
            "content_block_stop",
            json!({"type":"content_block_stop","index":0}),
        ),
        anthropic_event(
            "content_block_start",
            json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
        ),
    ];
    for _ in 0..20 {
        events.push(anthropic_event(
            "content_block_delta",
            json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"z".repeat(500)}}),
        ));
    }
    events.push(anthropic_event(
        "content_block_stop",
        json!({"type":"content_block_stop","index":1}),
    ));
    events.push(anthropic_event(
        "message_delta",
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":9}}),
    ));
    events.push(anthropic_event(
        "message_stop",
        json!({"type":"message_stop"}),
    ));
    w.mock.reply(reply_sse(events));
    let got = run_turn(&w, params("s1", &id, "Query, then talk"), |_| {
        Answer::Nothing
    })
    .await;
    let Some(AiEvent::Done { stop, .. }) = terminal(&got) else {
        panic!("{:?}", terminal(&got))
    };
    assert_eq!(*stop, AiStop::TooLong);
    assert!(
        !got.iter().any(|e| matches!(
            e,
            AiEvent::ToolCall { .. }
                | AiEvent::ToolDone { .. }
                | AiEvent::ApprovalRequired { .. }
                | AiEvent::ClientTool { .. }
        )),
        "{got:?}"
    );
    let stored = w.messages("chat-1").await;
    assert_eq!(stored[1].parts, None, "{:?}", stored[1].parts);
    assert_cut(&stored[1].content, &text_of(&got), CAP);
    assert_eq!(w.mock.requests().len(), 1, "no second round");
}

/// With no message limit (desktop, demo) the reply is cut at Core's own
/// ceiling, 1 MiB.
#[tokio::test]
async fn without_a_limit_a_reply_is_cut_at_one_mib() {
    use seaquel_core::ai::limits::MAX_REPLY_BYTES;
    assert_eq!(MAX_REPLY_BYTES, 1024 * 1024);
    let (w, id) = chat_world(Setup::default()).await;
    w.mock.reply(endless_text(&"x".repeat(60_000)));
    let events = run_turn(&w, params("s1", &id, "Write forever"), |_| Answer::Nothing).await;
    assert!(
        matches!(
            terminal(&events),
            Some(AiEvent::Done {
                stop: AiStop::TooLong,
                ..
            })
        ),
        "{:?}",
        terminal(&events)
    );
    assert_cut(
        &w.messages("chat-1").await[1].content,
        &text_of(&events),
        MAX_REPLY_BYTES,
    );
    assert!(w.mock.client_gone(Duration::from_secs(5)).await);
}

#[tokio::test]
async fn a_provider_error_stores_what_streamed_and_carries_it() {
    let (w, id) = chat_world(Setup::default()).await;
    w.mock.reply(reply_sse(
        seaquel_ai::testing::scripts::anthropic_error_midstream("Overloaded"),
    ));
    let events = run_turn(&w, params("s1", &id, "Explain things"), |_| Answer::Nothing).await;
    let Some(AiEvent::Error {
        code,
        message,
        messages: Some(messages),
        seq: Some(_),
    }) = terminal(&events)
    else {
        panic!("{events:?}")
    };
    assert_eq!(
        (code.as_str(), message.as_str()),
        ("PROVIDER_ERROR", "Overloaded")
    );
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1].content, text_of(&events));
    assert_eq!(&w.messages("chat-1").await, messages);
}

/// A message limit too small for the cut note (probe F2 changed this case:
/// the reply used to stream in full and fail to store): the reply is cut
/// at the limit itself, stored without the note, and the turn ends
/// `tooLong`.
#[tokio::test]
async fn a_reply_past_a_tiny_limit_is_cut_without_its_note() {
    let s = Setup {
        state_limits: StateLimits {
            max_message_bytes: Some(40),
            ..common::web_state_limits()
        },
        ..Setup::default()
    };
    let (w, id) = chat_world(s).await;
    w.mock.reply(reply_sse(anthropic_text(
        "This reply is longer than forty bytes, by far.",
    )));
    let events = run_turn(&w, params("s1", &id, "Short question"), |_| Answer::Nothing).await;
    let Some(AiEvent::Done { messages, stop, .. }) = terminal(&events) else {
        panic!("{events:?}")
    };
    assert_eq!(*stop, AiStop::TooLong);
    assert_eq!(text_of(&events), "This reply is longer than forty bytes, b");
    assert_eq!(messages[1].content, text_of(&events));
    assert_eq!(&w.messages("chat-1").await, messages);
}

/// The reply's tool calls take it past the size the chat allows: it is
/// stored again without them, keeping its text, and the turn ends `done`.
#[tokio::test]
async fn a_reply_whose_parts_are_too_large_is_stored_without_them() {
    let s = Setup {
        state_limits: StateLimits {
            max_message_bytes: Some(600),
            ..common::web_state_limits()
        },
        ..Setup::default()
    };
    let (w, id) = chat_world(s).await;
    w.db.answers.lock().unwrap().insert(
        "SELECT wide".into(),
        json!({"columns": ["n"], "rows": [["x".repeat(800)]]}),
    );
    w.mock
        .reply(tools("Looking. ", &[query_call("c1", "SELECT wide")]))
        .reply(reply_sse(anthropic_text("Done.")));
    let mut p = params("s1", &id, "Wide one");
    p.approval = Approval::AllowAll;
    let events = run_turn(&w, p, |_| Answer::Nothing).await;
    let Some(AiEvent::Done { messages, seq, .. }) = terminal(&events) else {
        panic!("{events:?}")
    };
    assert_eq!(messages[1].content, "Looking. Done.");
    assert_eq!(messages[1].parts, None);
    assert_eq!(&w.messages("chat-1").await, messages);
    // The reply's write is the last but the chat event after it.
    assert_eq!(seq.n + 1, w.ws.change_seq().n);
}

/// A stalled provider times out on the executor's clock: a clock that runs
/// a thousand times fast turns 120 s into 120 ms.
#[tokio::test]
async fn a_stalled_provider_ends_with_timeout_on_the_executors_clock() {
    let s = Setup {
        executor: Some(Arc::new(FastClock)),
        ..Setup::default()
    };
    let (w, id) = chat_world(s).await;
    w.mock.reply(Reply::Sse {
        events: anthropic_text("Partial")[..3].to_vec(),
        piece: 64,
        gap: Duration::ZERO,
        end: SseEnd::Stall,
    });
    let started = std::time::Instant::now();
    let events = run_turn(&w, params("s1", &id, "Hang please"), |_| Answer::Nothing).await;
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(
        matches!(terminal(&events), Some(AiEvent::Error { code, .. }) if code == "TIMEOUT"),
        "{events:?}"
    );
    assert!(w.mock.client_gone(Duration::from_secs(5)).await);
    assert_eq!(w.messages("chat-1").await[1].content, "Partial");
}

struct FastClock;

impl seaquel_runtime::Executor for FastClock {
    fn spawn(&self, future: futures::future::BoxFuture<'static, ()>) {
        seaquel_runtime::TokioExecutor.spawn(future)
    }

    fn sleep(&self, duration: Duration) -> futures::future::BoxFuture<'static, ()> {
        seaquel_runtime::TokioExecutor.sleep(duration / 1000)
    }

    fn unix_time(&self) -> Duration {
        seaquel_runtime::TokioExecutor.unix_time()
    }

    fn monotonic(&self) -> Duration {
        seaquel_runtime::TokioExecutor.monotonic() * 1000
    }
}

#[tokio::test]
async fn a_cancel_mid_stream_drops_the_response_and_stores_what_streamed() {
    let (w, id) = chat_world(Setup::default()).await;
    w.mock.reply(Reply::Sse {
        events: anthropic_text("Partial ")[..3].to_vec(),
        piece: 64,
        gap: Duration::ZERO,
        end: SseEnd::Stall,
    });
    let events = run_turn(&w, params("s1", &id, "Stream then stop"), |e| match e {
        AiEvent::Text { .. } => Answer::Cancel,
        _ => Answer::Nothing,
    })
    .await;
    assert!(terminal(&events).is_none());
    assert!(w.mock.client_gone(Duration::from_secs(5)).await);
    let rows = w.messages("chat-1").await;
    assert_eq!(rows[1].content, "Partial ");
}

/// The web's eviction: `close_all` ends the turn like a cancel, its waiter
/// goes, and the provider's response is dropped.
#[tokio::test]
async fn close_all_during_a_turn_ends_it() {
    let (w, id) = chat_world(Setup::default()).await;
    w.mock
        .reply(tools("Checking. ", &[query_call("c1", "SELECT 1 AS n")]));
    let ws = w.ws.clone();
    let core = &w.core;
    let mut stream = ws.ai_chat(
        core,
        params("s1", &id, "Count it"),
        seaquel_core::WriteOrigin::none(),
    );
    let mut events = Vec::new();
    while let Some(e) = stream.next().await {
        if matches!(e, AiEvent::ApprovalRequired { .. }) {
            ws.close_all(core).await;
        }
        events.push(e);
    }
    assert!(terminal(&events).is_none());
    assert_eq!(ws.ai_waiter_count(), 0);
    assert_eq!(ws.ai_turn_count(), 0);
    assert_eq!(w.messages("chat-1").await[1].content, "Checking. ");
}

/// Text events are coalesced, and none is lost at the end.
#[tokio::test]
async fn text_is_coalesced_and_nothing_is_lost() {
    let (w, id) = chat_world(Setup::default()).await;
    let pieces: Vec<String> = (0..200).map(|i| format!("w{i} ")).collect();
    let mut events = vec![seaquel_ai::testing::scripts::anthropic_event(
        "message_start",
        json!({"type":"message_start","message":{"id":"m","role":"assistant","content":[]}}),
    )];
    for p in &pieces {
        events.push(seaquel_ai::testing::scripts::anthropic_event(
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":p}}),
        ));
    }
    events.push(seaquel_ai::testing::scripts::anthropic_event(
        "message_delta",
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}}),
    ));
    w.mock.reply(Reply::Sse {
        events,
        piece: 512,
        gap: Duration::ZERO,
        end: SseEnd::Finish,
    });
    let got = run_turn(&w, params("s1", &id, "Many words please"), |_| {
        Answer::Nothing
    })
    .await;
    let texts = got
        .iter()
        .filter(|e| matches!(e, AiEvent::Text { .. }))
        .count();
    assert!(texts < 50, "{texts} text events for 200 deltas");
    assert_eq!(text_of(&got), pieces.concat());
}

// ── Real engines ──

#[tokio::test]
async fn a_tool_runs_on_a_real_sqlite_and_duckdb_read_only() {
    for ty in ["sqlite", "duckdb"] {
        let s = Setup {
            scripted: false,
            conns: vec![Conn::new("conn-1", ty)],
            ..Setup::default()
        };
        if ty == "duckdb" && !duckdb_helper_built() {
            continue; // no helper (`common/duckdb.rs`)
        }
        let (w, id) = chat_world(s).await;
        w.mock
            .reply(tools(
                "",
                &[
                    query_call("c1", "SELECT 41 + 1 AS n"),
                    query_call("c2", "CREATE TABLE t (a INT)"),
                ],
            ))
            .reply(reply_sse(anthropic_text("Done.")));
        let mut p = params("s1", &id, "Run things");
        p.approval = Approval::AllowAll;
        let events = run_turn(&w, p, |_| Answer::Nothing).await;
        assert!(
            matches!(terminal(&events), Some(AiEvent::Done { .. })),
            "{ty}"
        );
        let results = &w.http.sent()[1].body["messages"][2]["content"];
        assert_eq!(
            results[0]["content"],
            r#"{"columns":["n"],"rowCount":1,"rows":[[42]],"truncated":false}"#,
            "{ty}"
        );
        assert!(
            results[1]["content"]
                .as_str()
                .unwrap()
                .starts_with("READ_ONLY: "),
            "{ty}: {}",
            results[1]["content"]
        );
    }
}

// ── Logs ──

#[tokio::test]
async fn logs_hold_usage_and_no_key_prompt_sql_or_reply() {
    common::capture_logs();
    let (w, id) = chat_world(Setup::default()).await;
    w.mock
        .reply(tools(
            "MARKER_REPLY_t4a ",
            &[query_call("c1", "SELECT 'MARKER_SQL_t4a' AS n")],
        ))
        .reply(reply_sse(anthropic_text("MARKER_DONE_t4a")));
    let mut p = params("s1", &id, "MARKER_PROMPT_t4a please");
    p.approval = Approval::AllowAll;
    run_turn(&w, p, |_| Answer::Nothing).await;
    let logged = common::logged();
    for f in [
        TEST_KEY,
        "MARKER_PROMPT_t4a",
        "MARKER_SQL_t4a",
        "MARKER_REPLY_t4a",
        "MARKER_DONE_t4a",
        "/v1/messages",
    ] {
        assert!(!logged.contains(f), "a log line holds {f}");
    }
    // Usage is logged, as numbers only (the key-values of each line).
    let usage: Vec<&str> = logged
        .lines()
        .filter(|l| l.contains("Token usage"))
        .collect();
    assert!(!usage.is_empty(), "usage is logged");
    for line in usage {
        let kvs = line.split("Token usage").nth(1).unwrap();
        for kv in kvs.split_whitespace() {
            let (k, v) = kv.split_once('=').unwrap();
            let numeric = v.chars().all(|c| c.is_ascii_digit()) || v == "None";
            assert!(
                k == "activity" || numeric,
                "a usage value that isn't a number: {line}"
            );
        }
    }
}

// ── Review fixes ──

/// I1: a stream id a turn runs under can't be taken by a second turn or a
/// query stream, and the first turn stays cancellable.
#[tokio::test]
async fn a_running_turns_stream_id_is_refused_and_the_turn_stays_cancellable() {
    let (w, id) = chat_world(Setup::default()).await;
    w.chat("chat-2", "conn-1").await;
    w.mock.reply(Reply::Sse {
        events: anthropic_text("Partial ")[..3].to_vec(),
        piece: 64,
        gap: Duration::ZERO,
        end: SseEnd::Stall,
    });
    let mut first = w.ws.ai_chat(
        &w.core,
        params("s1", &id, "First turn"),
        seaquel_core::WriteOrigin::none(),
    );
    let mut seen = Vec::new();
    while let Some(e) = first.next().await {
        let text = matches!(e, AiEvent::Text { .. });
        seen.push(e);
        if text {
            break;
        }
    }
    // A second turn under the same id, on another chat.
    let mut p = params("s1", &id, "Second turn");
    p.chat_id = "chat-2".into();
    p.user_message.id = "x-u".into();
    p.assistant_message_id = "x-a".into();
    let second = run_turn(&w, p, |_| Answer::Nothing).await;
    assert!(
        matches!(&second[..], [AiEvent::Error { code, .. }] if code == "INVALID_ARGUMENT"),
        "{second:?}"
    );
    // A query stream under it.
    let events: Vec<_> =
        w.ws.query_stream(
            &w.core,
            "s1".into(),
            id.clone(),
            "SELECT 1".into(),
            vec![],
            seaquel_core::QueryOptions::default(),
        )
        .collect()
        .await;
    assert!(
        matches!(&events[..], [seaquel_core::StreamEvent::Error { code, .. }] if code == "INVALID_ARGUMENT"),
        "{events:?}"
    );
    // The first turn still stops.
    w.ws.cancel(&w.core, "s1");
    let rest: Vec<_> = tokio::time::timeout(Duration::from_secs(10), first.collect::<Vec<_>>())
        .await
        .expect("the first turn ends");
    assert!(rest.iter().all(|e| !e.is_terminal()), "{rest:?}");
    assert_eq!(w.messages("chat-1").await[1].content, "Partial ");
    assert!(w.messages("chat-2").await.is_empty());
}

/// M4: one turn per chat at a time.
#[tokio::test]
async fn a_second_turn_on_a_busy_chat_is_turn_in_progress() {
    let (w, id) = chat_world(Setup::default()).await;
    w.mock.reply(Reply::Hang);
    let mut first = w.ws.ai_chat(
        &w.core,
        params("s1", &id, "First turn"),
        seaquel_core::WriteOrigin::none(),
    );
    assert!(matches!(first.next().await, Some(AiEvent::Started { .. })));
    let second = run_turn(&w, params("s2", &id, "Second turn"), |_| Answer::Nothing).await;
    assert!(
        matches!(&second[..], [AiEvent::Error { code, .. }] if code == "TURN_IN_PROGRESS"),
        "{second:?}"
    );
    assert_eq!(w.messages("chat-1").await.len(), 1, "only the first turn's");
}

/// M1: text held by the coalescer when the turn is cancelled is dropped.
#[tokio::test]
async fn no_text_follows_a_cancel() {
    let (w, id) = chat_world(Setup::default()).await;
    let mut events = vec![seaquel_ai::testing::scripts::anthropic_event(
        "message_start",
        json!({"type":"message_start","message":{"id":"m","role":"assistant","content":[]}}),
    )];
    for i in 0..50 {
        events.push(seaquel_ai::testing::scripts::anthropic_event(
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":format!("w{i} ")}}),
        ));
    }
    w.mock.reply(Reply::Sse {
        events,
        piece: 4096,
        gap: Duration::ZERO,
        end: SseEnd::Stall,
    });
    let mut cancelled = false;
    let mut after = Vec::new();
    run_turn(&w, params("s1", &id, "Stop at once"), |e| {
        if cancelled {
            after.push(format!("{e:?}"));
            return Answer::Nothing;
        }
        if matches!(e, AiEvent::Text { .. }) {
            cancelled = true;
            return Answer::Cancel;
        }
        Answer::Nothing
    })
    .await;
    assert!(cancelled);
    assert!(after.is_empty(), "{after:?}");
}

/// Flag 2: a turned-off assistant (and the inline prompt) refuses before
/// the keychain is read.
#[tokio::test]
async fn a_disabled_assistant_reads_no_key() {
    let (w, id) = chat_world(Setup {
        enabled: false,
        ..Setup::default()
    })
    .await;
    let events = run_turn(&w, params("s1", &id, "Anything"), |_| Answer::Nothing).await;
    assert!(
        matches!(&events[..], [AiEvent::Error { code, .. }] if code == "AI_DISABLED"),
        "{events:?}"
    );
    let err =
        w.ws.ai_generate(
            &w.core,
            seaquel_core::ai::GenerateParams {
                connection_id: "conn-1".into(),
                request: "anything".into(),
                existing_query: String::new(),
                api_key: None,
                provider_id: None,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "AI_DISABLED");
    let store = w.store.as_ref().unwrap();
    assert_eq!(store.gets.load(std::sync::atomic::Ordering::SeqCst), 0);
}

/// M8: the inline prompt's request and editor text are bounded like a
/// message on the web.
#[tokio::test]
async fn generate_refuses_text_past_the_message_cap() {
    let (w, _) = chat_world(Setup {
        limits: AiLimits {
            max_message_bytes: Some(8),
            ..AiLimits::default()
        },
        ..Setup::default()
    })
    .await;
    for (request, existing) in [("a long request", ""), ("short", "SELECT a_long_column")] {
        let err =
            w.ws.ai_generate(
                &w.core,
                seaquel_core::ai::GenerateParams {
                    connection_id: "conn-1".into(),
                    request: request.into(),
                    existing_query: existing.into(),
                    api_key: None,
                    provider_id: None,
                },
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, "MESSAGE_TOO_LONG");
    }
    assert!(w.http.sent().is_empty());
}

/// M6: a saved target can't be recorded as another saved connection.
#[tokio::test]
async fn a_saved_connect_naming_another_saved_id_is_refused() {
    let mut setup = Setup::default();
    setup.conns.push(Conn::new("conn-2", "postgres"));
    let w = world(setup).await;
    let err =
        w.ws.connect(
            &w.core,
            seaquel_core::ConnectRequest::saved("conn-1")
                .with_saved_connection_id(Some("conn-2".into())),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");
    assert!(w.ws.connection_ids(&w.core).is_empty());
}

/// I3: a put's parts are checked and count toward the chat's budget, and
/// a put without them keeps the stored ones.
#[tokio::test]
async fn parts_count_toward_the_chat_budget() {
    let (w, _) = chat_world(Setup {
        state_limits: StateLimits {
            max_chat_bytes: Some(2_000),
            max_message_bytes: Some(1_500),
            ..common::web_state_limits()
        },
        ..Setup::default()
    })
    .await;
    let origin = seaquel_core::WriteOrigin::none();
    let msg = |id: &str, parts: Option<serde_json::Value>| {
        let mut m = json!({"id": id, "role": "assistant", "content": "short",
                           "timestamp": T0});
        if let Some(p) = parts {
            m["parts"] = p;
        }
        serde_json::from_value::<seaquel_core::domain::state::ChatMessageDraft>(m).unwrap()
    };
    let big = json!([{"round": 0, "type": "text", "text": "y".repeat(1_200)}]);
    // Not a list.
    let err =
        w.ws.put_chat_messages(&w.core, &origin, "chat-1", vec![msg("m0", Some(json!({})))])
            .await
            .unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");
    let done =
        w.ws.put_chat_messages(
            &w.core,
            &origin,
            "chat-1",
            vec![msg("m1", Some(big.clone()))],
        )
        .await
        .unwrap();
    assert!(done.value.stored_bytes > 1_200, "parts are counted");
    // A second message as large passes the 2,000 bytes.
    let err =
        w.ws.put_chat_messages(&w.core, &origin, "chat-1", vec![msg("m2", Some(big))])
            .await
            .unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");
    // A put of m1 without parts keeps them, and their bytes.
    let again =
        w.ws.put_chat_messages(&w.core, &origin, "chat-1", vec![msg("m1", None)])
            .await
            .unwrap();
    assert_eq!(again.value.stored_bytes, done.value.stored_bytes);
    assert!(w.messages("chat-1").await[0].parts.is_some());
}

// ── Re-review cleanups ──

/// The reply with its tool calls passes `max_chat_bytes` (the user's write
/// only kept room for one message's worth): `CHAT_FULL`, so it is stored
/// again without them.
#[tokio::test]
async fn a_reply_past_the_chat_budget_is_stored_without_its_parts() {
    common::capture_logs();
    let (w, id) = chat_world(Setup {
        state_limits: StateLimits {
            max_message_bytes: Some(1_600),
            max_chat_bytes: Some(1_650),
            ..common::web_state_limits()
        },
        ..Setup::default()
    })
    .await;
    w.db.answers.lock().unwrap().insert(
        "SELECT mid".into(),
        json!({"columns": ["n"], "rows": [["y".repeat(150)]]}),
    );
    let last = "a".repeat(700);
    w.mock
        .reply(tools("Look. ", &[query_call("c1", "SELECT mid")]))
        .reply(reply_sse(anthropic_text(&last)));
    let mut p = params("s1", &id, "Mid one");
    p.approval = Approval::AllowAll;
    let events = run_turn(&w, p, |_| Answer::Nothing).await;
    let Some(AiEvent::Done { messages, .. }) = terminal(&events) else {
        panic!("{events:?}")
    };
    assert_eq!(messages[1].content, format!("Look. {last}"));
    assert_eq!(messages[1].parts, None);
    assert_eq!(&w.messages("chat-1").await, messages);
    assert!(
        common::logged()
            .lines()
            .any(|l| l.contains("without its tool calls") && l.contains("code=CHAT_FULL")),
        "the retry followed a CHAT_FULL"
    );
}

/// A chat takes a new turn once the last one ended, by `done`, a cancel
/// or an `error`.
#[tokio::test]
async fn a_chat_takes_a_new_turn_after_each_ending() {
    let (w, id) = chat_world(Setup::default()).await;
    let ended_by = |events: &[AiEvent]| match terminal(events) {
        Some(AiEvent::Done { .. }) => "done",
        Some(AiEvent::Error { code, .. }) if code == "TURN_IN_PROGRESS" => "busy",
        Some(AiEvent::Error { .. }) => "error",
        _ => "cancelled",
    };
    w.mock.reply(reply_sse(anthropic_text("One.")));
    let e = run_turn(&w, params("s1", &id, "Turn one"), |_| Answer::Nothing).await;
    assert_eq!(ended_by(&e), "done");
    w.mock.reply(Reply::Sse {
        events: anthropic_text("Two ")[..3].to_vec(),
        piece: 64,
        gap: Duration::ZERO,
        end: SseEnd::Stall,
    });
    let e = run_turn(&w, params("s2", &id, "Turn two"), |e| match e {
        AiEvent::Text { .. } => Answer::Cancel,
        _ => Answer::Nothing,
    })
    .await;
    assert_eq!(ended_by(&e), "cancelled");
    w.mock.reply(Reply::json(
        500,
        &json!({"type": "error", "error": {"type": "api_error", "message": "boom"}}),
    ));
    let e = run_turn(&w, params("s3", &id, "Turn three"), |_| Answer::Nothing).await;
    assert_eq!(ended_by(&e), "error");
    w.mock.reply(reply_sse(anthropic_text("Four.")));
    let e = run_turn(&w, params("s4", &id, "Turn four"), |_| Answer::Nothing).await;
    assert_eq!(ended_by(&e), "done");
    assert_eq!(w.ws.ai_turn_count(), 0);
}

/// A turn asked for after `close_all` says so instead of ending silently.
#[tokio::test]
async fn a_turn_after_close_all_is_workspace_closed() {
    let (w, id) = chat_world(Setup::default()).await;
    w.ws.close_all(&w.core).await;
    let events = run_turn(&w, params("s1", &id, "Too late"), |_| Answer::Nothing).await;
    assert!(
        matches!(&events[..], [AiEvent::Error { code, messages: None, .. }] if code == "WORKSPACE_CLOSED"),
        "{events:?}"
    );
    assert!(w.messages("chat-1").await.is_empty());
    assert!(w.http.sent().is_empty());
}

// ── The key in a provider's message (Task 8 review) ──

/// A turn with the visitor's key: the mock's answer echoes it.
fn keyed(id: &str, stream: &str) -> seaquel_core::ai::ChatParams {
    let mut p = params(stream, id, "Hello there");
    p.api_key = Some(TEST_KEY.into());
    p.provider_id = Some("prov-1".into());
    p
}

fn error_message(events: &[AiEvent]) -> String {
    match terminal(events) {
        Some(AiEvent::Error { message, .. }) => message.clone(),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_401_that_echoes_the_key_shows_it_redacted() {
    let (w, id) = chat_world(Setup::default()).await;
    w.mock.reply(Reply::json(
        401,
        &json!({"error": {"type": "authentication_error", "message": format!("invalid x-api-key: {TEST_KEY}")}}),
    ));
    let events = run_turn(&w, keyed(&id, "s1"), |_| Answer::Nothing).await;
    let message = error_message(&events);
    assert_eq!(message, "invalid x-api-key: <redacted>");
    assert!(!format!("{events:?}").contains(TEST_KEY));
}

#[tokio::test]
async fn a_stream_error_and_a_text_body_that_echo_the_key_show_it_redacted() {
    let (w, id) = chat_world(Setup::default()).await;
    w.mock.reply(reply_sse(
        seaquel_ai::testing::scripts::anthropic_error_midstream(&format!("bad key {TEST_KEY}!")),
    ));
    let events = run_turn(&w, keyed(&id, "s1"), |_| Answer::Nothing).await;
    assert_eq!(error_message(&events), "bad key <redacted>!");

    // `ai.test`'s plain-text 401, the key twice.
    w.mock.reply(Reply::Raw {
        status: 401,
        headers: vec![("content-type".into(), "text/plain".into())],
        body: format!("{TEST_KEY} is not {TEST_KEY}").into_bytes(),
    });
    let err =
        w.ws.ai_test(&w.core, "prov-1", Some(TEST_KEY.into()))
            .await
            .unwrap_err();
    assert_eq!(err.message, "<redacted> is not <redacted>");
}

#[tokio::test]
async fn the_key_is_redacted_before_the_message_is_cut() {
    let (w, id) = chat_world(Setup::default()).await;
    // The key straddles the 1 KiB cut: cut first, its first bytes would show.
    let message = format!("{}{TEST_KEY} and more", "x".repeat(1020));
    w.mock
        .reply(Reply::json(401, &json!({"error": {"message": message}})));
    let events = run_turn(&w, keyed(&id, "s1"), |_| Answer::Nothing).await;
    let shown = error_message(&events);
    assert!(shown.len() <= 1024, "{}", shown.len());
    assert!(!shown.contains("test-"), "a piece of the key shows");
    assert_eq!(&shown[1020..], "<red");
}

// ── The inline prompt's mentions (phase 7a Task 7) ──

/// The TUI's Ask AI completes `@` names and sends the request as typed:
/// Core resolves the mentions for `ai.generate` as it does for a turn (a
/// table, a saved query, a dashboard; `@"…"` for a name with spaces), and
/// a request without any is sent unchanged.
#[tokio::test]
async fn generate_resolves_mentions_as_a_turn_does() {
    let mut setup = Setup::default();
    let mut off = Conn::new("conn-2", "postgres");
    off.share_schema = Some(false);
    setup.conns.push(off);
    let w = world(setup).await;
    *w.db.schema.lock().unwrap() = vec![serde_json::from_value(json!({
        "name": "invoices", "schema": "public", "type": "table",
        "columns": [{"name": "id", "type": "integer", "nullable": false,
                     "isPrimaryKey": true, "isForeignKey": false}],
        "indexes": []
    }))
    .unwrap()];
    common::insert_rows(
        w.ws.storage(),
        "saved_queries",
        &[
            json!({"id": "saved-1", "project_id": "p1", "name": "Paid totals",
                 "query": "SELECT sum(total) FROM invoices", "created_at": T0, "updated_at": T0}),
        ],
    )
    .await;
    w.connect("conn-1").await;
    let answer = json!({"content": [{"type": "text", "text": "```sql\nSELECT 1\n```"}]});
    for _ in 0..3 {
        w.mock.reply(Reply::json(200, &answer));
    }
    let generate_on = |connection: &str, request: &str| seaquel_core::ai::GenerateParams {
        connection_id: connection.into(),
        request: request.into(),
        existing_query: String::new(),
        api_key: None,
        provider_id: None,
    };
    let generate = |request: &str| seaquel_core::ai::GenerateParams {
        connection_id: "conn-1".into(),
        request: request.into(),
        existing_query: String::new(),
        api_key: None,
        provider_id: None,
    };
    let sql =
        w.ws.ai_generate(
            &w.core,
            generate("top payers in @invoices like @\"Paid totals\""),
        )
        .await
        .unwrap();
    assert_eq!(sql, "SELECT 1");
    let sql =
        w.ws.ai_generate(&w.core, generate("no mention here"))
            .await
            .unwrap();
    assert_eq!(sql, "SELECT 1");
    let sent = w.http.sent();
    let user = |i: usize| {
        sent[i].body["messages"][0]["content"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let first = user(0);
    assert!(
        first.starts_with("top payers in @invoices like @\"Paid totals\"\n\nReferenced context:\n"),
        "{first}"
    );
    assert!(first.contains("Table: public.invoices"), "{first}");
    assert!(first.contains("Saved query: Paid totals"), "{first}");
    assert_eq!(user(1), "no mention here");

    // Schema sharing off (Decision 24 of phase 7a): the request goes byte
    // for byte as typed, with no context.
    let typed = "top payers in @invoices like @\"Paid totals\"";
    w.ws.ai_generate(&w.core, generate_on("conn-2", typed))
        .await
        .unwrap();
    let sent = w.http.sent();
    assert_eq!(
        sent[2].body["messages"][0]["content"].as_str().unwrap(),
        typed
    );
}
