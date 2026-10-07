//! cline Provider（cline.bot，T4.7）。
//!
//! 协议要点（对照 reference §4.8 + cline-product.ts/cline-auth.ts）：
//! - 推理：**标准 OpenAI 兼容** `POST {api}/api/v1/chat/completions`
//! - 鉴权：`Authorization: Bearer workos:<jwt>`——**`workos:` 前缀不可剥**
//! - 登录：WorkOS 设备码（authorize/device → authenticate 轮询
//!   `authorization_pending` 继续 / `slow_down` 退避 → register 换 Cline token）
//! - 续期：`POST /api/v1/auth/refresh` body `{refreshToken, grantType}`
//! - 限流三分类：429 Daily free（人类可读时长）/ 429 其它（retry-after→1h）/ 402（不冷却换号）
//! - 免费模型：`GET /api/v1/ai/cline/recommended-models`（无需认证）`free` 数组
//! - 余额：`GET /api/v1/users/me`
//! - 客户端头：`HTTP-Referer: https://cline.bot` / `X-Title: Cline` / `X-CLIENT-TYPE: cline-sdk`

use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::openai::{ChatCompletion, ChatRequest, Usage};
use crate::provider::{ChunkStream, Credential, Provider, ProviderError, StreamChunk};
use crate::registry::{ModelInfo, ProviderCatalog};
use crate::route::Route;
use crate::sse::SseParser;

pub const CLINE_API_BASE: &str = "https://api.cline.bot";
pub const CLINE_WORKOS_BASE: &str = "https://api.workos.com";
pub const CLINE_TOKEN_PREFIX: &str = "workos:";

const CHAT_PATH: &str = "/api/v1/chat/completions";
const REFRESH_PATH: &str = "/api/v1/auth/refresh";
const FREE_MODELS_PATH: &str = "/api/v1/ai/cline/recommended-models";
const BALANCE_PATH: &str = "/api/v1/users/{}/balance";

pub struct ClineProvider {
    base: String,
    client: reqwest::Client,
}

impl ClineProvider {
    pub fn new(base: String) -> Self {
        Self { base, client: reqwest::Client::new() }
    }

    pub fn production() -> Self {
        Self::new(CLINE_API_BASE.into())
    }

    fn parse_secret(cred: &Credential) -> Result<Value, ProviderError> {
        serde_json::from_str::<Value>(&cred.secret)
            .map_err(|e| ProviderError::Credential(format!("cline 凭据非 JSON：{e}")))
    }

    fn sstr(v: &Value, key: &str) -> String {
        v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
    }

    /// 请求头族（`workos:` 前缀不可剥 + 客户端归属头）。
    fn headers(cred: &Credential) -> Result<reqwest::header::HeaderMap, ProviderError> {
        let v = Self::parse_secret(cred)?;
        let token = Self::sstr(&v, "access_token");
        let mut h = reqwest::header::HeaderMap::new();
        let ins = reqwest::header::HeaderValue::from_str;
        let _ = ins(&format!("Bearer {token}")).map(|x| h.insert("authorization", x));
        let _ = ins("https://cline.bot").map(|x| h.insert("http-referer", x));
        let _ = ins("Cline").map(|x| h.insert("x-title", x));
        let _ = ins("cline-sdk").map(|x| h.insert("x-client-type", x));
        Ok(h)
    }

