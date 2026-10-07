//! buddy Provider（腾讯 CodeBuddy 中国版，T4.9）。
//!
//! 协议要点（对照 reference §4.2）：
//! - 推理：标准 OpenAI SSE `POST {api}/v2/chat/completions`
//! - 登录：external-link 轮询式（`/v2/plugin/auth/state` → 浏览器 →
//!   `/v2/plugin/auth/token?state=` 轮询，1s 间隔 5 分钟超时，11217=继续）
//! - 续期：`POST /v2/plugin/auth/token/refresh` + **`X-Refresh-Token` 头**
//! - 请求头：`X-Domain`（产品优先 `||` 不能 `??`）+ `X-Product-Code` 等
//! - 限流：业务码 `11140` 按账号生效（文案说内容审核但不是 AUTH）
//! - 余额：`POST /v2/billing/meter/get-user-resource`（body `{}`，双层嵌套）
//! - 签到：两步 `checkin-activity-status` → `daily-checkin`（幂等=400+10001）

use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::openai::{ChatCompletion, ChatRequest, Usage};
use crate::provider::{ChunkStream, Credential, Provider, ProviderError, StreamChunk};
use crate::registry::{ModelInfo, ProviderCatalog};
use crate::route::Route;
use crate::sse::SseParser;

pub const BUDDY_API_BASE: &str = "https://copilot.tencent.com";
pub const BUDDY_PRODUCT_CODE: &str = "codebuddy";
pub const BUDDY_PRODUCT: &str = "CodeBuddy";
pub const BUDDY_IDE_NAME: &str = "Trae";
pub const BUDDY_IDE_VERSION: &str = "0.5.0";

const CHAT_PATH: &str = "/v2/chat/completions";
const AUTH_STATE_PATH: &str = "/v2/plugin/auth/state";
const AUTH_TOKEN_PATH: &str = "/v2/plugin/auth/token";
const REFRESH_PATH: &str = "/v2/plugin/auth/token/refresh";
const BALANCE_PATH: &str = "/v2/billing/meter/get-user-resource";

#[derive(Debug, Clone, PartialEq)]
pub struct BuddyBalance {
    /// Σ Accounts[].CapacityRemainPrecise
    pub total_remaining: f64,
}

pub struct BuddyProvider {
    base: String,
    domain: String,
    client: reqwest::Client,
}

impl BuddyProvider {
    pub fn new(base: String) -> Self {
        Self { base, domain: String::new(), client: reqwest::Client::new() }
    }

    pub fn production() -> Self {
        Self::new(BUDDY_API_BASE.into())
    }

    /// X-Domain 值可由凭据/产品配置覆盖。
    pub fn with_domain(mut self, domain: String) -> Self {
        self.domain = domain;
        self
    }

    fn parse_secret(cred: &Credential) -> Result<Value, ProviderError> {
        serde_json::from_str::<Value>(&cred.secret)
            .map_err(|e| ProviderError::Credential(format!("buddy 凭据非 JSON：{e}")))
    }

    fn sstr(v: &Value, key: &str) -> String {
        v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
    }

    /// 推理头族（buddy-adapter.ts:1948-1984）。
    /// X-Domain：**产品优先 `||`**（空串时 `??` 不生效——protocol-wire.md:671-694）。
    fn headers(cred: &Credential, domain_override: &str) -> Result<reqwest::header::HeaderMap, ProviderError> {
        let v = Self::parse_secret(cred)?;
        let token = Self::sstr(&v, "access_token");
        let domain = if !domain_override.is_empty() {
            domain_override
        } else {
            &Self::sstr(&v, "domain")
        };
        let mut h = reqwest::header::HeaderMap::new();
        let ins = reqwest::header::HeaderValue::from_str;
        let _ = ins(&format!("Bearer {token}")).map(|x| h.insert("authorization", x));
        let _ = ins("text/event-stream").map(|x| h.insert("accept", x));
        // X-Domain 产品优先（|| 语义——空串时 ?? 不生效）
        if !domain.is_empty() {
            let _ = ins(domain).map(|x| h.insert("x-domain", x));
        }
        let _ = ins(BUDDY_PRODUCT_CODE).map(|x| h.insert("x-product-code", x));
        let _ = ins(BUDDY_PRODUCT).map(|x| h.insert("x-product", x));
        let _ = ins(BUDDY_IDE_NAME).map(|x| h.insert("x-ide-name", x));
        let _ = ins("icube").map(|x| h.insert("x-ide-type", x));
        let _ = ins(BUDDY_IDE_VERSION).map(|x| h.insert("x-ide-version", x));
        let _ = ins("conversation").map(|x| h.insert("x-agent-purpose", x));
        Ok(h)
    }

