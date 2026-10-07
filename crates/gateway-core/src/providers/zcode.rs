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
    /// 订阅腿端点（默认生产 api.z.ai；测试注入 stub）。
    coding_plan_base: String,
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
            coding_plan_base: CODING_PLAN_API_BASE.into(),
        }
    }

    /// 注入订阅腿端点（测试）。
    pub fn with_coding_plan_base(mut self, base: String) -> Self {
        self.coding_plan_base = base;
        self
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
                    .filter(|t| !t.is_empty() && !t.starts_with('{'))
                    .ok_or_else(|| ProviderError::Upstream("login poll ok but no token".into()))?;
                // OAuth token 宽松提取（登录结果必然带 zai 或 bigmodel 之一，
                // 是订阅腿换 key 的原料）；没有则裸串（历史兼容）
                let mut m = serde_json::Map::new();
                m.insert("zcode_jwt".into(), Value::String(token.to_string()));
                for (field, names) in [
                    ("zai_access_token", ["zaiAccessToken", "zai_access_token", "zai_token"]),
                    ("bigmodel_access_token", ["bigmodelAccessToken", "bigmodel_access_token", "bigmodel_token"]),
                ] {
                    for n in names {
                        if let Some(t) = v.get(n).and_then(Value::as_str).filter(|t| !t.is_empty()) {
                            m.insert(field.into(), Value::String(t.to_string()));
                            break;
                        }
                    }
                }
                let secret = if m.len() > 1 { Value::Object(m).to_string() } else { token.to_string() };
                Ok(Some(Credential {
                    account_id: format!("zcode-{}", &flow.flow_id[..8.min(flow.flow_id.len())]),
                    secret,
                }))
            }
            _ => Ok(None),
        }
    }

    /// 完整登录：init + 轮询到拿到凭据（默认节奏由 `login` 提供）。
    /// 阻塞轮询到登录成功；成功后**顺手换** coding-plan 的 api-key（付费
    /// 订阅腿）——换不到不报错（没订阅是常态），按登录渠道写
    /// coding_plan_key_zai / coding_plan_key_bigmodel（zcode-auth.ts:592-613）。
    pub async fn login_with_interval(&self, interval: std::time::Duration) -> Result<Credential, ProviderError> {
        let flow = self.login_init().await?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
        loop {
            if let Some(cred) = self.login_poll(&flow).await? {
                return Ok(self.attach_coding_plan_key(cred).await);
            }
            if std::time::Instant::now() > deadline {
                return Err(ProviderError::Upstream("login poll timeout (300s)".into()));
            }
            tokio::time::sleep(interval).await;
        }
    }

    /// 登录成功后顺手换订阅 key（失败吞掉，不影响 start-plan）。
    pub async fn attach_coding_plan_key(&self, cred: Credential) -> Credential {
        let Ok(Some(key)) = self.resolve_coding_plan_key(&cred).await else {
            return cred;
        };
        let Ok(mut v) = serde_json::from_str::<Value>(&cred.secret) else {
            return cred;
        };
        let Some(obj) = v.as_object_mut() else { return cred };
        // 按「拿到哪个 OAuth token」落位（zai ?? bigmodel 优先级一致）
        let field = if obj.contains_key("zai_access_token") {
            "coding_plan_key_zai"
        } else {
            "coding_plan_key_bigmodel"
        };
        obj.insert(field.into(), Value::String(key));
        Credential { secret: Value::Object(obj.clone()).to_string(), ..cred }
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
        let zc = parse_zcode_cred(&cred.secret);
        self.identity_headers_raw(&zc)
    }

    fn identity_headers_raw(&self, zc: &ZcodeCred) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        let ins = reqwest::header::HeaderValue::from_str;
        let _ = ins(&format!("Bearer {}", zc.zcode_jwt)).map(|v| h.insert("authorization", v));
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

    /// 按通道发送推理请求；429 + 1005/1113（两通道额度独立）且另一通道
    /// 可用时换腿重发一次。401/1002（同凭据）、3012（风控重试加重惩罚）、
    /// 3009（并发走退避）**都不换**。
    async fn dispatch(
        &self,
        cred: &Credential,
        route: &Route,
        body: &Value,
    ) -> Result<Result<reqwest::Response, ProviderError>, ProviderError> {
        let zc = parse_zcode_cred(&cred.secret);
        let channel = resolve_channel_for(&route.model, cred);
        let other = !channel;
        let send = |ch: ZcodeChannel| {
            let url = match ch {
                ZcodeChannel::StartPlan => format!("{}{}", self.base, MESSAGES_PATH),
                ZcodeChannel::CodingPlan => format!("{}{}", self.coding_plan_base, CODING_PLAN_MESSAGES_PATH),
            };
            let headers = self.identity_headers_for(&zc, ch);
            let client = &self.client;
            async move {
                client
                    .post(&url)
                    .headers(headers)
                    .json(body)
                    .send()
                    .await
                    .map_err(|e| ProviderError::Upstream(e.to_string()))
            }
        };
        let resp = send(channel).await?;
        if resp.status().as_u16() != 429 {
            return Ok(Ok(resp));
        }
        let retry_after = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());
        let text = resp.text().await.unwrap_or_default();
        let quota_like = text.contains("1005") || text.contains("1113");
        if quota_like && channel_available(other, &zc) {
            // 换腿重发一次（不重产 captcha——param 一次性，这里本就不带）
            let resp2 = send(other).await?;
            return Ok(Ok(resp2));
        }
        Ok(Err(ProviderError::RateLimited {
            retry_after_secs: Some(retry_after.unwrap_or(60)),
            msg: truncate(&text),
        }))
    }

    async fn post_messages(&self, cred: &Credential, route: &Route, req: &ChatRequest) -> Result<ChatCompletion, ProviderError> {
        // 上游要裸模型名（route.model 已剥离 provider 前缀）
        let mut upstream_req = req.clone();
        upstream_req.model = route.model.clone();
        let body = openai_to_anthropic_body(&upstream_req, self.system_prefix.as_deref());
        let resp = match self.dispatch(cred, route, &body).await? {
            Ok(resp) => resp,
            Err(e) => return Err(e),
        };
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
        let resp = match self.dispatch(cred, route, &body).await? {
            Ok(resp) => resp,
            Err(e) => return Err(e),
        };
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

// ───────────── 订阅双通道（T4.16，参考 zcode-transport.ts + zcode-pool oauth.rs） ─────────────

/// 订阅腿端点（官方规则：订阅制才走 api.z.ai）。
pub const CODING_PLAN_API_BASE: &str = "https://api.z.ai";
const CODING_PLAN_MESSAGES_PATH: &str = "/api/anthropic/v1/messages";
/// 官方写死的 key 名（**不是 "zcode"**——写错换取永远 no-key）。
const BIZ_API_KEY_NAME: &str = "zcode-api-key";

/// 两条上游通道。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZcodeChannel {
    /// 免费积分（zcode.z.ai + zcode_jwt），模型同属两通道时优先。
    StartPlan,
    /// 付费订阅（api.z.ai + coding_plan_key_*）。
    CodingPlan,
}

