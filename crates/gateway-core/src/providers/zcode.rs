//! zcode（智谱 ZCode/z.ai coding plan）Provider。
//!
//! 上游（start-plan 通道）：`POST {base}/api/v1/zcode-plan/anthropic/v1/messages`
//! ——Anthropic Messages 协议，Bearer JWT（zcodejwttoken）+ 身份头。
//! coding-plan 订阅通道（api.z.ai/api/anthropic，OAuth 换 coding_plan_key）见
//! 文件尾「订阅双通道」节。
//!
//! 错误语义（对照 deepseek-harness-codearts zcode-adapter.ts:1856-1872
//! `httpErrorCodeForZcode` 的判定顺序）：
//! - 并发限流（3009 / "concurrency limit" 文案，任意状态码）→ RateLimited
//! - 3007（验证码）→ RateLimited（验证码载体链路在 T6.3 接入后自动恢复）
//! - 3012（风控，语义判据见 [`is_unusual_activity`]）→ RateLimited 30 分钟
//!   （账号冷却惩罚：30min→24h→停用，编排层据此冷却该账号并换号）
//! - 401 / 业务码 1002 → Credential（JWT 失效，不可续期，需重登）
//! - 429 → RateLimited（读 Retry-After，缺省 60s）
//! - 400 → BadRequest；5xx → Upstream
//!
//! 3012 要求 system 携带官方身份块（cliPrefix+stable，逐字官方文本，
//! zcode-identity.ts 实测矩阵：无 system/仅 cliPrefix 均 405+3012）。
//! 真实上游前必须通过 `with_system_prefix` 注入从本机 ZCode 安装提取的块
//! （zcode-pool prompt.rs 的做法）；stub 测试不需要。

use async_trait::async_trait;
use serde_json::{json, Map, Value};

use crate::anthropic::openai_to_anthropic_body;
use crate::openai::{ChatCompletion, ChatRequest, OutMessage, Usage};
use crate::provider::{ChunkStream, Credential, Provider, ProviderError, StreamChunk};
use crate::registry::{ModelInfo, ProviderCatalog};
use crate::route::Route;

pub const DEFAULT_BASE: &str = "https://zcode.z.ai";
/// 官方客户端版本（身份头 X-ZCode-App-Version / UA 用）。
pub const DEFAULT_CLIENT_VERSION: &str = "3.14.4";

/// CLI 设备码登录流程句柄。
pub struct ZcodeLoginFlow {
    pub flow_id: String,
    pub auth_url: String,
    /// init 时自生成的 32 字节 hex（轮询沿用）。
    bearer: String,
    /// 服务端建议的轮询间隔（秒，≥1；登录编排可参考）。
    pub poll_interval_secs: u64,
    /// 授权流程过期时刻（Unix 秒；0 = 未下发）。
    pub expires_at: u64,
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
    /// 可领活动摘要（来自 preview；**每日赠送不在 balances 里**，只在
    /// preview.plans——只读 buckets 面板会显示 0。失败降级为空数组，
    /// 不影响 remaining/total 本身；enterprise 形态不下发也不发 preview）。
    pub claimable_plans: Vec<ZcodePlan>,
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

/// 可领套餐（preview.plans 的一项；量取自 entitlements[].grant_units）。
#[derive(Debug, Clone, PartialEq)]
pub struct ZcodePlan {
    pub plan_id: String,
    pub name: String,
    /// Σ(entitlements[] 中 meter=model_usage 且 unit_type=token 的 grant_units)。
    pub tokens: u64,
    /// 领取优先级（降序领取，与官方/第三方实现一致）。
    pub priority: i64,
}

/// 阿里云验证码凭据（T6.3 载体产出；本地校验见 [`captcha_param_valid`]，
/// 不合格不发——在索要验证的窗口里发了必 3007）。
#[derive(Debug, Clone)]
pub struct CaptchaParam {
    pub param: String,
    pub region: String,
}

/// 领取结果。3007 是预期内的「先探后取」状态，不算错误。
/// 1005 → Cooldown（next_at 取 data.plan.ends_at，zcode-pool claim.rs:68-81）。
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

/// 3012 的账号冷却（30 分钟；反复触发升级 24h/停用——zcode-identity.ts 维护警告）。
const IDENTITY_COOLDOWN_SECS: u64 = 30 * 60;

/// 3007 的重试间隔（换个新 param 就能过，短暂冷却即可）。
const CAPTCHA_COOLDOWN_SECS: u64 = 60;

/// 官方 client/configs 实测的思考档位（reasoning.levels 只见 low/max/high；
/// 未知档位直接下发可能被上游拒，zcode-adapter.ts:794-818）。
const KNOWN_EFFORT_LEVELS: [&str; 3] = ["low", "high", "max"];

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

    // ───────────── 登录（zcode-login.ts + zcode-pool oauth.rs 双参考） ─────────────

