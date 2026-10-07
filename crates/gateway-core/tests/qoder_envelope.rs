//! T4.5a qoder 响应信封解包（§3.8，纯函数）。
//! 行为来源：docs/reference/deepseek-harness-codearts.md §3.8——
//! - 每帧 SSE `data:{"headers":{…},"body":"<字符串化 JSON>","statusCodeValue":200}`，
//!   内层 body 未加密，只剥信封；
//! - 分类按 **JSON 结构**不嗅探子串：`choices`/`usage` → chunk；显式
//!   `code`/`message`/`error`/`statusCodeValue`/`type` → error；
//!   `null`/`{}`/空 → heartbeat **整帧跳过**（旧实现把心跳判成错误→
//!   正常回完报 SERVER 白重试 5 次，issue IKJOZ8）；
//! - 错误帧**保真转发**：`code` 必须保持独立字段（拼进 message 后缀会让
//!   排队识别永不命中，:182-199）。

use gateway_core::providers::qoder::{unwrap_envelope, EnvelopeFrame};
use serde_json::json;

#[test]
fn chunk_frame_unwrapped_from_envelope() {
    let inner = json!({ "choices": [{ "delta": { "content": "你好" } }], "usage": { "prompt_tokens": 3 } });
    let frame = json!({
        "headers": { "x": 1 },
        "body": inner.to_string(),
        "statusCodeValue": 200
    });
    match unwrap_envelope(&frame.to_string()).expect("chunk") {
        EnvelopeFrame::Chunk(v) => {
            assert_eq!(v["choices"][0]["delta"]["content"], json!("你好"));
            assert_eq!(v["usage"]["prompt_tokens"], json!(3));
        }
        other => panic!("应为 Chunk：{other:?}"),
    }
}

#[test]
fn error_frame_preserves_code_as_independent_field() {
    let inner = json!({ "code": "10605", "message": "model queued", "type": "model_error" });
    let frame = json!({
        "headers": {},
        "body": inner.to_string(),
        "statusCodeValue": 200
    });
    match unwrap_envelope(&frame.to_string()).expect("error") {
        EnvelopeFrame::Error { code, message } => {
            assert_eq!(code.as_ref().and_then(|c| c.as_str()), Some("10605"), "code 独立字段保真");
            assert_eq!(message.as_deref(), Some("model queued"), "message 不拼 code 后缀");
        }
        other => panic!("应为 Error：{other:?}"),
    }
}

#[test]
fn heartbeat_frames_are_skipped_whole() {
    for body in ["null", "{}", ""] {
        let frame = json!({ "headers": {}, "body": body, "statusCodeValue": 200 });
        match unwrap_envelope(&frame.to_string()).expect("合法帧") {
            EnvelopeFrame::Heartbeat => {}
            other => panic!("内层 {body:?} 应为心跳整帧跳过：{other:?}"),
        }
    }
}

#[test]
fn outer_non_200_status_is_error() {
    let inner = json!({ "code": 110, "message": "Billing daily count exceeded" });
    let frame = json!({ "headers": {}, "body": inner.to_string(), "statusCodeValue": 403 });
    match unwrap_envelope(&frame.to_string()).expect("error") {
        EnvelopeFrame::Error { code, .. } => {
            assert_eq!(code.as_ref().and_then(|c| c.as_i64()), Some(110));
        }
        other => panic!("应为 Error：{other:?}"),
    }
}

#[test]
fn classification_is_structural_not_substring_sniffing() {
    // message 文本里出现 "choices" 字样，但结构无 choices/usage 键 → 仍按 error
    let inner = json!({ "code": 1, "message": "unknown model choices missing" });
    let frame = json!({ "headers": {}, "body": inner.to_string(), "statusCodeValue": 200 });
    assert!(matches!(
        unwrap_envelope(&frame.to_string()).unwrap(),
        EnvelopeFrame::Error { .. }
    ));
    // 反向：有 choices 键，message 提到 error → 仍按 chunk
    let inner2 = json!({ "choices": [], "usage": {}, "message": "contains word error" });
    let frame2 = json!({ "headers": {}, "body": inner2.to_string(), "statusCodeValue": 200 });
    assert!(matches!(
        unwrap_envelope(&frame2.to_string()).unwrap(),
        EnvelopeFrame::Chunk(_)
    ));
}