impl std::ops::Not for ZcodeChannel {
    type Output = ZcodeChannel;
    fn not(self) -> ZcodeChannel {
        match self {
            ZcodeChannel::StartPlan => ZcodeChannel::CodingPlan,
            ZcodeChannel::CodingPlan => ZcodeChannel::StartPlan,
        }
    }
}

/// 两条通道各自承载的模型（transport.ts CHANNEL_MODELS，键为小写）。
const START_PLAN_MODELS: [&str; 3] = ["glm-5.3-flash", "glm-5.2", "glm-5-turbo"];
/// 订阅白名单只开官方 builtinModelIds 确认过的两个。
const CODING_PLAN_MODELS: [&str; 2] = ["glm-5.3", "glm-5.3-flash"];

/// 解析后的 zcode 凭据（secret 为 JSON 形态；裸串 = zcode_jwt 历史兼容）。
#[derive(Debug, Clone, Default)]
pub struct ZcodeCred {
    pub zcode_jwt: String,
    pub zai_access_token: Option<String>,
    pub bigmodel_access_token: Option<String>,
    pub coding_plan_key_zai: Option<String>,
    pub coding_plan_key_bigmodel: Option<String>,
}

pub fn parse_zcode_cred(secret: &str) -> ZcodeCred {
    let Ok(v) = serde_json::from_str::<Value>(secret) else {
        return ZcodeCred { zcode_jwt: secret.to_string(), ..Default::default() };
    };
    let s = |k: &str| v.get(k).and_then(Value::as_str).filter(|t| !t.is_empty()).map(str::to_string);
    ZcodeCred {
        zcode_jwt: s("zcode_jwt").unwrap_or_default(),
        zai_access_token: s("zai_access_token"),
        bigmodel_access_token: s("bigmodel_access_token"),
        coding_plan_key_zai: s("coding_plan_key_zai"),
        coding_plan_key_bigmodel: s("coding_plan_key_bigmodel"),
    }
}

fn channel_available(channel: ZcodeChannel, zc: &ZcodeCred) -> bool {
    match channel {
        ZcodeChannel::StartPlan => !zc.zcode_jwt.is_empty(),
        ZcodeChannel::CodingPlan => {
            zc.coding_plan_key_zai.as_deref().is_some_and(|k| !k.is_empty())
                || zc.coding_plan_key_bigmodel.as_deref().is_some_and(|k| !k.is_empty())
        }
    }
}

/// 模型 → 通道：同属两通道时 start-plan 优先；凭据不具备时回退 start-plan。
pub fn resolve_channel_for(model: &str, cred: &Credential) -> ZcodeChannel {
    let zc = parse_zcode_cred(&cred.secret);
    let key = model.trim().to_lowercase();
    if START_PLAN_MODELS.contains(&key.as_str()) && channel_available(ZcodeChannel::StartPlan, &zc) {
        return ZcodeChannel::StartPlan;
    }
    if CODING_PLAN_MODELS.contains(&key.as_str()) && channel_available(ZcodeChannel::CodingPlan, &zc) {
        return ZcodeChannel::CodingPlan;
    }
    ZcodeChannel::StartPlan
}

