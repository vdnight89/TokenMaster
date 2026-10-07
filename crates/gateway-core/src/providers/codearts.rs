//! codearts Provider（华为云 CodeArts，T4.12）。
//!
//! 协议要点（对照 reference §4.1）：
//! - 推理 `POST {base}/api/v2/chat/completions`（标准 OpenAI 兼容）
//! - 鉴权：华为 **SDK-HMAC-SHA256**（sign.ts:28-68）——canonical request
//!   七段式；签名头固定 host/x-sdk-date/x-sdk-content-sha256/x-security-token
//!   （+非 GET 的 content-type），Authorization=`SDK-HMAC-SHA256 Access=…,
//!   SignedHeaders=…,Signature=…`
//! - `Agent-Type: PromptCenter`/`X-Language: zh-cn` **签名后追加**
//! - `Chat-Id`/`Session-Id`/`lang:en` 同样签名后追加
//! - benefit 模型带 `maas_type: benefit` 头**参与签名**
//! - 排队（`TM.00001041`）10s 重试上限 180 次；额度（`InferHub.4291.200`）
//!   立即失败标记 UTC+8 当日 24:00
//! - **429 判据锚定独立数字** `(^|[^0-9])429([^0-9]|$)`——裸子串会命中 4291

use async_trait::async_trait;

use serde_json::{Map, Value};

use crate::openai::{ChatCompletion, ChatRequest, Usage};
use crate::provider::{ChunkStream, Credential, Provider, ProviderError, StreamChunk};
use crate::registry::{ModelInfo, ProviderCatalog};
use crate::route::Route;
use crate::sse::SseParser;

pub const CODEARTS_API_BASE: &str = "https://snap-access.cn-north-4.myhuaweicloud.com";

const CHAT_PATH: &str = "/api/v2/chat/completions";

pub struct CodeartsProvider {
    base: String,
    client: reqwest::Client,
}

/// 华为 SDK-HMAC-SHA256 签名（sign.ts:28-68）。
/// canonical request = `method\nuri(补尾斜杠)\nquery\n排序header行\n''\nSignedHeaders\npayloadHash`
/// `access_key` 进入 Authorization 的 `Access=` 段（上游按它定位账号；
/// 签名本体只用 SK 计算，但 Access 缺失/错值会被 401 拒绝）。
pub fn sdk_hmac_sha256_sign(
    method: &str,
    uri: &str,
    query: &str,
    headers: &[(String, String)],
    payload: &[u8],
    secret: &str,
    access_key: &str,
) -> String {
    use sha2::{Digest, Sha256};
    
    let payload_hash = {
        let mut h = Sha256::new();
        h.update(payload);
        format!("{:x}", h.finalize())
    };

    // 签名头按名称排序（固定四/五个 + 调用方可传额外）
    let mut signed: Vec<&(String, String)> = headers.iter().collect();
    signed.sort_by(|a, b| a.0.cmp(&b.0));
    let signed_headers: Vec<&str> = signed.iter().map(|(k, _)| k.as_str()).collect();
    let header_lines: String = signed
        .iter()
        .map(|(k, v)| format!("{k}:{v}\n"))
        .collect();

    // canonical URI 补尾斜杠
    let canonical_uri = if uri.ends_with('/') || uri.is_empty() { uri.to_string() } else { format!("{uri}/") };

    let canonical = format!(
        "{method}\n{canonical_uri}\n{query}\n{header_lines}\n{signed_headers}\n{payload_hash}",
        signed_headers = signed_headers.join(";"),
    );

    let string_to_sign = format!("SDK-HMAC-SHA256\n{}", {
        let mut h = Sha256::new();
        h.update(canonical.as_bytes());
        format!("{:x}", h.finalize())
    });

    use hmac::Mac as _;
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes())
        .expect("HMAC key init");
    mac.update(string_to_sign.as_bytes());
    let signature = format!("{:x}", mac.finalize().into_bytes());

    format!(
        "SDK-HMAC-SHA256 Access={access_key},SignedHeaders={},Signature={signature}",
        signed_headers.join(";"),
    )
}

