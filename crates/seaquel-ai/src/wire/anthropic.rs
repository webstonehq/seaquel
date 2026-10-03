//! Anthropic's Messages API: request bodies and the streamed events
//! (`message_start`, `content_block_start`/`_delta`/`_stop`,
//! `message_delta`, `message_stop`, `ping`, `error`).

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::{
    cut_message, event_json, safe_error_type, tool_input, Budget, Message, Role, Round, RoundEvent,
    StopReason, ToolCall, Usage, WireError, GENERATE_MAX_TOKENS, ROUND_MAX_TOKENS,
};
use crate::sse::SseEvent;

fn message(m: &Message) -> Value {
    match m {
        Message::User(text) => json!({"role": "user", "content": text}),
        Message::Assistant { text, tool_calls } if tool_calls.is_empty() => {
            json!({"role": "assistant", "content": text})
        }
        Message::Assistant { text, tool_calls } => {
            let mut content = Vec::new();
            if !text.is_empty() {
                content.push(json!({"type": "text", "text": text}));
            }
            for c in tool_calls {
                content.push(
                    json!({"type": "tool_use", "id": c.id, "name": c.name, "input": c.input}),
                );
            }
            json!({"role": "assistant", "content": content})
        }
        Message::ToolResults(results) => {
            let content: Vec<Value> = results
                .iter()
                .map(|r| {
                    let mut block = json!({"type": "tool_result", "tool_use_id": r.call_id, "content": r.content});
                    if r.is_error {
                        block["is_error"] = Value::Bool(true);
                    }
                    block
                })
                .collect();
            json!({"role": "user", "content": content})
        }
    }
}

pub(super) fn round_body(model: &str, round: &Round) -> Value {
    let mut body = json!({
        "model": model,
        "max_tokens": ROUND_MAX_TOKENS,
        "system": round.system,
        "messages": round.messages.iter().map(message).collect::<Vec<_>>(),
        "stream": true,
    });
    if !round.tools.is_empty() {
        body["tools"] = round
            .tools
            .iter()
            .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.input_schema}))
            .collect();
    }
    body
}

pub(super) fn generate_body(model: &str, system: &str, messages: &[(Role, String)]) -> Value {
    json!({
        "model": model,
        "max_tokens": GENERATE_MAX_TOKENS,
        "system": system,
        "messages": messages
            .iter()
            .map(|(role, content)| json!({"role": role.as_str(), "content": content}))
            .collect::<Vec<_>>(),
    })
}

enum Block {
    Text,
    Tool {
        id: String,
        name: String,
        json: String,
    },
    /// `thinking` and anything newer: its deltas are ignored.
    Other,
}

#[derive(Default)]
pub(super) struct State {
    blocks: BTreeMap<u64, Block>,
    budget: Budget,
    stop: Option<StopReason>,
    done: bool,
}

fn stop_reason(v: &str) -> StopReason {
    match v {
        "tool_use" => StopReason::ToolUse,
        "max_tokens" | "model_context_window_exceeded" => StopReason::MaxTokens,
        _ => StopReason::End,
    }
}

fn index(v: &Value) -> Result<u64, WireError> {
    v["index"]
        .as_u64()
        .ok_or(WireError::Malformed("a content block without an index"))
}

