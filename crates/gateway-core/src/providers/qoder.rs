//! qoder Provider（阿里系，缝 2）。
//!
//! 已覆盖：加密端点**响应信封解包**（T4.5a，qoder-envelope.ts）、credits/
//! 领取/推理错误分型（T4.5b，qoder-credits.ts + qoder-adapter.ts）、
//! 设备码登录/续期/userinfo 昵称补查（T4.20a，qoder.ts + qoder-oauth.ts）。
//!
//! 待续切片：WASM 请求加密（wasmtime 接线，qoder-auth-wasm.wasm）。
//! 发送循环的错误分型入口 [`map_infer_error_code`] 已就绪供其接线。

use serde_json::Value;

/// 剥信封后的一帧。
#[derive(Debug, Clone, PartialEq)]
pub enum EnvelopeFrame {
    /// 内层含 `choices`/`usage` 结构 → OpenAI chunk
    Chunk(Value),
    /// 错误帧：`code` 保持独立字段保真（拼进 message 会让排队识别永不命中，
    /// qoder-envelope.ts:182-199）；`message` 缺字符串字段时回退内层原文
    Error { code: Option<Value>, message: Option<String> },
    /// `null`/`{}`/空/裸标量 → 心跳，整帧跳过（误判成错误会白重试，issue IKJOZ8）
    Heartbeat,
}

/// 剥一帧 data 载荷。
///
/// 返回 `None` = 非信封载荷（非 JSON，或内层含 `[DONE]` 终止标记）——
/// 调用方应**原样透传**该行（与 qoder-envelope.ts:157-161 的 innerTextOf
/// null 透传、`[DONE]` 直通同语义），绝不能当损坏帧丢弃（丢了流不收尾）。
///
/// 分类**只看内层 JSON 结构**（qoder-envelope.ts:72-102 的 classifyInner），
/// 信封层 `statusCodeValue` 不参与——失败时错误信息就在内层 body 里：
/// - 内层解析失败 → **业务错误**（qoder-envelope.ts:80-83：纯文本错误体
///   如 `[FAIL]node:… msg:…`，判成心跳会静默吞掉真实失败）；
/// - `choices`/`usage` → chunk；错误键 `code`/`message`/`error`/
///   `statusCodeValue`/`type` **任一单独出现**即错误（qoder-envelope.ts:92-99）；
/// - `null`/`{}`/空/裸标量/数组 → 心跳整帧跳过。
pub fn unwrap_envelope(data: &str) -> Option<EnvelopeFrame> {
    let v: Value = serde_json::from_str(data.trim()).ok()?;
    if !v.is_object() {
        return Some(EnvelopeFrame::Heartbeat);
    }
    // 内层文本：body 为字符串时原样；非字符串 body 字符串化（innerTextOf 同义）；
    // 缺 body → 整帧按标准帧处理（innerTextOf 对缺 body 返回 null 的透传语义）。
    let inner_text: String = match v.get("body") {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(other) => other.to_string(),
        None => v.to_string(),
    };
    if inner_text.is_empty() {
        return Some(EnvelopeFrame::Heartbeat);
    }
    // [DONE] 子串语义保留（qoder-envelope.ts:75-76）：终止标记不是 JSON、也
    // 不能落进下面「解析失败=业务错误」的分支——返回 None 交调用方透传。
    if inner_text.contains("[DONE]") {
        return None;
    }
    match serde_json::from_str::<Value>(&inner_text) {
        Err(_) => Some(EnvelopeFrame::Error { code: None, message: Some(inner_text) }),
        Ok(inner) => Some(classify_inner_value(inner, &inner_text)),
    }
}

/// 按内层 JSON 结构分类（qoder-envelope.ts:72-102）。
fn classify_inner_value(inner: Value, fallback_message: &str) -> EnvelopeFrame {
    let Value::Object(m) = inner else {
        return EnvelopeFrame::Heartbeat; // null/裸标量/数组（:86-88）
    };
    if m.is_empty() {
        return EnvelopeFrame::Heartbeat;
    }
    // chunk 键优先（:91：`choices:[]` 也算——include_usage 的末帧形态）
    if m.get("choices").is_some_and(Value::is_array) || m.contains_key("usage") {
        return EnvelopeFrame::Chunk(Value::Object(m));
    }
    // 错误键任一单独出现即错误帧（:92-99——message/statusCodeValue 不要求
    // code 同时在场）
    if ["code", "message", "error", "statusCodeValue", "type"]
        .iter()
        .any(|k| m.contains_key(*k))
    {
        return EnvelopeFrame::Error {
            code: m.get("code").cloned(),
            message: Some(
                m.get("message")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| fallback_message.to_string()),
            ),
        };
    }
    // 其余未知结构按心跳跳过（不臆造错误，:101）
    EnvelopeFrame::Heartbeat
}

// ───────────────────── credits/领取/错误码（T4.5b） ─────────────────────

use crate::provider::{Credential, ProviderError};

