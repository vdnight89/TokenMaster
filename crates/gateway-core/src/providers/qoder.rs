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
/// Cosy-ClientType / Cosy-MachineType 具体值手册未载明，占位待 wire 核对。
const COSY_CLIENT_TYPE: &str = "ide";
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
