//! Anthropic Messages 协议面（/v1/messages）：请求/响应/SSE 与 OpenAI 面
//! 共享同一条 Provider 管线，只在边界做形状转换。
//! 请求：system 顶层字段 → system 消息；content 块数组取 text 段拼接。
//! 响应：message 形状；流式事件 message_start → content_block_* →
//! message_delta(usage) → message_stop，无 [DONE]。

use serde::Deserialize;
use serde_json::{json, Value};

use crate::openai::{ChatCompletion, ChatRequest, Usage};

#[derive(Debug, Deserialize)]
pub struct AnthropicRequest {
    pub model: String,
    #[serde(default)]
    pub max_tokens: Option<u64>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub system: Option<Value>,
    pub messages: Vec<crate::openai::ChatMessage>,
}

fn blocks_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

impl AnthropicRequest {
    pub fn into_chat_request(self) -> ChatRequest {
        let mut messages = Vec::with_capacity(self.messages.len() + 1);
        if let Some(sys) = &self.system {
            let text = blocks_text(sys);
            if !text.is_empty() {
                messages.push(crate::openai::ChatMessage {
                    role: "system".into(),
                    content: Value::String(text),
                });
            }
        }
        messages.extend(self.messages);
        let mut raw = serde_json::Map::new();
        if let Some(mt) = self.max_tokens {
            raw.insert("max_tokens".into(), json!(mt));
        }
        ChatRequest { model: self.model, messages, stream: self.stream, raw }
    }
}

/// ChatCompletion → Anthropic message 响应体。
pub fn message_from_completion(c: &ChatCompletion) -> Value {
    let text = c
        .choices
        .first()
        .map(|ch| ch.message.content.clone())
        .unwrap_or_default();
    json!({
        "id": format!("msg_{}", c.id.trim_start_matches("chatcmpl-")),
        "type": "message",
        "role": "assistant",
        "model": c.model,
        "content": [{ "type": "text", "text": text }],
        "stop_reason": map_stop_reason(c.choices.first().and_then(|ch| ch.finish_reason.as_deref())),
        "stop_sequence": Value::Null,
        "usage": {
            "input_tokens": c.usage.prompt_tokens,
            "output_tokens": c.usage.completion_tokens,
        }
    })
}

fn map_stop_reason(openai: Option<&str>) -> &'static str {
    match openai {
        Some("length") => "max_tokens",
        Some("tool_calls") => "tool_use",
        Some("stop") | None => "end_turn",
        _ => "end_turn",
    }
}

/// 流式事件构造器：把 StreamChunk 流翻译成 anthropic SSE 事件。
pub struct AnthropicEventBuilder {
    msg_id: String,
    model: String,
    input_tokens: u64,
    next_block: u32,
    open_block: Option<&'static str>, // "thinking" | "text"
}

impl AnthropicEventBuilder {
    pub fn new(model: String, input_tokens: u64) -> Self {
        Self {
            msg_id: format!("msg_{}", crate::key::random_id(12)),
            model,
            input_tokens,
            next_block: 0,
            open_block: None,
        }
    }

    pub fn message_start(&self) -> Value {
        json!({
            "type": "message_start",
            "message": {
                "id": self.msg_id, "type": "message", "role": "assistant",
                "model": self.model, "content": [],
                "stop_reason": Value::Null, "stop_sequence": Value::Null,
                "usage": { "input_tokens": self.input_tokens, "output_tokens": 0 }
            }
        })
    }

    /// 产出需要发送的事件（可能多条：先关旧块、开新块、再增量）。
    pub fn on_reasoning(&mut self, thinking: &str) -> Vec<Value> {
        let mut out = Vec::new();
        if self.open_block != Some("thinking") {
            out.extend(self.close_open_block());
            out.push(json!({ "type": "content_block_start", "index": self.next_block,
                "content_block": { "type": "thinking", "thinking": "" } }));
            self.next_block += 1;
            self.open_block = Some("thinking");
        }
        out.push(json!({ "type": "content_block_delta", "index": self.next_block - 1,
            "delta": { "type": "thinking_delta", "thinking": thinking } }));
        out
    }