    /// CLI 设备码登录：init 拿授权链接（浏览器打开），轮询直到用户完成授权。
    /// 响应字段在 `data` 下：`{code, msg, data:{flow_id, authorize_url,
    /// poll_token, expires_at, poll_interval_sec}}`（zcode-login.ts:198-241）。
    pub async fn login_init(&self) -> Result<ZcodeLoginFlow, ProviderError> {
        let bearer = crate::key::random_id(32); // 32 字节 hex，一次性登录种子
        let resp = self
            .client
            .post(format!("{}/api/v1/oauth/cli/init", self.base))
            .bearer_auth(&bearer)
            .json(&json!({ "provider": "bigmodel" }))
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        if status != 200 {
            return Err(ProviderError::Upstream(format!(
                "login init http {status}: {}",
                truncate(&text)
            )));
        }
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| ProviderError::Upstream(format!("login init body: {e}")))?;
        if biz_code_of(&v) != Some(0) {
            let msg = v.get("msg").and_then(Value::as_str).unwrap_or("");
            return Err(ProviderError::Upstream(format!(
                "login init rejected: {}",
                truncate(msg)
            )));
        }
        let data = v.get("data").cloned().unwrap_or(Value::Null);
        let flow_id = data
            .get("flow_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ProviderError::Upstream("login init missing data.flow_id".into()))?
            .to_string();
        let auth_url = data
            .get("authorize_url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        Ok(ZcodeLoginFlow {
            flow_id,
            auth_url,
            bearer,
            poll_interval_secs: data
                .get("poll_interval_sec")
                .and_then(Value::as_u64)
                .unwrap_or(2),
            expires_at: data.get("expires_at").and_then(Value::as_u64).unwrap_or(0),
        })
    }

    /// 轮询一次：`Ok(None)` = 尚未完成（pending / 网络抖动 / 5xx——继续轮询）；
    /// `Err` = 终态失败（4xx（除 408/429）或用户拒绝授权）。
    /// ready 载荷：`data:{status:"ready", token, user:{user_id}, zai/bigmodel:
    /// {access_token}}`（zcode-login.ts:287-386）。
    pub async fn login_poll(
        &self,
        flow: &ZcodeLoginFlow,
    ) -> Result<Option<Credential>, ProviderError> {
        let resp = match self
            .client
            .get(format!(
                "{}/api/v1/oauth/cli/poll/{}",
                self.base, flow.flow_id
            ))
            .bearer_auth(&flow.bearer)
            .send()
            .await
        {
            Ok(r) => r,
            // 网络抖动不算失败，让调用方继续轮询（zcode-login.ts:266-274）。
            Err(_) => return Ok(None),
        };
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        // 4xx（除 408/429）是终态失败；5xx 继续轮询。
        if (400..500).contains(&status) && status != 408 && status != 429 {
            return Err(ProviderError::Upstream(format!(
                "login poll rejected (http {status}): {}",
                truncate(&text)
            )));
        }
        if status != 200 {
            return Ok(None);
        }
        let v: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => return Ok(None), // 非 JSON 视作 pending（zcode-login.ts:300-302）
        };
        if biz_code_of(&v) != Some(0) {
            return Ok(None); // 业务码非 0 = 未就绪（zcode-login.ts:305-307）
        }
        let data = v.get("data").cloned().unwrap_or(Value::Null);
        match data.get("status").and_then(Value::as_str) {
            Some("ready") => {}
            Some("failed") => {
                return Err(ProviderError::Upstream("login denied or failed".into()));
            }
            _ => return Ok(None), // pending / 未知状态
        }
        let token = data
            .get("token")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|t| !t.is_empty() && !t.starts_with('{'))
            .ok_or_else(|| ProviderError::Upstream("login ready but no token".into()))?;
        // OAuth token 按登录渠道落位（zai ?? bigmodel 优先级与 transport 一致），
        // 是订阅腿换 key 的原料；两者都缺时裸串（历史兼容，订阅腿不可用）。
        let pick_access = |block: &str| {
            data.pointer(&format!("/{block}/access_token"))
                .or_else(|| data.pointer(&format!("/{block}/accessToken")))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_string)
        };
        let zai = pick_access("zai");
        let bigmodel = pick_access("bigmodel");
        let mut m = Map::new();
        m.insert("zcode_jwt".into(), Value::String(token.to_string()));
        if let Some(t) = zai {
            m.insert("zai_access_token".into(), Value::String(t));
        }
        if let Some(t) = bigmodel {
            m.insert("bigmodel_access_token".into(), Value::String(t));
        }
        let secret = if m.len() > 1 {
            Value::Object(m).to_string()
        } else {
            token.to_string()
        };
        // user_id 是唯一稳定账号标识（去重判据，zcode-auth.ts:550-559）。
        let user_id = data
            .pointer("/user/user_id")
            .or_else(|| data.pointer("/user/id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let account_id = user_id.unwrap_or_else(|| {
            format!("zcode-{}", flow.flow_id.chars().take(8).collect::<String>())
        });
        Ok(Some(Credential { account_id, secret }))
    }

    /// 阻塞轮询到登录成功；成功后**顺手换** coding-plan 的 api-key（付费
    /// 订阅腿）——换不到不报错（没订阅是常态），按登录渠道写
    /// coding_plan_key_zai / coding_plan_key_bigmodel（zcode-auth.ts:592-613）。
    /// 截止 = min(调用方超时 300s, 服务端 expires_at − 1s 余量)。
    pub async fn login_with_interval(
        &self,
        interval: std::time::Duration,
    ) -> Result<Credential, ProviderError> {
        let flow = self.login_init().await?;
        let budget = std::time::Duration::from_secs(300);
        let from_server = if flow.expires_at > 0 {
            std::time::Duration::from_secs(flow.expires_at)
                .saturating_sub(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default(),
                )
                .saturating_sub(std::time::Duration::from_secs(1))
        } else {
            budget
        };
        let deadline = std::time::Instant::now() + budget.min(from_server);
        loop {
            if let Some(cred) = self.login_poll(&flow).await? {
                return Ok(self.attach_coding_plan_key(cred).await);
            }
            if std::time::Instant::now() > deadline {
                return Err(ProviderError::Upstream("login poll timeout".into()));
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
        let Some(obj) = v.as_object_mut() else {
            return cred;
        };
        // 按「拿到哪个 OAuth token」落位（zai ?? bigmodel 优先级一致）
        let field = if obj.contains_key("zai_access_token") {
            "coding_plan_key_zai"
        } else {
            "coding_plan_key_bigmodel"
        };
        obj.insert(field.into(), Value::String(key));
        Credential {
            secret: Value::Object(obj.clone()).to_string(),
            ..cred
        }
    }

    // ───────────── 余额（upstream.ts:256-330） ─────────────

    /// 余额：`data.balances[]` 桶累加（available 优先于 remaining）；
    /// `displayMode=="enterprise"` 不下发额度数字。
    /// ⚠️ 每日赠送**不在 balances 里**，只在 preview.plans——这里合并
    /// claim_preview 摘要（失败降级为空），否则面板显示 0。
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
            return Err(endpoint_error("balance", status, &text));
        }
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| ProviderError::Upstream(format!("balance body: {e}")))?;
        let data = v.get("data").cloned().unwrap_or(Value::Null);
        if data.get("displayMode").and_then(Value::as_str) == Some("enterprise") {
            // 企业版不下发额度数字，也没有可领活动 ⇒ 不发 preview。
            return Ok(ZcodeBalance {
                enterprise: true,
                buckets: Vec::new(),
                remaining: 0,
                total: 0,
                expires_at: None,
                claimable_plans: Vec::new(),
            });
        }
        let str_of =
            |m: &Map<String, Value>, k: &str| m.get(k).and_then(Value::as_str).map(str::to_string);
        let u64_of = |m: &Map<String, Value>, k: &str| m.get(k).and_then(Value::as_u64);
        let mut buckets = Vec::new();
        if let Some(arr) = data.get("balances").and_then(Value::as_array) {
            for item in arr {
                let Some(obj) = item.as_object() else {
                    continue;
                };
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
            // 上游 JSON 数字不受信任：饱和加法防巨数溢出 panic
            remaining = remaining.saturating_add(b.available_units.or(b.remaining_units).unwrap_or(0));
            total = total.saturating_add(b.total_units.unwrap_or(0));
            if let Some(exp) = b.expires_at {
                expires_at = Some(match expires_at {
                    Some(prev) => prev.min(exp),
                    None => exp,
                });
            }
        }
        let claimable_plans = self.claim_preview(cred).await.unwrap_or_default();
        Ok(ZcodeBalance {
            enterprise: false,
            buckets,
            remaining,
            total,
            expires_at,
            claimable_plans,
        })
    }

    // ───────────── 领取链（zcode-auth.ts + upstream.ts） ─────────────

    /// 补激活事件（app_launch / app_daily_active，各一条独立上报）——
    /// 不补则 preview 恒空（「每日随机派发」实为按活跃信号决定）。
    /// 上报失败不阻塞（幂等：服务端按 device_mid + 日期去重；下一次会再补）。
    pub async fn report_activity(&self, cred: &Credential) -> Result<(), ProviderError> {
        for event in ["app_launch", "app_daily_active"] {
            let body = json!({
                "event": event,
                "device_mid": self.device_mid,
                "platform": "win32",
                "app_version": self.version,
            });
            // best-effort：单条失败不影响其余与后续 preview/claim。
            let _ = self
                .client
                .post(format!("{}/api/v1/event/report", self.base))
                .headers(self.identity_headers(cred))
                .json(&body)
                .send()
                .await;
        }
        Ok(())
    }

    /// 可领套餐列表（priority 降序，tiebreak plan_id 升序——zcode-pool claim.rs:291）。
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
            return Err(endpoint_error("preview", status, &text));
        }
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| ProviderError::Upstream(format!("preview body: {e}")))?;
        let mut plans = Vec::new();
        if let Some(arr) = v.pointer("/data/plans").and_then(Value::as_array) {
            for p in arr {
                let plan_id = p
                    .get("plan_id")
                    .or_else(|| p.get("planId"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                let Some(plan_id) = plan_id else { continue };
                let name = p
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .unwrap_or_default()
                    .to_string();
                let priority = p.get("priority").and_then(Value::as_i64).unwrap_or(0);
                // 额度只在 entitlements[] 里（plan 顶层没有），且只认
                // meter=model_usage && unit_type=token 的量（upstream.ts:440-448）。
                let tokens: u64 = p
                    .get("entitlements")
                    .and_then(Value::as_array)
                    .map(|ents| {
                        ents.iter()
                            .filter(|e| {
                                str_field(e, "meter").as_deref() == Some("model_usage")
                                    && str_field(e, "unit_type").as_deref() == Some("token")
                            })
                            .filter_map(|e| {
                                e.get("grant_units")
                                    .or_else(|| e.get("grantUnits"))
                                    .and_then(Value::as_f64)
                            })
                            .map(|n| n as u64)
                            .sum()
                    })
                    .unwrap_or(0);
                plans.push(ZcodePlan {
                    plan_id,
                    name,
                    tokens,
                    priority,
                });
            }
        }
        plans.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.plan_id.cmp(&b.plan_id)));
        Ok(plans)
    }

    /// 提交领取。无 captcha 时先探（3007 → NeedCaptcha）；带 captcha 时先本地
    /// 校验 param（长度 ≥200 + base64 JSON 含 certifyId/securityToken，不合格
    /// 在索要窗口必 3007，不发——zcode-captcha.ts:163-200）。
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
            .json(&json!({ "plan_id": plan_id }));
        if let Some(cap) = captcha {
            if !captcha_param_valid(&cap.param) {
                return Ok(ClaimOutcome::NeedCaptcha);
            }
            req = req
                .header("x-aliyun-captcha-verify-param", cap.param.trim())
                .header("x-aliyun-captcha-verify-region", cap.region.trim());
        }
        let resp = req
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        if status != 200 {
            return Err(endpoint_error("claim", status, &text));
        }
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| ProviderError::Upstream(format!("claim body: {e}")))?;
        // 业务码可能是纯数字字符串（{"code":"3012"} 实测出现过——
        // parseZcodeBusinessCode 的教训），不能只认 number。
        let ends_at = || {
            v.pointer("/data/plan/ends_at")
                .and_then(|x| {
                    x.as_str()
                        .map(str::to_string)
                        .or_else(|| x.as_i64().map(|n| n.to_string()))
                })
                .unwrap_or_default()
        };
        match biz_code_of(&v) {
            Some(0) => Ok(ClaimOutcome::Claimed { ends_at: ends_at() }),
            Some(1003) => Ok(ClaimOutcome::AlreadyClaimed),
            Some(1005) => Ok(ClaimOutcome::Cooldown { next_at: ends_at() }),
            Some(3007) => Ok(ClaimOutcome::NeedCaptcha),
            Some(3012) => Err(ProviderError::RateLimited {
                // 风控有账号冷却惩罚，绝不连续重试（zcode-auth.ts claimOnePlan 短路）
                retry_after_secs: Some(IDENTITY_COOLDOWN_SECS),
                msg: format!(
                    "claim blocked by risk control (3012); do not retry: {}",
                    truncate(&text)
                ),
            }),
            Some(code) => Err(ProviderError::Upstream(format!(
                "claim code {code}: {}",
                truncate(&text)
            ))),
            None => Err(ProviderError::Upstream(format!(
                "claim no code: {}",
                truncate(&text)
            ))),
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

    // ───────────── 推理请求组装（zcode-adapter.ts:750-818 + zcode-anthropic.ts） ─────────────

    /// OpenAI ChatRequest → zcode 的 Anthropic Messages 请求体。
    /// 在共享的 [`openai_to_anthropic_body`]（system 抽取/max_tokens/temperature）
    /// 之上补齐 zcode 差异项：
    /// - `role:tool` 消息 → `role:user` 的 `tool_result` 块（连续合并）；
    /// - assistant `tool_calls` → `content:[{type:"tool_use",input:<对象>}]`
    ///   （共享层不透传 tool_calls，故消息数组必须在这里从 ChatRequest 重建）；
    /// - `tools` OpenAI 嵌套形态 → Anthropic 扁平 `input_schema` 形态；
    /// - `stop` → `stop_sequences`（数组）；
    /// - `reasoning_effort` → `output_config.effort`（推理信封；协议名是
    ///   output_config.effort 而非 reasoning_effort，官方 client/configs 下发，
    ///   且只在模型声明过的档位内写）。
    fn build_body(&self, req: &ChatRequest, stream: bool) -> Value {
        let mut upstream_req = req.clone();
        upstream_req.model = req.model.trim_start_matches("zcode/").to_string();
        upstream_req.stream = stream;
        let mut body = openai_to_anthropic_body(&upstream_req, self.system_prefix.as_deref());
        body["messages"] = Value::Array(to_anthropic_messages(&req.messages));
        if let Some(tools) = flatten_tools(req.raw.get("tools")) {
            body["tools"] = Value::Array(tools);
        }
        if let Some(stop) = req.raw.get("stop") {
            let seqs = match stop {
                Value::String(s) if !s.is_empty() => json!([s]),
                Value::Array(a) if !a.is_empty() => Value::Array(a.clone()),
                _ => Value::Null,
            };
            if !seqs.is_null() {
                body["stop_sequences"] = seqs;
            }
        }
        if let Some(effort) = req
            .raw
            .get("reasoning_effort")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|e| KNOWN_EFFORT_LEVELS.contains(e))
        {
            body["output_config"] = json!({ "effort": effort });
        }
        body
    }

    /// 按通道发送推理请求；429 + 1005/1113（两通道额度独立）且另一通道
    /// 可用时换腿重发一次。401/1002（同凭据）、3012（风控重试加重惩罚）、
    /// 3009（并发走退避）**都不换**。换腿也失败 ⇒ 落回**原通道**的错误
    /// （不把「换腿失败」报成用户看到的那个错误，adapter.ts:1123-1128）。
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
                ZcodeChannel::CodingPlan => {
                    format!("{}{}", self.coding_plan_base, CODING_PLAN_MESSAGES_PATH)
                }
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
            // 换腿重发一次（不重产 captcha——param 一次性，这里本就不带）；
            // 只有换腿**成功**才采用其响应，否则保持原 429 的错误语义。
            if let Ok(resp2) = send(other).await {
                if resp2.status().is_success() {
                    return Ok(Ok(resp2));
                }
            }
        }
        Ok(Err(ProviderError::RateLimited {
            retry_after_secs: Some(retry_after.unwrap_or(60)),
            msg: truncate(&text),
        }))
    }

    async fn post_messages(
        &self,
        cred: &Credential,
        route: &Route,
        req: &ChatRequest,
    ) -> Result<ChatCompletion, ProviderError> {
        let body = self.build_body(req, false);
        let resp = match self.dispatch(cred, route, &body).await? {
            Ok(resp) => resp,
            Err(e) => return Err(e),
        };
        if resp.status().as_u16() == 200 {
            let v: Value = resp
                .json()
                .await
                .map_err(|e| ProviderError::Upstream(e.to_string()))?;
            return completion_from_zcode(&route.composite(), &v).map_err(ProviderError::Upstream);
        }
        Err(map_upstream_error(resp).await)
    }
}

