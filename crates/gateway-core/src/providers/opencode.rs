//! opencode Provider（OpenCode Zen，T4.15）。
//!
//! 协议要点（reference §4.13）：
//! - 推理：标准 OpenAI `/v1/chat/completions`
//! - **无机器指纹**：只发五头（x-opencode-project/session/request/client + UA）
//! - session id 形状受 FreeTier 门禁正则校验：`ses_` + 12 位小写 hex + 14 位 base62
//! - project id = 40 位小写 hex（sha1("git-remote:"+remote) 同形）
//! - 账号槽 + 匿名槽（`public` key）平权混合池：免费全槽轮换
//! - 错误分类按**响应体错误类型名**（`FreeUsageLimitError` 等）不按状态码

use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::openai::{ChatCompletion, ChatRequest, Usage};
use crate::provider::{ChunkStream, Credential, Provider, ProviderError, StreamChunk};
use crate::registry::{ModelInfo, ProviderCatalog};
use crate::route::Route;
use crate::sse::SseParser;

pub const OPENCODE_API_BASE: &str = "https://opencode.ai/zen";

const CHAT_PATH: &str = "/v1/chat/completions";

/// 匿名槽的固定 API key（官方 CLI 同款字面量 `public`）。
pub const OPENCODE_PUBLIC_KEY: &str = "public";

pub struct OpencodeProvider {
    base: String,
    client: reqwest::Client,
}

/// 生成符合 FreeTier 门禁正则的 session id：
/// `ses_` + 12 位小写 hex（6 字节时间戳）+ 14 位 base62（随机）。
pub fn generate_session_id() -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let ts_hex = format!("{ts:012x}");
    let rand_part = crate::key::random_id(7); // 7 bytes = 14 hex chars
    let rand_b62: String = rand_part
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(14)
        .collect();
    // 确保恰好 14 位
    let padded = format!("{rand_b62:0<14}");
    format!("ses_{ts_hex}{padded}")
}

/// 生成 project id：40 位小写 hex（sha1("git-remote:"+remote) 同形）。
pub fn generate_project_id(remote: &str) -> String {
    use sha1::Digest;
    let mut h = sha1::Sha1::new();
    h.update(format!("git-remote:{remote}"));
    format!("{:x}", h.finalize())
}

/// 错误分类按**响应体错误类型名**（不按状态码——Zen 额度错误可带 400/401/403/429 任一）。
pub fn classify_opencode_error(body: &str) -> ProviderError {
    if body.contains("FreeUsageLimitError") {
        return ProviderError::RateLimited {
            retry_after_secs: Some(3600),
            msg: format!("free usage limit: {body}"),
        };
    }
    if body.contains("GoUsageLimitError") {
        return ProviderError::RateLimited {
            retry_after_secs: Some(3600),
            msg: format!("go usage limit: {body}"),
        };
    }
    ProviderError::Upstream(body.to_string())
}

impl OpencodeProvider {
    pub fn new(base: String) -> Self {
        Self { base, client: reqwest::Client::new() }
    }

    pub fn production() -> Self {
        Self::new(OPENCODE_API_BASE.into())
    }