pub const DEFAULT_OPENAPI_BASE: &str = "https://openapi.qoder.sh";
const USAGE_PATH: &str = "/sash/api/v2/me/usage";
const CAMPAIGNS_PATH: &str = "/sash/api/v1/me/campaigns";
/// Cosy-ClientType 实测值 '10'（官方桌面 app 身份，qoder-product.ts:454-455；
/// 用 clientMetadata 的 '5' 时 campaigns 恒返回空）。
const COSY_CLIENT_TYPE: &str = "10";
/// MachineType 缺失时的回退占位值。
/// ⚠️ 未经验证：真实值来自 IDE `machine_token.json` 的 `{token,type}`
/// （qoder-machine.ts:47-53），占位对拿不到可领活动时应保守降级。
const COSY_MACHINE_TYPE: &str = "pc";

#[derive(Debug, Clone, PartialEq)]
pub struct QoderBalance {
    /// userQuota + addOnQuota + 专用资源包的剩余累加（只读 userQuota 会显示 0，
    /// qoder-credits.ts:27-29 实测该账号 userQuota.remaining=0 而 addOn=100）
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClaimOutcome {
    Claimed,
    /// 幂等重放（响应体 replayed:true，HTTP 仍 200；只看状态码会把
    /// 「今天已领」误报成「领取成功」，qoder-credits.ts:53-56）
    Replayed,
}

/// 推理错误分型后的处置动作——qoder-adapter.ts:745-751 决策表的 Rust 化，
/// 供 WASM 加密接线完成后的发送循环消费。
#[derive(Debug, Clone, PartialEq)]
pub enum InferErrorAction {
    /// 排队（业务码 10605，model-queue.ts:36）。**两形态都归这里**
    /// （qoder-adapter.ts:783-789：HTTP 403 + JSON body，或 HTTP 200 +
    /// SSE 内嵌 `{code:"10605"}` 帧）：按服务端延迟等待后重发整条请求；
    /// 排队与凭据/额度无关，不刷新不换号。
    Queued { retry_after_ms: Option<u64> },
    /// 重复请求（HTTP 409 duplicate_request，qoder-adapter.ts:834-838）：
    /// 凭据没问题，不刷新，原样重发一次。
    DuplicateResend,
    /// 认证失败（业务码 105 auth_error，或 HTTP 401/403 非排队非额度，
    /// qoder-adapter.ts:820-830）：续期凭据后重试——**每请求至多一次**
    /// （adapter 的 authRefreshed 闭锁语义，避免刷爆 userinfo；调用方
    /// 自行持锁保证）。
    RefreshAuth,
    /// 额度耗尽（业务码 110 Billing daily count exceeded，model-queue.ts:73）：
    /// 当天重试无用，不可重试——应切号或按 QUOTA_EXCEEDED 上抛
    /// （qoder-adapter.ts:875-889）。
    QuotaExceeded,
}

/// 推理错误分型。`code` 为信封错误帧/错误体里的业务码（字符串化后传入），
/// `http_status` 为该次响应的 HTTP 状态，`retry_after_ms` 为从排队信息
/// （message 内层 JSON 的 retry_after_ms/retryAfterMs/retryAfterSeconds×1000）
/// 解出的服务端延迟。返回 None = 无专属处置（走通用错误路径，不臆造）。
pub fn map_infer_error_code(
    http_status: u16,
    code: Option<&str>,
    retry_after_ms: Option<u64>,
) -> Option<InferErrorAction> {
    match code {
        // 排队优先于 401/403 判定（adapter 先 parseQueueError 再谈续期）
        Some("10605") => Some(InferErrorAction::Queued {
            retry_after_ms: retry_after_ms.map(|ms| ms.min(1_800_000)), // 上限 30 分钟
        }),
        Some("110") => Some(InferErrorAction::QuotaExceeded),
        _ if http_status == 409 => Some(InferErrorAction::DuplicateResend),
        _ if http_status == 401 || http_status == 403 || code == Some("105") => {
            Some(InferErrorAction::RefreshAuth)
        }
        _ => None,
    }
}

/// 信封错误帧 code → ProviderError。
/// 10605 排队（按服务端 retryAfterMs 等待，上限 30 分钟）；110 额度
/// （Billing daily count exceeded）不可重试；未知码返回 None 不臆造。
pub fn map_error_code(code: &str, retry_after_ms: Option<u64>) -> Option<ProviderError> {
    match map_infer_error_code(200, Some(code), retry_after_ms) {
        Some(InferErrorAction::Queued { retry_after_ms: ms }) => Some(ProviderError::RateLimited {
            retry_after_secs: ms.map(|v| (v / 1000).min(1800)),
            msg: "model queued (10605)".into(),
        }),
        Some(InferErrorAction::QuotaExceeded) => Some(ProviderError::BadRequest(
            "billing daily count exceeded (110): quota exhausted".into(),
        )),
        _ => None,
    }
}

pub struct QoderProvider {
    openapi_base: String,
    client: reqwest::Client,
}

impl QoderProvider {
    pub fn new(openapi_base: String) -> Self {
        Self { openapi_base, client: reqwest::Client::new() }
    }

    pub fn production() -> Self {
        Self::new(DEFAULT_OPENAPI_BASE.into())
    }

    fn parse_secret(cred: &Credential) -> Result<Value, ProviderError> {
        serde_json::from_str::<Value>(&cred.secret)
            .map_err(|e| ProviderError::Credential(format!("qoder 凭据非 JSON：{e}")))
    }

    fn sstr(v: &Value, key: &str) -> String {
        v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
    }

