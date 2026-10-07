//! loomy Provider（讯飞办公助手，T4.13）。
//!
//! 五个独特点（reference §4.9）：
//! 1. **短信验证码登录**（唯一）：`account.create` 返回 `loginMode:'sms'`，
//!    走 `login.sendSms`/`login.submitSms`（msgid 必须原样带回）
//! 2. **不能续期**（唯一）：无 refresh 端点，session 14 天
//! 3. **两套认证头**：chat 只认 `Bearer`，`/models`/`/points/*` 只认 `token:` 头
//! 4. 新手任务纯 API 直领（8 个任务，零 token 消耗）
//! 5. 积分两池：永久 `balance` 与每日赠送 `dailyBalance`（不回补）
//!
//! HMAC-SHA1 签名用于登录请求。

use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::openai::{ChatCompletion, ChatRequest, Usage};
use crate::provider::{ChunkStream, Credential, Provider, ProviderError, StreamChunk};
use crate::registry::{ModelInfo, ProviderCatalog};
use crate::route::Route;
use crate::sse::SseParser;

pub const LOOMY_API_BASE: &str = "https://loomyad.xifenn.cn/api/v1";
pub const LOOMY_ACCOUNT_BASE: &str = "https://account.xfinfr.com";

const CHAT_PATH: &str = "/chat/completions";
const MODELS_PATH: &str = "/models";
const POINTS_RECORDS_PATH: &str = "/points/records";
const FIRST_LOGIN_PATH: &str = "/points/first-login";

#[derive(Debug, Clone, PartialEq)]
pub struct LoomyBalance {
    pub permanent: f64,
    pub daily: f64,
}

pub struct LoomyProvider {
    base: String,
    client: reqwest::Client,
}

impl LoomyProvider {
    pub fn new(base: String) -> Self {
        Self { base, client: reqwest::Client::new() }
    }

    pub fn production() -> Self {
        Self::new(LOOMY_API_BASE.into())
    }

    fn parse_secret(cred: &Credential) -> Result<Value, ProviderError> {
        serde_json::from_str::<Value>(&cred.secret)
            .map_err(|e| ProviderError::Credential(format!("loomy 凭据非 JSON：{e}")))
    }

    fn sstr(v: &Value, key: &str) -> String {
        v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
    }

    /// **两套认证头**：chat 只认 `Bearer`，`/models`/`/points/*` 只认 `token:`。
    fn chat_headers(cred: &Credential) -> Result<reqwest::header::HeaderMap, ProviderError> {
        let v = Self::parse_secret(cred)?;
        let session = Self::sstr(&v, "session");
        let mut h = reqwest::header::HeaderMap::new();
        let ins = reqwest::header::HeaderValue::from_str;
        let _ = ins(&format!("Bearer {session}")).map(|x| h.insert("authorization", x));
        let _ = ins("application/json").map(|x| h.insert("content-type", x));
        Ok(h)
    }

    /// 元数据头：`token: <session>`（不带 Bearer）。
    fn meta_headers(cred: &Credential) -> Result<reqwest::header::HeaderMap, ProviderError> {
        let v = Self::parse_secret(cred)?;
        let session = Self::sstr(&v, "session");
        let mut h = reqwest::header::HeaderMap::new();
        let ins = reqwest::header::HeaderValue::from_str;
        let _ = ins(&session).map(|x| h.insert("token", x));
        Ok(h)
    }

    /// **不能续期**（唯一无 refresh 的 provider）——只探测。
    /// session 是登录时声明 `expire:1209600`（14 天）得来。
    pub async fn probe_session(&self, cred: &Credential) -> Result<bool, ProviderError> {
        let resp = self
            .client
            .get(format!("{}{}", self.base, MODELS_PATH))
            .headers(Self::meta_headers(cred)?)
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        Ok(resp.status().is_success())
    }

    /// 余额两池：永久 `balance` + 每日赠送 `dailyBalance`（不回补）。
    /// 走只读 `/points/records`（避免打开面板就触发签到）。
    pub async fn balance(&self, cred: &Credential) -> Result<LoomyBalance, ProviderError> {
        let resp = self
            .client
            .get(format!("{}{}", self.base, POINTS_RECORDS_PATH))
            .headers(Self::meta_headers(cred)?)
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if status != 200 {
            return Err(ProviderError::Upstream(format!("loomy balance http {status}")));
        }
        let v: Value = serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Upstream(format!("loomy balance 非 JSON: {e}")))?;
        Ok(LoomyBalance {
            permanent: v.get("balance").and_then(Value::as_f64).unwrap_or(0.0),
            daily: v.get("dailyBalance").and_then(Value::as_f64).unwrap_or(0.0),
        })
    }

    /// 「一键签到」= `POST /points/first-login`（语义是触发每日重置不是 +5000）。
    /// 幂等判据响应体 `alreadyProcessed`。
    pub async fn first_login(&self, cred: &Credential) -> Result<bool, ProviderError> {
        let resp = self
            .client
            .post(format!("{}{}", self.base, FIRST_LOGIN_PATH))
            .headers(Self::meta_headers(cred)?)
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if status != 200 {
            return Err(ProviderError::Upstream(format!("loomy first-login http {status}")));
        }
        let v: Value = serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Upstream(format!("loomy first-login 非 JSON: {e}")))?;
        Ok(v.get("alreadyProcessed").and_then(Value::as_bool).unwrap_or(false))
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
            .headers(Self::chat_headers(cred)?)
            .json(&Value::Object(body))
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?.to_vec();
        let text = String::from_utf8_lossy(&bytes).to_string();
        if status != 200 {
            return match status {
                401 | 403 => Err(ProviderError::Credential(format!("http {status}: {text}"))),
                429 => Err(ProviderError::RateLimited { retry_after_secs: Some(60), msg: text }),
                code => Err(ProviderError::Upstream(format!("http {code}: {text}"))),
            };
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
impl Provider for LoomyProvider {
    fn id(&self) -> &str {
        "loomy"
    }

    fn catalog(&self) -> ProviderCatalog {
        ProviderCatalog {
            id: "loomy".into(),
            models: vec![
                ModelInfo { id: "spark-x".into() },
                ModelInfo { id: "spark-pro".into() },
            ],
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
