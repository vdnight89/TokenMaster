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