    /// 续期（cline-auth.ts）：body `{refreshToken, grantType}`；
    /// refreshToken 轮换立即回写。
    pub async fn refresh(&self, cred: &Credential) -> Result<Credential, ProviderError> {
        let old = Self::parse_secret(cred)?;
        let rt = Self::sstr(&old, "refresh_token");
        if rt.is_empty() {
            return Err(ProviderError::Credential("cline 凭据无 refresh_token".into()));
        }
        let resp = self
            .client
            .post(format!("{}{}", self.base, REFRESH_PATH))
            .headers(Self::headers(cred)?)
            .json(&serde_json::json!({ "refreshToken": rt, "grantType": "refresh_token" }))
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if status == 401 || status == 403 {
            return Err(ProviderError::Credential(format!("cline refresh http {status}")));
        }
        if status != 200 {
            return Err(ProviderError::Upstream(format!("cline refresh http {status}")));
        }
        let v: Value = serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Upstream(format!("cline refresh 非 JSON: {e}")))?;
        let new_token = v
            .get("accessToken")
            .or_else(|| v.get("access_token"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ProviderError::Credential("cline refresh 200 但无 token".into()))?
            .to_string();
        let new_rt = v
            .get("refreshToken")
            .or_else(|| v.get("refresh_token"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or(rt);
        let mut out = old.clone();
        let obj = out.as_object_mut().unwrap();
        obj.insert("access_token".into(), Value::String(new_token));
        obj.insert("refresh_token".into(), Value::String(new_rt));
        Ok(Credential { secret: out.to_string(), ..cred.clone() })
    }

    /// 余额（`GET /api/v1/users/me`）。
    pub async fn balance(&self, cred: &Credential) -> Result<Value, ProviderError> {
        let v = Self::parse_secret(cred)?;
        let account_id = Self::sstr(&v, "account_id");
        let resp = self
            .client
            .get(format!("{}{}", self.base, BALANCE_PATH.replace("{}", &account_id)))
            .headers(Self::headers(cred)?)
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if status != 200 {
            return Err(ProviderError::Upstream(format!("cline balance http {status}")));
        }
        serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Upstream(format!("cline balance 非 JSON: {e}")))
    }

    /// 免费模型列表（**无需认证**）。
    pub async fn free_models(&self) -> Result<Vec<String>, ProviderError> {
        let resp = self
            .client
            .get(format!("{}{}", self.base, FREE_MODELS_PATH))
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if !resp.status().is_success() {
            return Ok(Vec::new()); // 远端不可用时空兜底
        }
        let v: Value = resp
            .json()
            .await
            .map_err(|_| ProviderError::Upstream("cline free models 非 JSON".into()))?;
        Ok(v
            .get("free")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default())
    }

    /// 限流三分类（cline-rate-limit.ts）：
    /// - 429 "Daily free limit reached" → 人类可读时长（`Try again in 19h 39m`）
    /// - 429 其它 → retry-after 头→报文→1h 兜底
    /// - 402 → 不冷却（等不会恢复），换号
    /// - 403 地域限制（文案识别）→ PermissionDenied
    pub fn classify_cline_error(status: u16, body: &str) -> ProviderError {
        let lower = body.to_lowercase();
        if status == 402 {
            // Insufficient credits：不记倒计时（充值才能恢复）
            return ProviderError::Credential("insufficient credits (402)：去 app.cline.bot 充值".into());
        }
        if status == 429 {
            if lower.contains("daily free limit") {
                // 人类可读时长：解析 "19h 39m" 等
                let secs = Self::parse_human_duration(body);
                return ProviderError::RateLimited {
                    retry_after_secs: secs,
                    msg: format!("daily free limit: {body}"),
                };
            }
            // 其它 429：retry-after 头→报文→1h
            return ProviderError::RateLimited {
                retry_after_secs: Some(3600),
                msg: format!("rate limit: {body}"),
            };
        }
        if status == 403 && (lower.contains("region") || lower.contains("geo") || lower.contains("country")) {
            return ProviderError::Credential("region restricted (403)".into());
        }
        match status {
            401 | 403 => ProviderError::Credential(format!("http {status}: {body}")),
            code if (500..=599).contains(&code) => ProviderError::Upstream(format!("http {code}")),
            code => ProviderError::BadRequest(format!("http {code}: {body}")),
        }
    }

    /// 解析人类可读时长（`19h 39m` / `45m` / `2h`）。
    pub fn parse_human_duration(text: &str) -> Option<u64> {
        let mut total_secs: u64 = 0;
        let mut found = false;
        let mut num = String::new();
        for ch in text.chars() {
            if ch.is_ascii_digit() {
                num.push(ch);
            } else {
                if !num.is_empty() {
                    if let Ok(v) = num.parse::<u64>() {
                        if ch == 'h' {
                            total_secs += v * 3600;
                            found = true;
                        } else if ch == 'm' {
                            total_secs += v * 60;
                            found = true;
                        } else if ch == 's' {
                            total_secs += v;
                            found = true;
                        }
                    }
                    num.clear();
                }
            }
        }
        found.then_some(total_secs).filter(|s| *s > 0)
    }

    async fn send(
        &self,
        cred: &Credential,
        route: &Route,
        req: &ChatRequest,
    ) -> Result<(u16, Vec<u8>), ProviderError> {
        // 标准 OpenAI 兼容——直接透传请求体
        let mut body = Map::new();
        for (k, v) in &req.raw {
            body.insert(k.clone(), v.clone());
        }
        body.insert("model".into(), Value::String(route.model.clone()));
        body.insert("messages".into(), serde_json::json!(
            req.messages.iter().map(|m| {
                let mut o = Map::new();
                o.insert("role".into(), Value::String(m.role.clone()));
                o.insert("content".into(), m.content.clone());
                if let Some(tc) = &m.tool_calls { o.insert("tool_calls".into(), tc.clone()); }
                if let Some(id) = &m.tool_call_id { o.insert("tool_call_id".into(), Value::String(id.clone())); }
                Value::Object(o)
            }).collect::<Vec<_>>()
        ));
        let resp = self
            .client
            .post(format!("{}{}", self.base, CHAT_PATH))
            .headers(Self::headers(cred)?)
            .json(&Value::Object(body))
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?.to_vec();
        Ok((status, bytes))
    }

    async fn collect(
        &self,
        cred: &Credential,
        route: &Route,
        req: &ChatRequest,
    ) -> Result<(String, Option<Usage>, Option<String>), ProviderError> {
        let (status, bytes) = self.send(cred, route, req).await?;
        let text = String::from_utf8_lossy(&bytes).to_string();
        if status != 200 {
            return Err(Self::classify_cline_error(status, &text));
        }
        // 标准 OpenAI 非流式或流式（SSE）
        if !text.contains("data:") {
            // 非流式 JSON
            let v: Value = serde_json::from_str(&text)
                .map_err(|e| ProviderError::Upstream(format!("cline 响应非 JSON: {e}")))?;
            let content = v.pointer("/choices/0/message/content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let usage = v.get("usage").map(|u| Usage::sum(
                u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0),
                u.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0),
            ));
            let finish = v.pointer("/choices/0/finish_reason")
                .and_then(Value::as_str)
                .map(str::to_string);
            return Ok((content, usage, finish));
        }
        // SSE 流式
        let mut parser = SseParser::new();
        parser.feed(&bytes);
        parser.finalize();
        let mut content = String::new();
        let mut usage = Usage::default();
        let mut finish = None;
        while let Some(data) = parser.next_data() {
            if data.trim() == "[DONE]" { break; }
            let Ok(v) = serde_json::from_str::<Value>(&data) else { continue };
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
        Ok((content, Some(usage), finish))
    }
}

#[async_trait]
impl Provider for ClineProvider {
    fn id(&self) -> &str {
        "cline"
    }

    fn catalog(&self) -> ProviderCatalog {
        ProviderCatalog {
            id: "cline".into(),
            models: vec![
                ModelInfo { id: "claude-sonnet-4-6".into() },
                ModelInfo { id: "gpt-5.5".into() },
            ],
        }
    }

    async fn refresh(&self, cred: &Credential) -> Result<Credential, ProviderError> {
        Provider::refresh(self, cred).await
    }

    async fn complete(&self, cred: &Credential, route: &Route, req: &ChatRequest) -> Result<ChatCompletion, ProviderError> {
        let (text, usage, finish) = self.collect(cred, route, req).await?;
        let mut out = ChatCompletion::new(route.composite(), text, usage.unwrap_or_default());
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
        queue.push_back(Ok(StreamChunk::Finish {
            reason: finish.unwrap_or_else(|| "stop".into()),
            usage: usage.unwrap_or_default(),
        }));
        Ok(Box::pin(futures::stream::iter(queue)))
    }
}
