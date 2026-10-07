//! lobsterai Provider（有道 LobsterAI，T4.11）。
//!
//! 协议要点（对照 reference §4.4）：
//! - 推理：标准 OpenAI SSE `POST {api}/api/proxy/v1/chat/completions`
//! - 登录：本地回调收 authCode 换 token（两个域名：portal 与 apiBase 分离）
//! - 凭据含 `uuid/first_keyfrom/latest_keyfrom` 身份字段（续期必填，
//!   丢失=静默续期失败只能重登）
//! - `stream` **恒 true**（`stream:false` 回 500）
//! - 余额 `GET /api/user/profile-summary`→`data.totalCreditsRemaining`
//! - 终态判定只有 HTTP 401/403 或业务码 40100/40101

use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::openai::{ChatCompletion, ChatRequest, Usage};
use crate::provider::{ChunkStream, Credential, Provider, ProviderError, StreamChunk};
use crate::registry::{ModelInfo, ProviderCatalog};
use crate::route::Route;
use crate::sse::SseParser;

pub const LOBSTERAI_API_BASE: &str = "https://lobsterai-server.youdao.com";
pub const LOBSTERAI_PORTAL: &str = "https://lobsterai.youdao.com";

const CHAT_PATH: &str = "/api/proxy/v1/chat/completions";
const PROFILE_PATH: &str = "/api/user/profile-summary";
const CLIENT_VERSION: &str = "1.5.10";

#[derive(Debug, Clone, PartialEq)]
pub struct LobsteraiBalance {
    pub total_credits_remaining: f64,
}

pub struct LobsteraiProvider {
    base: String,
    client: reqwest::Client,
}

impl LobsteraiProvider {
    pub fn new(base: String) -> Self {
        Self { base, client: reqwest::Client::new() }
    }

    pub fn production() -> Self {
        Self::new(LOBSTERAI_API_BASE.into())
    }

    fn parse_secret(cred: &Credential) -> Result<Value, ProviderError> {
        serde_json::from_str::<Value>(&cred.secret)
            .map_err(|e| ProviderError::Credential(format!("lobsterai 凭据非 JSON：{e}")))
    }

    fn sstr(v: &Value, key: &str) -> String {
        v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
    }

    /// 请求头（不发腾讯系归属头；`stream` 恒 true——`stream:false` 回 500）。
    fn headers(cred: &Credential) -> Result<reqwest::header::HeaderMap, ProviderError> {
        let v = Self::parse_secret(cred)?;
        let token = Self::sstr(&v, "access_token");
        let mut h = reqwest::header::HeaderMap::new();
        let ins = reqwest::header::HeaderValue::from_str;
        let _ = ins(&format!("Bearer {token}")).map(|x| h.insert("authorization", x));
        let _ = ins("application/json").map(|x| h.insert("content-type", x));
        let _ = ins("text/event-stream").map(|x| h.insert("accept", x));
        let _ = ins(&format!("LobsterAI/{CLIENT_VERSION}")).map(|x| h.insert("user-agent", x));
        let _ = ins("tools,files").map(|x| h.insert("x-lobsterai-client-capabilities", x));
        let _ = ins(CLIENT_VERSION).map(|x| h.insert("x-lobsterai-client-version", x));
        Ok(h)
    }