/// 辅助端点（balance/preview/claim）的 HTTP 错误映射：
/// 401 → Credential（JWT 失效，编排层应换号重登）；其余 → Upstream。
fn endpoint_error(what: &str, status: u16, text: &str) -> ProviderError {
    if status == 401 {
        ProviderError::Credential(format!("{what} http 401: {}", truncate(text)))
    } else {
        ProviderError::Upstream(format!("{what} http {status}: {}", truncate(text)))
    }
}

/// 解析上游业务码：认 number，**也认纯数字字符串**（`{"code":"3012"}` 在线上
/// 出现过，只认 number 会把风控码整条吃掉——parseZcodeBusinessCode 的教训）。
fn biz_code_of(v: &Value) -> Option<i64> {
    match v.get("code")? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => {
            let t = s.trim();
            if !t.is_empty() && t.chars().all(|c| c.is_ascii_digit() || c == '-') {
                t.parse().ok()
            } else {
                None
            }
        }
        _ => None,
    }
}

fn str_field(v: &Value, snake: &str) -> Option<String> {
    v.get(snake)
        .or_else(|| v.get(camel(snake)))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn camel(snake: &str) -> String {
    let mut out = String::with_capacity(snake.len());
    let mut upper = false;
    for c in snake.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// 上游是否表达了风控（3012）。判据分优先级（zcode-diagnostics.ts:175-179）：
/// ① 正文能解析出业务码 ⇒ 只信码（`{"code":3012}` 是风控，`13012` 不是）；
/// ② 无码才看正文语义：`unusual activity` / `blocked due to` 短语，或
///   词边界 `3012` 与 `block` 紧邻（≤10 字符，排除 `retry after 3012 ms`）。
fn is_unusual_activity(text: &str) -> bool {
    if let Ok(v) = serde_json::from_str::<Value>(text.trim()) {
        if let Some(code) = biz_code_of(&v) {
            return code == 3012;
        }
    }
    let lower = text.to_lowercase();
    if lower.contains("unusual activity") || lower.contains("blocked due to") {
        return true;
    }
    word_positions(&lower, "3012").any(|(start, end)| {
        let lo = start.saturating_sub(10);
        let hi = (end + 10).min(lower.len());
        lower.get(lo..hi).is_some_and(|w| w.contains("block"))
    })
}

/// 词边界匹配 `needle`（边界集 = 数字/字母/`_`/`-`，排除 `13012`、
/// `plan-3012-trust` 这类紧邻形态）的全部出现位置（字节区间）。
fn word_positions(haystack: &str, needle: &str) -> impl Iterator<Item = (usize, usize)> {
    let bytes = haystack.as_bytes();
    let is_boundary = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
    let mut from = 0;
    let mut out = Vec::new();
    while let Some(rel) = haystack[from..].find(needle) {
        let start = from + rel;
        let end = start + needle.len();
        let ok_before = start == 0 || !is_boundary(bytes[start - 1]);
        let ok_after = end == bytes.len() || !is_boundary(bytes[end]);
        if ok_before && ok_after {
            out.push((start, end));
        }
        from = end;
    }
    out.into_iter()
}

/// 非即改即用的错误响应 → ProviderError（complete 与 stream 共用）。
/// 判定顺序对照 zcode-adapter.ts:1856-1872 `httpErrorCodeForZcode`：
/// 并发文案 → 3007 → 3012 → 401/1002 → 429 → 5xx → 400。
async fn map_upstream_error(resp: reqwest::Response) -> ProviderError {
    let status = resp.status().as_u16();
    let retry_after = resp
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());
    let text = resp.text().await.unwrap_or_default();
    let lower = text.to_lowercase();
    // ① 并发限流：状态码只是门槛，判据在正文（装在 1005 码里也要先按并发报）
    if lower.contains("3009") || lower.contains("concurrency limit") {
        return ProviderError::RateLimited {
            retry_after_secs: Some(retry_after.unwrap_or(60)),
            msg: format!("concurrency limited: {}", truncate(&text)),
        };
    }
    // ② captcha 被拒：不判状态码（网关可能换壳包裹同一业务码）
    if text.contains("3007") || lower.contains("captcha verify failed") {
        return ProviderError::RateLimited {
            retry_after_secs: Some(retry_after.unwrap_or(CAPTCHA_COOLDOWN_SECS)),
            msg: format!("captcha required/rejected (3007): {}", truncate(&text)),
        };
    }
    // ③ 风控：有账号冷却惩罚（30min→24h→停用），编排层冷却该账号
    if is_unusual_activity(&text) {
        return ProviderError::RateLimited {
            retry_after_secs: Some(IDENTITY_COOLDOWN_SECS),
            msg: format!(
                "identity/risk-control blocked (3012); do not retry aggressively: {}",
                truncate(&text)
            ),
        };
    }
    // ④ 凭据失效（401 / 业务码 1002）→ 换号重登
    if status == 401 || biz_code_from_text(&text) == Some(1002) {
        return ProviderError::Credential(format!("credential rejected: {}", truncate(&text)));
    }
    match status {
        429 => ProviderError::RateLimited {
            retry_after_secs: Some(retry_after.unwrap_or(60)),
            msg: truncate(&text),
        },
        s if (500..=599).contains(&s) => {
            ProviderError::Upstream(format!("http {s}: {}", truncate(&text)))
        }
        400 => ProviderError::BadRequest(format!("http 400: {}", truncate(&text))),
        code => ProviderError::Upstream(format!("http {code}: {}", truncate(&text))),
    }
}

/// 从（可能非 JSON 的）正文里读业务码。
fn biz_code_from_text(text: &str) -> Option<i64> {
    serde_json::from_str::<Value>(text.trim())
        .ok()
        .and_then(|v| biz_code_of(&v))
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

/// 一个进行中的 tool_use 块（id/name 来自 content_block_start，参数增量
/// 来自 input_json_delta——zcode-anthropic.ts:583-654）。
#[derive(Default)]
struct ToolBlock {
    id: String,
    name: String,
    /// 是否已发出首片（OpenAI 约定 id/name 仅首片携带）。
    emitted: bool,
}

/// zcode 流式状态机：上游 Anthropic SSE 事件 → StreamChunk。
/// 对照参考 consumeAnthropicSse（zcode-anthropic.ts:448-829）：
/// - `error` 事件**必须报错**（静默当正常结束是 Qoder 同型缺陷）；
/// - `input_json_delta` → ToolCallDelta（工具调用流式）；
/// - 有工具调用时 finish 必须 `tool_calls`（不论 stop_reason 说什么）；
/// - usage 从 message_start 与 message_delta 两处收集（各带一半字段）；
/// - 200 但零内容事件 ⇒ 空响应错误（额度/权益形态，而非无声空回复）。
struct ZcodeStreamState {
    bytes: ByteChunkStream,
    parser: crate::sse::SseParser,
    input_tokens: u64,
    output_tokens: u64,
    stop_reason: String,
    queue: std::collections::VecDeque<Result<StreamChunk, ProviderError>>,
    role_sent: bool,
    done: bool,
    saw_content: bool,
    saw_message_stop: bool,
    /// anthropic 块 index → 工具块状态。
    tools: std::collections::BTreeMap<u64, ToolBlock>,
    /// anthropic 块 index → OpenAI tool_calls 序号（顺序重编）。
    tool_index: std::collections::BTreeMap<u64, u64>,
}

impl ZcodeStreamState {
    fn on_event(&mut self, data: &str) {
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            return; // 忽略无法解析的载荷（如 ping）
        };
        match v.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                if let Some(u) = v.pointer("/message/usage") {
                    if let Some(inp) = u.get("input_tokens").and_then(Value::as_u64) {
                        self.input_tokens = inp;
                    }
                }
                if !self.role_sent {
                    self.role_sent = true;
                    self.queue.push_back(Ok(StreamChunk::Role));
                }
            }
            Some("content_block_start") => {
                let index = v.get("index").and_then(Value::as_u64).unwrap_or(0);
                if v.pointer("/content_block/type").and_then(Value::as_str) == Some("tool_use") {
                    let block = v.get("content_block").cloned().unwrap_or(Value::Null);
                    let entry = self.tools.entry(index).or_default();
                    entry.id = block
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("call_{index}"));
                    entry.name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                }
            }
            Some("content_block_delta") => {
                let index = v.get("index").and_then(Value::as_u64).unwrap_or(0);
                let delta = &v["delta"];
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        if let Some(t) = delta.get("text").and_then(Value::as_str) {
                            if !t.is_empty() {
                                self.saw_content = true;
                            }
                            self.queue
                                .push_back(Ok(StreamChunk::Content(t.to_string())));
                        }
                    }
                    Some("thinking_delta") => {
                        if let Some(t) = delta.get("thinking").and_then(Value::as_str) {
                            if !t.is_empty() {
                                self.saw_content = true;
                            }
                            self.queue
                                .push_back(Ok(StreamChunk::Reasoning(t.to_string())));
                        }
                    }
                    Some("input_json_delta") => {
                        let partial = delta
                            .get("partial_json")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        let next = self.tool_index.len() as u64;
                        let openai_index = *self.tool_index.entry(index).or_insert(next);
                        let entry = self.tools.entry(index).or_default();
                        if !entry.emitted {
                            entry.emitted = true;
                            self.saw_content = true;
                            self.queue.push_back(Ok(StreamChunk::ToolCallDelta {
                                index: openai_index,
                                id: (!entry.id.is_empty()).then(|| entry.id.clone()),
                                name: (!entry.name.is_empty()).then(|| entry.name.clone()),
                                arguments: partial.to_string(),
                            }));
                        } else if !partial.is_empty() {
                            self.queue.push_back(Ok(StreamChunk::ToolCallDelta {
                                index: openai_index,
                                id: None,
                                name: None,
                                arguments: partial.to_string(),
                            }));
                        }
                    }
                    _ => {}
                }
            }
            Some("content_block_stop") => {
                // 工具块结束但从未给过参数增量 ⇒ 补一片空参数（OpenAI 面需要
                // 至少一片才能拼出 tool_calls；空参数在收尾聚合时补 "{}"）。
                let index = v.get("index").and_then(Value::as_u64).unwrap_or(0);
                if let Some(entry) = self.tools.get_mut(&index) {
                    if !entry.emitted && !entry.name.is_empty() {
                        entry.emitted = true;
                        self.saw_content = true;
                        let next = self.tool_index.len() as u64;
                        let openai_index = *self.tool_index.entry(index).or_insert(next);
                        self.queue.push_back(Ok(StreamChunk::ToolCallDelta {
                            index: openai_index,
                            id: (!entry.id.is_empty()).then(|| entry.id.clone()),
                            name: Some(entry.name.clone()),
                            arguments: String::new(),
                        }));
                    }
                }
            }
            Some("message_delta") => {
                if let Some(stop) = v.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.stop_reason = map_stop_reason(stop);
                }
                if let Some(u) = v.get("usage") {
                    if let Some(out) = u.get("output_tokens").and_then(Value::as_u64) {
                        self.output_tokens = out;
                    }
                    if let Some(inp) = u.get("input_tokens").and_then(Value::as_u64) {
                        self.input_tokens = inp; // 有值才覆盖（两处事件各带一半字段）
                    }
                }
            }
            Some("message_stop") => {
                self.saw_message_stop = true;
                // 200 但零内容（无 text/thinking/tool 增量）⇒ 空响应：额度/
                // 权益形态（秒回空）或上游静默——显式报错，绝不无声空回复
                // （consumeAnthropicSse 的 !sawAny → EMPTY_RESPONSE）。
                if !self.saw_content {
                    self.queue.push_back(Err(ProviderError::Upstream(
                        "empty response (no text/thinking/tool deltas)".into(),
                    )));
                    self.done = true;
                    return;
                }
                // 有工具调用时必须报 tool_calls（不论 stop_reason 说什么）：
                // 那是消费层决定「继续执行工具」的依据（zcode-anthropic.ts:822-827）。
                let reason = if self.tools.values().any(|t| !t.name.is_empty()) {
                    "tool_calls".to_string()
                } else {
                    self.stop_reason.clone()
                };
                let usage = Usage::sum(self.input_tokens, self.output_tokens);
                self.queue
                    .push_back(Ok(StreamChunk::Finish { reason, usage }));
            }
            Some("error") => {
                // 错误必须抛，绝不静默当「正常结束」（AGENTS.md 记过同型缺陷）；
                // overloaded 暂时性 → RateLimited，其余按上游故障。
                let err = &v["error"];
                let msg = err
                    .get("message")
                    .and_then(Value::as_str)
                    .or_else(|| err.as_str())
                    .unwrap_or("");
                let etype = err.get("type").and_then(Value::as_str).unwrap_or("");
                let e = if etype == "overloaded_error" {
                    ProviderError::RateLimited {
                        retry_after_secs: Some(60),
                        msg: truncate(msg),
                    }
                } else {
                    ProviderError::Upstream(if msg.is_empty() {
                        truncate(data)
                    } else {
                        truncate(msg)
                    })
                };
                self.queue.push_back(Err(e));
                self.done = true;
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
        // 通道模型并集（transport.ts CHANNEL_MODELS）：start-plan 承载
        // glm-5.3-flash/glm-5.2/glm-5-turbo，订阅腿承载 glm-5.3/glm-5.3-flash。
        ProviderCatalog {
            id: "zcode".into(),
            models: vec![
                ModelInfo {
                    id: "glm-5.3-flash".into(),
                },
                ModelInfo {
                    id: "glm-5.3".into(),
                },
                ModelInfo {
                    id: "glm-5.2".into(),
                },
                ModelInfo {
                    id: "glm-5-turbo".into(),
                },
            ],
        }
    }

    async fn complete(
        &self,
        cred: &Credential,
        route: &Route,
        req: &ChatRequest,
    ) -> Result<ChatCompletion, ProviderError> {
        self.post_messages(cred, route, req).await
    }

    async fn stream(
        &self,
        cred: &Credential,
        route: &Route,
        req: &ChatRequest,
    ) -> Result<ChunkStream, ProviderError> {
        let body = self.build_body(req, true);
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
            saw_content: false,
            saw_message_stop: false,
            tools: std::collections::BTreeMap::new(),
            tool_index: std::collections::BTreeMap::new(),
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
                            if st.done {
                                break;
                            }
                        }
                    }
                    Some(Err(e)) => {
                        st.done = true;
                        return Some((Err(ProviderError::Upstream(e.to_string())), st));
                    }
                    None => {
                        // 上游结束；没有 message_stop 就不伪造 Finish（截断保持诚实）。
                        st.done = true;
                        st.parser.finalize();
                        while let Some(data) = st.parser.next_data() {
                            st.on_event(&data);
                            if st.done {
                                break;
                            }
                        }
                        if let Some(item) = st.queue.pop_front() {
                            return Some((item, st));
                        }
                        // 200 但零内容事件（也无 error/Finish）⇒ 空响应：额度/
                        // 权益形态（秒回空）或上游静默，绝不能无声给出空回复。
                        if !st.saw_content && !st.saw_message_stop {
                            return Some((
                                Err(ProviderError::Upstream(
                                    "empty response (no text/thinking/tool deltas)".into(),
                                )),
                                st,
                            ));
                        }
                        return None;
                    }
                }
            }
        });
        Ok(Box::pin(stream))
    }
}

