//! zcode（智谱 ZCode/z.ai coding plan）Provider。
//!
//! 上游（start-plan 通道）：`POST {base}/api/v1/zcode-plan/anthropic/v1/messages`
//! ——Anthropic Messages 协议，Bearer JWT（zcodejwttoken）+ 身份头。
//! coding-plan 订阅通道（api.z.ai/api/anthropic，OAuth 换 coding_plan_key）在
//! 登录切片（T4.1b）接入。
//!
//! 错误语义（对照 reference/deepseek-harness-codearts.md zcode 节）：
//! - 401 → Credential（JWT 失效，不可续期，需重登）
//! - 429 → RateLimited（读 Retry-After，缺省 60s）
//! - 3012（身份块准入，HTTP 405）→ RateLimited 30 分钟（账号冷却惩罚）
//! - 3007（验证码）→ RateLimited（验证码载体链路在 T6.3 接入后自动恢复）
//!
//! 3012 要求 system 携带官方身份块（cliPrefix+stable 2898 字符，逐字官方文本）。
//! 真实上游前必须通过 `with_system_prefix` 注入从本机 ZCode 安装提取的块
//! （zcode-pool prompt.rs 的做法）；stub 测试不需要。

use async_trait::async_trait;
use serde_json::Value;

use crate::anthropic::{completion_from_anthropic, openai_to_anthropic_body};
use crate::openai::{ChatCompletion, ChatRequest};
use crate::provider::{ChunkStream, Credential, Provider, ProviderError, StreamChunk};
use crate::registry::{ModelInfo, ProviderCatalog};
use crate::route::Route;

pub const DEFAULT_BASE: &str = "https://zcode.z.ai";
pub const DEFAULT_CLIENT_VERSION: &str = "3.14.4";

/// start-plan 通道推理路径。
const MESSAGES_PATH: &str = "/api/v1/zcode-plan/anthropic/v1/messages";

/// 3012 的账号冷却（30 分钟，对照 dsh 消融实测）。
const IDENTITY_COOLDOWN_SECS: u64 = 30 * 60;

pub struct ZcodeProvider {
    base: String,
    client: reqwest::Client,
    version: String,
    device_mid: String,
    system_prefix: Option<String>,
}

impl ZcodeProvider {
    pub fn new(base: String) -> Self {
        Self {
            base,
            client: reqwest::Client::new(),
            version: DEFAULT_CLIENT_VERSION.into(),
            device_mid: format!(
                "{}-{}-{}-{}",
                crate::key::random_id(4),
                crate::key::random_id(2),
                crate::key::random_id(2),
                crate::key::random_id(6)
            ),
            system_prefix: None,
        }
    }

    pub fn production() -> Self {
        Self::new(DEFAULT_BASE.into())
    }

    /// 注入官方身份块（zcode-pool prompt.rs 从本机 ZCode 提取；真实上游必需）。
    pub fn with_system_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.system_prefix = Some(prefix.into());
        self
    }

    fn identity_headers(&self, cred: &Credential) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        let ins = reqwest::header::HeaderValue::from_str;
        let _ = ins(&format!("Bearer {}", cred.secret)).map(|v| h.insert("authorization", v));
        let _ = ins("2023-06-01").map(|v| h.insert("anthropic-version", v));
        let _ = ins(&format!("ZCode/{}", self.version)).map(|v| h.insert("user-agent", v));
        let _ = ins("https://zcode.z.ai").map(|v| h.insert("http-referer", v));
        let _ = ins(&self.version).map(|v| h.insert("x-zcode-app-version", v));
        let _ = ins("stable").map(|v| h.insert("x-release-channel", v));
        let _ = ins("zh-CN").map(|v| h.insert("x-client-language", v));
        let _ = ins("Asia/Shanghai").map(|v| h.insert("x-client-timezone", v));
        let _ = ins(&self.device_mid).map(|v| h.insert("x-device-mid", v));
        let _ = ins("win32").map(|v| h.insert("x-platform", v));
        let _ = ins("windows").map(|v| h.insert("x-os-category", v));
        h
    }

    async fn post_messages(&self, cred: &Credential, route: &Route, req: &ChatRequest) -> Result<ChatCompletion, ProviderError> {
        // 上游要裸模型名（route.model 已剥离 provider 前缀）
        let mut upstream_req = req.clone();
        upstream_req.model = route.model.clone();
        let body = openai_to_anthropic_body(&upstream_req, self.system_prefix.as_deref());
        let resp = self
            .client
            .post(format!("{}{}", self.base, MESSAGES_PATH))
            .headers(self.identity_headers(cred))
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if resp.status().as_u16() == 200 {
            let v: Value = resp.json().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
            return completion_from_anthropic(&route.composite(), &v)
                .map_err(ProviderError::Upstream);
        }
        Err(map_upstream_error(resp).await)
    }
}

/// 非即改即用的错误响应 → ProviderError（complete 与 stream 共用）。
async fn map_upstream_error(resp: reqwest::Response) -> ProviderError {
    let status = resp.status().as_u16();
    let retry_after = resp
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());
    let text = resp.text().await.unwrap_or_default();
    match status {
        401 => ProviderError::Credential(format!("401 jwt rejected: {}", truncate(&text))),
        429 => ProviderError::RateLimited {
            retry_after_secs: Some(retry_after.unwrap_or(60)),
            msg: truncate(&text),
        },
        // 3012：官方以 405 + code 3012 表达身份块准入失败（dsh 实测）
        405 if text.contains("3012") => ProviderError::RateLimited {
            retry_after_secs: Some(IDENTITY_COOLDOWN_SECS),
            msg: format!("identity block rejected (3012): {}", truncate(&text)),
        },
        400 if text.contains("3007") => ProviderError::RateLimited {
            retry_after_secs: Some(60),
            msg: "captcha required (3007)".into(),
        },
        code => ProviderError::Upstream(format!("http {code}: {}", truncate(&text))),
    }
}

