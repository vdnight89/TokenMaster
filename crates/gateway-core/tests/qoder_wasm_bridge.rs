//! T4.20b qoder WASM 桥功能测试（feature = "wasm" 才编译）。
//! 用真实 wasm（crates/gateway-core/wasm/qoder-auth-wasm.wasm）跑通四步
//! 调用链：generate_runtime_auth_fields → qodercontext_new →
//! qodercontext_prepareInferRequest → requestresult_*。
//!
//! 运行：cargo test -p gateway-core --features wasm --test qoder_wasm_bridge

#![cfg(feature = "wasm")]

use gateway_core::providers::qoder_wasm::QoderWasm;

#[test]
fn wasm_bridge_runs_full_call_chain() {
    let mut w = QoderWasm::new().expect("wasm 实例化");
    // ① 运行时鉴权字段（内部走 crypto.getRandomValues 路径）
    let fields = w
        .generate_runtime_auth_fields("u-777", "oauth-token-x")
        .expect("auth fields");
    assert!(!fields.encrypt_user_info.is_empty(), "encrypt_user_info 非空");
    assert!(!fields.key.is_empty(), "key 非空");
    // ② QoderContext 构造（userInfoJson 内嵌鉴权字段）
    let user_info = serde_json::json!({
        "uid": "u-777",
        "encrypt_user_info": fields.encrypt_user_info,
        "key": fields.key,
        "organization_id": "",
        "organization_tags": [],
        "data_policy_agreed": false,
    })
    .to_string();
    let meta = serde_json::json!({
        "client_type": "5",
        "business_product": "cli",
        "business_type": "agent",
        "scene": "assistant",
    })
    .to_string();
    let ctx = w
        .context_new("machine-id-1", "1.1.49", &user_info, &meta)
        .expect("context_new");
    assert!(ctx != 0, "上下文句柄非零：{ctx}");
    // ③ 加密推理请求（business 字段必填——缺失会路由到故障节点）
    let payload = serde_json::json!({
        "request_id": "req-1", "request_set_id": "req-1", "chat_record_id": "req-1",
        "session_id": "sess-1", "stream": true, "chat_task": "FREE_INPUT",
        "chat_context": { "text": "你好", "features": [] },
        "is_reply": true, "is_retry": false, "source": 1, "version": "3",
        "agent_id": "agent_common", "task_id": "common",
        "session_type": "qodercli", "aliyun_user_type": "",
        "model_config": { "key": "qfmodel", "display_name": "qf", "model": "",
                          "format": "openai", "is_vl": false, "is_reasoning": false,
                          "api_key": "", "url": "", "source": "system", "max_input_tokens": 0 },
        "custom_model": null, "system": [], "messages": [], "tools": [],
        "parameters": {}, "business": { "type": "agent" },
    })
    .to_string();
    let req = w
        .prepare_infer_request(ctx, "api2.qoder.sh", &payload, "qfmodel", "system")
        .expect("prepare infer");
    // URL 是完整加密端点（含 Encode=1 等固定 query）
    assert!(
        req.url.contains("agent_chat_generation") && req.url.contains("Encode=1"),
        "加密端点 URL：{}",
        req.url
    );
    // headers 由 WASM 签名生成（Bearer COSY.<载荷>.<签名>）——原样透传约定
    let authz = req
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
        .map(|(_, v)| v.clone())
        .expect("Authorization 头存在");
    assert!(authz.starts_with("Bearer COSY."), "WASM 签名 Authorization：{authz}");
    assert!(!req.body.is_empty(), "加密 body 非空");
}

#[test]
fn wasm_bridge_is_deterministic_per_instance_state() {
    // 同一实例可重复构造请求（上下文复用）
    let mut w = QoderWasm::new().expect("wasm");
    let fields = w.generate_runtime_auth_fields("u-1", "tok").expect("fields");
    let user_info = serde_json::json!({
        "uid": "u-1", "encrypt_user_info": fields.encrypt_user_info, "key": fields.key,
        "organization_id": "", "organization_tags": [], "data_policy_agreed": false,
    })
    .to_string();
    let meta = r#"{"client_type":"5","business_product":"cli","business_type":"agent","scene":"assistant"}"#;
    let ctx = w.context_new("m-1", "1.1.49", &user_info, meta).expect("ctx");
    let payload = r#"{"business":{"type":"agent"},"messages":[],"tools":[]}"#;
    for _ in 0..2 {
        let req = w
            .prepare_infer_request(ctx, "api2.qoder.sh", payload, "qfmodel", "system")
            .expect("repeat prepare");
        assert!(req.url.starts_with("https://"), "{}", req.url);
    }
}