// ───────────── Anthropic 形状转换（zcode-anthropic.ts:69-262） ─────────────

/// assistant 工具调用参数 → Anthropic 的 `input`（**对象**，不是 JSON 字符串）。
/// 解析失败给哨兵对象（不补 `{}`：残缺参数应让上游报 schema 错并重试，
/// 而不是被静默当成「无参数调用」——zcode-anthropic.ts:69-86）。
fn parse_tool_arguments(raw: Option<&Value>) -> Value {
    match raw {
        None | Some(Value::Null) => json!({}),
        Some(v @ Value::Object(_)) => v.clone(),
        Some(Value::String(s)) => {
            let t = s.trim();
            if t.is_empty() {
                json!({})
            } else {
                match serde_json::from_str::<Value>(t) {
                    Ok(parsed) if parsed.is_object() => parsed,
                    _ => json!({ "__zcodeUnparsableArguments": t }),
                }
            }
        }
        Some(_) => json!({}),
    }
}

/// OpenAI 形态消息 → Anthropic Messages 形态：
/// - `role:tool` → 包成 `role:user` 的 `tool_result` 块，连续多条合并进同一条
///   user 消息（Anthropic 允许，且比发多条 user 更贴近官方形态）；
/// - assistant 的 `tool_calls` → `content:[{type:"tool_use",…}]`，`arguments`
///   （字符串）→ `input`（对象）；文本与 tool_use 可共存；
/// - 空 assistant 消息丢弃（会让上游 400）；孤儿 tool 结果丢弃（同上）；
/// - system 消息由共享层抽走，这里跳过。
fn to_anthropic_messages(msgs: &[crate::openai::ChatMessage]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::with_capacity(msgs.len());
    for m in msgs {
        match m.role.as_str() {
            "system" => {} // 共享层已抽到顶层 system
            "tool" => {
                let tool_use_id = m.tool_call_id.as_deref().map(str::trim).unwrap_or("");
                if tool_use_id.is_empty() {
                    continue; // 孤儿工具结果：丢弃（Anthropic 会 400）
                }
                let block = json!({
                    "type": "tool_result",
                    "tool_use_id": tool_use_id,
                    "content": m.text(),
                });
                // 连续多条 tool 结果合并进同一条 user 消息（Anthropic 允许，
                // 且比发多条 user 更贴近官方形态）。
                let mergeable = out.last().is_some_and(|last| {
                    last.get("role").and_then(Value::as_str) == Some("user")
                        && last.get("content").is_some_and(Value::is_array)
                });
                if mergeable {
                    if let Some(arr) = out
                        .last_mut()
                        .and_then(|last| last.get_mut("content"))
                        .and_then(Value::as_array_mut)
                    {
                        arr.push(block);
                    }
                } else {
                    out.push(json!({ "role": "user", "content": [block] }));
                }
            }
            "assistant" => {
                let mut blocks: Vec<Value> = Vec::new();
                let text = m.text();
                if !text.is_empty() {
                    blocks.push(json!({ "type": "text", "text": text }));
                }
                if let Some(calls) = m.tool_calls.as_ref().and_then(Value::as_array) {
                    for c in calls {
                        let name = c
                            .pointer("/function/name")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|s| !s.is_empty());
                        let Some(name) = name else { continue };
                        let id = c
                            .get("id")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string)
                            .unwrap_or_else(|| format!("call_{}", out.len()));
                        blocks.push(json!({
                            "type": "tool_use",
                            "id": id,
                            "name": name,
                            "input": parse_tool_arguments(c.pointer("/function/arguments")),
                        }));
                    }
                }
                if blocks.is_empty() {
                    continue; // 空 assistant 消息会让上游 400
                }
                out.push(json!({ "role": "assistant", "content": blocks }));
            }
            // user（含多模态数组）原样透传。
            _ => out.push(json!({ "role": m.role, "content": m.content })),
        }
    }
    out
}

