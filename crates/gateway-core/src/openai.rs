//! OpenAI Chat 协议类型（请求/响应/用量）。
//! 只建模网关需要读写的字段；未知字段在反序列化时保留在 `raw`
//! 里原样透传（各 Provider 上游差异大，网关不丢字段）。

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub stream: bool,
    /// 其余参数（temperature、max_tokens、tools…）原样透传。
    #[serde(flatten)]
    pub raw: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(default)]
    pub content: Value,
    /// assistant 消息的工具调用（OpenAI `tool_calls` 形态，原样保留）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Value>,
    /// `role:tool` 消息关联的调用 id（映射到 assistant `tool_calls[].id`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// assistant 消息的思考链（DeepSeek 风格历史回传；commandcode 需带回上游）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}

impl ChatMessage {
    /// 纯文本视图（多模态数组取全部 text 段拼接）。
    pub fn text(&self) -> String {
        match &self.content {
            Value::String(s) => s.clone(),
            Value::Array(parts) => parts
                .iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join(""),
            _ => String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatCompletion {
    pub id: String,
    pub object: &'static str,
    pub created: u64,
    pub model: String,
    pub choices: Vec<Choice>,
    pub usage: Usage,
}

impl ChatCompletion {
    pub fn new(model: String, content: String, usage: Usage) -> Self {
        Self {
            id: format!("chatcmpl-{}", crate::key::random_id(12)),
            object: "chat.completion",
            created: now_ts(),
            model,
            choices: vec![Choice {
                index: 0,
                message: OutMessage { role: "assistant".into(), content, tool_calls: None },
                finish_reason: Some("stop".into()),
            }],
            usage,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Choice {
    pub index: u32,
    pub message: OutMessage,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OutMessage {
    pub role: String,
    pub content: String,
    /// OpenAI `tool_calls` 形态（仅工具调用响应携带）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Value>,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

impl Usage {
    pub fn sum(p: u64, c: u64) -> Self {
        // 上游 JSON 的 token 数不受信任：饱和加法防溢出 panic
        Self { prompt_tokens: p, completion_tokens: c, total_tokens: p.saturating_add(c) }
    }
}

pub fn now_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
