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
use serde_json::{Map, Value};

use crate::anthropic::{completion_from_anthropic, openai_to_anthropic_body};
use crate::openai::{ChatCompletion, ChatRequest};
use crate::provider::{ChunkStream, Credential, Provider, ProviderError, StreamChunk};
use crate::registry::{ModelInfo, ProviderCatalog};
use crate::route::Route;

pub const DEFAULT_BASE: &str = "https://zcode.z.ai";
pub const DEFAULT_CLIENT_VERSION: &str = "3.14.4";

/// CLI 设备码登录流程句柄。
pub struct ZcodeLoginFlow {
    pub flow_id: String,
    pub auth_url: String,
    /// init 时自生成的 32 字节 hex（轮询沿用）。
    bearer: String,
}

/// 余额（单位：token）。
#[derive(Debug, Clone, PartialEq)]
pub struct ZcodeBalance {
    /// 企业版形态：不下发额度数字（buckets 空、remaining/total=0）。
    pub enterprise: bool,
    pub buckets: Vec<ZcodeBalanceBucket>,
    /// Σ(available_units ?? remaining_units)；**单位是 token**（unit_type）。
    pub remaining: u64,
    pub total: u64,
    /// 最早到期时间（Unix 秒），供展示解禁时刻。
    pub expires_at: Option<u64>,
}

/// billing/balance 的一个额度桶（参考 upstream.ts:119-146 实测形状）。
#[derive(Debug, Clone, PartialEq)]
pub struct ZcodeBalanceBucket {
    pub plan_id: Option<String>,
    pub show_name: Option<String>,
    /// 实测 "token"——不是积分，展示按 M 量级。
    pub unit_type: Option<String>,
    pub meter: Option<String>,
    pub total_units: Option<u64>,
    pub used_units: Option<u64>,
    pub remaining_units: Option<u64>,
    pub available_units: Option<u64>,
    pub expires_at: Option<u64>,
}

/// 可领套餐（entitlements 里 meter=model_usage 且 unit_type=token 的量）。
#[derive(Debug, Clone, PartialEq)]
pub struct ZcodePlan {
    pub plan_id: String,
    pub title: String,
    pub tokens: u64,
}

/// 阿里云验证码凭据（T6.3 载体产出；param 本地校验长度 ≥200，不合格不发）。
#[derive(Debug, Clone)]
pub struct CaptchaParam {
    pub param: String,
    pub region: String,
}

