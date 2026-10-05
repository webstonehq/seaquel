#![allow(clippy::disallowed_types)] // the budget tests time themselves; native only

//! The providers' wire: request bodies and headers as the TypeScript client
//! sent them (`services/ai/providers.ts`), and decoding of every ending a
//! round can have. Task 3 replays Task 1's recorded bodies on top.

use serde_json::{json, Value};

use seaquel_ai::http::Method;
use seaquel_ai::testing::scripts;
use seaquel_ai::testing::TEST_KEY;
use seaquel_ai::wire::{
    check_status, check_status_with, decode_generate, decode_models, generate_request,
    models_request, round_request, Decoder, Message, Provider, ProviderKind, Role, Round,
    RoundEvent, StopReason, ToolCall, ToolResult, ToolSpec, Usage, WireError,
};

fn anthropic() -> Provider {
    Provider {
        kind: ProviderKind::Anthropic,
        base_url: None,
        model: "claude-test".into(),
    }
}

fn openai(base: Option<&str>) -> Provider {
    Provider {
        kind: ProviderKind::OpenAiCompatible,
        base_url: base.map(String::from),
        model: "gpt-test".into(),
    }
}

fn run_query_spec() -> ToolSpec {
    ToolSpec {
        name: "run_query".into(),
        description: "Run a read-only SQL query.".into(),
        input_schema: json!({"type":"object","properties":{"sql":{"type":"string"}},"required":["sql"]}),
    }
}

fn plain_round() -> Round {
    Round {
        system: "You are a SQL assistant.".into(),
        messages: vec![
            Message::User("Hi".into()),
            Message::Assistant {
                text: "Hello!".into(),
                tool_calls: vec![],
            },
            Message::User("How many orders?".into()),
        ],
        tools: vec![],
    }
}

/// A round after the model called two tools; the second failed.
fn tool_round() -> Round {
    Round {
        system: "sys".into(),
        messages: vec![
            Message::User("Count orders".into()),
            Message::Assistant {
                text: "Let me check.".into(),
                tool_calls: vec![
                    ToolCall {
                        id: "call_1".into(),
                        name: "run_query".into(),
                        input: json!({"sql":"SELECT count(*) FROM orders"}),
                    },
                    ToolCall {
                        id: "call_2".into(),
                        name: "run_query".into(),
                        input: json!({"sql":"DELETE FROM orders"}),
                    },
                ],
            },
            Message::ToolResults(vec![
                ToolResult {
                    call_id: "call_1".into(),
                    content: "{\"rows\":[[3]]}".into(),
                    is_error: false,
                },
                ToolResult {
                    call_id: "call_2".into(),
                    content: "READ_ONLY: only read-only queries".into(),
                    is_error: true,
                },
            ]),
        ],
        tools: vec![run_query_spec()],
    }
}

fn body(req: &seaquel_ai::http::HttpRequest) -> Value {
    serde_json::from_slice(&req.body).expect("the body is JSON")
}

fn header_names(req: &seaquel_ai::http::HttpRequest) -> Vec<&str> {
    req.headers.iter().map(|(n, _)| n.as_str()).collect()
}

// ------------------------------------------------------------- requests --

#[test]
fn anthropic_round_body_and_headers_match_the_typescript() {
    let req = round_request(&anthropic(), Some(TEST_KEY), &plain_round(), false);
    assert_eq!(req.method, Method::Post);
    assert_eq!(req.url, "https://api.anthropic.com/v1/messages");
    assert_eq!(
        header_names(&req),
        ["content-type", "x-api-key", "anthropic-version"]
    );
    assert_eq!(req.header("content-type"), Some("application/json"));
    assert_eq!(req.header("x-api-key"), Some(TEST_KEY));
    assert_eq!(req.header("anthropic-version"), Some("2023-06-01"));
    // No tools: no `tools` field, as `if (tools.length > 0)`.
    assert_eq!(
        body(&req),
        json!({
            "model": "claude-test",
            "max_tokens": 4096,
            "system": "You are a SQL assistant.",
            "messages": [
                {"role":"user","content":"Hi"},
                {"role":"assistant","content":"Hello!"},
                {"role":"user","content":"How many orders?"}
            ],
            "stream": true
        })
    );
}

#[test]
fn anthropic_round_sends_every_tool_call_and_result_in_order() {
    let req = round_request(&anthropic(), Some(TEST_KEY), &tool_round(), false);
    assert_eq!(
        body(&req),
        json!({
            "model": "claude-test",
            "max_tokens": 4096,
            "system": "sys",
            "messages": [
                {"role":"user","content":"Count orders"},
                {"role":"assistant","content":[
                    {"type":"text","text":"Let me check."},
                    {"type":"tool_use","id":"call_1","name":"run_query","input":{"sql":"SELECT count(*) FROM orders"}},
                    {"type":"tool_use","id":"call_2","name":"run_query","input":{"sql":"DELETE FROM orders"}}
                ]},
                {"role":"user","content":[
                    {"type":"tool_result","tool_use_id":"call_1","content":"{\"rows\":[[3]]}"},
                    {"type":"tool_result","tool_use_id":"call_2","content":"READ_ONLY: only read-only queries","is_error":true}
                ]}
            ],
            "stream": true,
            "tools": [{"name":"run_query","description":"Run a read-only SQL query.","input_schema":{"type":"object","properties":{"sql":{"type":"string"}},"required":["sql"]}}]
        })
    );
}