    /// 五头（无机器指纹——1.18.22 逐行核对）：四个 `x-opencode-*` + `User-Agent`
    /// （**不发** `x-session-affinity`/`X-Session-Id`——那是非 opencode 分支的头）。
    fn headers(cred: &Credential, project_id: &str, session_id: &str) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        let ins = reqwest::header::HeaderValue::from_str;
        let _ = ins(&format!("Bearer {}", cred.secret)).map(|x| h.insert("authorization", x));
        let _ = ins(project_id).map(|x| h.insert("x-opencode-project", x));
        let _ = ins(session_id).map(|x| h.insert("x-opencode-session", x));
        let _ = ins(&format!("req-{}", crate::key::random_id(8))).map(|x| h.insert("x-opencode-request", x));
        let _ = ins("opencode/1.18.22").map(|x| h.insert("x-opencode-client", x));
        let _ = ins("opencode/1.18.22").map(|x| h.insert("user-agent", x));
        h
    }

    async fn collect(
        &self,
        cred: &Credential,
        route: &Route,
        req: &ChatRequest,
    ) -> Result<(String, Usage, Option<String>), ProviderError> {
        let session_id = generate_session_id();
        let project_id = generate_project_id("origin");

        let mut body = Map::new();
        for (k, v) in &req.raw {
            body.insert(k.clone(), v.clone());
        }
        body.insert("model".into(), Value::String(route.model.clone()));
        // tool_calls/tool_call_id 原样带回（§7.1.2 静默丢弃是最大敌人）
        body.insert("messages".into(), Value::Array(
            req.messages.iter().map(|m| {
                let mut o = Map::new();
                o.insert("role".into(), Value::String(m.role.clone()));
                o.insert("content".into(), m.content.clone());
                if let Some(tc) = &m.tool_calls { o.insert("tool_calls".into(), tc.clone()); }
                if let Some(id) = &m.tool_call_id { o.insert("tool_call_id".into(), Value::String(id.clone())); }
                Value::Object(o)
            }).collect::<Vec<_>>()
        ));
        body.insert("stream".into(), Value::Bool(true));

        let resp = self
            .client
            .post(format!("{}{}", self.base, CHAT_PATH))
            .headers(Self::headers(cred, &project_id, &session_id))
            .json(&Value::Object(body))
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?.to_vec();
        let text = String::from_utf8_lossy(&bytes).to_string();
        if status != 200 {
            return Err(classify_opencode_error(&text));
        }
        let mut parser = SseParser::new();
        parser.feed(&bytes);
        parser.finalize();
        let mut content = String::new();
        let mut usage = Usage::default();
        let mut finish = None;
        while let Some(data) = parser.next_data() {
            if data.trim() == "[DONE]" { break; }
            if let Ok(v) = serde_json::from_str::<Value>(&data) {
                if let Some(t) = v.pointer("/choices/0/delta/content").and_then(Value::as_str) {
                    content.push_str(t);
                }
                if let Some(u) = v.get("usage") {
                    usage = Usage::sum(
                        u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0),
                        u.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0),
                    );
                }
                if let Some(fr) = v.pointer("/choices/0/finish_reason").and_then(Value::as_str) {
                    finish = Some(fr.to_string());
                }
            }
        }
        Ok((content, usage, finish))
    }
}

#[async_trait]
impl Provider for OpencodeProvider {
    fn id(&self) -> &str {
        "opencode"
    }

    fn catalog(&self) -> ProviderCatalog {
        ProviderCatalog {
            id: "opencode".into(),
            models: vec![ModelInfo { id: "qwen-3.5-coder".into() }],
        }
    }

    async fn complete(&self, cred: &Credential, route: &Route, req: &ChatRequest) -> Result<ChatCompletion, ProviderError> {
        let (text, usage, finish) = self.collect(cred, route, req).await?;
        let mut out = ChatCompletion::new(route.composite(), text, usage);
        if let Some(reason) = finish {
            if reason != "stop" {
                out.choices[0].finish_reason = Some(reason);
            }
        }
        Ok(out)
    }

    async fn stream(&self, cred: &Credential, route: &Route, req: &ChatRequest) -> Result<ChunkStream, ProviderError> {
        let (text, usage, finish) = self.collect(cred, route, req).await?;
        let mut queue: std::collections::VecDeque<Result<StreamChunk, ProviderError>> =
            std::collections::VecDeque::new();
        queue.push_back(Ok(StreamChunk::Role));
        if !text.is_empty() {
            queue.push_back(Ok(StreamChunk::Content(text)));
        }
        queue.push_back(Ok(StreamChunk::Finish { reason: finish.unwrap_or_else(|| "stop".into()), usage }));
        Ok(Box::pin(futures::stream::iter(queue)))
    }
}