    /// external-link 登录第一步：`POST /v2/plugin/auth/state?platform=ide`
    /// → 返回 login_url + state。
    pub async fn login_state(&self) -> Result<(String, String), ProviderError> {
        let resp = self
            .client
            .post(format!("{}{}?platform=ide", self.base, AUTH_STATE_PATH))
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if status != 200 {
            return Err(ProviderError::Upstream(format!("auth state http {status}")));
        }
        let v: Value = serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Upstream(format!("auth state 非 JSON: {e}")))?;
        let state = v.get("state").or_else(|| v.get("data").and_then(|d| d.get("state")))
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Upstream("auth state 缺 state".into()))?
            .to_string();
        let url = v.get("login_url").or_else(|| v.get("url"))
            .and_then(Value::as_str)
            .unwrap_or("https://www.codebuddy.cn/login")
            .to_string();
        Ok((state, url))
    }

    /// 轮询 token：11217=未就绪继续。
    pub async fn login_poll(&self, state: &str) -> Result<Option<Credential>, ProviderError> {
        let resp = self
            .client
            .get(format!("{}{}?state={}", self.base, AUTH_TOKEN_PATH, state))
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        // 11217 = 未就绪继续轮询
        if v.get("code").and_then(Value::as_i64) == Some(11217) {
            return Ok(None);
        }
        if status != 200 {
            return Err(ProviderError::Upstream(format!("auth token http {status}")));
        }
        let token = v.pointer("/data/access_token")
            .or_else(|| v.get("access_token"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ProviderError::Upstream("auth token 缺 access_token".into()))?
            .to_string();
        let refresh = v.pointer("/data/refresh_token")
            .or_else(|| v.get("refresh_token"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let secret = serde_json::json!({
            "access_token": token,
            "refresh_token": refresh,
        })
        .to_string();
        Ok(Some(Credential { account_id: String::new(), secret }))
    }

    /// 续期：`POST /v2/plugin/auth/token/refresh` + **`X-Refresh-Token` 头**。
    pub async fn refresh(&self, cred: &Credential) -> Result<Credential, ProviderError> {
        let old = Self::parse_secret(cred)?;
        let rt = Self::sstr(&old, "refresh_token");
        if rt.is_empty() {
            return Err(ProviderError::Credential("buddy 凭据无 refresh_token".into()));
        }
        let mut h = reqwest::header::HeaderMap::new();
        let ins = reqwest::header::HeaderValue::from_str;
        let _ = ins(&format!("Bearer {}", Self::sstr(&old, "access_token"))).map(|x| h.insert("authorization", x));
        let _ = ins(&rt).map(|x| h.insert("x-refresh-token", x));
        let _ = ins("application/json").map(|x| h.insert("content-type", x));
        let resp = self
            .client
            .post(format!("{}{}", self.base, REFRESH_PATH))
            .headers(h)
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if status == 401 || status == 403 {
            return Err(ProviderError::Credential(format!("buddy refresh http {status}")));
        }
        if status != 200 {
            return Err(ProviderError::Upstream(format!("buddy refresh http {status}")));
        }
        let v: Value = serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Upstream(format!("buddy refresh 非 JSON: {e}")))?;
        let new_token = v
            .pointer("/data/access_token")
            .or_else(|| v.get("access_token"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ProviderError::Credential("buddy refresh 200 但无 token".into()))?
            .to_string();
        let new_rt = v
            .pointer("/data/refresh_token")
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

    /// 余额：`POST /v2/billing/meter/get-user-resource`（body `{}`，**双层嵌套**
    /// `data.Response.Data.Accounts[]`；总额用 `CapacityRemainPrecise` 相加）。
    pub async fn balance(&self, cred: &Credential) -> Result<BuddyBalance, ProviderError> {
        let resp = self
            .client
            .post(format!("{}{}", self.base, BALANCE_PATH))
            .headers(Self::headers(cred, &self.domain)?)
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if status != 200 {
            return Err(ProviderError::Upstream(format!("buddy balance http {status}")));
        }
        let v: Value = serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Upstream(format!("buddy balance 非 JSON: {e}")))?;
        let accounts = v
            .pointer("/data/Response/Data/Accounts")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let total: f64 = accounts
            .iter()
            .filter_map(|a| {
                a.get("CapacityRemainPrecise")
                    .and_then(Value::as_str)
                    .or_else(|| a.get("capacity_remain_precise").and_then(Value::as_str))
                    .and_then(|s| s.parse::<f64>().ok())
            })
            .sum();
        Ok(BuddyBalance { total_remaining: total })
    }

    /// 11140 业务码：按账号生效（文案说内容审核但**不是 AUTH**）。
    pub fn classify_buddy_error(status: u16, body: &str) -> ProviderError {
        let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        let code = v.get("code").and_then(Value::as_i64);
        if code == Some(11140) {
            return ProviderError::RateLimited {
                retry_after_secs: Some(1800), // 30 分钟（按账号冷却）
                msg: format!("request illegal (11140)：{body}"),
            };
        }
        match status {
            401 | 403 => ProviderError::Credential(format!("http {status}: {body}")),
            429 => ProviderError::RateLimited { retry_after_secs: Some(60), msg: body.into() },
            code if (500..=599).contains(&code) => ProviderError::Upstream(format!("http {code}")),
            code => ProviderError::BadRequest(format!("http {code}: {body}")),
        }
    }

    async fn collect(
        &self,
        cred: &Credential,
        route: &Route,
        req: &ChatRequest,
    ) -> Result<(String, Usage, Option<String>), ProviderError> {
        let mut body = Map::new();
        for (k, v) in &req.raw {
            body.insert(k.clone(), v.clone());
        }
        body.insert("model".into(), Value::String(route.model.clone()));
        body.insert("messages".into(), serde_json::json!(
            req.messages.iter().map(|m| serde_json::json!({
                "role": m.role, "content": m.content
            })).collect::<Vec<_>>()
        ));
        body.insert("stream".into(), Value::Bool(true));
        let resp = self
            .client
            .post(format!("{}{}", self.base, CHAT_PATH))
            .headers(Self::headers(cred, &self.domain)?)
            .json(&Value::Object(body))
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?.to_vec();
        let text = String::from_utf8_lossy(&bytes).to_string();
        if status != 200 {
            return Err(Self::classify_buddy_error(status, &text));
        }
        // 标准 OpenAI SSE
        let mut parser = SseParser::new();
        parser.feed(&bytes);
        parser.finalize();
        let mut content = String::new();
        let mut usage = Usage::default();
        let mut finish = None;
        while let Some(data) = parser.next_data() {
            if data.trim() == "[DONE]" { break; }
            let Ok(v) = serde_json::from_str::<Value>(&data) else { continue };
            // 200 + SSE 错误帧（11140 第三条通道）
            if let Some(code) = v.get("code").and_then(Value::as_i64) {
                if code == 11140 {
                    return Err(Self::classify_buddy_error(200, &data));
                }
            }
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
        Ok((content, usage, finish))
    }
}

#[async_trait]
impl Provider for BuddyProvider {
    fn id(&self) -> &str {
        "buddy"
    }

    fn catalog(&self) -> ProviderCatalog {
        ProviderCatalog {
            id: "buddy".into(),
            models: vec![
                ModelInfo { id: "Deepseek-V4.1-Flash".into() },
                ModelInfo { id: "hy4-preview-f".into() },
            ],
        }
    }

    async fn refresh(&self, cred: &Credential) -> Result<Credential, ProviderError> {
        Provider::refresh(self, cred).await
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
        queue.push_back(Ok(StreamChunk::Finish {
            reason: finish.unwrap_or_else(|| "stop".into()),
            usage,
        }));
        Ok(Box::pin(futures::stream::iter(queue)))
    }
}