#[test]
fn anthropic_assistant_with_calls_and_no_text_has_no_text_block() {
    let mut round = tool_round();
    if let Message::Assistant { text, .. } = &mut round.messages[1] {
        text.clear();
    }
    let b = body(&round_request(&anthropic(), Some(TEST_KEY), &round, false));
    assert_eq!(b["messages"][1]["content"][0]["type"], "tool_use");
}

#[test]
fn the_browser_adds_anthropics_direct_access_header_and_native_does_not() {
    let browser = round_request(&anthropic(), Some(TEST_KEY), &plain_round(), true);
    assert_eq!(
        browser.header("anthropic-dangerous-direct-browser-access"),
        Some("true")
    );
    let native = round_request(&anthropic(), Some(TEST_KEY), &plain_round(), false);
    assert_eq!(
        native.header("anthropic-dangerous-direct-browser-access"),
        None
    );
    let models = models_request(&anthropic(), Some(TEST_KEY), true);
    assert_eq!(
        models.header("anthropic-dangerous-direct-browser-access"),
        Some("true")
    );
    // Not an Anthropic header: OpenAI-compatible requests never carry it.
    let o = round_request(&openai(None), Some(TEST_KEY), &plain_round(), true);
    assert_eq!(o.header("anthropic-dangerous-direct-browser-access"), None);
}

#[test]
fn an_anthropic_base_url_replaces_the_api_host() {
    let p = Provider {
        base_url: Some("http://127.0.0.1:9/".into()),
        ..anthropic()
    };
    assert_eq!(
        round_request(&p, Some(TEST_KEY), &plain_round(), false).url,
        "http://127.0.0.1:9/v1/messages"
    );
    assert_eq!(
        models_request(&p, Some(TEST_KEY), false).url,
        "http://127.0.0.1:9/v1/models"
    );
}

#[test]
fn openai_round_body_and_headers_match_the_typescript() {
    let req = round_request(&openai(None), Some(TEST_KEY), &plain_round(), false);
    assert_eq!(req.url, "https://api.openai.com/v1/chat/completions");
    assert_eq!(header_names(&req), ["content-type", "authorization"]);
    assert_eq!(
        req.header("authorization"),
        Some(format!("Bearer {TEST_KEY}").as_str())
    );
    assert_eq!(
        body(&req),
        json!({
            "model": "gpt-test",
            "messages": [
                {"role":"system","content":"You are a SQL assistant."},
                {"role":"user","content":"Hi"},
                {"role":"assistant","content":"Hello!"},
                {"role":"user","content":"How many orders?"}
            ],
            "stream": true
        })
    );
}

#[test]
fn openai_round_sends_every_tool_call_and_result_in_order() {
    let req = round_request(
        &openai(Some("http://127.0.0.1:11434/v1/")),
        None,
        &tool_round(),
        false,
    );
    // One trailing `/` dropped; no key: no authorization header.
    assert_eq!(req.url, "http://127.0.0.1:11434/v1/chat/completions");
    assert_eq!(header_names(&req), ["content-type"]);
    let b = body(&req);
    assert_eq!(
        b,
        json!({
            "model": "gpt-test",
            "messages": [
                {"role":"system","content":"sys"},
                {"role":"user","content":"Count orders"},
                {"role":"assistant","tool_calls":[
                    {"id":"call_1","type":"function","function":{"name":"run_query","arguments":"{\"sql\":\"SELECT count(*) FROM orders\"}"}},
                    {"id":"call_2","type":"function","function":{"name":"run_query","arguments":"{\"sql\":\"DELETE FROM orders\"}"}}
                ],"content":"Let me check."},
                {"role":"tool","tool_call_id":"call_1","content":"{\"rows\":[[3]]}"},
                {"role":"tool","tool_call_id":"call_2","content":"Error: READ_ONLY: only read-only queries"}
            ],
            "stream": true,
            "tools": [{"type":"function","function":{"name":"run_query","description":"Run a read-only SQL query.","parameters":{"type":"object","properties":{"sql":{"type":"string"}},"required":["sql"]}}}]
        })
    );
}

#[test]
fn openai_assistant_with_calls_and_no_text_has_no_content() {
    let mut round = tool_round();
    if let Message::Assistant { text, .. } = &mut round.messages[1] {
        text.clear();
    }
    let b = body(&round_request(&openai(None), None, &round, false));
    assert!(
        b["messages"][2].get("content").is_none(),
        "{}",
        b["messages"][2]
    );
}

#[test]
fn an_empty_key_sends_no_authorization() {
    let req = round_request(&openai(None), Some(""), &plain_round(), false);
    assert_eq!(req.header("authorization"), None);
    let req = round_request(&anthropic(), None, &plain_round(), false);
    assert_eq!(req.header("x-api-key"), None);
}

#[test]
fn generate_requests_match_the_typescript() {
    let msgs = vec![(Role::User, "Write a query that lists users".to_string())];
    let a = generate_request(&anthropic(), Some(TEST_KEY), "sys", &msgs, false);
    assert_eq!(a.url, "https://api.anthropic.com/v1/messages");
    assert_eq!(
        header_names(&a),
        ["content-type", "x-api-key", "anthropic-version"]
    );
    assert_eq!(
        body(&a),
        json!({"model":"claude-test","max_tokens":2048,"system":"sys","messages":[{"role":"user","content":"Write a query that lists users"}]})
    );
    let o = generate_request(&openai(None), Some(TEST_KEY), "sys", &msgs, false);
    assert_eq!(o.url, "https://api.openai.com/v1/chat/completions");
    assert_eq!(
        body(&o),
        json!({"model":"gpt-test","messages":[{"role":"system","content":"sys"},{"role":"user","content":"Write a query that lists users"}]})
    );
}

