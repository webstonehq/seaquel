//! OpenAI-compatible Chat Completions: request bodies and the streamed
//! chunks (`choices[0].delta` with `content` and `tool_calls[]` by index,
//! `finish_reason`, an optional `usage` chunk, `[DONE]`).

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::{
    error_fields, event_json, tool_input, Budget, Message, Role, Round, RoundEvent, StopReason,
    ToolCall, Usage, WireError,
};
use crate::sse::SseEvent;

/// The prefix that marks a failed tool call's result: the
/// format has no error flag.
pub const TOOL_ERROR_PREFIX: &str = "Error: ";

fn messages(system: &str, rest: &[Message]) -> Vec<Value> {
    let mut out = vec![json!({"role": "system", "content": system})];
    for m in rest {
        match m {
            Message::User(text) => out.push(json!({"role": "user", "content": text})),
            Message::Assistant { text, tool_calls } if tool_calls.is_empty() => {
                out.push(json!({"role": "assistant", "content": text}));
            }
            Message::Assistant { text, tool_calls } => {
                let calls: Vec<Value> = tool_calls
                    .iter()
                    .map(|c| json!({"id": c.id, "type": "function", "function": {"name": c.name, "arguments": c.input.to_string()}}))
                    .collect();
                let mut msg = json!({"role": "assistant", "tool_calls": calls});
                if !text.is_empty() {
                    msg["content"] = Value::String(text.clone());
                }
                out.push(msg);
            }
            Message::ToolResults(results) => {
                for r in results {
                    let content = if r.is_error {
                        format!("{TOOL_ERROR_PREFIX}{}", r.content)
                    } else {
                        r.content.clone()
                    };
                    out.push(
                        json!({"role": "tool", "tool_call_id": r.call_id, "content": content}),
                    );
                }
            }
        }
    }
    out
}

pub(super) fn round_body(model: &str, round: &Round) -> Value {
    let mut body = json!({
        "model": model,
        "messages": messages(&round.system, &round.messages),
        "stream": true,
    });
    if !round.tools.is_empty() {
        body["tools"] = round
            .tools
            .iter()
            .map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.input_schema}}))
            .collect();
    }
    body
}

pub(super) fn generate_body(model: &str, system: &str, msgs: &[(Role, String)]) -> Value {
    let rest: Vec<Message> = msgs
        .iter()
        .map(|(role, content)| match role {
            Role::User => Message::User(content.clone()),
            Role::Assistant => Message::Assistant {
                text: content.clone(),
                tool_calls: Vec::new(),
            },
        })
        .collect();
    json!({"model": model, "messages": messages(system, &rest)})
}

#[derive(Default)]
struct PendingCall {
    id: String,
    name: String,
    args: String,
}

#[derive(Default)]
pub(super) struct State {
    calls: BTreeMap<u64, PendingCall>,
    budget: Budget,
    finish: Option<StopReason>,
    done: bool,
}

fn stop_reason(v: &str) -> StopReason {
    match v {
        "tool_calls" | "function_call" => StopReason::ToolUse,
        "length" => StopReason::MaxTokens,
        _ => StopReason::End,
    }
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
        if event.data.trim() == "[DONE]" {
            return self.end(out);
        }
        let v = event_json(&event.data)?;
        if !v["error"].is_null() {
            let (error_type, message) = error_fields(&v, secret);
            return Err(WireError::Stream {
                error_type,
                message,
            });
        }
        let usage = &v["usage"];
        if usage.is_object() {
            let u = Usage {
                input_tokens: usage["prompt_tokens"].as_u64(),
                output_tokens: usage["completion_tokens"].as_u64(),
            };
            if u != Usage::default() {
                out.push(RoundEvent::Usage(u));
            }
        }
        let choice = &v["choices"][0];
        if let Some(t) = choice["delta"]["content"]
            .as_str()
            .filter(|t| !t.is_empty())
        {
            self.budget.text(t.len())?;
            out.push(RoundEvent::Text(t.to_string()));
        }
        if let Some(tcs) = choice["delta"]["tool_calls"].as_array() {
            for tc in tcs {
                let index = tc["index"].as_u64().unwrap_or(0);
                if !self.calls.contains_key(&index) {
                    Budget::open(self.calls.len())?;
                    self.budget.call()?;
                }
                let entry = self.calls.entry(index).or_default();
                if let Some(id) = tc["id"].as_str().filter(|s| !s.is_empty()) {
                    entry.id = Budget::name(id)?.to_string();
                }
                if let Some(name) = tc["function"]["name"].as_str().filter(|s| !s.is_empty()) {
                    entry.name = Budget::name(name)?.to_string();
                }
                if let Some(a) = tc["function"]["arguments"].as_str() {
                    self.budget.args(a.len())?;
                    entry.args.push_str(a);
                }
            }
        }
        if let Some(r) = choice["finish_reason"].as_str() {
            self.finish = Some(stop_reason(r));
        }
        Ok(())
    }

    /// `[DONE]`, or the body's end after a `finish_reason`: the calls in
    /// index order, then the stop. `[DONE]` with no `finish_reason` (some
    /// compatible servers) ends with `ToolUse` when calls came, else `End`.
    fn end(&mut self, out: &mut Vec<RoundEvent>) -> Result<(), WireError> {
        let calls = std::mem::take(&mut self.calls);
        let had_calls = !calls.is_empty();
        for (i, c) in calls {
            if c.name.is_empty() {
                return Err(WireError::Malformed("a tool call without a name"));
            }
            out.push(RoundEvent::ToolCall(ToolCall {
                id: if c.id.is_empty() {
                    format!("call_{i}")
                } else {
                    c.id
                },
                name: c.name,
                input: tool_input(&c.args)?,
            }));
        }
        let stop = self.finish.unwrap_or(if had_calls {
            StopReason::ToolUse
        } else {
            StopReason::End
        });
        out.push(RoundEvent::Stop(stop));
        self.done = true;
        Ok(())
    }

    pub(super) fn finish(&mut self, out: &mut Vec<RoundEvent>) -> Result<(), WireError> {
        if self.done {
            return Ok(());
        }
        if self.finish.is_none() {
            return Err(WireError::Incomplete);
        }
        self.end(out)
    }
}