impl ZcodeProvider {
    /// 通道化请求头：start-plan 原样；coding-plan 换 Bearer key 并**删
    /// HTTP-Referer**（transport.ts:126）。
    fn identity_headers_for(&self, zc: &ZcodeCred, channel: ZcodeChannel) -> reqwest::header::HeaderMap {
        let mut h = self.identity_headers_raw(zc);
        match channel {
            ZcodeChannel::StartPlan => {}
            ZcodeChannel::CodingPlan => {
                let key = zc
                    .coding_plan_key_zai
                    .as_deref()
                    .or(zc.coding_plan_key_bigmodel.as_deref())
                    .unwrap_or_default();
                let _ = reqwest::header::HeaderValue::from_str(&format!("Bearer {key}"))
                    .map(|v| h.insert("authorization", v));
                h.remove("http-referer");
            }
        }
        h
    }

    /// 三步现换订阅 key（transport.ts:257-294；**只 GET 不建**——zcode-pool
    /// 会 POST 创建，保守对齐 harness）。失败吞成 None（没订阅是常态，
    /// start-plan 不受影响）。
    pub async fn resolve_coding_plan_key(&self, cred: &Credential) -> Result<Option<String>, ProviderError> {
        let zc = parse_zcode_cred(&cred.secret);
        let Some(token) = zc.zai_access_token.as_deref().or(zc.bigmodel_access_token.as_deref()) else {
            return Ok(None); // 无 OAuth token 短路，不发请求
        };
        let get_json = |url: String, auth: String| {
            let client = &self.client;
            async move {
                let resp = client
                    .get(&url)
                    .header("authorization", format!("Bearer {auth}"))
                    .header("content-type", "application/json")
                    .send()
                    .await
                    .ok()?;
                if !resp.status().is_success() {
                    return None;
                }
                resp.json::<Value>().await.ok()
            }
        };
        // ① 组织与项目（挑默认机构/默认项目，projectType=="2" 剔除）
        let info = get_json(
            format!("{}/api/biz/customer/getCustomerInfo", self.coding_plan_base),
            token.to_string(),
        )
        .await
        .or_else(|| None);
        let Some(info) = info else { return Ok(None) };
        let Some((org, proj)) = pick_org_project(&info) else { return Ok(None) };
        // ② 列 api_keys 找 zcode-api-key
        let keys_url = format!(
            "{}/api/biz/v1/organization/{org}/projects/{proj}/api_keys",
            self.coding_plan_base
        );
        let Some(list) = get_json(keys_url.clone(), token.to_string()).await else { return Ok(None) };
        let api_key = list
            .as_array()
            .and_then(|arr| {
                arr.iter()
                    .find(|k| k.get("name").and_then(Value::as_str) == Some(BIZ_API_KEY_NAME))
                    .and_then(|k| k.get("apiKey").and_then(Value::as_str))
                    .map(str::to_string)
            })
            .filter(|k| !k.trim().is_empty());
        let Some(api_key) = api_key else { return Ok(None) };
        // ③ copy 取 secretKey → "apiKey.secret"
        let copied = get_json(format!("{keys_url}/copy/{api_key}"), token.to_string()).await;
        let secret = copied
            .as_ref()
            .and_then(|v| v.get("secretKey").and_then(Value::as_str))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        Ok(Some(match secret {
            Some(s) => format!("{api_key}.{s}"),
            None => return Ok(None),
        }))
    }
}

/// 挑默认机构/项目：projectType=="2" 剔除；名称含「默认」优先，否则第一个
/// 有合法项目的机构（zcode-pool oauth.rs:494-540）。
fn pick_org_project(customer: &Value) -> Option<(String, String)> {
    let root = customer.get("data").unwrap_or(customer);
    let orgs = root.get("organizations")?.as_array()?;
    let id_of = |v: &Value| -> Option<String> {
        match v {
            Value::String(s) => (!s.is_empty()).then(|| s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        }
    };
    let keep = |p: &Value| -> bool {
        match p.get("projectType") {
            Some(Value::String(s)) => s.trim() != "2",
            Some(Value::Number(n)) => n.to_string() != "2",
            _ => true,
        }
    };
    let mut fallback: Option<(String, String)> = None;
    for o in orgs {
        let Some(org_id) = o.get("organizationId").and_then(id_of) else { continue };
        let org_is_default = o
            .get("organizationName")
            .and_then(Value::as_str)
            .is_some_and(|n| n.contains("默认"));
        let projects: Vec<&Value> = o
            .get("projects")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().filter(|p| keep(p)).collect())
            .unwrap_or_default();
        if projects.is_empty() {
            continue;
        }
        let default_proj = projects.iter().find(|p| {
            p.get("projectName").and_then(Value::as_str).is_some_and(|n| n.contains("默认"))
        });
        let chosen = default_proj.copied().or_else(|| projects.first().copied())?;
        let Some(proj_id) = chosen.get("projectId").and_then(id_of) else { continue };
        if org_is_default {
            return Some((org_id, proj_id));
        }
        fallback.get_or_insert((org_id, proj_id));
    }
    fallback
}