/// 429 判据锚定独立数字（`(^|[^0-9])429([^0-9]|$)`）——裸子串会命中 `4291`。
pub fn is_429_standalone(text: &str) -> bool {
    let bytes = text.as_bytes();
    for i in 0..bytes.len().saturating_sub(2) {
        if &bytes[i..i + 3] == b"429" {
            let prev_ok = i == 0 || !bytes[i - 1].is_ascii_digit();
            let next_ok = i + 3 >= bytes.len() || !bytes[i + 3].is_ascii_digit();
            if prev_ok && next_ok {
                return true;
            }
        }
    }
    false
}

/// 额度用尽判据（§4.1：子串 `4291` **或** `insufficient quota` 文案兜底）。
/// 注意必须排在 429 判据之前——`4291` 的裸子串含 `429`。
pub fn is_quota_exhausted(text: &str) -> bool {
    text.contains("4291") || text.to_lowercase().contains("insufficient quota")
}

/// benefit（免费额度）模型集合（§4.1 模型表；动态 gateway/config ∪ 静态兜底
/// 中的静态兜底部分——远端 benefit 集合下发属后续接线，当前先静态判定）。
pub fn is_benefit_model(model: &str) -> bool {
    matches!(model, "glm-5.3-flash" | "deepseek-v4.1-flash")
}

/// 排队判据（`TM.00001041` 或 `InferHub.ModelArts.81111.429`）。
pub fn is_queued(text: &str) -> bool {
    text.contains("TM.00001041") || text.contains("81111.429")
}

#[async_trait]
impl Provider for CodeartsProvider {
    fn id(&self) -> &str {
        "codearts"
    }