#[test]
fn models_requests_match_the_typescript() {
    let a = models_request(&anthropic(), Some(TEST_KEY), false);
    assert_eq!(a.method, Method::Get);
    assert_eq!(a.url, "https://api.anthropic.com/v1/models");
    assert_eq!(header_names(&a), ["x-api-key", "anthropic-version"]);
    assert!(a.body.is_empty());
    let o = models_request(
        &openai(Some("http://127.0.0.1:1/v1")),
        Some(TEST_KEY),
        false,
    );
    assert_eq!(o.method, Method::Get);
    assert_eq!(o.url, "http://127.0.0.1:1/v1/models");
    assert_eq!(header_names(&o), ["authorization"]);
    let keyless = models_request(&openai(Some("http://127.0.0.1:1/v1")), None, false);
    assert!(keyless.headers.is_empty());
}

#[test]
fn no_debug_output_carries_the_key_or_the_conversation() {
    let req = round_request(&anthropic(), Some(TEST_KEY), &tool_round(), false);
    let shown = format!("{req:?} {:?} {:?}", tool_round(), anthropic());
    assert!(!shown.contains(TEST_KEY), "{shown}");
    assert!(!shown.contains("orders"), "{shown}");
    assert!(!shown.contains("Count"), "{shown}");
}

// ------------------------------------------------------------- decoding --

fn decode_in_pieces(
    kind: ProviderKind,
    bytes: &[u8],
    cuts: &[usize],
) -> Result<Vec<RoundEvent>, WireError> {
    let mut d = Decoder::new(kind);
    let mut out = Vec::new();
    let mut start = 0;
    for &cut in cuts {
        d.feed(&bytes[start..cut], &mut out)?;
        start = cut;
    }
    d.feed(&bytes[start..], &mut out)?;
    d.finish(&mut out)?;
    Ok(out)
}

fn decode(kind: ProviderKind, events: &[String]) -> Result<Vec<RoundEvent>, WireError> {
    decode_in_pieces(kind, scripts::joined(events).as_bytes(), &[])
}