    /// /sash/ 端点公共头（qoder-credits.ts:229-240）：Accept + Bearer +
    /// Cosy-ClientType（'10'）+ UA。machine 头对用量端点不敏感，不在此层加。
    fn usage_headers(cred: &Credential) -> Result<reqwest::header::HeaderMap, ProviderError> {
        let v = Self::parse_secret(cred)?;
        let mut h = reqwest::header::HeaderMap::new();
        let ins = reqwest::header::HeaderValue::from_str;
        let _ = ins("application/json").map(|x| h.insert("accept", x));
        let _ = ins(&format!("Bearer {}", Self::sstr(&v, "access_token")))
            .map(|x| h.insert("authorization", x));
        let _ = ins(COSY_CLIENT_TYPE).map(|x| h.insert("cosy-client-type", x));
        let _ = ins("Qoder").map(|x| h.insert("user-agent", x));
        Ok(h)
    }

    /// 活动/领取请求头：必需**成对** machine 头（消融实验 qoder-credits.ts:202-218
    /// ——只带 ClientType:'10' 时仅回 1 条 VIEW_DETAILS，看不到可领活动）。
    ///
    /// 值来源（qoder-machine.ts:47-53）：IDE `machine_token.json` 的
    /// `{token,type}` → `Cosy-MachineToken`=token、`Cosy-MachineType`=type。
    /// 本网关从凭据**可选** `machine_token`/`machine_type` 字段读（登录侧应
    /// 把该文件值灌入凭据）；缺失时回退 `machine_id`/占位值——
    /// ⚠️ **回退值未经 wire 验证**，真实身份对拿不到时服务端可能不下发
    /// 可领活动（保守降级，不发空头）。
    fn campaign_headers(cred: &Credential) -> Result<reqwest::header::HeaderMap, ProviderError> {
        let v = Self::parse_secret(cred)?;
        let mut h = Self::usage_headers(cred)?;
        let ins = reqwest::header::HeaderValue::from_str;
        let token = {
            let t = Self::sstr(&v, "machine_token");
            if t.is_empty() { Self::sstr(&v, "machine_id") } else { t }
        };
        let mtype = {
            let t = Self::sstr(&v, "machine_type");
            if t.is_empty() { COSY_MACHINE_TYPE.to_string() } else { t }
        };
        // 必须成对出现（缺一即失效，qoder-machine.ts:33-38 消融表）——
        // 连回退 token 都拿不到时**整对不发**（保守降级，不发半对/空头）
        if !token.is_empty() {
            let _ = ins(&token).map(|x| h.insert("cosy-machinetoken", x));
            let _ = ins(&mtype).map(|x| h.insert("cosy-machinetype", x));
        }
        Ok(h)
    }

    async fn get_json(
        &self,
        path: &str,
        headers: reqwest::header::HeaderMap,
    ) -> Result<Value, ProviderError> {
        let resp = self
            .client
            .get(format!("{}{}", self.openapi_base, path))
            .headers(headers)
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if status != 200 {
            return Err(ProviderError::Upstream(format!("http {status}: {}", String::from_utf8_lossy(&bytes))));
        }
        serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Upstream(format!("响应非 JSON: {e}")))
    }

    /// 读 quota 数字（容忍字符串编码，qoder-credits.ts:243-252 readNumber）。
    fn read_num(v: &Value, key: &str) -> Option<f64> {
        match v.get(key) {
            Some(Value::Number(n)) => n.as_f64(),
            Some(Value::String(s)) => s.trim().parse::<f64>().ok(),
            _ => None,
        }
    }

    /// 单包剩余：`remaining` 优先，缺失按 `max(0, total - used)`
    /// （qoder-credits.ts:283-291）；负值 clamp 到 0；三字段全缺 → 不是包。
    fn package_remaining(quota: &Value) -> Option<u64> {
        let total = Self::read_num(quota, "total");
        let used = Self::read_num(quota, "used");
        let remaining = Self::read_num(quota, "remaining");
        if total.is_none() && used.is_none() && remaining.is_none() {
            return None;
        }
        let raw = match remaining {
            Some(r) => r.max(0.0),
            None => (total.unwrap_or(0.0) - used.unwrap_or(0.0)).max(0.0),
        };
        Some(raw.round() as u64)
    }

    /// 余额 = userQuota + addOnQuota（对象形态）+ dedicatedResourcePackages
    /// 多包剩余累加。响应形状按 2026-09-19 实测（qoder-credits.ts:18-25）：
    /// `{displayMode, qoderUsage:{userQuota:{total,used,remaining},…}}`。
    ///
    /// - 企业版（displayMode=enterprise）无额度数字只有外链，报错不报 0
    ///   （qoder-credits.ts:353-355）；
    /// - 一个包都解析不出 → 视为「查不到」而非「余额 0」（:378-380）。
    pub async fn balance(&self, cred: &Credential) -> Result<QoderBalance, ProviderError> {
        let v = self.get_json(USAGE_PATH, Self::usage_headers(cred)?).await?;
        if v.get("displayMode").and_then(Value::as_str) == Some("enterprise") {
            return Err(ProviderError::Upstream(
                "qoder enterprise 账号不提供额度数字（仅外链）".into(),
            ));
        }
        let usage = v.get("qoderUsage").ok_or_else(|| {
            ProviderError::Upstream("qoder usage 响应缺 qoderUsage（形状与预期不符）".into())
        })?;
        let mut total = 0u64;
        let mut packages = 0usize;
        for key in ["userQuota", "addOnQuota"] {
            if let Some(r) = usage.get(key).and_then(Self::package_remaining) {
                total += r;
                packages += 1;
            }
        }
        if let Some(arr) = usage.get("dedicatedResourcePackages").and_then(Value::as_array) {
            for item in arr {
                if let Some(r) = Self::package_remaining(item) {
                    total += r;
                    packages += 1;
                }
            }
        }
        if packages == 0 {
            return Err(ProviderError::Upstream(
                "qoder usage 无任何可解析额度包（查不到 ≠ 余额 0）".into(),
            ));
        }
        Ok(QoderBalance { total })
    }

