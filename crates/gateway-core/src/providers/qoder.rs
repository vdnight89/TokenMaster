//! qoder Provider（阿里系，缝 2）。
//!
//! 本切片（T4.5a）：加密端点的**响应信封解包**（reference §3.8）——
//! 每帧 SSE `data:{"headers":{…},"body":"<字符串化 JSON>","statusCodeValue":200}`，
//! 内层 body 未加密，只剥信封。分类按 **JSON 结构**不嗅探子串。
//!
//! 待续切片：WASM 请求加密（wasmtime 接线）、PKCE 设备码登录、
//! credits/领取（§4.5）。

use serde_json::Value;

/// 剥信封后的一帧。
#[derive(Debug, Clone, PartialEq)]
pub enum EnvelopeFrame {
    /// 内层含 `choices`/`usage` 结构 → OpenAI chunk
    Chunk(Value),
    /// 错误帧：`code` 保持独立字段保真（拼进 message 会让排队识别永不命中）
    Error { code: Option<Value>, message: Option<String> },
    /// `null`/`{}`/空 → 心跳，整帧跳过（误判成错误会白重试，issue IKJOZ8）
    Heartbeat,
}

/// 剥一帧 data 载荷。非 JSON 信封返回 None（调用方按损坏帧处理）。
pub fn unwrap_envelope(data: &str) -> Option<EnvelopeFrame> {
    let v: Value = serde_json::from_str(data.trim()).ok()?;
    if !v.is_object() {
        return Some(EnvelopeFrame::Heartbeat);
    }
    // 信封层状态非 200 → 错误帧
    let outer_status = v.get("statusCodeValue").and_then(Value::as_i64).unwrap_or(200);
    // 内层：body 是字符串化 JSON（未加密）
    let inner: Value = match v.get("body") {
        Some(Value::String(s)) => {
            let t = s.trim();
            if t.is_empty() {
                return Some(EnvelopeFrame::Heartbeat);
            }
            match serde_json::from_str(t) {
                Ok(inner) => inner,
                Err(_) => return Some(EnvelopeFrame::Heartbeat),
            }
        }
        Some(other) => other.clone(),
        None => v.clone(),
    };
    match &inner {
        Value::Null => Some(EnvelopeFrame::Heartbeat),
        Value::Object(m) => {
            if m.is_empty() {
                return Some(EnvelopeFrame::Heartbeat);
            }
            // 结构分类（顺序即优先级）：chunk 键 → chunk；错误键 → error
            if m.contains_key("choices") || m.contains_key("usage") {
                return Some(EnvelopeFrame::Chunk(inner));
            }
            if m.contains_key("code") || m.contains_key("error") || m.contains_key("type") {
                return Some(EnvelopeFrame::Error {
                    code: m.get("code").cloned(),
                    message: m.get("message").and_then(Value::as_str).map(str::to_string),
                });
            }
            if outer_status != 200 {
                return Some(EnvelopeFrame::Error {
                    code: v.get("statusCodeValue").cloned(),
                    message: None,
                });
            }
            // 其余未知结构按心跳跳过（不臆造错误）
            Some(EnvelopeFrame::Heartbeat)
        }
        _ => Some(EnvelopeFrame::Heartbeat),
    }
}

// ───────────────────── credits/领取/错误码（T4.5b） ─────────────────────

use crate::provider::{Credential, ProviderError};

pub const DEFAULT_OPENAPI_BASE: &str = "https://openapi.qoder.sh";
const USAGE_PATH: &str = "/sash/api/v2/me/usage";
const CAMPAIGNS_PATH: &str = "/sash/api/v1/me/campaigns";
/// Cosy-ClientType 实测值 '10'（qoder-product.ts:454-455）。
/// Cosy-MachineType 值手册未逐字载明，占位待 wire 核对。
const COSY_CLIENT_TYPE: &str = "10";
const COSY_MACHINE_TYPE: &str = "pc";