/// Text deltas joined, so a test doesn't depend on how they were split.
fn text_of(events: &[RoundEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            RoundEvent::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect()
}

fn calls_of(events: &[RoundEvent]) -> Vec<(String, String, Value)> {
    events
        .iter()
        .filter_map(|e| match e {
            RoundEvent::ToolCall(c) => Some((c.id.clone(), c.name.clone(), c.input.clone())),
            _ => None,
        })
        .collect()
}

fn stops_of(events: &[RoundEvent]) -> Vec<StopReason> {
    events
        .iter()
        .filter_map(|e| match e {
            RoundEvent::Stop(s) => Some(*s),
            _ => None,
        })
        .collect()
}

#[test]
fn anthropic_two_tool_calls_in_a_round_decode_in_order() {
    let events = decode(ProviderKind::Anthropic, &scripts::anthropic_s1_round()).unwrap();
    assert_eq!(text_of(&events), "Let me check the café table ☕. ");
    assert_eq!(
        calls_of(&events),
        vec![
            (
                "toolu_1".to_string(),
                "run_query".to_string(),
                json!({"sql": "SELECT count(*) FROM café WHERE note = '☕'"})
            ),
            (
                "toolu_2".to_string(),
                "describe_table".to_string(),
                json!({"table":"café"})
            ),
        ]
    );
    assert_eq!(stops_of(&events), [StopReason::ToolUse]);
    assert!(matches!(
        events.last(),
        Some(RoundEvent::Stop(StopReason::ToolUse))
    ));
    assert!(events.contains(&RoundEvent::Usage(Usage {
        input_tokens: Some(12),
        output_tokens: None
    })));
    assert!(events.contains(&RoundEvent::Usage(Usage {
        input_tokens: None,
        output_tokens: Some(42)
    })));
}

#[test]
fn every_split_position_decodes_the_same_round() {
    for (kind, events) in [
        (ProviderKind::Anthropic, scripts::anthropic_s1_round()),
        (ProviderKind::OpenAiCompatible, scripts::openai_s1_round()),
    ] {
        let joined = scripts::joined(&events);
        let bytes = joined.as_bytes();
        let whole = decode_in_pieces(kind, bytes, &[]).unwrap();
        for cut in 0..=bytes.len() {
            let got = decode_in_pieces(kind, bytes, &[cut]).unwrap();
            assert_eq!(text_of(&got), text_of(&whole), "{kind:?} cut {cut}");
            assert_eq!(calls_of(&got), calls_of(&whole), "{kind:?} cut {cut}");
            assert_eq!(stops_of(&got), stops_of(&whole), "{kind:?} cut {cut}");
        }
    }
}

#[test]
fn anthropic_text_round_ends() {
    let events = decode(
        ProviderKind::Anthropic,
        &scripts::anthropic_text("There are 3 rows."),
    )
    .unwrap();
    assert_eq!(text_of(&events), "There are 3 rows.");
    assert!(calls_of(&events).is_empty());
    assert_eq!(stops_of(&events), [StopReason::End]);
}

#[test]
fn anthropic_error_event_midstream_is_a_provider_error() {
    let mut d = Decoder::new(ProviderKind::Anthropic);
    let mut out = Vec::new();
    let joined = scripts::joined(&scripts::anthropic_error_midstream("Overloaded MARKER"));
    let err = d.feed(joined.as_bytes(), &mut out).unwrap_err();
    // The half answer was decoded before the error.
    assert_eq!(text_of(&out), "Half an ans");
    assert_eq!(err.code(), "PROVIDER_ERROR");
    assert_eq!(err.error_type(), Some("overloaded_error"));
    assert_eq!(err.provider_message(), Some("Overloaded MARKER"));
    // Neither Debug nor Display carries the provider's message.
    assert!(!format!("{err:?} {err}").contains("MARKER"));
    assert!(format!("{err}").contains("overloaded_error"));
}

#[test]
fn anthropic_max_tokens_is_its_own_stop() {
    let events = decode(
        ProviderKind::Anthropic,
        &scripts::anthropic_max_tokens("cut sho"),
    )
    .unwrap();
    assert_eq!(stops_of(&events), [StopReason::MaxTokens]);
    assert_eq!(text_of(&events), "cut sho");
}

#[test]
fn anthropic_malformed_event_is_a_provider_error() {
    let mut events = scripts::anthropic_text("x");
    events.insert(2, "event: content_block_delta\ndata: {not json\n\n".into());
    let err = decode(ProviderKind::Anthropic, &events).unwrap_err();
    assert!(matches!(err, WireError::Malformed(_)), "{err:?}");
    assert_eq!(err.code(), "PROVIDER_ERROR");
}

#[test]
fn anthropic_delta_for_no_tool_block_is_malformed() {
    let events = vec![scripts::anthropic_event(
        "content_block_delta",
        json!({"type":"content_block_delta","index":3,"delta":{"type":"input_json_delta","partial_json":"{}"}}),
    )];
    assert!(matches!(
        decode(ProviderKind::Anthropic, &events),
        Err(WireError::Malformed(_))
    ));
}

#[test]
fn anthropic_tool_with_no_input_deltas_is_an_empty_object() {
    let events = scripts::anthropic_tools("", &[("toolu_1", "list_tables", "")]);
    let got = decode(ProviderKind::Anthropic, &events).unwrap();
    assert_eq!(
        calls_of(&got),
        vec![("toolu_1".into(), "list_tables".into(), json!({}))]
    );
}

#[test]
fn anthropic_tool_input_that_isnt_a_json_object_is_malformed() {
    for bad in [r#"{"sql": "SELECT"#, "[1,2]", "\"text\""] {
        let events = scripts::anthropic_tools("", &[("toolu_1", "run_query", bad)]);
        assert!(
            matches!(
                decode(ProviderKind::Anthropic, &events),
                Err(WireError::Malformed(_))
            ),
            "{bad}"
        );
    }
}

#[test]
fn a_stream_that_ends_before_the_model_finished_is_incomplete() {
    // Anthropic: no message_delta/message_stop.
    let mut events = scripts::anthropic_text("par");
    events.truncate(events.len() - 2);
    assert_eq!(
        decode(ProviderKind::Anthropic, &events).unwrap_err(),
        WireError::Incomplete
    );
    // OpenAI: no finish_reason and no [DONE].
    let mut events = scripts::openai_text("par");
    events.truncate(2);
    assert_eq!(
        decode(ProviderKind::OpenAiCompatible, &events).unwrap_err(),
        WireError::Incomplete
    );
    // Nothing at all.
    assert_eq!(
        decode(ProviderKind::Anthropic, &[]).unwrap_err(),
        WireError::Incomplete
    );
}

#[test]
fn anthropic_stop_known_but_message_stop_missing_still_ends() {
    let mut events = scripts::anthropic_text("ok");
    events.pop(); // message_stop
    let got = decode(ProviderKind::Anthropic, &events).unwrap();
    assert_eq!(stops_of(&got), [StopReason::End]);
}

#[test]
fn openai_parallel_tool_calls_decode_in_index_order() {
    let events = decode(ProviderKind::OpenAiCompatible, &scripts::openai_s1_round()).unwrap();
    assert_eq!(text_of(&events), "Checking ☕");
    assert_eq!(
        calls_of(&events),
        vec![
            (
                "call_a".into(),
                "run_query".into(),
                json!({"sql":"SELECT 1 AS é"})
            ),
            ("call_b".into(), "list_tables".into(), json!({})),
        ]
    );
    assert_eq!(stops_of(&events), [StopReason::ToolUse]);
    assert!(matches!(events.last(), Some(RoundEvent::Stop(_))));
}

#[test]
fn openai_text_and_length() {
    let events = decode(
        ProviderKind::OpenAiCompatible,
        &scripts::openai_text("hello"),
    )
    .unwrap();
    assert_eq!(text_of(&events), "hello");
    assert_eq!(stops_of(&events), [StopReason::End]);
    let events = decode(
        ProviderKind::OpenAiCompatible,
        &scripts::openai_length("cut"),
    )
    .unwrap();
    assert_eq!(stops_of(&events), [StopReason::MaxTokens]);
}

#[test]
fn openai_early_done_ends_the_round_and_what_follows_is_ignored() {
    // `[DONE]` before any finish_reason: the round ends there, with what
    // it had, and later chunks don't count.
    let events = vec![
        scripts::openai_chunk(
            json!({"choices":[{"index":0,"delta":{"content":"partial"},"finish_reason":null}]}),
        ),
        scripts::openai_done(),
        scripts::openai_chunk(
            json!({"choices":[{"index":0,"delta":{"content":" more"},"finish_reason":null}]}),
        ),
    ];
    let got = decode(ProviderKind::OpenAiCompatible, &events).unwrap();
    assert_eq!(text_of(&got), "partial");
    assert_eq!(stops_of(&got), [StopReason::End]);
    // With calls pending at an early [DONE], they still come out.
    let events = vec![
        scripts::openai_chunk(
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"list_tables","arguments":"{}"}}]},"finish_reason":null}]}),
        ),
        scripts::openai_done(),
    ];
    let got = decode(ProviderKind::OpenAiCompatible, &events).unwrap();
    assert_eq!(calls_of(&got).len(), 1);
    assert_eq!(stops_of(&got), [StopReason::ToolUse]);
}

