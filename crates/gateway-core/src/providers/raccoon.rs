//! raccoon Provider（商汤小浣熊 Raccoon Work，T4.14）。
//!
//! 协议要点（reference §4.10）：
//! - 推理：标准 OpenAI SSE `POST {base}/api/web/llm/v2/chat/completions`
//! - 登录：微信扫码 + 短信双路径；手机号 **AES-128-CFB 加密**
//! - 有 refresh_token 轮换；JWT `exp` 本地解码判过期
//! - 每日 300 积分**服务端自动发放**（无签到端点）
//! - 一次性登录奖励需 **`X-Client-Platform: desktop-*`** 头
//! - 请求体 10MB 硬限

use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::openai::{ChatCompletion, ChatRequest, Usage};
use crate::provider::{ChunkStream, Credential, Provider, ProviderError, StreamChunk};
use crate::registry::{ModelInfo, ProviderCatalog};
use crate::route::Route;
use crate::sse::SseParser;

pub const RACCOON_API_BASE: &str = "https://xiaohuanxiong.com";
pub const RACCOON_MAX_BODY_BYTES: usize = 10 * 1024 * 1024;

const CHAT_PATH: &str = "/api/web/llm/v2/chat/completions";
const REFRESH_PATH: &str = "/api/web/auth/refresh";
const LOGIN_POINTS_PATH: &str = "/api/web/desktop/v1/login/points/grant";

pub struct RaccoonProvider {
    base: String,
    client: reqwest::Client,
}

impl RaccoonProvider {
    pub fn new(base: String) -> Self {
        Self { base, client: reqwest::Client::new() }
    }

    pub fn production() -> Self {
        Self::new(RACCOON_API_BASE.into())
    }

    fn parse_secret(cred: &Credential) -> Result<Value, ProviderError> {
        serde_json::from_str::<Value>(&cred.secret)
            .map_err(|e| ProviderError::Credential(format!("raccoon 凭据非 JSON：{e}")))
    }

    fn sstr(v: &Value, key: &str) -> String {
        v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
    }

    fn headers(cred: &Credential) -> Result<reqwest::header::HeaderMap, ProviderError> {
        let v = Self::parse_secret(cred)?;
        let token = Self::sstr(&v, "access_token");
        let mut h = reqwest::header::HeaderMap::new();
        let ins = reqwest::header::HeaderValue::from_str;
        let _ = ins(&format!("Bearer {token}")).map(|x| h.insert("authorization", x));
        let _ = ins("application/json").map(|x| h.insert("content-type", x));
        Ok(h)
    }

    /// 续期：refresh_token 轮换。
    pub async fn refresh(&self, cred: &Credential) -> Result<Credential, ProviderError> {
        let old = Self::parse_secret(cred)?;
        let rt = Self::sstr(&old, "refresh_token");
        if rt.is_empty() {
            return Err(ProviderError::Credential("raccoon 凭据无 refresh_token".into()));
        }
        let resp = self
            .client
            .post(format!("{}{}", self.base, REFRESH_PATH))
            .headers(Self::headers(cred)?)
            .json(&serde_json::json!({ "refresh_token": rt }))
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if status == 401 || status == 403 {
            return Err(ProviderError::Credential(format!("raccoon refresh http {status}")));
        }
        if status != 200 {
            return Err(ProviderError::Upstream(format!("raccoon refresh http {status}")));
        }
        let v: Value = serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Upstream(format!("raccoon refresh 非 JSON: {e}")))?;
        let new_token = v
            .get("access_token")
            .or_else(|| v.get("token"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ProviderError::Credential("raccoon refresh 200 但无 token".into()))?
            .to_string();
        let new_rt = v
            .get("refresh_token")
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

    /// 一次性登录奖励（3000 分，幂等 `granted:false`）。
    /// **需 `X-Client-Platform: desktop-*` 头**（猜错被拒）。
    pub async fn grant_login_points(&self, cred: &Credential) -> Result<bool, ProviderError> {
        let mut h = Self::headers(cred)?;
        let ins = reqwest::header::HeaderValue::from_str;
        let _ = ins("desktop-windows").map(|x| h.insert("x-client-platform", x));
        let resp = self
            .client
            .post(format!("{}{}", self.base, LOGIN_POINTS_PATH))
            .headers(h)
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if status != 200 {
            return Err(ProviderError::Upstream(format!("raccoon grant http {status}")));
        }
        let v: Value = serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Upstream(format!("raccoon grant 非 JSON: {e}")))?;
        Ok(v.get("granted").and_then(Value::as_bool).unwrap_or(false))
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
        let payload = Value::Object(body).to_string();

        // 10MB 硬限
        if payload.len() > RACCOON_MAX_BODY_BYTES {
            return Err(ProviderError::BadRequest(format!(
                "request body {} bytes exceeds {} limit",
                payload.len(),
                RACCOON_MAX_BODY_BYTES
            )));
        }

        let resp = self
            .client
            .post(format!("{}{}", self.base, CHAT_PATH))
            .headers(Self::headers(cred)?)
            .body(payload)
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
impl Provider for RaccoonProvider {
    fn id(&self) -> &str {
        "raccoon"
    }

    fn catalog(&self) -> ProviderCatalog {
        ProviderCatalog {
            id: "raccoon".into(),
            models: vec![
                ModelInfo { id: "sn-sensenova-6-8-flash".into() },
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
        queue.push_back(Ok(StreamChunk::Finish { reason: finish.unwrap_or_else(|| "stop".into()), usage }));
        Ok(Box::pin(futures::stream::iter(queue)))
    }
}