#[derive(Debug, Clone, PartialEq)]
pub struct QoderBalance {
    /// userQuota + addOnQuota 多包剩余累加（只读 userQuota 会显示 0）
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClaimOutcome {
    Claimed,
    /// 幂等重放（响应体 replayed:true，HTTP 仍 200）
    Replayed,
}

/// 信封错误帧 code → ProviderError。
/// 10605 排队（按服务端 retryAfterMs 等待，上限 30 分钟）；110 额度
/// （Billing daily count exceeded）不可重试；未知码返回 None 不臆造。
pub fn map_error_code(code: &str, retry_after_ms: Option<u64>) -> Option<ProviderError> {
    match code {
        "10605" => {
            let secs = retry_after_ms.map(|ms| (ms / 1000).min(1800));
            Some(ProviderError::RateLimited {
                retry_after_secs: secs,
                msg: "model queued (10605)".into(),
            })
        }
        "110" => Some(ProviderError::BadRequest(
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

    /// 余额请求头：只需 Bearer + Cosy-ClientType。
    fn usage_headers(cred: &Credential) -> Result<reqwest::header::HeaderMap, ProviderError> {
        let v = Self::parse_secret(cred)?;
        let mut h = reqwest::header::HeaderMap::new();
        let ins = reqwest::header::HeaderValue::from_str;
        let _ = ins(&format!("Bearer {}", Self::sstr(&v, "access_token")))
            .map(|x| h.insert("authorization", x));
        let _ = ins(COSY_CLIENT_TYPE).map(|x| h.insert("cosy-client-type", x));
        Ok(h)
    }

    /// 活动/领取请求头：必需**成对** machine 头（缺了只见 VIEW_DETAILS
    /// 看不到可领活动）；machine token 取凭据 machine_id。
    fn campaign_headers(cred: &Credential) -> Result<reqwest::header::HeaderMap, ProviderError> {
        let v = Self::parse_secret(cred)?;
        let mut h = Self::usage_headers(cred)?;
        let ins = reqwest::header::HeaderValue::from_str;
        let _ = ins(&Self::sstr(&v, "machine_id")).map(|x| h.insert("cosy-machinetoken", x));
        let _ = ins(COSY_MACHINE_TYPE).map(|x| h.insert("cosy-machinetype", x));
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

    /// 余额 = userQuota + addOnQuota 多包剩余累加。
    /// ⚠️ 包内剩余字段名按 `remaining`、包裹层级按一层 `data`（手册未载明，
    /// wire 核对后校准）。
    pub async fn balance(&self, cred: &Credential) -> Result<QoderBalance, ProviderError> {
        let v = self.get_json(USAGE_PATH, Self::usage_headers(cred)?).await?;
        let sum = |key: &str| -> u64 {
            v.pointer(&format!("/data/{key}"))
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .map(|p| p.get("remaining").and_then(Value::as_u64).unwrap_or(0))
                        .sum()
                })
                .unwrap_or(0)
        };
        Ok(QoderBalance { total: sum("userQuota") + sum("addOnQuota") })
    }

    /// 可领活动列表：只保留 `CLAIM_BENEFIT` + `CLAIMABLE`。
    pub async fn campaigns(&self, cred: &Credential) -> Result<Vec<Value>, ProviderError> {
        let v = self.get_json(CAMPAIGNS_PATH, Self::campaign_headers(cred)?).await?;
        let list = v
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(list
            .into_iter()
            .filter(|c| {
                c.get("type").and_then(Value::as_str) == Some("CLAIM_BENEFIT")
                    && c.get("status").and_then(Value::as_str) == Some("CLAIMABLE")
            })
            .collect())
    }

    /// 领取：body 空；幂等判据是响应体 `replayed:true`（HTTP 仍 200）。
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
            Ok(ClaimOutcome::Replayed)
        } else {
            Ok(ClaimOutcome::Claimed)
        }
    }
}


// ───────────── T4.20a：设备码登录 / 续期（qoder.ts/qoder-oauth.ts） ─────────────

/// qoder 国际版公开 client_id（qoder-product.ts:442-446；prod 必须用 prod 的
/// client_id，用错回调报"参数无效"）。
pub const QODER_CLIENT_ID: &str = "e883ade2-e6e3-4d6d-adf7-f92ceff5fdcb";
/// 轮询节奏（qoder.ts:16-20）：总超时 300s、网络失败容忍 5 次。
const POLL_DEADLINE: std::time::Duration = std::time::Duration::from_secs(300);
const POLL_NET_FAIL_MAX: u32 = 5;

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
                Ok(Some(v)) => return Ok(self.credential_from(&v, sess)),
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