#[test]
fn openai_finish_without_done_ends_at_the_end_of_the_body() {
    let mut events = scripts::openai_text("hi");
    events.pop(); // [DONE]
    let got = decode(ProviderKind::OpenAiCompatible, &events).unwrap();
    assert_eq!(stops_of(&got), [StopReason::End]);
}

#[test]
fn openai_usage_after_finish_is_read() {
    let events = vec![
        scripts::openai_chunk(
            json!({"choices":[{"index":0,"delta":{"content":"x"},"finish_reason":"stop"}]}),
        ),
        scripts::openai_chunk(
            json!({"choices":[],"usage":{"prompt_tokens":7,"completion_tokens":2}}),
        ),
        scripts::openai_done(),
    ];
    let got = decode(ProviderKind::OpenAiCompatible, &events).unwrap();
    assert!(got.contains(&RoundEvent::Usage(Usage {
        input_tokens: Some(7),
        output_tokens: Some(2)
    })));
    assert!(matches!(
        got.last(),
        Some(RoundEvent::Stop(StopReason::End))
    ));
}

#[test]
fn openai_error_object_in_the_stream_is_a_provider_error() {
    let events = vec![
        scripts::openai_chunk(
            json!({"choices":[{"index":0,"delta":{"content":"a"},"finish_reason":null}]}),
        ),
        scripts::openai_chunk(json!({"error":{"message":"MARKER boom","type":"server_error"}})),
    ];
    let err = decode(ProviderKind::OpenAiCompatible, &events).unwrap_err();
    assert_eq!(err.error_type(), Some("server_error"));
    assert_eq!(err.provider_message(), Some("MARKER boom"));
    assert!(!format!("{err:?} {err}").contains("MARKER"));
}

#[test]
fn openai_malformed_event_and_nameless_call_are_errors() {
    let events = vec!["data: {nope\n\n".to_string(), scripts::openai_done()];
    assert!(matches!(
        decode(ProviderKind::OpenAiCompatible, &events),
        Err(WireError::Malformed(_))
    ));
    let events = vec![
        scripts::openai_chunk(
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"arguments":"{}"}}]},"finish_reason":"tool_calls"}]}),
        ),
        scripts::openai_done(),
    ];
    assert!(matches!(
        decode(ProviderKind::OpenAiCompatible, &events),
        Err(WireError::Malformed(_))
    ));
}

#[test]
fn openai_call_without_an_id_gets_one_from_its_index() {
    let events = vec![
        scripts::openai_chunk(
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":2,"function":{"name":"list_tables","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}),
        ),
        scripts::openai_done(),
    ];
    let got = decode(ProviderKind::OpenAiCompatible, &events).unwrap();
    assert_eq!(calls_of(&got)[0].0, "call_2");
}

// --------------------------------------------------------------- status --

#[test]
fn status_2xx_is_ok() {
    assert_eq!(check_status(200, b""), Ok(()));
    assert_eq!(check_status(204, b"junk"), Ok(()));
}

#[test]
fn status_429_is_rate_limited() {
    let err = check_status(
        429,
        br#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#,
    )
    .unwrap_err();
    assert_eq!(err.code(), "RATE_LIMITED");
    assert_eq!(err.status(), Some(429));
    assert_eq!(err.error_type(), Some("rate_limit_error"));
    assert_eq!(err.provider_message(), Some("slow down"));
}

