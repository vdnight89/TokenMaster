//! gemini Provider（Google Cloud Code Assist 免费线，Antigravity 客户端身份伪装）。
//!
//! 上游：`POST {base}/v1internal:streamGenerateContent?alt=sse`（SSE 帧）。
//! 协议要点（对照 reference/deepseek-harness-codearts.md §3.6/§4.14）：
//! - 双层信封**每层键字母序**（Go map 序列化语义；serde_json preserve_order 下
//!   显式按序重建，`alphabetize`）。
//! - 五个身份头**写死**（x-machine-id/x-vscode-sessionid 是占位串，不生成随机值）；
//!   流式请求刻意不带 `Accept`；不发 `x-goog-api-key`。
//! - 模型名是准入键：必须带 `-low/-medium/-high/-tiered` 档位后缀，
//!   对外只暴露主名，出站默认补 `-medium`。
//! - system 抽到顶层 `systemInstruction`；assistant 角色 → `model`。
//!
//! project 动态探测（loadCodeAssist）、thoughtSignature 跨轮回填、sessionId 升代
//! 自愈在后续切片接入（T4.2b/c）。

use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::openai::{ChatCompletion, ChatRequest, Usage};
use crate::provider::{ChunkStream, Credential, Provider, ProviderError, StreamChunk};
use crate::registry::{ModelInfo, ProviderCatalog};
use crate::route::Route;
use crate::sse::SseParser;

pub const DEFAULT_BASE: &str = "https://daily-cloudcode-pa.googleapis.com";
const GENERATE_PATH: &str = "/v1internal:streamGenerateContent?alt=sse";
const CLIENT_UA: &str = "antigravity/4.3.0 (cmdc-pak)";
const DEFAULT_EFFORT_SUFFIX: &str = "-medium";

pub struct GeminiProvider {
    base: String,
    project: String,
    session_id: String,
    client: reqwest::Client,
}

impl GeminiProvider {
    pub fn new(base: String, project: String) -> Self {
        Self {
            base,
            project,
            session_id: format!("sess-{}", crate::key::random_id(8)),
            client: reqwest::Client::new(),
        }
    }

    pub fn production() -> Self {
        Self::new(DEFAULT_BASE.into(), "aicode-consumers".into())
    }

    fn identity_headers(&self, cred: &Credential) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        let ins = reqwest::header::HeaderValue::from_str;
        let _ = ins(&format!("Bearer {}", cred.secret)).map(|v| h.insert("authorization", v));
        let _ = ins(CLIENT_UA).map(|v| h.insert("user-agent", v));
        let _ = ins("antigravity").map(|v| h.insert("x-client-name", v));
        let _ = ins("4.3.0").map(|v| h.insert("x-client-version", v));
        let _ = ins("cmdc-pak").map(|v| h.insert("x-machine-id", v));
        let _ = ins("proxy").map(|v| h.insert("x-vscode-sessionid", v));
        h
    }

    /// 对外主名 → 上游准入名（无档位后缀时默认 `-medium`）。
    fn wire_model(model: &str) -> String {
        const SUFFIXES: [&str; 4] = ["-low", "-medium", "-high", "-tiered"];
        if SUFFIXES.iter().any(|s| model.ends_with(s)) {
            model.to_string()
        } else {
            format!("{model}{DEFAULT_EFFORT_SUFFIX}")
        }
    }

    fn build_envelope(&self, route: &Route, req: &ChatRequest) -> Value {
        let mut contents: Vec<Value> = Vec::new();
        let mut system_parts: Vec<String> = Vec::new();
        for m in &req.messages {
            if m.role == "system" {
                let t = m.text();
                if !t.is_empty() {
                    system_parts.push(t);
                }
                continue;
            }
            let role = if m.role == "assistant" { "model" } else { "user" };
            contents.push(serde_json::json!({ "parts": [{ "text": m.text() }], "role": role }));
        }
        let mut request = Map::new();
        request.insert("contents".into(), Value::Array(contents));
        let mut generation = Map::new();
        if let Some(t) = req.raw.get("temperature") {
            generation.insert("temperature".into(), t.clone());
        }
        request.insert("generationConfig".into(), Value::Object(generation));
        request.insert("sessionId".into(), Value::String(self.session_id.clone()));
        if !system_parts.is_empty() {
            request.insert(
                "systemInstruction".into(),
                serde_json::json!({ "parts": [{ "text": system_parts.join("\n\n") }] }),
            );
        }
        let mut root = Map::new();
        root.insert("model".into(), Value::String(Self::wire_model(&route.model)));
        root.insert("project".into(), Value::String(self.project.clone()));
        root.insert("request".into(), Value::Object(request));
        root.insert("requestId".into(), Value::String(crate::key::random_id(10)));
        root.insert("userAgent".into(), Value::String(CLIENT_UA.into()));
        alphabetize(&Value::Object(root))
    }

    async fn send(
        &self,
        cred: &Credential,
        route: &Route,
        req: &ChatRequest,
    ) -> Result<reqwest::Response, ProviderError> {
        let body = self.build_envelope(route, req);
        self.client
            .post(format!("{}{}", self.base, GENERATE_PATH))
            .headers(self.identity_headers(cred))
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))
    }
}