/// Anthropic stop_reason → OpenAI finish_reason。
fn map_stop_reason(r: &str) -> String {
    match r {
        "max_tokens" => "length".into(),
        "tool_use" => "tool_calls".into(),
        _ => "stop".into(),
    }
}

type ByteChunkStream = futures::stream::BoxStream<'static, Result<Vec<u8>, reqwest::Error>>;

/// zcode 流式状态机：上游 Anthropic SSE 事件 → StreamChunk。
struct ZcodeStreamState {
    bytes: ByteChunkStream,
    parser: crate::sse::SseParser,
    input_tokens: u64,
    output_tokens: u64,
    stop_reason: String,
    queue: std::collections::VecDeque<Result<StreamChunk, ProviderError>>,
    role_sent: bool,
    done: bool,
}

impl ZcodeStreamState {
    fn on_event(&mut self, data: &str) {
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            return; // 忽略无法解析的载荷（如 ping）
        };
        match v.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                self.input_tokens = v["message"]["usage"]["input_tokens"].as_u64().unwrap_or(0);
                if !self.role_sent {
                    self.role_sent = true;
                    self.queue.push_back(Ok(StreamChunk::Role));
                }
            }
            Some("content_block_delta") => {
                let delta = &v["delta"];
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        if let Some(t) = delta.get("text").and_then(Value::as_str) {
                            self.queue.push_back(Ok(StreamChunk::Content(t.to_string())));
                        }
                    }
                    Some("thinking_delta") => {
                        if let Some(t) = delta.get("thinking").and_then(Value::as_str) {
                            self.queue.push_back(Ok(StreamChunk::Reasoning(t.to_string())));
                        }
                    }
                    _ => {}
                }
            }
            Some("message_delta") => {
                if let Some(stop) = v["delta"]["stop_reason"].as_str() {
                    self.stop_reason = map_stop_reason(stop);
                }
                if let Some(out) = v["usage"]["output_tokens"].as_u64() {
                    self.output_tokens = out;
                }
            }
            Some("message_stop") => {
                let usage = crate::openai::Usage::sum(self.input_tokens, self.output_tokens);
                self.queue.push_back(Ok(StreamChunk::Finish {
                    reason: self.stop_reason.clone(),
                    usage,
                }));
            }
            _ => {}
        }
    }
}

fn truncate(s: &str) -> String {
    s.chars().take(180).collect()
}

#[async_trait]
impl Provider for ZcodeProvider {
    fn id(&self) -> &str {
        "zcode"
    }

    fn catalog(&self) -> ProviderCatalog {
        ProviderCatalog {
            id: "zcode".into(),
            models: vec![
                ModelInfo { id: "glm-4.7".into() },
                ModelInfo { id: "glm-4.7-air".into() },
                ModelInfo { id: "glm-4.6".into() },
            ],
        }
    }

    async fn complete(&self, cred: &Credential, route: &Route, req: &ChatRequest) -> Result<ChatCompletion, ProviderError> {
        self.post_messages(cred, route, req).await
    }

    async fn stream(&self, cred: &Credential, route: &Route, req: &ChatRequest) -> Result<ChunkStream, ProviderError> {
        let mut upstream_req = req.clone();
        upstream_req.model = route.model.clone();
        upstream_req.stream = true;
        let body = openai_to_anthropic_body(&upstream_req, self.system_prefix.as_deref());
        let resp = self
            .client
            .post(format!("{}{}", self.base, MESSAGES_PATH))
            .headers(self.identity_headers(cred))
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if resp.status().as_u16() != 200 {
            return Err(map_upstream_error(resp).await);
        }
        let bytes: ByteChunkStream = {
            use futures::StreamExt;
            resp.bytes_stream().map(|r| r.map(|b| b.to_vec())).boxed()
        };
        let state = ZcodeStreamState {
            bytes,
            parser: crate::sse::SseParser::new(),
            input_tokens: 0,
            output_tokens: 0,
            stop_reason: "stop".into(),
            queue: std::collections::VecDeque::new(),
            role_sent: false,
            done: false,
        };
        let stream = futures::stream::unfold(state, |mut st| async move {
            use futures::StreamExt;
            loop {
                if let Some(item) = st.queue.pop_front() {
                    return Some((item, st));
                }
                if st.done {
                    return None;
                }
                match st.bytes.next().await {
                    Some(Ok(chunk)) => {
                        st.parser.feed(&chunk);
                        while let Some(data) = st.parser.next_data() {
                            st.on_event(&data);
                        }
                    }
                    Some(Err(e)) => {
                        st.done = true;
                        return Some((Err(ProviderError::Upstream(e.to_string())), st));
                    }
                    None => {
                        // 上游结束；没有 message_stop 就不伪造 Finish（截断保持诚实）
                        st.done = true;
                        st.parser.finalize();
                        while let Some(data) = st.parser.next_data() {
                            st.on_event(&data);
                        }
                        if let Some(item) = st.queue.pop_front() {
                            return Some((item, st));
                        }
                        return None;
                    }
                }
            }
        });
        Ok(Box::pin(stream))
    }
}
