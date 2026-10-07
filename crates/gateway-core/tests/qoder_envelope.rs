//! T4.5a qoder 响应信封解包（§3.8，纯函数）。
//! 行为来源：deepseek-harness-codearts/src/qoder-envelope.ts——
//! - 每帧 SSE `data:{"headers":{…},"body":"<字符串化 JSON>","statusCodeValue":200}`，
//!   内层 body 未加密，只剥信封；
//! - 分类按 **JSON 结构**不嗅探子串、只看内层（classifyInner :72-102）：
//!   `choices`/`usage` → chunk；错误键 `code`/`message`/`error`/
//!   `statusCodeValue`/`type` **任一单独出现** → error（:92-99）；
//!   `null`/`{}`/空/裸标量 → heartbeat **整帧跳过**（旧实现把心跳判成错误→
//!   正常回完报 SERVER 白重试 5 次，issue IKJOZ8）；
//! - 内层解析失败 → **业务错误**而非心跳（:80-83——纯文本错误体如
//!   `[FAIL]node:… msg:…`，判成心跳会静默吞掉真实失败）；
//! - 内层含 `[DONE]` → 返回 None 交调用方透传（:75-76 子串语义保留）；
//! - 错误帧**保真转发**：`code` 必须保持独立字段（拼进 message 后缀会让
//!   排队识别永不命中，:182-199）；message 缺字符串字段时回退内层原文。

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

#[test]
fn inner_parse_failure_is_business_error_not_heartbeat() {
    // 纯文本错误体（qoder-envelope.ts:80-83：判成心跳会静默吞掉真实失败，
    // 「静默停止」缺陷正是这个方向）
    let frame = json!({
        "headers": {},
        "body": "[FAIL]node:oa_qwen-plus-2025-04-28 msg:Execution failed",
        "statusCodeValue": 200
    });
    match unwrap_envelope(&frame.to_string()).expect("应为业务错误") {
        EnvelopeFrame::Error { code, message } => {
            assert!(code.is_none(), "纯文本错误体无 code 字段");
            assert_eq!(
                message.as_deref(),
                Some("[FAIL]node:oa_qwen-plus-2025-04-28 msg:Execution failed"),
                "message 回退内层原文"
            );
        }
        other => panic!("解析失败应为 Error：{other:?}"),
    }
}

#[test]
fn message_or_status_code_value_alone_classifies_as_error() {
    // message 单独出现（qoder-envelope.ts:92-99——不要求 code 同时在场）
    let f1 = json!({ "headers": {}, "body": "{\"message\":\"upstream degraded\"}", "statusCodeValue": 200 });
    assert!(matches!(
        unwrap_envelope(&f1.to_string()).unwrap(),
        EnvelopeFrame::Error { .. }
    ));
    // statusCodeValue 单独出现（内层结构里的，非信封层）
    let f2 = json!({ "headers": {}, "body": "{\"statusCodeValue\":500}", "statusCodeValue": 200 });
    assert!(matches!(
        unwrap_envelope(&f2.to_string()).unwrap(),
        EnvelopeFrame::Error { .. }
    ));
    // 无任何已知键 → 心跳（不臆造错误）；信封层状态非 200 也不参与分类
    // （classifyInner 只看内层，qoder-envelope.ts:72-102）
    let f3 = json!({ "headers": {}, "body": "{\"foo\":1}", "statusCodeValue": 502 });
    assert!(matches!(
        unwrap_envelope(&f3.to_string()).unwrap(),
        EnvelopeFrame::Heartbeat
    ));
}

#[test]
fn error_message_falls_back_to_raw_inner_text() {
    // error 键存在但 message 非字符串 → message 回退内层原文（:174-181）
    let frame = json!({ "headers": {}, "body": "{\"error\":{\"type\":\"x\"}}", "statusCodeValue": 200 });
    match unwrap_envelope(&frame.to_string()).unwrap() {
        EnvelopeFrame::Error { code, message } => {
            assert!(code.is_none());
            assert_eq!(message.as_deref(), Some("{\"error\":{\"type\":\"x\"}}"));
        }
        other => panic!("应为 Error：{other:?}"),
    }
}

#[test]
fn done_marker_is_passed_through_to_caller() {
    // [DONE] 子串语义保留（qoder-envelope.ts:75-76）：终止标记不是 JSON，
    // 绝不能落进「解析失败=业务错误」分支（流会以伪错误终止），也不能丢
    // （丢了对端不收尾）——返回 None 交调用方原样透传。
    let frame = json!({ "headers": {}, "body": "[DONE]", "statusCodeValue": 200 });
    assert!(unwrap_envelope(&frame.to_string()).is_none());
}

#[test]
fn standard_frame_without_envelope_is_classified_directly() {
    // 缺 body 字段 → 不是信封，按标准帧直接分类（innerTextOf null 的透传语义）
    let chunk = json!({ "choices": [{ "delta": { "content": "x" } }] });
    assert!(matches!(
        unwrap_envelope(&chunk.to_string()).unwrap(),
        EnvelopeFrame::Chunk(_)
    ));
    let err = json!({ "code": "10605", "message": "queued" });
    assert!(matches!(
        unwrap_envelope(&err.to_string()).unwrap(),
        EnvelopeFrame::Error { .. }
    ));
    // 非 JSON 载荷 → None（调用方透传/按损坏帧处理）
    assert!(unwrap_envelope("event: finish").is_none());
}