    pub fn on_content(&mut self, text: &str) -> Vec<Value> {
        let mut out = Vec::new();
        if self.open_block != Some("text") {
            out.extend(self.close_open_block());
            out.push(json!({ "type": "content_block_start", "index": self.next_block,
                "content_block": { "type": "text", "text": "" } }));
            self.next_block += 1;
            self.open_block = Some("text");
        }
        out.push(json!({ "type": "content_block_delta", "index": self.next_block - 1,
            "delta": { "type": "text_delta", "text": text } }));
        out
    }

    pub fn on_finish(&mut self, reason: &str, usage: &Usage) -> Vec<Value> {
        let mut out = self.close_open_block();
        out.push(json!({
            "type": "message_delta",
            "delta": { "stop_reason": map_stop_reason(Some(reason)), "stop_sequence": Value::Null },
            "usage": { "output_tokens": usage.completion_tokens }
        }));
        out.push(json!({ "type": "message_stop" }));
        out
    }

    fn close_open_block(&mut self) -> Vec<Value> {
        let mut out = Vec::new();
        if self.open_block.take().is_some() {
            out.push(json!({ "type": "content_block_stop", "index": self.next_block - 1 }));
        }
        out
    }
}

/// OpenAI ChatRequest → Anthropic Messages 请求体（出站方向，供 zcode/minimax
/// 这类 Anthropic 协议上游使用）。system 消息合并为顶层 `system`；
/// `system_prefix`（如 zcode 3012 官方身份块）置于最前。
pub fn openai_to_anthropic_body(req: &ChatRequest, system_prefix: Option<&str>) -> Value {
    let mut system_parts: Vec<String> = Vec::new();
    if let Some(p) = system_prefix {
        system_parts.push(p.to_string());
    }
    let mut messages: Vec<Value> = Vec::new();
    for m in &req.messages {
        if m.role == "system" {
            let t = m.text();
            if !t.is_empty() {
                system_parts.push(t);
            }
            continue;
        }
        messages.push(json!({ "role": m.role, "content": m.content }));
    }
    let max_tokens = req.raw.get("max_tokens").and_then(Value::as_u64).unwrap_or(8192);
    let mut body = json!({
        "model": &req.model,
        "max_tokens": max_tokens,
        "messages": messages,
    });
    if !system_parts.is_empty() {
        body["system"] = json!(system_parts.join("\n\n"));
    }
    if let Some(temp) = req.raw.get("temperature") {
        body["temperature"] = temp.clone();
    }
    body
}

/// 上游 Anthropic message 响应 → ChatCompletion。
pub fn completion_from_anthropic(model: &str, v: &Value) -> Result<ChatCompletion, String> {
    let content = v
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<String>()
        })
        .unwrap_or_default();
    if content.is_empty() && v.get("type").and_then(Value::as_str) != Some("message") {
        return Err(format!("unexpected upstream body: {}", v));
    }
    let usage = Usage {
        prompt_tokens: v["usage"]["input_tokens"].as_u64().unwrap_or(0),
        completion_tokens: v["usage"]["output_tokens"].as_u64().unwrap_or(0),
        total_tokens: v["usage"]["input_tokens"].as_u64().unwrap_or(0)
            + v["usage"]["output_tokens"].as_u64().unwrap_or(0),
    };
    let stop = v.get("stop_reason").and_then(Value::as_str).unwrap_or("stop");
    let finish = match stop {
        "max_tokens" => "length".to_string(),
        "tool_use" => "tool_calls".to_string(),
        other => other.to_string(),
    };
    let id = v.get("id").and_then(Value::as_str).unwrap_or("msg").to_string();
    Ok(ChatCompletion {
        id: format!("chatcmpl-{id}"),
        object: "chat.completion",
        created: crate::openai::now_ts(),
        model: model.to_string(),
        choices: vec![crate::openai::Choice {
            index: 0,
            message: crate::openai::OutMessage { role: "assistant".into(), content },
            finish_reason: Some(finish),
        }],
        usage,
    })
}