/// OpenAI 嵌套工具表 → Anthropic 扁平 `input_schema` 形态
/// （`tools[].{name,description,input_schema}`，不是 OpenAI 的
/// `tools[].function.{…}`——喂错形态上游必 400）。
fn flatten_tools(raw: Option<&Value>) -> Option<Vec<Value>> {
    let arr = raw?.as_array()?;
    let mut out = Vec::with_capacity(arr.len());
    for t in arr {
        // 兼容已解包形态（部分客户端直接给 {name, description, parameters}）。
        let f = t.get("function").unwrap_or(t);
        let Some(name) = f
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let mut o = json!({
            "name": name,
            "input_schema": f
                .get("parameters")
                .filter(|p| p.is_object())
                .cloned()
                .unwrap_or_else(|| json!({ "type": "object", "properties": {} })),
        });
        if let Some(d) = f
            .get("description")
            .and_then(Value::as_str)
            .filter(|d| !d.is_empty())
        {
            o["description"] = json!(d);
        }
        out.push(o);
    }
    (!out.is_empty()).then_some(out)
}

/// 上游 Anthropic message 响应 → ChatCompletion（zcode 本地版：共享的
/// `completion_from_anthropic` 只取文本，这里补齐 tool_use 块 → OpenAI
/// `tool_calls`；纯空响应（无文本也无工具）按空响应报错）。
fn completion_from_zcode(model: &str, v: &Value) -> Result<ChatCompletion, String> {
    let Some(blocks) = v.get("content").and_then(Value::as_array) else {
        if v.get("type").and_then(Value::as_str) != Some("message") {
            return Err(format!("unexpected upstream body: {v}"));
        }
        return Err("empty response (no content blocks)".into());
    };
    let content: String = blocks
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("");
    let tool_calls: Vec<Value> = blocks
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
        .filter_map(|b| {
            let name = b.get("name").and_then(Value::as_str)?;
            Some(json!({
                "id": b.get("id").and_then(Value::as_str).unwrap_or("call_zcode"),
                "type": "function",
                "function": {
                    "name": name,
                    // Anthropic wire 上是对象；OpenAI 面要 JSON 字符串。
                    "arguments": Value::Object(
                        b.get("input").and_then(Value::as_object).cloned().unwrap_or_default(),
                    )
                    .to_string(),
                }
            }))
        })
        .collect();
    if content.is_empty() && tool_calls.is_empty() {
        return Err("empty response (no text/tool blocks)".into());
    }
    let usage = Usage {
        prompt_tokens: v["usage"]["input_tokens"].as_u64().unwrap_or(0),
        completion_tokens: v["usage"]["output_tokens"].as_u64().unwrap_or(0),
        // 上报数字不受信任：饱和加法防溢出 panic
        total_tokens: v["usage"]["input_tokens"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(v["usage"]["output_tokens"].as_u64().unwrap_or(0)),
    };
    let stop = v
        .get("stop_reason")
        .and_then(Value::as_str)
        .unwrap_or("stop");
    let finish = map_stop_reason(stop);
    let id = v
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("msg")
        .to_string();
    Ok(ChatCompletion {
        id: format!("chatcmpl-{id}"),
        object: "chat.completion",
        created: crate::openai::now_ts(),
        model: model.to_string(),
        choices: vec![crate::openai::Choice {
            index: 0,
            message: OutMessage {
                role: "assistant".into(),
                content,
                tool_calls: (!tool_calls.is_empty()).then_some(Value::Array(tool_calls)),
            },
            finish_reason: Some(finish),
        }],
        usage,
    })
}

// ───────────── captcha 本地校验（zcode-captcha.ts:163-200） ─────────────

/// 阿里云 captcha param 的合法判据（三条全中才发）：
/// 1. 长度 ≥ 200（实测合法值 280；SDK 降级垃圾约 76）；
/// 2. 是 base64 且能解出 JSON；
/// 3. 含非空 `certifyId` 且 `securityToken` ≥ 50（实测合法值 128）。
///
/// 任一条不中即判为降级——发了必 3007，不如不发。
fn captcha_param_valid(param: &str) -> bool {
    use base64::Engine as _;
    if param.len() < 200 {
        return false;
    }
    let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(param.trim()) else {
        return false;
    };
    let Ok(parsed) = serde_json::from_slice::<Value>(&decoded) else {
        return false;
    };
    parsed
        .get("certifyId")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty())
        && parsed
            .get("securityToken")
            .and_then(Value::as_str)
            .is_some_and(|t| t.len() >= 50)
}