    /// 可领活动列表：只保留 `CLAIM_BENEFIT` + `CLAIMABLE`（actionType/
    /// claimStatus 字段名，qoder-credits.ts:395-447）。返回原始条目
    /// （含 campaignId/benefit 等）。
    pub async fn campaigns(&self, cred: &Credential) -> Result<Vec<Value>, ProviderError> {
        let v = self.get_json(CAMPAIGNS_PATH, Self::campaign_headers(cred)?).await?;
        let list = v
            .get("campaigns")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(list
            .into_iter()
            .filter(|c| {
                c.get("actionType").and_then(Value::as_str) == Some("CLAIM_BENEFIT")
                    && c.get("claimStatus").and_then(Value::as_str) == Some("CLAIMABLE")
            })
            .collect())
    }

    /// 领取：body 空（抓包 content-length:0，qoder-credits.ts:58-59）；
    /// 幂等判据是响应体 `replayed:true`（HTTP 仍 200）；`status` 在场但非
    /// CLAIMED → 领取未成功（:637-639）。
    pub async fn claim(&self, cred: &Credential, campaign_id: &str) -> Result<ClaimOutcome, ProviderError> {
        let resp = self
            .client
            .post(format!("{}{CAMPAIGNS_PATH}/{campaign_id}/claim", self.openapi_base))
            .headers(Self::campaign_headers(cred)?)
            .header("content-type", "application/json")
            .body(String::new())
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if status != 200 {
            return Err(ProviderError::Upstream(format!("claim http {status}")));
        }
        let v: Value = serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Upstream(format!("claim 响应非 JSON: {e}")))?;
        if v.get("replayed").and_then(Value::as_bool) == Some(true) {
            return Ok(ClaimOutcome::Replayed);
        }
        if let Some(st) = v.get("status").and_then(Value::as_str) {
            if st != "CLAIMED" {
                return Err(ProviderError::Upstream(format!("claim 未成功（status={st}）")));
            }
        }
        Ok(ClaimOutcome::Claimed)
    }
}


// ───────────── T4.20a：设备码登录 / 续期（qoder.ts/qoder-oauth.ts） ─────────────

/// qoder 国际版公开 client_id（qoder-product.ts:442-446；prod 必须用 prod 的
/// client_id，用错回调报"参数无效"）。
pub const QODER_CLIENT_ID: &str = "e883ade2-e6e3-4d6d-adf7-f92ceff5fdcb";
/// 轮询节奏（qoder.ts:16-20）：总超时 300s、网络失败容忍 5 次。
const POLL_DEADLINE: std::time::Duration = std::time::Duration::from_secs(300);
const POLL_NET_FAIL_MAX: u32 = 5;
/// 用户信息路径（挂 openApiBase，qoder.ts:29）。
const USERINFO_PATH: &str = "/api/v1/userinfo";

/// 本地生成的设备码会话（**没有"从 API 拿 code"这一步**——nonce/PKCE/
/// machine_id 全本地生成；machine_id 随凭据持久化，非硬件指纹）。
pub struct QoderDeviceSession {
    pub verifier: String,
    pub challenge: String,
    pub nonce: String,
    pub machine_id: String,
}