impl State {
    pub(super) fn handle(
        &mut self,
        event: &SseEvent,
        out: &mut Vec<RoundEvent>,
        secret: Option<&str>,
    ) -> Result<(), WireError> {
        if self.done {
            return Ok(());
        }
        let v = event_json(&event.data)?;
        let kind = v["type"].as_str().or(event.name.as_deref()).unwrap_or("");
        match kind {
            "message_start" => {
                if let Some(n) = v["message"]["usage"]["input_tokens"].as_u64() {
                    out.push(RoundEvent::Usage(Usage {
                        input_tokens: Some(n),
                        output_tokens: None,
                    }));
                }
            }
            "content_block_start" => {
                let i = index(&v)?;
                if !self.blocks.contains_key(&i) {
                    Budget::open(self.blocks.len())?;
                }
                let cb = &v["content_block"];
                let block = match cb["type"].as_str() {
                    Some("text") => {
                        if let Some(t) = cb["text"].as_str().filter(|t| !t.is_empty()) {
                            self.budget.text(t.len())?;
                            out.push(RoundEvent::Text(t.to_string()));
                        }
                        Block::Text
                    }
                    Some("tool_use") => {
                        let name = Budget::name(cb["name"].as_str().unwrap_or(""))?;
                        if name.is_empty() {
                            return Err(WireError::Malformed("a tool call without a name"));
                        }
                        let id = Budget::name(cb["id"].as_str().unwrap_or(""))?;
                        self.budget.call()?;
                        Block::Tool {
                            id: id.to_string(),
                            name: name.to_string(),
                            json: String::new(),
                        }
                    }
                    _ => Block::Other,
                };
                self.blocks.insert(i, block);
            }
            "content_block_delta" => {
                let i = index(&v)?;
                let delta = &v["delta"];
                match delta["type"].as_str() {
                    Some("text_delta") => {
                        if let Some(t) = delta["text"].as_str().filter(|t| !t.is_empty()) {
                            self.budget.text(t.len())?;
                            out.push(RoundEvent::Text(t.to_string()));
                        }
                    }
                    Some("input_json_delta") => match self.blocks.get_mut(&i) {
                        Some(Block::Tool { json, .. }) => {
                            let piece = delta["partial_json"].as_str().unwrap_or("");
                            self.budget.args(piece.len())?;
                            json.push_str(piece);
                        }
                        _ => return Err(WireError::Malformed("tool arguments for no tool call")),
                    },
                    _ => {}
                }
            }
            "content_block_stop" => {
                let i = index(&v)?;
                if let Some(block) = self.blocks.remove(&i) {
                    emit_tool(i, block, out)?;
                }
            }
            "message_delta" => {
                if let Some(r) = v["delta"]["stop_reason"].as_str() {
                    self.stop = Some(stop_reason(r));
                }
                if let Some(n) = v["usage"]["output_tokens"].as_u64() {
                    out.push(RoundEvent::Usage(Usage {
                        input_tokens: None,
                        output_tokens: Some(n),
                    }));
                }
            }
            "message_stop" => self.end(out)?,
            "error" => {
                let e = &v["error"];
                return Err(WireError::Stream {
                    error_type: safe_error_type(e["type"].as_str()),
                    message: cut_message(e["message"].as_str(), secret),
                });
            }
            // `ping` and event types newer than this code.
            _ => {}
        }
        Ok(())
    }

    /// The tool calls still open, in index order, then the stop.
    fn end(&mut self, out: &mut Vec<RoundEvent>) -> Result<(), WireError> {
        for (i, block) in std::mem::take(&mut self.blocks) {
            emit_tool(i, block, out)?;
        }
        out.push(RoundEvent::Stop(self.stop.unwrap_or(StopReason::End)));
        self.done = true;
        Ok(())
    }

    /// The body ended: fine after `message_stop`, or after a stop reason
    /// without it; otherwise the answer was cut off.
    pub(super) fn finish(&mut self, out: &mut Vec<RoundEvent>) -> Result<(), WireError> {
        if self.done {
            return Ok(());
        }
        if self.stop.is_none() {
            return Err(WireError::Incomplete);
        }
        self.end(out)
    }
}

fn emit_tool(index: u64, block: Block, out: &mut Vec<RoundEvent>) -> Result<(), WireError> {
    if let Block::Tool { id, name, json } = block {
        let id = if id.is_empty() {
            format!("toolu_{index}")
        } else {
            id
        };
        out.push(RoundEvent::ToolCall(ToolCall {
            id,
            name,
            input: tool_input(&json)?,
        }));
    }
    Ok(())
}