// ───────────── 订阅双通道（参考 zcode-transport.ts + zcode-pool oauth.rs） ─────────────

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
        return ZcodeCred {
            zcode_jwt: secret.to_string(),
            ..Default::default()
        };
    };
    let s = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
    };
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
            zc.coding_plan_key_zai
                .as_deref()
                .is_some_and(|k| !k.is_empty())
                || zc
                    .coding_plan_key_bigmodel
                    .as_deref()
                    .is_some_and(|k| !k.is_empty())
        }
    }
}

/// 模型 → 通道：同属两通道时 start-plan 优先；凭据不具备时回退 start-plan。
pub fn resolve_channel_for(model: &str, cred: &Credential) -> ZcodeChannel {
    let zc = parse_zcode_cred(&cred.secret);
    let key = model.trim().to_lowercase();
    if START_PLAN_MODELS.contains(&key.as_str()) && channel_available(ZcodeChannel::StartPlan, &zc)
    {
        return ZcodeChannel::StartPlan;
    }
    if CODING_PLAN_MODELS.contains(&key.as_str())
        && channel_available(ZcodeChannel::CodingPlan, &zc)
    {
        return ZcodeChannel::CodingPlan;
    }
    ZcodeChannel::StartPlan
}

impl ZcodeProvider {
    /// 官方「来源标识」请求头（upstream.ts buildZcodeHeaders 的 11 项——
    /// authorization + anthropic-version + UA/HTTP-Referer/X-ZCode-App-Version/
    /// X-Release-Channel/X-Client-Language/X-Client-Timezone/X-Device-Mid/
    /// X-Platform/X-Os-Category）。X-Device-Mid 是硬需求（缺它 billing 全家
    /// 桶回 400 code 3001）；其余是「像官方客户端」的一部分。
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

