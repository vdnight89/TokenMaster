//! minimax Provider（MiniMax Code 中国版，T4.8）。
//!
//! 协议要点（对照 reference §4.11 + minimax-product.ts:154-200）：
//! - 推理：**Anthropic Messages** 协议 `POST {agent}/mavis/api/v1/llm/v1/messages`
//!   （鉴权只要 Authorization + Content-Type + Accept SSE——**不需要 anthropic-version**）
//! - 登录：设备码+PKCE 两步式（`/oauth2/device/code` → 浏览器 authorize →
//!   `/oauth2/token` 轮询）；**pending 是 HTTP 200 + status:"pending"**（不是 400）
//! - 续期：refresh_token 轮换立即回写（`/oauth2/token` grant=refresh_token）
//! - 签到：timezone_id 是 **query** 必填（放头回 1406010011 且 HTTP 200）；
//!   points 总数 + bonus_points 是其中额外部分**不得相加**；幂等判据 claim_result
//! - 余额：平铺无 data 键；Σdetails[].remaining_amount（字符串须挡空串）

use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::openai::{ChatCompletion, ChatRequest, Usage};
use crate::provider::{ChunkStream, Credential, Provider, ProviderError, StreamChunk};
use crate::registry::{ModelInfo, ProviderCatalog};
use crate::route::Route;

pub const MINIMAX_ACCOUNT_BASE: &str = "https://account.minimax.cn";
pub const MINIMAX_AGENT_BASE: &str = "https://agent.minimax.cn";
pub const MINIMAX_CLIENT_ID: &str = "mcode-public";
pub const MINIMAX_AUDIENCE: &str = "agent-backend";
pub const MINIMAX_SCOPE: &str = "agent.default";

const DEVICE_CODE_PATH: &str = "/oauth2/device/code";
const TOKEN_PATH: &str = "/oauth2/token";
const INFER_PATH: &str = "/mavis/api/v1/llm/v1/messages";
const SIGNIN_STATUS_PATH: &str = "/minimax-cloud/api/v1/signin/status";
const SIGNIN_CLAIM_PATH: &str = "/minimax-cloud/api/v1/signin/claim";
const CREDIT_DETAILS_PATH: &str = "/minimax-cloud/api/v1/credit/details";

/// 签到结果。
#[derive(Debug, Clone, PartialEq)]
pub struct MinimaxSignin {
    pub points: u64,
    pub bonus_points: u64,
    pub already_claimed: bool,
}

/// 余额。
#[derive(Debug, Clone, PartialEq)]
pub struct MinimaxBalance {
    pub total_remaining: u64,
}

pub struct MinimaxProvider {
    account_base: String,
    agent_base: String,
    client: reqwest::Client,
}

impl MinimaxProvider {
    pub fn new(account_base: String, agent_base: String) -> Self {
        Self { account_base, agent_base, client: reqwest::Client::new() }
    }

    pub fn production() -> Self {
        Self::new(MINIMAX_ACCOUNT_BASE.into(), MINIMAX_AGENT_BASE.into())
    }

    fn parse_secret(cred: &Credential) -> Result<Value, ProviderError> {
        serde_json::from_str::<Value>(&cred.secret)
            .map_err(|e| ProviderError::Credential(format!("minimax 凭据非 JSON：{e}")))
    }

    fn sstr(v: &Value, key: &str) -> String {
        v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
    }

    /// 推理头：只要 Authorization + Content-Type + Accept SSE
    /// （**不需要 anthropic-version**——实测）。
    fn infer_headers(cred: &Credential) -> Result<reqwest::header::HeaderMap, ProviderError> {
        let v = Self::parse_secret(cred)?;
        let token = Self::sstr(&v, "access_token");
        let mut h = reqwest::header::HeaderMap::new();
        let ins = reqwest::header::HeaderValue::from_str;
        let _ = ins(&format!("Bearer {token}")).map(|x| h.insert("authorization", x));
        let _ = ins("application/json").map(|x| h.insert("content-type", x));
        let _ = ins("text/event-stream").map(|x| h.insert("accept", x));
        Ok(h)
    }