    /// 续期：凭据含 `uuid/first_keyfrom/latest_keyfrom` 身份字段
    /// （丢失=静默续期失败只能重登）。
    pub async fn refresh(&self, cred: &Credential) -> Result<Credential, ProviderError> {
        let old = Self::parse_secret(cred)?;
        let rt = Self::sstr(&old, "refresh_token");
        if rt.is_empty() {
            return Err(ProviderError::Credential("lobsterai 凭据无 refresh_token".into()));
        }
        let uuid = Self::sstr(&old, "uuid");
        let first_keyfrom = Self::sstr(&old, "first_keyfrom");
        let latest_keyfrom = Self::sstr(&old, "latest_keyfrom");
        if uuid.is_empty() || first_keyfrom.is_empty() {
            return Err(ProviderError::Credential(
                "lobsterai 凭据缺 uuid/first_keyfrom 身份字段（续期必填，丢失只能重登）".into(),
            ));
        }
        let resp = self
            .client
            .post(format!("{}/api/auth/refresh", self.base))
            .headers(Self::headers(cred)?)
            .json(&serde_json::json!({
                "refresh_token": rt,
                "uuid": uuid,
                "first_keyfrom": first_keyfrom,
                "latest_keyfrom": latest_keyfrom,
            }))
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if status == 401 || status == 403 {
            return Err(ProviderError::Credential(format!("lobsterai refresh http {status}")));
        }
        if status != 200 {
            return Err(ProviderError::Upstream(format!("lobsterai refresh http {status}")));
        }
        let v: Value = serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Upstream(format!("lobsterai refresh 非 JSON: {e}")))?;
        let new_token = v
            .get("access_token")
            .or_else(|| v.get("token"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ProviderError::Credential("lobsterai refresh 200 但无 token".into()))?
            .to_string();
        let new_rt = v
            .get("refresh_token")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or(rt);
        let mut out = old.clone();
        // 凭据可能解析成 JSON 但不是对象（裸数字/数组）——不能 unwrap，
        // 按凭据错误上抛（手工/损坏凭据不应 panic 网关）。
        let obj = out
            .as_object_mut()
            .ok_or_else(|| ProviderError::Credential("lobsterai 凭据非 JSON 对象".into()))?;
        obj.insert("access_token".into(), Value::String(new_token));
        obj.insert("refresh_token".into(), Value::String(new_rt));
        Ok(Credential { secret: out.to_string(), ..cred.clone() })
    }

    /// 余额：`GET /api/user/profile-summary`→`data.totalCreditsRemaining`
    /// （**不要**用 `/api/user/quota`，只有 freeCreditsTotal=300）。
    pub async fn balance(&self, cred: &Credential) -> Result<LobsteraiBalance, ProviderError> {
        let resp = self
            .client
            .get(format!("{}{}", self.base, PROFILE_PATH))
            .headers(Self::headers(cred)?)
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if status != 200 {
            return Err(ProviderError::Upstream(format!("lobsterai balance http {status}")));
        }
        let v: Value = serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Upstream(format!("lobsterai balance 非 JSON: {e}")))?;
        let total = v
            .pointer("/data/totalCreditsRemaining")
            .and_then(Value::as_f64)
            .or_else(|| v.get("totalCreditsRemaining").and_then(Value::as_f64))
            .unwrap_or(0.0);
        Ok(LobsteraiBalance { total_credits_remaining: total })
    }

    /// 终态判定：只有 HTTP 401/403 或业务码 40100/40101
    /// （网络抖动可重试不误判重登）。
    pub fn classify_lobsterai_error(status: u16, body: &str) -> ProviderError {
        let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        let code = v.get("code").and_then(Value::as_i64);
        if status == 401 || status == 403 || code == Some(40100) || code == Some(40101) {
            return ProviderError::Credential(format!("lobsterai session dead: {body}"));
        }
        match status {
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
        // stream 恒 true（stream:false 回 500）
        body.insert("stream".into(), Value::Bool(true));
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
        let text = String::from_utf8_lossy(&bytes).to_string();
        if status != 200 {
            return Err(Self::classify_lobsterai_error(status, &text));
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
impl Provider for LobsteraiProvider {
    fn id(&self) -> &str {
        "lobsterai"
    }

    fn catalog(&self) -> ProviderCatalog {
        ProviderCatalog {
            id: "lobsterai".into(),
            models: vec![
                ModelInfo { id: "deepseek-v4-flash".into() },
                ModelInfo { id: "glm-5.3".into() },
            ],
        }
    }

    async fn refresh(&self, cred: &Credential) -> Result<Credential, ProviderError> {
        // 显式走固有实现：`Provider::refresh(self, cred)` 会解析到本 trait 方法
        // 自身（无限递归→栈溢出）；刷新调度器经 dyn Provider 进来的正是这里。
        LobsteraiProvider::refresh(self, cred).await
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