    fn identity_headers(&self, cred: &Credential) -> reqwest::header::HeaderMap {
        let zc = parse_zcode_cred(&cred.secret);
        self.identity_headers_raw(&zc)
    }

    /// 通道化请求头：start-plan 原样；coding-plan 换 Bearer key 并**删
    /// HTTP-Referer**（transport.ts:126）。
    fn identity_headers_for(
        &self,
        zc: &ZcodeCred,
        channel: ZcodeChannel,
    ) -> reqwest::header::HeaderMap {
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

    /// 换取链路的请求头（官方 createBizAuthHeaders 同款，transport.ts:210-220）。
    fn biz_headers(&self, token: &str) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        let ins = reqwest::header::HeaderValue::from_str;
        let _ = ins(&format!("Bearer {token}")).map(|v| h.insert("authorization", v));
        let _ = ins("application/json").map(|v| h.insert("content-type", v));
        let _ = ins(&format!("ZCode/{}", self.version)).map(|v| h.insert("user-agent", v));
        let _ = ins("win32").map(|v| h.insert("x-platform", v));
        let _ = ins(&self.device_mid).map(|v| h.insert("x-device-mid", v));
        h
    }

    /// 三步现换订阅 key（transport.ts:257-294；**只 GET 不建**——zcode-pool
    /// 会 POST 创建，保守对齐 harness）。失败吞成 None（没订阅是常态，
    /// start-plan 不受影响）。
    pub async fn resolve_coding_plan_key(
        &self,
        cred: &Credential,
    ) -> Result<Option<String>, ProviderError> {
        let zc = parse_zcode_cred(&cred.secret);
        let Some(token) = zc
            .zai_access_token
            .as_deref()
            .or(zc.bigmodel_access_token.as_deref())
        else {
            return Ok(None); // 无 OAuth token 短路，不发请求
        };
        let get_json = |url: String, headers: reqwest::header::HeaderMap| {
            let client = &self.client;
            async move {
                let resp = client.get(&url).headers(headers).send().await.ok()?;
                if !resp.status().is_success() {
                    return None;
                }
                resp.json::<Value>().await.ok()
            }
        };
        // ① 组织与项目（挑默认机构/默认项目，projectType=="2" 剔除）
        let info = get_json(
            format!("{}/api/biz/customer/getCustomerInfo", self.coding_plan_base),
            self.biz_headers(token),
        )
        .await;
        let Some(info) = info else { return Ok(None) };
        let Some((org, proj)) = pick_org_project(&info) else {
            return Ok(None);
        };
        // ② 列 api_keys 找 zcode-api-key
        let keys_url = format!(
            "{}/api/biz/v1/organization/{org}/projects/{proj}/api_keys",
            self.coding_plan_base
        );
        let Some(list) = get_json(keys_url.clone(), self.biz_headers(token)).await else {
            return Ok(None);
        };
        let api_key = list
            .as_array()
            .and_then(|arr| {
                arr.iter()
                    .find(|k| k.get("name").and_then(Value::as_str) == Some(BIZ_API_KEY_NAME))
                    .and_then(|k| k.get("apiKey").and_then(Value::as_str))
                    .map(str::to_string)
            })
            .filter(|k| !k.trim().is_empty());
        let Some(api_key) = api_key else {
            return Ok(None);
        };
        // ③ copy 取 secretKey → "apiKey.secret"
        let copied = get_json(
            format!("{keys_url}/copy/{api_key}"),
            self.biz_headers(token),
        )
        .await;
        let secret = copied
            .as_ref()
            .and_then(|v| v.get("secretKey").and_then(Value::as_str))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        Ok(secret.map(|s| format!("{api_key}.{s}")))
    }
}