    fn catalog(&self) -> ProviderCatalog {
        ProviderCatalog {
            id: "codearts".into(),
            models: vec![
                ModelInfo { id: "glm-5.2".into() },
                ModelInfo { id: "deepseek-v4-flash".into() },
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
        queue.push_back(Ok(StreamChunk::Finish {
            reason: finish.unwrap_or_else(|| "stop".into()),
            usage,
        }));
        Ok(Box::pin(futures::stream::iter(queue)))
    }
}

impl CodeartsProvider {
    pub fn new(base: String) -> Self {
        Self { base, client: reqwest::Client::new() }
    }

    pub fn production() -> Self {
        Self::new(CODEARTS_API_BASE.into())
    }

    fn parse_secret(cred: &Credential) -> Result<Value, ProviderError> {
        serde_json::from_str::<Value>(&cred.secret)
            .map_err(|e| ProviderError::Credential(format!("codearts 凭据非 JSON：{e}")))
    }

    fn sstr(v: &Value, key: &str) -> String {
        v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
    }

    /// 统一收流：请求恒 `stream:true`，响应两种形态都认
    /// （SSE 优先；非 SSE 的纯 JSON——错误重定向/网关直答——也能解）。
    async fn collect(
        &self,
        cred: &Credential,
        route: &Route,
        req: &ChatRequest,
    ) -> Result<(String, Usage, Option<String>), ProviderError> {
        let (status, body) = self.send(cred, route, req).await?;
        if status != 200 {
            return Err(Self::map_error(status, &body));
        }
        if !body.contains("data:") {
            // 非流式 JSON
            let v: Value = serde_json::from_str(&body)
                .map_err(|e| ProviderError::Upstream(format!("codearts 响应非 JSON: {e}")))?;
            let content = v.pointer("/choices/0/message/content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let usage = v.get("usage").map(|u| Usage::sum(
                u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0),
                u.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0),
            )).unwrap_or_default();
            let finish = v.pointer("/choices/0/finish_reason")
                .and_then(Value::as_str)
                .map(str::to_string);
            return Ok((content, usage, finish));
        }
        let mut parser = SseParser::new();
        parser.feed(body.as_bytes());
        parser.finalize();
        let mut text = String::new();
        let mut usage = Usage::default();
        let mut finish = None;
        while let Some(data) = parser.next_data() {
            if data.trim() == "[DONE]" { break; }
            let Ok(v) = serde_json::from_str::<Value>(&data) else { continue };
            if let Some(t) = v.pointer("/choices/0/delta/content").and_then(Value::as_str) {
                text.push_str(t);
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
        Ok((text, usage, finish))
    }

    async fn send(
        &self,
        cred: &Credential,
        route: &Route,
        req: &ChatRequest,
    ) -> Result<(u16, String), ProviderError> {
        let v = Self::parse_secret(cred)?;
        let ak = Self::sstr(&v, "access_key");
        let sk = Self::sstr(&v, "secret_key");
        let token = Self::sstr(&v, "security_token");
        if ak.is_empty() || sk.is_empty() {
            return Err(ProviderError::Credential("codearts 凭据缺 access_key/secret_key".into()));
        }

        let mut body = Map::new();
        for (k, val) in &req.raw {
            body.insert(k.clone(), val.clone());
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
        let payload = Value::Object(body).to_string();

        // SDK-HMAC-SHA256 签名头（固定 host/date/sha256/token + content-type）。
        // x-security-token 是固定签名头（§4.1 签名头表）——在场必须进签名，
        // 否则持有临时凭据的请求会被 401 拒。benefit 模型的 `maas_type: benefit`
        // 同样参与签名（extraSignedHeaders，缺失回 404 model is not registered）。
        use sha2::{Digest, Sha256};
        let now = chrono_like_now();
        let payload_hash = {
            let mut h = Sha256::new();
            h.update(payload.as_bytes());
            format!("{:x}", h.finalize())
        };
        let host = self.base
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap_or("");
        let ph = payload_hash.clone();
        let mut signed_headers = vec![
            ("content-type".to_string(), "application/json".to_string()),
            ("host".to_string(), host.to_string()),
            ("x-sdk-content-sha256".to_string(), ph),
            ("x-sdk-date".to_string(), now.clone()),
        ];
        if !token.is_empty() {
            signed_headers.push(("x-security-token".to_string(), token.clone()));
        }
        if is_benefit_model(&route.model) {
            signed_headers.push(("maas_type".to_string(), "benefit".to_string()));
        }
        let auth = sdk_hmac_sha256_sign("POST", CHAT_PATH, "", &signed_headers, payload.as_bytes(), &sk, &ak);

        let mut h = reqwest::header::HeaderMap::new();
        let ins = reqwest::header::HeaderValue::from_str;
        let _ = ins(&auth).map(|x| h.insert("authorization", x));
        let _ = ins("application/json").map(|x| h.insert("content-type", x));
        let _ = ins(&now).map(|x| h.insert("x-sdk-date", x));
        let _ = ins(&payload_hash).map(|x| h.insert("x-sdk-content-sha256", x));
        if !token.is_empty() {
            let _ = ins(&token).map(|x| h.insert("x-security-token", x));
        }
        if is_benefit_model(&route.model) {
            let _ = ins("benefit").map(|x| h.insert("maas_type", x));
        }
        // 签名后追加（不进签名）：Agent-Type/X-Language（积分侧实测进签名则 401
        // APIG.0301）+ Chat-Id/Session-Id/lang（llm-adapter.ts:1205-1208）
        let _ = ins("PromptCenter").map(|x| h.insert("agent-type", x));
        let _ = ins("zh-cn").map(|x| h.insert("x-language", x));
        let _ = ins(&crate::key::random_id(16)).map(|x| h.insert("chat-id", x));
        let _ = ins(&crate::key::random_id(16)).map(|x| h.insert("session-id", x));
        let _ = ins("en").map(|x| h.insert("lang", x));

        let resp = self
            .client
            .post(format!("{}{}", self.base, CHAT_PATH))
            .headers(h)
            .body(payload)
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        Ok((status, text))
    }

    fn map_error(status: u16, body: &str) -> ProviderError {
        if is_queued(body) {
            return ProviderError::RateLimited { retry_after_secs: Some(10), msg: format!("queued: {body}") };
        }
        if is_quota_exhausted(body) {
            return ProviderError::Credential(format!("quota exhausted: {body}"));
        }
        // 429 判据锚定独立数字（裸子串会命中 4291）
        if status == 429 || is_429_standalone(body) {
            return ProviderError::RateLimited { retry_after_secs: Some(10), msg: format!("rate limit: {body}") };
        }
        match status {
            401 | 403 => ProviderError::Credential(format!("http {status}: {body}")),
            code if (500..=599).contains(&code) => ProviderError::Upstream(format!("http {code}")),
            code => ProviderError::BadRequest(format!("http {code}: {body}")),
        }
    }
}

fn chrono_like_now() -> String {
    // ISO 8601 basic format YYYYMMDDTHHMMSSZ
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = now / 86400;
    let (y, m, d) = civil_from_days(days as i64);
    let secs = now % 86400;
    format!("{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z", secs / 3600, (secs % 3600) / 60, secs % 60)
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}
