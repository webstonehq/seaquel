//! Scripted provider streams, as the providers document them: what the
//! mock sends and what the wire tests decode. Spike S1's shapes.

use serde_json::{json, Value};

/// One Anthropic SSE event, with `\r\n` line endings as S1's mock sent.
pub fn anthropic_event(name: &str, data: Value) -> String {
    format!("event: {name}\r\ndata: {data}\r\n\r\n")
}

/// One OpenAI-compatible chunk (`data:` only, `\n`).
pub fn openai_chunk(data: Value) -> String {
    format!("data: {data}\n\n")
}

pub fn openai_done() -> String {
    "data: [DONE]\n\n".to_string()
}

fn message_start(id: &str) -> String {
    anthropic_event(
        "message_start",
        json!({"type":"message_start","message":{"id":id,"role":"assistant","content":[],"usage":{"input_tokens":12,"output_tokens":1}}}),
    )
}

fn text_block(index: usize, text: &str) -> Vec<String> {
    vec![
        anthropic_event(
            "content_block_start",
            json!({"type":"content_block_start","index":index,"content_block":{"type":"text","text":""}}),
        ),
        anthropic_event(
            "content_block_delta",
            json!({"type":"content_block_delta","index":index,"delta":{"type":"text_delta","text":text}}),
        ),
        anthropic_event(
            "content_block_stop",
            json!({"type":"content_block_stop","index":index}),
        ),
    ]
}

/// A `tool_use` block whose input arrives in pieces of `piece` characters
/// (S1: 7), so the JSON is only whole at the block's stop.
fn tool_block(index: usize, id: &str, name: &str, input: &str, piece: usize) -> Vec<String> {
    let mut v = vec![anthropic_event(
        "content_block_start",
        json!({"type":"content_block_start","index":index,"content_block":{"type":"tool_use","id":id,"name":name,"input":{}}}),
    )];
    let chars: Vec<char> = input.chars().collect();
    for part in chars.chunks(piece.max(1)) {
        let s: String = part.iter().collect();
        v.push(anthropic_event(
            "content_block_delta",
            json!({"type":"content_block_delta","index":index,"delta":{"type":"input_json_delta","partial_json":s}}),
        ));
    }
    v.push(anthropic_event(
        "content_block_stop",
        json!({"type":"content_block_stop","index":index}),
    ));
    v
}

fn message_end(stop_reason: &str, output_tokens: u64) -> Vec<String> {
    vec![
        anthropic_event(
            "message_delta",
            json!({"type":"message_delta","delta":{"stop_reason":stop_reason,"stop_sequence":null},"usage":{"output_tokens":output_tokens}}),
        ),
        anthropic_event("message_stop", json!({"type":"message_stop"})),
    ]
}

/// A round that answers `text` and ends (`end_turn`).
pub fn anthropic_text(text: &str) -> Vec<String> {
    let mut v = vec![message_start("msg_text")];
    v.extend(text_block(0, text));
    v.extend(message_end("end_turn", 5));
    v
}

/// A round that says `text`, then calls each `(id, name, input JSON)` in
/// order, and stops with `tool_use`. A ping sits after the start, as the
/// real API sends.
pub fn anthropic_tools(text: &str, calls: &[(&str, &str, &str)]) -> Vec<String> {
    let mut v = vec![
        message_start("msg_tools"),
        anthropic_event("ping", json!({"type":"ping"})),
    ];
    let mut index = 0;
    if !text.is_empty() {
        v.extend(text_block(index, text));
        index += 1;
    }
    for (id, name, input) in calls {
        v.extend(tool_block(index, id, name, input, 7));
        index += 1;
    }
    v.extend(message_end("tool_use", 42));
    v
}

/// S1's first round: text with multi-byte characters, then two tool calls.
pub fn anthropic_s1_round() -> Vec<String> {
    anthropic_tools(
        "Let me check the café table ☕. ",
        &[
            (
                "toolu_1",
                "run_query",
                r#"{"sql": "SELECT count(*) FROM café WHERE note = '☕'"}"#,
            ),
            ("toolu_2", "describe_table", r#"{"table":"café"}"#),
        ],
    )
}

/// Half an answer, then an `error` event (`overloaded_error`), as S1.
pub fn anthropic_error_midstream(message: &str) -> Vec<String> {
    vec![
        message_start("msg_err"),
        anthropic_event(
            "content_block_start",
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        ),
        anthropic_event(
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Half an ans"}}),
        ),
        anthropic_event(
            "error",
            json!({"type":"error","error":{"type":"overloaded_error","message":message}}),
        ),
    ]
}

/// A round cut by `max_tokens`.
pub fn anthropic_max_tokens(text: &str) -> Vec<String> {
    let mut v = vec![message_start("msg_max")];
    v.extend(text_block(0, text));
    v.extend(message_end("max_tokens", 4096));
    v
}

fn openai_delta(delta: Value, finish: Value) -> String {
    openai_chunk(
        json!({"id":"chatcmpl-1","object":"chat.completion.chunk","choices":[{"index":0,"delta":delta,"finish_reason":finish}]}),
    )
}

/// A round that answers `text` and stops.
pub fn openai_text(text: &str) -> Vec<String> {
    vec![
        openai_delta(json!({"role":"assistant","content":""}), Value::Null),
        openai_delta(json!({"content":text}), Value::Null),
        openai_delta(json!({}), json!("stop")),
        openai_done(),
    ]
}

/// S1's OpenAI round: two parallel tool calls whose `arguments` pieces
/// interleave, `finish_reason: "tool_calls"`, `[DONE]`.
pub fn openai_s1_round() -> Vec<String> {
    vec![
        openai_delta(
            json!({"role":"assistant","content":"Checking ☕"}),
            Value::Null,
        ),
        openai_delta(
            json!({"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"run_query","arguments":""}}]}),
            Value::Null,
        ),
        openai_delta(
            json!({"tool_calls":[{"index":0,"function":{"arguments":"{\"sql\":\"SELECT 1"}}]}),
            Value::Null,
        ),
        openai_delta(
            json!({"tool_calls":[{"index":1,"id":"call_b","type":"function","function":{"name":"list_tables","arguments":"{}"}}]}),
            Value::Null,
        ),
        openai_delta(
            json!({"tool_calls":[{"index":0,"function":{"arguments":" AS é\"}"}}]}),
            Value::Null,
        ),
        openai_delta(json!({}), json!("tool_calls")),
        openai_done(),
    ]
}

/// A round cut by the token limit (`finish_reason: "length"`).
pub fn openai_length(text: &str) -> Vec<String> {
    vec![
        openai_delta(json!({"content":text}), Value::Null),
        openai_delta(json!({}), json!("length")),
        openai_done(),
    ]
}

/// The whole stream as one string.
pub fn joined(events: &[String]) -> String {
    events.concat()
}