#[test]
fn status_500_reads_both_error_shapes() {
    let a = check_status(
        500,
        br#"{"type":"error","error":{"type":"api_error","message":"Internal MARKER"}}"#,
    )
    .unwrap_err();
    assert_eq!(a.code(), "PROVIDER_ERROR");
    assert_eq!(a.error_type(), Some("api_error"));
    assert_eq!(a.provider_message(), Some("Internal MARKER"));
    assert!(!format!("{a:?} {a}").contains("MARKER"));
    assert!(format!("{a}").contains("500"));
    let o = check_status(
        401,
        br#"{"error":{"message":"Incorrect API key","type":"invalid_request_error","code":"invalid_api_key"}}"#,
    )
    .unwrap_err();
    assert_eq!(o.error_type(), Some("invalid_request_error"));
    assert_eq!(o.provider_message(), Some("Incorrect API key"));
    // An error that is only a string (some OpenAI-compatible servers).
    let s = check_status(404, br#"{"error":"model 'x' not found"}"#).unwrap_err();
    assert_eq!(s.provider_message(), Some("model 'x' not found"));
    assert_eq!(s.error_type(), None);
}

#[test]
fn a_body_that_isnt_json_is_kept_as_text() {
    let e = check_status(502, b"  404 page not found\n").unwrap_err();
    assert_eq!(e.provider_message(), Some("404 page not found"));
    let e = check_status(500, b"").unwrap_err();
    assert_eq!(e.provider_message(), None);
}

#[test]
fn a_redirect_is_a_provider_error() {
    let e = check_status(302, b"").unwrap_err();
    assert_eq!(e.code(), "PROVIDER_ERROR");
    assert_eq!(e.status(), Some(302));
}

#[test]
fn the_message_is_cut_at_1kb_on_a_char_boundary_and_the_type_must_be_an_identifier() {
    let long = "é".repeat(1000); // 2,000 bytes
    let b = json!({"error":{"type":"has spaces; and more","message":long}}).to_string();
    let e = check_status(500, b.as_bytes()).unwrap_err();
    let m = e.provider_message().unwrap();
    assert!(m.len() <= 1024, "{}", m.len());
    assert!(m.len() >= 1022);
    assert!(long.starts_with(m));
    assert_eq!(e.error_type(), None);
    let b = json!({"error":{"type":"x".repeat(65)}}).to_string();
    assert_eq!(
        check_status(500, b.as_bytes()).unwrap_err().error_type(),
        None
    );
}

// ------------------------------------------------------- non-streaming --

#[test]
fn generate_answers_decode() {
    assert_eq!(
        decode_generate(
            ProviderKind::Anthropic,
            br#"{"content":[{"type":"text","text":"```sql\nSELECT 1\n```"}],"stop_reason":"end_turn"}"#
        )
        .unwrap(),
        "```sql\nSELECT 1\n```"
    );
    assert_eq!(
        decode_generate(ProviderKind::Anthropic, br#"{"content":[]}"#).unwrap(),
        ""
    );
    assert_eq!(
        decode_generate(
            ProviderKind::OpenAiCompatible,
            br#"{"choices":[{"message":{"role":"assistant","content":"SELECT 2"}}]}"#
        )
        .unwrap(),
        "SELECT 2"
    );
    assert_eq!(
        decode_generate(ProviderKind::OpenAiCompatible, br#"{"choices":[]}"#).unwrap(),
        ""
    );
    assert!(matches!(
        decode_generate(ProviderKind::OpenAiCompatible, b"nope"),
        Err(WireError::Malformed(_))
    ));
}

#[test]
fn models_answers_decode() {
    assert_eq!(
        decode_models(br#"{"data":[{"id":"a","type":"model"},{"id":"b"}],"has_more":false}"#)
            .unwrap(),
        ["a", "b"]
    );
    assert!(matches!(decode_models(b"{}"), Err(WireError::Malformed(_))));
    assert!(matches!(
        decode_models(b"[1]"),
        Err(WireError::Malformed(_))
    ));
}

// ------------------------------------------------- per-round budget --

use seaquel_ai::wire::{MAX_OPEN_CALLS, MAX_ROUND_TEXT_BYTES, MAX_TOOL_ARGUMENT_BYTES};

/// Feeds events one by one until the decoder refuses; returns how many it
/// took. Panics if it takes them all.
fn fed_until_refused(kind: ProviderKind, events: impl Iterator<Item = String>) -> usize {
    let mut d = Decoder::new(kind);
    let mut out = Vec::new();
    for (i, e) in events.enumerate() {
        match d.feed(e.as_bytes(), &mut out) {
            Ok(()) => {}
            Err(err) => {
                assert!(matches!(err, WireError::Malformed(_)), "{err:?}");
                return i;
            }
        }
        out.clear();
    }
    panic!("the decoder took every event");
}

#[test]
fn the_limits_are_what_the_review_asked() {
    assert_eq!(MAX_OPEN_CALLS, 64);
    assert_eq!(MAX_TOOL_ARGUMENT_BYTES, 16 * 1024 * 1024);
    assert_eq!(MAX_ROUND_TEXT_BYTES, 16 * 1024 * 1024);
}

#[test]
fn openai_tool_arguments_past_the_budget_are_refused() {
    let big = "a".repeat(1 << 20);
    let started = std::time::Instant::now();
    let took = fed_until_refused(
        ProviderKind::OpenAiCompatible,
        (0..64).map(|_| {
            scripts::openai_chunk(json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"name":"x","arguments":big}}]},"finish_reason":null}]}))
        }),
    );
    assert!(took <= 16, "took {took} MiB");
    assert!(started.elapsed().as_secs() < 10);
}

