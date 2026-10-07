//! T4.12 codearts provider（缝 2：stub 上游）。
//! 行为来源：reference §4.1——
//! - SDK-HMAC-SHA256 签名：七段式 canonical request
//! - `Agent-Type: PromptCenter`/`X-Language: zh-cn` **签名后追加**
//! - 429 判据锚定独立数字（裸子串会命中 4291——额度码）
//! - 排队（TM.00001041）10s 重试；额度（InferHub.4291.200）立即失败

use gateway_core::providers::codearts::{
    is_429_standalone, is_quota_exhausted, is_queued, sdk_hmac_sha256_sign,
};

#[test]
fn hmac_sign_produces_correct_structure() {
    let headers = vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("host".to_string(), "example.com".to_string()),
        ("x-sdk-content-sha256".to_string(), "abc123".to_string()),
        ("x-sdk-date".to_string(), "20261007T120000Z".to_string()),
    ];
    let sig = sdk_hmac_sha256_sign("POST", "/api/v2/chat/completions", "", &headers, b"{}", "test-secret", "test-ak");
    assert!(sig.starts_with("SDK-HMAC-SHA256 Access=test-ak,"), "{sig}");
    assert!(sig.contains("SignedHeaders=content-type;host;x-sdk-content-sha256;x-sdk-date"), "{sig}");
    assert!(sig.contains("Signature="), "{sig}");
    // 签名是 64 位 hex
    let sig_part = sig.rsplit('=').next().unwrap();
    assert_eq!(sig_part.len(), 64, "SHA-256 hex：{sig_part}");
}

#[test]
fn hmac_sign_deterministic() {
    let headers = vec![
        ("host".to_string(), "h".to_string()),
        ("x-sdk-date".to_string(), "d".to_string()),
    ];
    let s1 = sdk_hmac_sha256_sign("GET", "/", "", &headers, b"", "key", "ak");
    let s2 = sdk_hmac_sha256_sign("GET", "/", "", &headers, b"", "key", "ak");
    assert_eq!(s1, s2);
    // 不同 key 不同签名
    let s3 = sdk_hmac_sha256_sign("GET", "/", "", &headers, b"", "other", "ak");
    assert_ne!(s1, s3);
}

#[test]
fn four29_anchor_does_not_hit_4291() {
    // "4291" 不应命中 429 判据（是额度码不是限流码）
    assert!(!is_429_standalone("code 4291 insufficient"), "4291 不含独立 429");
    assert!(is_429_standalone("rate limit 429 too many"), "独立 429 命中");
    assert!(is_429_standalone("HTTP 429"), "HTTP 429 命中");
    assert!(!is_429_standalone("no code here"), "无 429 不命中");
}

#[test]
fn quota_exhausted_vs_queued() {
    assert!(is_quota_exhausted("InferHub.4291.200 insufficient quota"));
    assert!(!is_quota_exhausted("rate limit 429"));
    assert!(is_queued("TM.00001041 queue full"));
    assert!(is_queued("InferHub.ModelArts.81111.429"));
    assert!(!is_queued("normal response"));
}