fn random_uuidish() -> String {
    let hex = crate::key::random_id(16);
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

fn base64url_nopad(data: &[u8]) -> String {
    const TBL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(TBL[(n >> 18) as usize & 63] as char);
        out.push(TBL[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(TBL[(n >> 6) as usize & 63] as char);
        }
        if chunk.len() > 2 {
            out.push(TBL[n as usize & 63] as char);
        }
    }
    out
}

/// 授权 URL（qoder.ts:109-118）。
pub fn build_qoder_auth_url(sess: &QoderDeviceSession) -> String {
    format!(
        "https://qoder.com/device/selectAccounts?challenge={}&challenge_method=S256&nonce={}&machine_id={}&client_id={}",
        sess.challenge, sess.nonce, sess.machine_id, QODER_CLIENT_ID
    )
}

/// 轮询 URL（GET + query，无请求体，qoder.ts:127-134）。
pub fn build_qoder_poll_url(sess: &QoderDeviceSession) -> String {
    format!(
        "/api/v1/deviceToken/poll?nonce={}&verifier={}&challenge_method=S256",
        sess.nonce, sess.verifier
    )
}

/// OAuth 客户端：轮询 + 续期。
pub struct QoderOAuth {
    openapi_base: String,
    client: reqwest::Client,
}

impl QoderOAuth {
    pub fn new_for_test(openapi_base: String) -> Self {
        Self { openapi_base, client: reqwest::Client::new() }
    }

    /// PKCE verifier：43..128 长度、unreserved 字符集（qoder.ts:39,56-69）；
    /// challenge = base64url(sha256(verifier)) **无 padding**。
    pub fn create_device_session(&self) -> QoderDeviceSession {
        const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
        let mut rng = rand::rng();
        let len = 43 + rand::Rng::random_range(&mut rng, 0..86);
        let verifier: String = (0..len)
            .map(|_| CHARSET[rand::Rng::random_range(&mut rng, 0..CHARSET.len())] as char)
            .collect();
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(verifier.as_bytes());
        let challenge = base64url_nopad(&h.finalize());
        QoderDeviceSession {
            verifier,
            challenge,
            nonce: random_uuidish(),
            machine_id: random_uuidish(),
        }
    }

    /// 轮询到 token：404/2xx 无 token 继续；其他非 2xx 抛错。
    pub async fn poll_until_token(
        &self,
        sess: &QoderDeviceSession,
        interval: std::time::Duration,
    ) -> Result<Credential, ProviderError> {
        let deadline = std::time::Instant::now() + POLL_DEADLINE;
        let mut net_fails = 0u32;
        loop {
            match self.poll_once(sess).await {
                Ok(Some(v)) => return Ok(self.complete_credential(&v, sess).await),
                Ok(None) => {}
                Err(e) => {
                    net_fails += 1;
                    if net_fails > POLL_NET_FAIL_MAX {
                        return Err(e);
                    }
                }
            }
            if std::time::Instant::now() > deadline {
                return Err(ProviderError::Upstream("qoder poll timeout (300s)".into()));
            }
            tokio::time::sleep(interval).await;
        }
    }

    async fn poll_once(&self, sess: &QoderDeviceSession) -> Result<Option<Value>, ProviderError> {
        let resp = self
            .client
            .get(format!("{}{}", self.openapi_base, build_qoder_poll_url(sess)))
            .header("accept", "application/json")
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if status == 404 {
            return Ok(None); // 用户尚未授权，继续轮询（body errorCode:NotFound）
        }
        if (200..300).contains(&status) {
            let v: Value = serde_json::from_slice(&bytes).ok().unwrap_or(Value::Null);
            let has_token = ["token", "device_token", "access_token"]
                .iter()
                .any(|k| v.get(*k).and_then(Value::as_str).is_some_and(|s| !s.is_empty()));
            return Ok(has_token.then_some(v));
        }
        Err(ProviderError::Upstream(format!(
            "qoder poll http {status}: {}",
            String::from_utf8_lossy(&bytes)
        )))
    }

    fn credential_from(&self, v: &Value, sess: &QoderDeviceSession) -> Credential {
        let g = |names: [&str; 3]| -> Option<String> {
            for n in names {
                if let Some(s) = v.get(n).and_then(Value::as_str).filter(|s| !s.is_empty()) {
                    return Some(s.to_string());
                }
            }
            None
        };
        let token = g(["token", "device_token", "access_token"]).unwrap_or_default();
        let refresh = g(["refresh_token", "refreshToken", "refresh_token2"]).unwrap_or_default();
        let uid = g(["user_id", "userId", "uid"]).unwrap_or_default();
        let nickname = g(["user_name", "name", "username"]).unwrap_or_default();
        let expires_at = v.get("expires_at").and_then(Value::as_u64).unwrap_or(0);
        let secret = serde_json::json!({
            "access_token": token,
            "security_oauth_token": token, // 双写同值（客户端取用前者优先）
            "refresh_token": refresh,
            "uid": uid,
            "nickname": nickname,
            "machine_id": sess.machine_id,
            "expires_at": expires_at,
        })
        .to_string();
        Credential { account_id: String::new(), secret }
    }

    /// 轮询成功后的凭据组装：先按响应构造，再补查 userinfo 昵称
    /// （qoder.ts:344-363：设备码响应通常**没有** user_name——实测 4 账号
    /// 全缺，userinfo 的 `name` 是唯一可靠来源，登录成功后必须补一次）。
    /// userinfo 失败不致命：退回响应里的 user_name（可能为空）。
    async fn complete_credential(&self, v: &Value, sess: &QoderDeviceSession) -> Credential {
        let cred = self.credential_from(v, sess);
        let bearer = serde_json::from_str::<Value>(&cred.secret)
            .ok()
            .and_then(|s| {
                ["security_oauth_token", "access_token"].iter().find_map(|k| {
                    s.get(*k)
                        .and_then(Value::as_str)
                        .filter(|t| !t.is_empty())
                        .map(str::to_string)
                })
            })
            .unwrap_or_default();
        match self.fetch_user_nickname(&bearer).await {
            Some(name) => with_nickname(cred, &name),
            None => cred,
        }
    }

    /// GET /api/v1/userinfo 取展示名（qoder.ts:371-394）：Bearer 头（security_
    /// oauth_token 优先），读 `name` 字段 trim 后非空才算。任何失败返回
    /// None——昵称只是展示信息，不能让登录整体失败。
    pub async fn fetch_user_nickname(&self, bearer: &str) -> Option<String> {
        if bearer.is_empty() {
            return None;
        }
        let resp = self
            .client
            .get(format!("{}{USERINFO_PATH}", self.openapi_base))
            .header("accept", "application/json")
            .header("authorization", format!("Bearer {bearer}"))
            .send()
            .await
            .ok()?;
        if resp.status().as_u16() != 200 {
            return None;
        }
        let v: Value = resp.json().await.ok()?;
        let name = v.get("name")?.as_str()?.trim();
        (!name.is_empty()).then(|| name.to_string())
    }

    /// 续期（qoder-auth.ts:395-438）：body {refresh_token, machine_id}；
    /// 终态=401/403 或 200 无 token；回写保留 machine_id/uid/nickname。
    pub async fn refresh(&self, cred: &Credential) -> Result<Credential, ProviderError> {
        let Ok(old) = serde_json::from_str::<Value>(&cred.secret) else {
            return Err(ProviderError::Credential("qoder 凭据非 JSON".into()));
        };
        let rt = old.get("refresh_token").and_then(Value::as_str).unwrap_or_default().to_string();
        let mid = old.get("machine_id").and_then(Value::as_str).unwrap_or_default().to_string();
        if rt.is_empty() {
            return Err(ProviderError::Credential("qoder 凭据无 refresh_token".into()));
        }
        let resp = self
            .client
            .post(format!("{}/api/v1/deviceToken/refresh", self.openapi_base))
            .header("content-type", "application/json")
            .header("accept", "application/json")
            .header("user-agent", "qoder/1.0.0")
            .json(&serde_json::json!({ "refresh_token": rt, "machine_id": mid }))
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if status == 401 || status == 403 {
            return Err(ProviderError::Credential(format!("qoder refresh http {status}")));
        }
        if status != 200 {
            return Err(ProviderError::Upstream(format!("qoder refresh http {status}")));
        }
        let v: Value = serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Upstream(format!("qoder refresh 非 JSON: {e}")))?;
        let token = ["device_token", "token", "access_token"]
            .iter()
            .find_map(|k| v.get(*k).and_then(Value::as_str).filter(|s| !s.is_empty()))
            .ok_or_else(|| ProviderError::Credential("qoder refresh 200 但无 token（终态）".into()))?
            .to_string();
        let new_rt = v
            .get("refresh_token")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or(rt);
        let mut out = old.clone();
        let obj = out.as_object_mut().unwrap();
        obj.insert("access_token".into(), Value::String(token.clone()));
        obj.insert("security_oauth_token".into(), Value::String(token));
        obj.insert("refresh_token".into(), Value::String(new_rt));
        if let Some(exp) = v.get("expires_at").and_then(Value::as_u64) {
            obj.insert("expires_at".into(), Value::from(exp));
        }
        Ok(Credential { secret: out.to_string(), ..cred.clone() })
    }
}