/// 挑默认机构/项目：projectType=="2" 剔除；名称含「默认」优先，否则第一个
/// 有合法项目的机构（官方 pickOrgAndProject，zcode-pool oauth.rs:494-540）。
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
        let Some(org_id) = o.get("organizationId").and_then(id_of) else {
            continue;
        };
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
            p.get("projectName")
                .and_then(Value::as_str)
                .is_some_and(|n| n.contains("默认"))
        });
        let chosen = default_proj
            .copied()
            .or_else(|| projects.first().copied())?;
        let Some(proj_id) = chosen.get("projectId").and_then(id_of) else {
            continue;
        };
        if org_is_default {
            return Some((org_id, proj_id));
        }
        fallback.get_or_insert((org_id, proj_id));
    }
    fallback
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unusual_activity_predicate_matches_reference() {
        // ① 权威业务码优先：{"code":3012} 命中，13012 / plan-3012-trust 不命中
        assert!(is_unusual_activity(r#"{"code":3012,"msg":"blocked"}"#));
        assert!(is_unusual_activity(r#"{"code":"3012"}"#));
        assert!(!is_unusual_activity(r#"{"code":13012}"#));
        assert!(!is_unusual_activity(r#"{"plan_id":"plan-3012-trust"}"#));
        // ② 语义短语
        assert!(is_unusual_activity(
            "request has been blocked due to unusual activity."
        ));
        // ③ 词边界 + block 紧邻；retry-after 毫秒数不命中
        assert!(is_unusual_activity("3012 blocked by waf"));
        assert!(!is_unusual_activity(
            "too many requests, retry after 3012 ms"
        ));
        assert!(!is_unusual_activity(
            "rate limited: request blocked, retry after 3012 ms"
        ));
    }

    #[test]
    fn captcha_param_validation() {
        use base64::engine::general_purpose::STANDARD;
        use base64::Engine as _;
        let encode = |payload: &str| STANDARD.encode(payload);
        let valid = encode(
            format!(
                r#"{{"certifyId":"{}","securityToken":"{}"}}"#,
                "c".repeat(80),
                "t".repeat(80)
            )
            .as_str(),
        );
        assert!(
            captcha_param_valid(&valid),
            "合法 param（长度/结构/token 全达标）"
        );
        assert!(
            !captcha_param_valid(&"x".repeat(220)),
            "纯垃圾串不是 base64-JSON"
        );
        assert!(!captcha_param_valid(&valid[..199]), "长度不足 200");
        let short_token = encode(
            format!(
                r#"{{"certifyId":"{}","securityToken":"short"}}"#,
                "c".repeat(150)
            )
            .as_str(),
        );
        assert!(short_token.len() >= 200);
        assert!(!captcha_param_valid(&short_token), "securityToken < 50");
        let no_certify = encode(
            format!(
                r#"{{"note":"{}","securityToken":"{}"}}"#,
                "n".repeat(200),
                "t".repeat(60)
            )
            .as_str(),
        );
        assert!(!captcha_param_valid(&no_certify), "缺 certifyId");
    }

    #[test]
    fn tool_arguments_object_semantics() {
        assert_eq!(parse_tool_arguments(None), json!({}));
        assert_eq!(parse_tool_arguments(Some(&json!("  "))), json!({}));
        assert_eq!(
            parse_tool_arguments(Some(&json!(r#"{"a":1}"#))),
            json!({"a": 1})
        );
        // 残缺 JSON 给哨兵，不补 {}（让上游报 schema 错）
        assert_eq!(
            parse_tool_arguments(Some(&json!("{broken"))),
            json!({ "__zcodeUnparsableArguments": "{broken" })
        );
    }

    #[test]
    fn flattens_openai_tools_to_anthropic_shape() {
        let tools = json!([
            { "type": "function", "function": {
                "name": "get_weather", "description": "查天气",
                "parameters": { "type": "object", "properties": { "city": { "type": "string" } } } } },
            { "type": "function", "function": { "name": "no_schema" } }
        ]);
        let flat = flatten_tools(Some(&tools)).unwrap();
        assert_eq!(flat.len(), 2);
        assert_eq!(flat[0]["name"], "get_weather");
        assert_eq!(flat[0]["description"], "查天气");
        assert!(
            flat[0]["input_schema"].is_object(),
            "扁平 input_schema，非嵌套 function"
        );
        assert_eq!(
            flat[1]["input_schema"]["type"], "object",
            "缺 schema 兜底空对象"
        );
    }

    #[test]
    fn converts_tool_history_to_anthropic_messages() {
        let msgs: Vec<crate::openai::ChatMessage> = serde_json::from_value(json!([
            { "role": "user", "content": "查天气" },
            { "role": "assistant", "content": "", "tool_calls": [
                { "id": "call_1", "function": { "name": "get_weather", "arguments": "{\"city\":\"北京\"}" } }
            ]},
            { "role": "tool", "tool_call_id": "call_1", "content": "晴" },
            { "role": "user", "content": "谢谢" },
            { "role": "system", "content": "被共享层抽走" }
        ]))
        .unwrap();
        let out = to_anthropic_messages(&msgs);
        assert_eq!(out.len(), 4, "system 跳过（共享层抽顶层）");
        assert_eq!(out[0]["role"], "user");
        assert_eq!(out[1]["content"][0]["type"], "tool_use");
        assert_eq!(
            out[1]["content"][0]["input"]["city"], "北京",
            "arguments 字符串 → input 对象"
        );
        assert_eq!(out[2]["role"], "user", "tool 结果包成 user 消息");
        assert_eq!(out[2]["content"][0]["type"], "tool_result");
        assert_eq!(out[2]["content"][0]["tool_use_id"], "call_1");
        assert_eq!(out[3]["role"], "user");
        // 连续两条 tool 结果合并进同一条 user 消息
        let multi: Vec<crate::openai::ChatMessage> = serde_json::from_value(json!([
            { "role": "assistant", "content": "", "tool_calls": [
                { "id": "c1", "function": { "name": "f", "arguments": "{}" } },
                { "id": "c2", "function": { "name": "g", "arguments": "{}" } }
            ]},
            { "role": "tool", "tool_call_id": "c1", "content": "r1" },
            { "role": "tool", "tool_call_id": "c2", "content": "r2" }
        ]))
        .unwrap();
        let out2 = to_anthropic_messages(&multi);
        assert_eq!(out2.len(), 2, "两条 tool 结果合并为一条 user");
        assert_eq!(out2[1]["content"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn completion_from_zcode_maps_tool_blocks() {
        let v = json!({
            "id": "msg_9", "type": "message", "role": "assistant",
            "content": [
                { "type": "text", "text": "调用工具" },
                { "type": "tool_use", "id": "tu_1", "name": "get_weather", "input": { "city": "北京" } }
            ],
            "stop_reason": "tool_use",
            "usage": { "input_tokens": 10, "output_tokens": 5 }
        });
        let c = completion_from_zcode("zcode/glm-5.3", &v).unwrap();
        assert_eq!(c.choices[0].message.content, "调用工具");
        let calls = c.choices[0].message.tool_calls.clone().unwrap();
        assert_eq!(calls[0]["function"]["name"], "get_weather");
        assert_eq!(calls[0]["function"]["arguments"], r#"{"city":"北京"}"#);
        assert_eq!(c.choices[0].finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(c.usage.total_tokens, 15);
        // 完全空响应 → 错误（不是无声空回复）
        let empty = json!({ "id": "m", "type": "message", "content": [], "usage": {} });
        assert!(completion_from_zcode("zcode/glm-5.3", &empty).is_err());
    }
}