/// 递归按键字母序重建对象（serde_json preserve_order 下即输出字母序）。
pub fn alphabetize(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let mut out = Map::new();
            for k in keys {
                out.insert(k.clone(), alphabetize(&m[k]));
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(alphabetize).collect()),
        other => other.clone(),
    }
}

/// HTTP 状态 → ProviderError（公开供单测）。
pub fn map_status_error(status: u16, msg: String) -> ProviderError {
    match status {
        401 => ProviderError::Credential(format!("401 token rejected: {msg}")),
        429 => ProviderError::RateLimited { retry_after_secs: Some(60), msg: format!("429 quota: {msg}") },
        404 => ProviderError::BadRequest(format!("404 model not admitted (needs effort suffix): {msg}")),
        code => ProviderError::Upstream(format!("http {code}: {msg}")),
    }
}

/// 一帧 SSE data：返回 (本帧文本增量, usage 是否出现, usage)。
fn parse_frame(v: &Value) -> (String, Option<Usage>) {
    let text: String = v
        .pointer("/candidates/0/content/parts")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<String>()
        })
        .unwrap_or_default();
    let usage = v.get("usageMetadata").map(|u| Usage {
        prompt_tokens: u.get("promptTokenCount").and_then(Value::as_u64).unwrap_or(0),
        completion_tokens: u.get("candidatesTokenCount").and_then(Value::as_u64).unwrap_or(0),
        total_tokens: u.get("promptTokenCount").and_then(Value::as_u64).unwrap_or(0)
            + u.get("candidatesTokenCount").and_then(Value::as_u64).unwrap_or(0),
    });
    (text, usage)
}

#[async_trait]
impl Provider for GeminiProvider {
    fn id(&self) -> &str {
        "gemini"
    }

    fn catalog(&self) -> ProviderCatalog {
        ProviderCatalog {
            id: "gemini".into(),
            models: vec![
                ModelInfo { id: "gemini-3-pro".into() },
                ModelInfo { id: "gemini-3-flash".into() },
            ],
        }
    }

    async fn complete(&self, cred: &Credential, route: &Route, req: &ChatRequest) -> Result<ChatCompletion, ProviderError> {
        let resp = self.send(cred, route, req).await?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if status != 200 {
            return Err(map_status_error(status, String::from_utf8_lossy(&bytes).to_string()));
        }
        let mut parser = SseParser::new();
        parser.feed(&bytes);
        parser.finalize();
        let mut text = String::new();
        let mut usage = Usage::default();
        while let Some(data) = parser.next_data() {
            let Ok(v) = serde_json::from_str::<Value>(&data) else { continue };
            let (t, u) = parse_frame(&v);
            text.push_str(&t);
            if let Some(u2) = u {
                usage = u2;
            }
        }
        Ok(ChatCompletion::new(route.composite(), text, usage))
    }

    async fn stream(&self, cred: &Credential, route: &Route, req: &ChatRequest) -> Result<ChunkStream, ProviderError> {
        let resp = self.send(cred, route, req).await?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if status != 200 {
            return Err(map_status_error(status, String::from_utf8_lossy(&bytes).to_string()));
        }
        let mut parser = SseParser::new();
        parser.feed(&bytes);
        parser.finalize();
        let mut queue: std::collections::VecDeque<Result<StreamChunk, ProviderError>> =
            std::collections::VecDeque::new();
        queue.push_back(Ok(StreamChunk::Role));
        let mut usage = Usage::default();
        while let Some(data) = parser.next_data() {
            let Ok(v) = serde_json::from_str::<Value>(&data) else { continue };
            let (t, u) = parse_frame(&v);
            if !t.is_empty() {
                queue.push_back(Ok(StreamChunk::Content(t)));
            }
            if let Some(u2) = u {
                usage = u2;
            }
        }
        // 上游一次性返回帧流；帧尽即完成
        queue.push_back(Ok(StreamChunk::Finish { reason: "stop".into(), usage }));
        Ok(Box::pin(futures::stream::iter(queue)))
    }
}