/// 把昵称写进凭据 secret（qoder.ts:405-411 withQoderNickname：空值不写入）。
fn with_nickname(cred: Credential, nickname: &str) -> Credential {
    if nickname.is_empty() {
        return cred;
    }
    let Ok(mut v) = serde_json::from_str::<Value>(&cred.secret) else {
        return cred;
    };
    if let Some(obj) = v.as_object_mut() {
        obj.insert("nickname".into(), Value::String(nickname.to_string()));
    }
    Credential { secret: v.to_string(), ..cred }
}

// ───────────── T4.20b：WASM 加密推理接线（feature = "wasm"） ─────────────

/// 加密推理载荷（buildQoderInferPayload，qoder-wasm.ts:245-326）。
/// ⚠️ `business:{type:"agent"}` 必填——缺失会被路由到故障节点
/// （qoder-adapter.ts:728-739 实测）；`tools` 顶层必发（空也要发）。
#[cfg(feature = "wasm")]
pub fn build_infer_payload(
    route: &crate::route::Route,
    req: &crate::openai::ChatRequest,
) -> Value {
    let request_id = format!("req-{}", crate::key::random_id(12));
    let session_id = {
        let hex = crate::key::random_id(16);
        format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
    };
    let user_text = req
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.text())
        .unwrap_or_default();
    let system_text: Vec<String> = req
        .messages
        .iter()
        .filter(|m| m.role == "system")
        .map(|m| m.text())
        .filter(|t| !t.is_empty())
        .collect();
    let mut parameters = serde_json::Map::new();
    if let Some(mt) = req.raw.get("max_tokens") {
        parameters.insert("max_tokens".into(), mt.clone());
    }
    if let Some(e) = req.raw.get("reasoning_effort") {
        parameters.insert("reasoning_effort".into(), e.clone());
        parameters.insert("enable_thinking".into(), Value::Bool(e.as_str() != Some("none")));
    }
    let messages: Vec<Value> = req
        .messages
        .iter()
        .filter(|m| m.role != "system")
        .map(|m| {
            let mut o = serde_json::Map::new();
            o.insert("role".into(), Value::String(m.role.clone()));
            o.insert("content".into(), m.content.clone());
            if let Some(tc) = &m.tool_calls {
                o.insert("tool_calls".into(), tc.clone());
            }
            if let Some(id) = &m.tool_call_id {
                o.insert("tool_call_id".into(), Value::String(id.clone()));
            }
            Value::Object(o)
        })
        .collect();
    serde_json::json!({
        "request_id": request_id, "request_set_id": request_id, "chat_record_id": request_id,
        "session_id": session_id, "stream": true, "chat_task": "FREE_INPUT",
        "chat_context": {
            "text": user_text, "features": [],
            "extra": { "context": [], "modelConfig": { "key": route.model, "is_reasoning": false }, "originalContent": user_text },
            "chatPrompt": "", "imageUrls": null,
        },
        "is_reply": true, "is_retry": false, "source": 1, "version": "3",
        "agent_id": "agent_common", "task_id": "common",
        "session_type": "qodercli", "aliyun_user_type": "",
        "model_config": {
            "key": route.model, "display_name": "", "model": "", "format": "openai",
            "is_vl": true, "is_reasoning": false, "api_key": "", "url": "",
            "source": "system", "max_input_tokens": 200_000,
        },
        "custom_model": null,
        "system": if system_text.is_empty() { vec![Value::Null] } else {
            system_text.iter().map(|t| serde_json::json!({ "type": "text", "text": t })).collect()
        },
        "messages": if messages.is_empty() {
            vec![serde_json::json!({ "role": "user", "content": user_text })]
        } else { messages },
        "tools": Value::Array(req.raw.get("tools").and_then(Value::as_array).cloned().unwrap_or_default()),
        "parameters": Value::Object(parameters),
        "business": { "type": "agent" },
    })
}