    /// 设备码登录（两步式：先申请 device_code，返回 login_url + device_code）。
    pub async fn device_code_start(&self) -> Result<(String, String), ProviderError> {
        let resp = self
            .client
            .post(format!("{}{}", self.account_base, DEVICE_CODE_PATH))
            .form(&[
                ("client_id", MINIMAX_CLIENT_ID),
                ("audience", MINIMAX_AUDIENCE),
                ("scope", MINIMAX_SCOPE),
            ])
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if status != 200 {
            return Err(ProviderError::Upstream(format!("device code http {status}")));
        }
        let v: Value = serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Upstream(format!("device code 非 JSON: {e}")))?;
        let device_code = v
            .get("device_code")
            .or_else(|| v.get("deviceCode"))
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Upstream("device code 缺 device_code".into()))?
            .to_string();
        let login_url = v
            .get("login_url")
            .or_else(|| v.get("loginUrl"))
            .or_else(|| v.get("url"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        Ok((device_code, login_url))
    }

    /// 轮询 token：**pending 是 HTTP 200 + status:"pending"**（不是 OAuth 标准 400）。
    pub async fn poll_token(&self, device_code: &str) -> Result<Option<Credential>, ProviderError> {
        let resp = self
            .client
            .post(format!("{}{}", self.account_base, TOKEN_PATH))
            .form(&[
                ("client_id", MINIMAX_CLIENT_ID),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", device_code),
            ])
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        // pending 两种形态：200+status:"pending" 或 400+error:"authorization_pending"
        let is_pending = (status == 200 && v.get("status").and_then(Value::as_str) == Some("pending"))
            || (status == 400
                && v.get("error").and_then(Value::as_str) == Some("authorization_pending"));
        if is_pending {
            return Ok(None);
        }
        if status != 200 {
            return Err(ProviderError::Upstream(format!("minimax poll http {status}")));
        }
        let token = v
            .get("access_token")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ProviderError::Upstream("minimax poll 200 但无 token".into()))?
            .to_string();
        let refresh = v.get("refresh_token").and_then(Value::as_str).unwrap_or_default().to_string();
        let secret = serde_json::json!({
            "access_token": token,
            "refresh_token": refresh,
        })
        .to_string();
        Ok(Some(Credential { account_id: String::new(), secret }))
    }

    /// 续期：refresh_token 轮换立即回写。
    pub async fn refresh(&self, cred: &Credential) -> Result<Credential, ProviderError> {
        let old = Self::parse_secret(cred)?;
        let rt = Self::sstr(&old, "refresh_token");
        if rt.is_empty() {
            return Err(ProviderError::Credential("minimax 凭据无 refresh_token".into()));
        }
        let resp = self
            .client
            .post(format!("{}{}", self.account_base, TOKEN_PATH))
            .form(&[
                ("client_id", MINIMAX_CLIENT_ID),
                ("grant_type", "refresh_token"),
                ("refresh_token", &rt),
            ])
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if status == 401 || status == 403 {
            return Err(ProviderError::Credential(format!("minimax refresh http {status}")));
        }
        if status != 200 {
            return Err(ProviderError::Upstream(format!("minimax refresh http {status}")));
        }
        let v: Value = serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Upstream(format!("minimax refresh 非 JSON: {e}")))?;
        let new_token = v
            .get("access_token")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ProviderError::Credential("minimax refresh 200 但无 token".into()))?
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
            .ok_or_else(|| ProviderError::Credential("minimax 凭据非 JSON 对象".into()))?;
        obj.insert("access_token".into(), Value::String(new_token));
        obj.insert("refresh_token".into(), Value::String(new_rt));
        Ok(Credential { secret: out.to_string(), ..cred.clone() })
    }

    /// 签到状态（timezone_id 是 **query** 必填——放头回 1406010011 且 HTTP 200）。
    pub async fn signin_status(&self, cred: &Credential) -> Result<MinimaxSignin, ProviderError> {
        let tz = "Asia/Shanghai";
        let resp = self
            .client
            .get(format!("{}{}?timezone_id={}", self.agent_base, SIGNIN_STATUS_PATH, tz))
            .headers(Self::infer_headers(cred)?)
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if status != 200 {
            return Err(ProviderError::Upstream(format!("signin status http {status}")));
        }
        let v: Value = serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Upstream(format!("signin status 非 JSON: {e}")))?;
        // 业务码在 base_resp.status_code
        if let Some(code) = v.pointer("/base_resp/status_code").and_then(Value::as_i64) {
            if code != 0 {
                return Err(ProviderError::Upstream(format!("signin status code {code}")));
            }
        }
        Ok(MinimaxSignin {
            points: v.get("points").and_then(Value::as_u64).unwrap_or(0),
            bonus_points: v.get("bonus_points").and_then(Value::as_u64).unwrap_or(0),
            already_claimed: v.get("is_today").and_then(Value::as_bool).unwrap_or(false)
                && v.get("status").and_then(Value::as_i64) == Some(3),
        })
    }

    /// 签到领取：幂等判据 claim_result（1=真领 2=已领，重复仍 200）。
    pub async fn signin_claim(&self, cred: &Credential) -> Result<MinimaxSignin, ProviderError> {
        let tz = "Asia/Shanghai";
        let resp = self
            .client
            .post(format!("{}{}?timezone_id={}", self.agent_base, SIGNIN_CLAIM_PATH, tz))
            .headers(Self::infer_headers(cred)?)
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if status != 200 {
            return Err(ProviderError::Upstream(format!("signin claim http {status}")));
        }
        let v: Value = serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Upstream(format!("signin claim 非 JSON: {e}")))?;
        let claim_result = v.get("claim_result").and_then(Value::as_i64).unwrap_or(0);
        if claim_result == 0 {
            return Err(ProviderError::Upstream("signin claim_result=0（失败）".into()));
        }
        // 领取后补查 status 取真实数值（claim 响应可能无 points）
        self.signin_status(cred).await
    }

    /// 余额：Σdetails[].remaining_amount（字符串、挡空串；total_count 是记录条数不是余额）。
    pub async fn balance(&self, cred: &Credential) -> Result<MinimaxBalance, ProviderError> {
        let resp = self
            .client
            .get(format!("{}{}", self.agent_base, CREDIT_DETAILS_PATH))
            .headers(Self::infer_headers(cred)?)
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if status != 200 {
            return Err(ProviderError::Upstream(format!("credit details http {status}")));
        }
        let v: Value = serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Upstream(format!("credit details 非 JSON: {e}")))?;
        // 平铺响应（无 data 键兼容）+ 空明细整个缺失是有效结果
        let details = v
            .get("details")
            .or_else(|| v.pointer("/data/details"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let total: f64 = details
            .iter()
            .filter_map(|d| {
                d.get("remaining_amount")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .and_then(|s| s.parse::<f64>().ok())
            })
            .sum();
        Ok(MinimaxBalance { total_remaining: total as u64 })
    }

    /// OpenAI → Anthropic Messages 转换（简化版，对齐 §3.7）。
    fn to_anthropic_body(route: &Route, req: &ChatRequest) -> Value {
        let mut messages: Vec<Value> = Vec::new();
        let mut system_text: Vec<String> = Vec::new();
        for m in &req.messages {
            match m.role.as_str() {
                "system" => {
                    let t = m.text();
                    if !t.is_empty() {
                        system_text.push(t);
                    }
                }
                _ => {
                    messages.push(serde_json::json!({
                        "role": m.role,
                        "content": m.text(),
                    }));
                }
            }
        }
        let mut body = Map::new();
        body.insert("model".into(), Value::String(route.model.clone()));
        if !system_text.is_empty() {
            body.insert("system".into(), Value::String(system_text.join("\n\n")));
        }
        body.insert("messages".into(), Value::Array(messages));
        body.insert("max_tokens".into(), Value::from(4096));
        body.insert("stream".into(), Value::Bool(true));
        Value::Object(body)
    }

    /// 从 Anthropic SSE 响应提取文本/用量。
    fn parse_anthropic_sse(body: &str) -> (String, Usage, Option<String>) {
        let mut text = String::new();
        let mut usage = Usage::default();
        let mut finish = None;
        for line in body.lines() {
            let Some(data) = line.strip_prefix("data: ") else { continue };
            let Ok(v) = serde_json::from_str::<Value>(data) else { continue };
            match v.get("type").and_then(Value::as_str).unwrap_or_default() {
                "content_block_delta" => {
                    if let Some(t) = v.pointer("/delta/text").and_then(Value::as_str) {
                        text.push_str(t);
                    }
                }
                "message_delta" => {
                    if let Some(sr) = v.pointer("/delta/stop_reason").and_then(Value::as_str) {
                        finish = Some(
                            match sr {
                                "max_tokens" => "length".to_string(),
                                "tool_use" => "tool_calls".to_string(),
                                _ => "stop".to_string(),
                            },
                        );
                    }
                    if let Some(u) = v.get("usage") {
                        usage = Usage::sum(
                            u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
                            u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0),
                        );
                    }
                }
                "message_start" => {
                    if let Some(u) = v.pointer("/message/usage") {
                        usage.prompt_tokens = u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
                    }
                }
                _ => {}
            }
        }
        (text, usage, finish)
    }

    async fn collect(
        &self,
        cred: &Credential,
        route: &Route,
        req: &ChatRequest,
    ) -> Result<(String, Usage, Option<String>), ProviderError> {
        let body = Self::to_anthropic_body(route, req);
        let resp = self
            .client
            .post(format!("{}{}", self.agent_base, INFER_PATH))
            .headers(Self::infer_headers(cred)?)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let text = String::from_utf8_lossy(&bytes).to_string();
        if status != 200 {
            return match status {
                401 | 403 => Err(ProviderError::Credential(format!("http {status}: {text}"))),
                429 => Err(ProviderError::RateLimited { retry_after_secs: Some(60), msg: text }),
                code => Err(ProviderError::Upstream(format!("http {code}: {text}"))),
            };
        }
        let (content, usage, finish) = Self::parse_anthropic_sse(&text);
        Ok((content, usage, finish))
    }
}

#[async_trait]
impl Provider for MinimaxProvider {
    fn id(&self) -> &str {
        "minimax"
    }

    fn catalog(&self) -> ProviderCatalog {
        ProviderCatalog {
            id: "minimax".into(),
            models: vec![
                ModelInfo { id: "MiniMax-M3.1-Flash-Preview".into() },
                ModelInfo { id: "MiniMax-M3".into() },
                ModelInfo { id: "MiniMax-M2.7".into() },
            ],
        }
    }

    async fn refresh(&self, cred: &Credential) -> Result<Credential, ProviderError> {
        // 显式走固有实现：`Provider::refresh(self, cred)` 会解析到本 trait 方法
        // 自身（无限递归→栈溢出）；刷新调度器经 dyn Provider 进来的正是这里。
        MinimaxProvider::refresh(self, cred).await
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