#[test]
fn openai_calls_past_64_indices_are_refused() {
    let started = std::time::Instant::now();
    let took = fed_until_refused(
        ProviderKind::OpenAiCompatible,
        (0..200_000u64).map(|i| {
            scripts::openai_chunk(json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":i,"function":{"name":"x"}}]},"finish_reason":null}]}))
        }),
    );
    assert_eq!(took, 64);
    assert!(started.elapsed().as_secs() < 10);
    // One chunk naming 200,000 indices is refused too.
    let many: Vec<Value> = (0..200_000u64)
        .map(|i| json!({"index": i, "function": {"name": "x"}}))
        .collect();
    let one = scripts::openai_chunk(
        json!({"choices":[{"index":0,"delta":{"tool_calls":many},"finish_reason":null}]}),
    );
    assert_eq!(
        fed_until_refused(ProviderKind::OpenAiCompatible, std::iter::once(one)),
        0
    );
}

#[test]
fn anthropic_blocks_past_64_open_are_refused() {
    let took = fed_until_refused(
        ProviderKind::Anthropic,
        (0..200_000u64).map(|i| {
            scripts::anthropic_event(
                "content_block_start",
                json!({"type":"content_block_start","index":i,"content_block":{"type":"tool_use","id":format!("t{i}"),"name":"x","input":{}}}),
            )
        }),
    );
    assert_eq!(took, 64);
    // Closed blocks free their slot.
    let mut events = Vec::new();
    for i in 0..200u64 {
        events.push(scripts::anthropic_event(
            "content_block_start",
            json!({"type":"content_block_start","index":i,"content_block":{"type":"text","text":""}}),
        ));
        events.push(scripts::anthropic_event(
            "content_block_stop",
            json!({"type":"content_block_stop","index":i}),
        ));
    }
    let mut d = Decoder::new(ProviderKind::Anthropic);
    let mut out = Vec::new();
    d.feed(scripts::joined(&events).as_bytes(), &mut out)
        .unwrap();
}

#[test]
fn anthropic_tool_arguments_past_the_budget_are_refused() {
    let big = "a".repeat(1 << 20);
    let start = scripts::anthropic_event(
        "content_block_start",
        json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"x","input":{}}}),
    );
    let took = fed_until_refused(
        ProviderKind::Anthropic,
        std::iter::once(start).chain((0..64).map(|_| {
            scripts::anthropic_event(
                "content_block_delta",
                json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":big}}),
            )
        })),
    );
    assert!(took <= 17, "took {took}");
}

#[test]
fn a_rounds_text_past_the_budget_is_refused() {
    let big = "a".repeat(1 << 20);
    let took = fed_until_refused(
        ProviderKind::Anthropic,
        (0..64).map(|_| {
            scripts::anthropic_event(
                "content_block_delta",
                json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":big}}),
            )
        }),
    );
    assert!(took <= 16, "took {took}");
    let took = fed_until_refused(
        ProviderKind::OpenAiCompatible,
        (0..64).map(|_| {
            scripts::openai_chunk(
                json!({"choices":[{"index":0,"delta":{"content":big},"finish_reason":null}]}),
            )
        }),
    );
    assert!(took <= 16, "took {took}");
}

// ------------------------------------------------------------ fuzz --

/// xorshift64*, seeded, so a failure reproduces.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Random bytes, valid streams with bytes flipped, and spliced fragments,
/// fed in random pieces: the decoder answers or refuses, never panics.
#[test]
fn the_decoder_never_panics_on_random_input() {
    let valid: Vec<String> = [
        scripts::joined(&scripts::anthropic_s1_round()),
        scripts::joined(&scripts::anthropic_error_midstream("x")),
        scripts::joined(&scripts::openai_s1_round()),
        scripts::joined(&scripts::openai_length("y")),
    ]
    .into();
    let alphabet: &[u8] =
        b"data: event:{}[]\",:\n\r0123456789indextypetool_callsdelta\\\xC3\xA9\xFF";
    let mut rng = Rng(0x5EA0_0E1A_2026_1008);
    for _ in 0..3000 {
        let mut bytes: Vec<u8> = match rng.below(3) {
            0 => (0..rng.below(400))
                .map(|_| alphabet[rng.below(alphabet.len())])
                .collect(),
            1 => {
                let mut b = valid[rng.below(valid.len())].clone().into_bytes();
                for _ in 0..rng.below(8) {
                    let i = rng.below(b.len());
                    b[i] = alphabet[rng.below(alphabet.len())];
                }
                b
            }
            _ => {
                let a = valid[rng.below(valid.len())].as_bytes();
                let b = valid[rng.below(valid.len())].as_bytes();
                let mut v = a[..rng.below(a.len())].to_vec();
                v.extend_from_slice(&b[rng.below(b.len())..]);
                v
            }
        };
        if rng.below(4) == 0 {
            bytes.extend_from_slice(b"\n\n");
        }
        for kind in [ProviderKind::Anthropic, ProviderKind::OpenAiCompatible] {
            let mut d = Decoder::new(kind);
            let mut out = Vec::new();
            let mut rest = &bytes[..];
            let mut failed = false;
            while !rest.is_empty() {
                let n = 1 + rng.below(rest.len());
                if d.feed(&rest[..n], &mut out).is_err() {
                    failed = true;
                    break;
                }
                rest = &rest[n..];
            }
            if !failed {
                let _ = d.finish(&mut out);
            }
        }
    }
}

// --------------------------------------------------- models cap --

#[test]
fn the_model_list_has_its_own_larger_cap() {
    use seaquel_ai::wire::MAX_MODELS_BODY_BYTES;
    assert_eq!(MAX_MODELS_BODY_BYTES, 8 * 1024 * 1024);
    // OpenRouter-sized: ~1 MB, past the 64 KiB error-body cap.
    let models: Vec<Value> = (0..4000)
        .map(|i| json!({"id": format!("vendor/model-{i}"), "description": "d".repeat(200)}))
        .collect();
    let body = json!({"data": models}).to_string();
    assert!(body.len() > 900_000 && body.len() < MAX_MODELS_BODY_BYTES);
    assert_eq!(decode_models(body.as_bytes()).unwrap().len(), 4000);
}

// ------------------------------------------- names, ids and call count --

use seaquel_ai::wire::{MAX_ROUND_CALLS, MAX_TOOL_NAME_BYTES};