/// 领取结果。3007 是预期内的「先探后取」状态，不算错误。
#[derive(Debug, Clone, PartialEq)]
pub enum ClaimOutcome {
    Claimed { ends_at: String },
    AlreadyClaimed,
    Cooldown { next_at: String },
    NeedCaptcha,
    NoPlans,
}

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

    /// CLI 设备码登录：init 拿授权链接（浏览器打开），轮询直到用户完成授权。
    pub async fn login_init(&self) -> Result<ZcodeLoginFlow, ProviderError> {
        let bearer = crate::key::random_id(32); // 32 字节 hex，一次性登录种子
        let resp = self
            .client
            .post(format!("{}/api/v1/oauth/cli/init", self.base))
            .bearer_auth(&bearer)
            .json(&serde_json::json!({ "provider": "bigmodel" }))
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        if status != 200 {
            return Err(ProviderError::Upstream(format!("login init http {status}: {}", truncate(&text))));
        }
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| ProviderError::Upstream(format!("login init body: {e}")))?;
        let flow_id = v
            .get("flow_id")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Upstream("login init missing flow_id".into()))?
            .to_string();
        let auth_url = v
            .get("url")
            .or_else(|| v.get("auth_url"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        Ok(ZcodeLoginFlow { flow_id, auth_url, bearer })
    }

    /// 轮询一次：None = 用户尚未完成授权。
    pub async fn login_poll(&self, flow: &ZcodeLoginFlow) -> Result<Option<Credential>, ProviderError> {
        let resp = self
            .client
            .get(format!("{}/api/v1/oauth/cli/poll/{}", self.base, flow.flow_id))
            .bearer_auth(&flow.bearer)
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        if status != 200 {
            return Err(ProviderError::Upstream(format!("login poll http {status}: {}", truncate(&text))));
        }
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| ProviderError::Upstream(format!("login poll body: {e}")))?;
        match v.get("status").and_then(Value::as_str) {
            Some("ok") | Some("succeeded") => {
                let token = v
                    .get("zcodejwttoken")
                    .or_else(|| v.pointer("/token/zcodejwttoken"))
                    .or_else(|| v.get("token"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| ProviderError::Upstream("login poll ok but no token".into()))?;
                Ok(Some(Credential {
                    account_id: format!("zcode-{}", &flow.flow_id[..8.min(flow.flow_id.len())]),
                    secret: token.to_string(),
                }))
            }
            _ => Ok(None),
        }
    }

    /// 完整登录：init + 轮询到拿到凭据（默认节奏由 `login` 提供）。
    pub async fn login_with_interval(&self, interval: std::time::Duration) -> Result<Credential, ProviderError> {
        let flow = self.login_init().await?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
        loop {
            if let Some(cred) = self.login_poll(&flow).await? {
                return Ok(cred);
            }
            if std::time::Instant::now() > deadline {
                return Err(ProviderError::Upstream("login poll timeout (300s)".into()));
            }
            tokio::time::sleep(interval).await;
        }
    }

    /// 余额（token 计）：需 Authorization + X-Device-Mid（identity_headers 已带）。
    /// 余额：`data.balances[]` 桶累加（available 优先于 remaining）；
    /// `displayMode=="enterprise"` 不下发额度数字。
    /// ⚠️ 每日赠送**不在 balances 里**，只在 preview.plans——余额展示
    /// 需另取 claim_preview 合并，否则面板显示 0（参考 upstream.ts:151-163）。
    pub async fn balance(&self, cred: &Credential) -> Result<ZcodeBalance, ProviderError> {
        let resp = self
            .client
            .get(format!("{}/api/v1/zcode-plan/billing/balance", self.base))
            .headers(self.identity_headers(cred))
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        if status != 200 {
            return Err(ProviderError::Upstream(format!("balance http {status}: {}", truncate(&text))));
        }
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| ProviderError::Upstream(format!("balance body: {e}")))?;
        let data = v.get("data").cloned().unwrap_or(Value::Null);
        if data.get("displayMode").and_then(Value::as_str) == Some("enterprise") {
            return Ok(ZcodeBalance {
                enterprise: true,
                buckets: Vec::new(),
                remaining: 0,
                total: 0,
                expires_at: None,
            });
        }
        let str_of = |m: &Map<String, Value>, k: &str| m.get(k).and_then(Value::as_str).map(str::to_string);
        let u64_of = |m: &Map<String, Value>, k: &str| m.get(k).and_then(Value::as_u64);
        let mut buckets = Vec::new();
        if let Some(arr) = data.get("balances").and_then(Value::as_array) {
            for item in arr {
                let Some(obj) = item.as_object() else { continue };
                buckets.push(ZcodeBalanceBucket {
                    plan_id: str_of(obj, "plan_id"),
                    show_name: str_of(obj, "show_name"),
                    unit_type: str_of(obj, "unit_type"),
                    meter: str_of(obj, "meter"),
                    total_units: u64_of(obj, "total_units"),
                    used_units: u64_of(obj, "used_units"),
                    remaining_units: u64_of(obj, "remaining_units"),
                    available_units: u64_of(obj, "available_units"),
                    expires_at: u64_of(obj, "expires_at"),
                });
            }
        }
        let mut remaining = 0u64;
        let mut total = 0u64;
        let mut expires_at: Option<u64> = None;
        for b in &buckets {
            remaining += b.available_units.or(b.remaining_units).unwrap_or(0);
            total += b.total_units.unwrap_or(0);
            if let Some(exp) = b.expires_at {
                expires_at = Some(match expires_at {
                    Some(prev) => prev.min(exp),
                    None => exp,
                });
            }
        }
        Ok(ZcodeBalance { enterprise: false, buckets, remaining, total, expires_at })
    }

    /// 补激活事件（app_launch/app_daily_active/app_login_success）——
    /// 不补则 preview 恒空（「每日随机派发」实为按活跃信号决定）。
    pub async fn report_activity(&self, cred: &Credential) -> Result<(), ProviderError> {
        let events: Vec<Value> = ["app_launch", "app_daily_active", "app_login_success"]
            .iter()
            .map(|name| {
                serde_json::json!({
                    "name": name,
                    "user_id": cred.account_id,
                    "device_mid": self.device_mid,
                })
            })
            .collect();
        let resp = self
            .client
            .post(format!("{}/api/v1/event/report", self.base))
            .headers(self.identity_headers(cred))
            .json(&serde_json::json!({ "events": events }))
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        if status != 200 {
            return Err(ProviderError::Upstream(format!("event report http {status}: {}", truncate(&text))));
        }
        Ok(())
    }

    /// 可领套餐列表。
    pub async fn claim_preview(&self, cred: &Credential) -> Result<Vec<ZcodePlan>, ProviderError> {
        let resp = self
            .client
            .get(format!(
                "{}/api/v1/zcode-plan/billing/preview?app_version={}&platform=win32",
                self.base, self.version
            ))
            .headers(self.identity_headers(cred))
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        if status != 200 {
            return Err(ProviderError::Upstream(format!("preview http {status}: {}", truncate(&text))));
        }
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| ProviderError::Upstream(format!("preview body: {e}")))?;
        let mut plans = Vec::new();
        if let Some(arr) = v.pointer("/data/plans").and_then(Value::as_array) {
            for p in arr {
                let plan_id = p
                    .get("plan_id")
                    .or_else(|| p.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if plan_id.is_empty() {
                    continue;
                }
                let title = p
                    .get("title")
                    .or_else(|| p.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let tokens: u64 = p
                    .get("entitlements")
                    .and_then(Value::as_array)
                    .map(|ents| {
                        ents.iter()
                            .filter(|e| {
                                e.get("meter").and_then(Value::as_str) == Some("model_usage")
                                    && e.get("unit_type").and_then(Value::as_str) == Some("token")
                            })
                            .filter_map(|e| {
                                e.get("amount").and_then(Value::as_u64).or_else(|| {
                                    e.get("amount").and_then(Value::as_str).and_then(|s| s.parse().ok())
                                })
                            })
                            .sum()
                    })
                    .unwrap_or(0);
                plans.push(ZcodePlan { plan_id, title, tokens });
            }
        }
        Ok(plans)
    }

    /// 提交领取。无 captcha 时先探（3007 → NeedCaptcha）；带 captcha 时先本地校验
    /// param 长度 ≥200（dsh 实测：不合格 param 在索要窗口必 3007，不发）。
    pub async fn submit_claim(
        &self,
        cred: &Credential,
        plan_id: &str,
        captcha: Option<&CaptchaParam>,
    ) -> Result<ClaimOutcome, ProviderError> {
        let mut req = self
            .client
            .post(format!("{}/api/v1/zcode-plan/billing/claim", self.base))
            .headers(self.identity_headers(cred))
            .json(&serde_json::json!({ "plan_id": plan_id }));
        if let Some(cap) = captcha {
            if cap.param.len() < 200 {
                return Ok(ClaimOutcome::NeedCaptcha);
            }
            req = req
                .header("x-aliyun-captcha-verify-param", &cap.param)
                .header("x-aliyun-captcha-verify-region", &cap.region);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        if status != 200 {
            return Err(ProviderError::Upstream(format!("claim http {status}: {}", truncate(&text))));
        }
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| ProviderError::Upstream(format!("claim body: {e}")))?;
        let ends_at = || {
            v.pointer("/data/plan/ends_at")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        match v.get("code").and_then(Value::as_i64) {
            Some(0) => Ok(ClaimOutcome::Claimed { ends_at: ends_at() }),
            Some(1003) => Ok(ClaimOutcome::AlreadyClaimed),
            Some(1005) => Ok(ClaimOutcome::Cooldown { next_at: ends_at() }),
            Some(3007) => Ok(ClaimOutcome::NeedCaptcha),
            Some(code) => Err(ProviderError::Upstream(format!("claim code {code}: {}", truncate(&text)))),
            None => Err(ProviderError::Upstream(format!("claim no code: {}", truncate(&text)))),
        }
    }

    /// 每日领取编排：补事件 → preview → 第一档先探（无验证码头）。
    /// NeedCaptcha 时由 T6.3 验证码载体产出 param 后调 submit_claim 重发。
    pub async fn claim_daily(&self, cred: &Credential) -> Result<ClaimOutcome, ProviderError> {
        self.report_activity(cred).await?;
        let plans = self.claim_preview(cred).await?;
        let Some(first) = plans.first() else {
            return Ok(ClaimOutcome::NoPlans);
        };
        self.submit_claim(cred, &first.plan_id, None).await
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