#[cfg(feature = "wasm")]
mod wasm_infer {
    use super::*;
    use crate::openai::{ChatCompletion, Usage};
    use crate::provider::{ChunkStream, Provider, ProviderError, StreamChunk};
    use crate::registry::{ModelInfo, ProviderCatalog};
    use crate::openai::ChatRequest;
    use crate::route::Route;
    use crate::sse::SseParser;
    use async_trait::async_trait;

    const COSY_VERSION: &str = "1.1.49";

    struct Agg {
        text: String,
        reasoning: String,
        tool_calls: Vec<(Option<String>, Option<String>, String)>,
        finish: Option<(String, Usage)>,
    }

    fn consume_chunk(v: &Value, agg: &mut Agg) {
        let choice = v.pointer("/choices/0").cloned().unwrap_or(Value::Null);
        if let Some(d) = choice.get("delta") {
            if let Some(t) = d.get("content").and_then(Value::as_str) {
                agg.text.push_str(t);
            }
            if let Some(t) = d.get("reasoning_content").and_then(Value::as_str) {
                agg.reasoning.push_str(t);
            }
            if let Some(tcs) = d.get("tool_calls").and_then(Value::as_array) {
                for tc in tcs {
                    let id = tc.get("id").and_then(Value::as_str).map(str::to_string);
                    let name = tc.pointer("/function/name").and_then(Value::as_str).map(str::to_string);
                    let args = tc.pointer("/function/arguments").and_then(Value::as_str).unwrap_or_default().to_string();
                    agg.tool_calls.push((id, name, args));
                }
            }
        }
        if let Some(fr) = choice.get("finish_reason").and_then(Value::as_str) {
            if !fr.is_empty() {
                let usage = agg.finish.as_ref().map(|(_, u)| *u).unwrap_or_default();
                agg.finish = Some((fr.to_string(), usage));
            }
        }
        if let Some(u) = v.get("usage") {
            let usage = Usage::sum(
                u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0),
                u.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0),
            );
            let reason = agg.finish.take().map(|(r, _)| r).unwrap_or_else(|| "stop".into());
            agg.finish = Some((reason, usage));
        }
    }

    fn aggregate_sse(body: &str) -> Result<Agg, ProviderError> {
        let mut agg = Agg { text: String::new(), reasoning: String::new(), tool_calls: Vec::new(), finish: None };
        let mut parser = SseParser::new();
        parser.feed(body.as_bytes());
        parser.finalize();
        while let Some(data) = parser.next_data() {
            if data.trim() == "[DONE]" {
                continue;
            }
            match unwrap_envelope(&data) {
                Some(EnvelopeFrame::Chunk(v)) => consume_chunk(&v, &mut agg),
                Some(EnvelopeFrame::Error { code, message }) => {
                    let code_s = code.as_ref().map(|c| c.to_string()).unwrap_or_default();
                    if let Some(e) = map_error_code(&code_s, None) {
                        return Err(e);
                    }
                    return Err(ProviderError::Upstream(format!("qoder error frame: {code_s} {message:?}")));
                }
                _ => {}
            }
        }
        Ok(agg)
    }

    /// 同步桥：wasm 调用是同步语义，放独立线程跑（含内部 current_thread
    /// runtime 发 HTTP）；headers 原样透传（覆盖 Authorization → 403）。
    fn send_encrypted(cred: &Credential, route: &Route, req: &ChatRequest) -> Result<(u16, String), ProviderError> {
        let secret = cred.secret.clone();
        let route_m = route.model.clone();
        let payload = build_infer_payload(route, req).to_string();
        std::thread::spawn(move || -> Result<(u16, String), ProviderError> {
            let mut wasm = crate::providers::qoder_wasm::QoderWasm::new()
                .map_err(|e| ProviderError::Upstream(e.to_string()))?;
            let v: Value = serde_json::from_str(&secret)
                .map_err(|e| ProviderError::Credential(format!("qoder 凭据非 JSON：{e}")))?;
            let uid = v.get("uid").and_then(Value::as_str).unwrap_or_default().to_string();
            let token = v
                .get("security_oauth_token")
                .or_else(|| v.get("access_token"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if uid.is_empty() || token.is_empty() {
                return Err(ProviderError::Credential(
                    "qoder 凭据缺 uid/security_oauth_token（加密推理必需，漏读只能走公开端点）".into(),
                ));
            }
            let machine = v.get("machine_id").and_then(Value::as_str).unwrap_or_default().to_string();
            let fields = wasm
                .generate_runtime_auth_fields(&uid, &token)
                .map_err(|e| ProviderError::Upstream(e.to_string()))?;
            let user_info = serde_json::json!({
                "uid": uid, "encrypt_user_info": fields.encrypt_user_info, "key": fields.key,
                "organization_id": "", "organization_tags": [], "data_policy_agreed": false,
            })
            .to_string();
            let meta = r#"{"client_type":"5","business_product":"cli","business_type":"agent","scene":"assistant"}"#;
            let ctx = wasm
                .context_new(&machine, COSY_VERSION, &user_info, meta)
                .map_err(|e| ProviderError::Upstream(e.to_string()))?;
            let infer = wasm
                .prepare_infer_request(ctx, "api2.qoder.sh", &payload, &route_m, "system")
                .map_err(|e| ProviderError::Upstream(e.to_string()))?;
            let mut h = reqwest::header::HeaderMap::new();
            for (k, val) in &infer.headers {
                if let (Ok(hk), Ok(hv)) = (
                    reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                    reqwest::header::HeaderValue::from_str(val),
                ) {
                    h.insert(hk, hv);
                }
            }
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()
                .map_err(|e| ProviderError::Upstream(e.to_string()))?;
            rt.block_on(async {
                let resp = reqwest::Client::new()
                    .post(&infer.url)
                    .headers(h)
                    .header("accept", "text/event-stream")
                    .body(infer.body.clone())
                    .send()
                    .await
                    .map_err(|e| ProviderError::Upstream(e.to_string()))?;
                let status = resp.status().as_u16();
                let text = resp.text().await.unwrap_or_default();
                Ok((status, text))
            })
        })
        .join()
        .map_err(|_| ProviderError::Upstream("wasm 线程 panic".into()))?
    }

    #[async_trait]
    impl Provider for QoderProvider {
        fn id(&self) -> &str {
            "qoder"
        }

        fn catalog(&self) -> ProviderCatalog {
            ProviderCatalog {
                id: "qoder".into(),
                models: vec![ModelInfo { id: "qfmodel".into() }, ModelInfo { id: "dmodel".into() }],
            }
        }

        async fn complete(&self, cred: &Credential, route: &Route, req: &ChatRequest) -> Result<ChatCompletion, ProviderError> {
            let (status, body) = send_encrypted(cred, route, req)?;
            if status != 200 {
                return Err(ProviderError::Upstream(format!(
                    "qoder infer http {status}: {}",
                    &body[..body.len().min(200)]
                )));
            }
            let agg = aggregate_sse(&body)?;
            let (reason, usage) = agg.finish.unwrap_or_else(|| ("stop".into(), Usage::default()));
            let mut out = ChatCompletion::new(route.composite(), agg.text, usage);
            if !agg.tool_calls.is_empty() {
                let calls: Vec<Value> = agg
                    .tool_calls
                    .iter()
                    .map(|(id, name, args)| {
                        serde_json::json!({
                            "id": id.clone().unwrap_or_default(), "type": "function",
                            "function": { "name": name.clone().unwrap_or_default(), "arguments": args }
                        })
                    })
                    .collect();
                out.choices[0].message.tool_calls = Some(Value::Array(calls));
            }
            if reason != "stop" {
                out.choices[0].finish_reason = Some(reason);
            }
            Ok(out)
        }

        async fn stream(&self, cred: &Credential, route: &Route, req: &ChatRequest) -> Result<ChunkStream, ProviderError> {
            let (status, body) = send_encrypted(cred, route, req)?;
            if status != 200 {
                return Err(ProviderError::Upstream(format!("qoder infer http {status}")));
            }
            let agg = aggregate_sse(&body)?;
            let (reason, usage) = agg.finish.unwrap_or_else(|| ("stop".into(), Usage::default()));
            let mut queue: std::collections::VecDeque<Result<StreamChunk, ProviderError>> =
                std::collections::VecDeque::new();
            queue.push_back(Ok(StreamChunk::Role));
            if !agg.reasoning.is_empty() {
                queue.push_back(Ok(StreamChunk::Reasoning(agg.reasoning)));
            }
            if !agg.text.is_empty() {
                queue.push_back(Ok(StreamChunk::Content(agg.text)));
            }
            for (index, (id, name, args)) in agg.tool_calls.into_iter().enumerate() {
                queue.push_back(Ok(StreamChunk::ToolCallDelta { index: index as u64, id, name, arguments: args }));
            }
            queue.push_back(Ok(StreamChunk::Finish { reason, usage }));
            Ok(Box::pin(futures::stream::iter(queue)))
        }
    }
}