#[test]
fn the_name_and_call_limits_are_what_the_re_review_asked() {
    assert_eq!(MAX_TOOL_NAME_BYTES, 256);
    assert_eq!(MAX_ROUND_CALLS, 64);
}

fn anthropic_tool_start(i: u64, id: &str, name: &str) -> String {
    scripts::anthropic_event(
        "content_block_start",
        json!({"type":"content_block_start","index":i,"content_block":{"type":"tool_use","id":id,"name":name,"input":{}}}),
    )
}

fn openai_tool_chunk(i: u64, id: Option<&str>, name: Option<&str>) -> String {
    let mut tc = json!({"index": i, "function": {}});
    if let Some(id) = id {
        tc["id"] = json!(id);
    }
    if let Some(name) = name {
        tc["function"]["name"] = json!(name);
    }
    scripts::openai_chunk(
        json!({"choices":[{"index":0,"delta":{"tool_calls":[tc]},"finish_reason":null}]}),
    )
}

fn feed_one(kind: ProviderKind, event: &str) -> Result<(), WireError> {
    let mut d = Decoder::new(kind);
    let mut out = Vec::new();
    d.feed(event.as_bytes(), &mut out)
}

#[test]
fn tool_names_and_ids_past_256_bytes_are_refused() {
    let ok = "n".repeat(256);
    let long = "n".repeat(257);
    let a = ProviderKind::Anthropic;
    assert_eq!(feed_one(a, &anthropic_tool_start(0, &ok, &ok)), Ok(()));
    for event in [
        anthropic_tool_start(0, "t", &long),
        anthropic_tool_start(0, &long, "x"),
    ] {
        assert!(matches!(feed_one(a, &event), Err(WireError::Malformed(_))));
    }
    let o = ProviderKind::OpenAiCompatible;
    assert_eq!(
        feed_one(o, &openai_tool_chunk(0, Some(&ok), Some(&ok))),
        Ok(())
    );
    for event in [
        openai_tool_chunk(0, None, Some(&long)),
        openai_tool_chunk(0, Some(&long), Some("x")),
    ] {
        assert!(matches!(feed_one(o, &event), Err(WireError::Malformed(_))));
    }
    // The probe's case: 64 calls with 7 MiB names and ids are refused at
    // the first, holding nothing.
    let huge = "n".repeat(7 << 20);
    assert!(matches!(
        feed_one(o, &openai_tool_chunk(0, Some(&huge), Some(&huge))),
        Err(WireError::Malformed(_))
    ));
}

/// A closed Anthropic block frees its slot, but a round still has at most
/// `MAX_ROUND_CALLS` tool calls in all.
#[test]
fn a_round_has_at_most_64_tool_calls_in_all() {
    let events = (0..200u64).flat_map(|i| {
        [
            anthropic_tool_start(i, &format!("t{i}"), "list_tables"),
            scripts::anthropic_event(
                "content_block_stop",
                json!({"type":"content_block_stop","index":i}),
            ),
        ]
    });
    // Event 128 is the 65th call's start.
    assert_eq!(fed_until_refused(ProviderKind::Anthropic, events), 128);
}

// ── The supplied key in a provider's message ──

#[test]
fn a_status_message_that_echoes_the_key_is_redacted_before_the_cut() {
    let body =
        json!({"error": {"type": "authentication_error", "message": format!("bad {TEST_KEY}")}});
    let err = check_status_with(401, body.to_string().as_bytes(), Some(TEST_KEY)).unwrap_err();
    assert_eq!(err.provider_message(), Some("bad <redacted>"));
    // A plain body, the key twice.
    let err = check_status_with(
        500,
        format!("{TEST_KEY}/{TEST_KEY}").as_bytes(),
        Some(TEST_KEY),
    )
    .unwrap_err();
    assert_eq!(err.provider_message(), Some("<redacted>/<redacted>"));
    // Straddling the 1 KiB cut: redacted first, so none of it shows.
    let long = format!("{}{TEST_KEY}", "x".repeat(1020));
    let body = json!({"error": {"message": long}});
    let err = check_status_with(401, body.to_string().as_bytes(), Some(TEST_KEY)).unwrap_err();
    assert_eq!(&err.provider_message().unwrap()[1020..], "<red");
    // No key, or an empty one: the message as it was.
    let err = check_status(401, format!("bad {TEST_KEY}").as_bytes()).unwrap_err();
    assert_eq!(
        err.provider_message(),
        Some(format!("bad {TEST_KEY}").as_str())
    );
    let err = check_status_with(401, b"bad", Some("")).unwrap_err();
    assert_eq!(err.provider_message(), Some("bad"));
}

#[test]
fn a_stream_error_that_echoes_the_key_is_redacted() {
    let mut d = Decoder::with_secret(ProviderKind::Anthropic, Some(TEST_KEY));
    let joined = scripts::joined(&scripts::anthropic_error_midstream(&format!(
        "no {TEST_KEY}"
    )));
    let err = d.feed(joined.as_bytes(), &mut Vec::new()).unwrap_err();
    assert_eq!(err.provider_message(), Some("no <redacted>"));

    let mut d = Decoder::with_secret(ProviderKind::OpenAiCompatible, Some(TEST_KEY));
    let event = scripts::openai_chunk(json!({"error": {"message": format!("no {TEST_KEY}")}}));
    let err = d.feed(event.as_bytes(), &mut Vec::new()).unwrap_err();
    assert_eq!(err.provider_message(), Some("no <redacted>"));
}
