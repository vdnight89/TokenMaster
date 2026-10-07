//! T4.4b trae SOLO 推理通道（缝 2：stub 上游）。
//! 逐条对照参考实现（deepseek-harness-codearts/src/trae.ts）：
//! - `transformToSOLOBody`（:1545）：stream 恒 true、function=solo_work_lite、
//!   model→config_name+model 双字段（`__dev` 后缀消除）、content 字符串→
//!   `[{type:"text"}]`、assistant tool_calls function→function_call（无 name
//!   剔除、全剔删键）、tool_choice 归一（none 删 tools）、tools.parameters
//!   对象→JSON 字符串、max_tokens 钳 64000；**不写 reasoning_content**
//!   （serializeTraeMessages/transformSOLOMessage 均不序列化思考）。
//! - `traeSOLOHeaders`（:204）：产品常量实测值 + X-Machine-Id 可轮换派生。
//! - `parseTraeSSELine`（:1743）：event/data 行；output{response,
//!   reasoning_content,tool_calls}、token_usage{prompt_tokens,
//!   completion_tokens}、done{finish_reason}、error{code,message}；
//!   tool_calls 归一 function_call→function、删 namespace/partial_arguments。
//! - 错误分类（trae-errors.ts:80-126 + cooldown-table）：业务码先于状态码；
//!   4008→24h、1005→12h、4011→60s、4001=模型不可调用（BadRequest 不换号）、
//!   401/会话失效标记→重登。
//! - Max 模式成套字段（trae.ts:1508-1525）：strategy=max + 三件套。

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use futures::StreamExt;
use gateway_core::openai::ChatRequest;
use gateway_core::provider::{Provider, ProviderError, StreamChunk};
use gateway_core::providers::trae::{
    classify_trae_error, derive_rotating_machine_id, parse_solo_sse, trae_max_mode_fields,
    TraeProvider, TRAE_MAX_COMPLETION_TOKENS, TRAE_MAX_CONTEXT_TOKENS,
    TRAE_MAX_MODE_OUTPUT_TOKENS, TRAE_MAX_MODE_TYPE, TRAE_MAX_PROMPT_TOKENS,
};
use gateway_core::route::Route;
use gateway_core::Credential;
use serde_json::{json, Value};

#[derive(Default)]
struct Cap {
    body: Option<Value>,
    headers: Option<HeaderMap>,
}

async fn stub_chat(State(cap): State<Arc<Mutex<Cap>>>, h: HeaderMap, body: axum::extract::Request) -> Response {
    let bytes = axum::body::to_bytes(body.into_body(), 8 << 20).await.unwrap();
    {
        let mut c = cap.lock().unwrap();
        c.body = serde_json::from_slice(&bytes).ok();
        c.headers = Some(h.clone());
    }
    let sse = concat!(
        "event: metadata\ndata: {\"sid\":\"x\"}\n\n",
        "event: output\ndata: {\"response\":\"你\",\"reasoning_content\":\"想想\"}\n\n",
        "event: output\ndata: {\"response\":\"好\"}\n\n",
        "event: output\ndata: {\"tool_calls\":[{\"index\":0,\"id\":\"tc1\",\"function_call\":{\"name\":\"bash\",\"arguments\":\"{\\\"cmd\\\":\\\"ls\\\"}\",\"namespace\":\"x\",\"partial_arguments\":\"y\"}}]}\n\n",
        "event: token_usage\ndata: {\"prompt_tokens\":11,\"completion_tokens\":7,\"reasoning_tokens\":3}\n\n",
        "event: done\ndata: {\"finish_reason\":\"tool_calls\"}\n\n",
    );
    (StatusCode::OK, [("content-type", "text/event-stream")], sse.to_string()).into_response()
}

async fn spawn() -> (String, Arc<Mutex<Cap>>) {
    let cap = Arc::new(Mutex::new(Cap::default()));
    let app = Router::new()
        .route("/api/agent/v3/llm_utils_chat", post(stub_chat))
        .with_state(cap.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub serve") });
    (format!("http://{addr}"), cap)
}

fn pv(base: String) -> TraeProvider {
    TraeProvider::with_agent_base("http://unused".into(), base)
}

fn cred() -> Credential {
    Credential {
        account_id: "t1".into(),
        secret: json!({
            "access_token": "jwt-tok",
            "refresh_token": "rt-1",
            "uid": "u-9527",
            "machine_id": "0123456789abcdef0123456789abcdef",
            "device_id": "fedcba9876543210fedcba9876543210"
        })
        .to_string(),
    }
}

fn route() -> Route {
    Route { provider: "trae".into(), model: "glm-5.2__dev".into() }
}

fn solo_req() -> ChatRequest {
    serde_json::from_value(json!({
        "model": "trae/glm-5.2",
        "max_tokens": 131072,
        "tool_choice": { "type": "function", "function": { "name": "bash" } },
        "tools": [{
            "type": "function",
            "function": {
                "name": "bash",
                "parameters": { "type": "object", "properties": { "cmd": { "type": "string" } } }
            }
        }],
        "messages": [
            { "role": "user", "content": "查一下" },
            { "role": "assistant", "content": null, "reasoning_content": "上一轮思考不该回传", "tool_calls": [
                { "id": "tc0", "type": "function", "function": { "name": "bash", "arguments": "{\"cmd\":\"ls\"}" } },
                { "id": "bad", "type": "function", "function": { "arguments": "{}" } }
            ]},
            { "role": "tool", "tool_call_id": "tc0", "content": "done" }
        ]
    }))
    .unwrap()
}

#[tokio::test]
async fn solo_body_transform_matches_reference() {
    let (base, cap) = spawn().await;
    pv(base).complete(&cred(), &route(), &solo_req()).await.unwrap();
    let v = cap.lock().unwrap().body.clone().unwrap();

    assert_eq!(v["function"], json!("solo_work_lite"));
    assert_eq!(v["stream"], json!(true));
    // model → config_name + model 双字段，__dev 后缀消除
    assert_eq!(v["config_name"], json!("glm-5.2"));
    assert_eq!(v["model"], json!("glm-5.2"));
    // max_tokens 钳 64000（客户端索要 131072 会把上游打 4xx）
    assert_eq!(v["max_tokens"], json!(TRAE_MAX_COMPLETION_TOKENS));
    // tool_choice {type:function} → 字符串 name
    assert_eq!(v["tool_choice"], json!("bash"));
    // tools.parameters 对象 → JSON 字符串
    let params = v["tools"][0]["function"]["parameters"].as_str().unwrap();
    assert!(params.starts_with('{') && params.contains("\"cmd\""), "parameters 须字符串化：{params}");

    let msgs = v["messages"].as_array().unwrap().clone();
    // user content 字符串 → [{type:text,text}]
    assert_eq!(msgs[0]["content"], json!([{ "type": "text", "text": "查一下" }]));
    // assistant：content null 跳过键；tool_calls function→function_call；无 name 剔除
    assert!(msgs[1].get("content").is_none(), "null content 跳过键");
    // 输入消息不写 reasoning_content（参考 serializeTraeMessages /
    // transformSOLOMessage 均不写该键；思考回传上游会污染上下文）
    assert!(
        msgs.iter().all(|m| m.get("reasoning_content").is_none()),
        "reasoning_content 不得序列化进上游请求体"
    );
    let tcs = msgs[1]["tool_calls"].as_array().unwrap().clone();
    assert_eq!(tcs.len(), 1, "无 function_call.name 的 tool_call 剔除");
    assert_eq!(tcs[0]["function_call"]["name"], json!("bash"), "SOLO 字段名 function_call");
    assert!(tcs[0].get("function").is_none());
    assert_eq!(msgs[2]["role"], json!("tool"));
}

#[tokio::test]
async fn solo_headers_match_reference_product_constants() {
    let (base, cap) = spawn().await;
    pv(base).complete(&cred(), &route(), &solo_req()).await.unwrap();
    let h = cap.lock().unwrap().headers.clone().unwrap();
    assert_eq!(h.get("user-agent").unwrap(), "Trae/0.1.52");
    assert_eq!(h.get("x-app-id").unwrap(), "6eefa01c-1036-4c7e-9ca5-d891f63bfcd8");
    assert_eq!(h.get("x-app-version").unwrap(), "default");
    assert_eq!(h.get("x-ide-version").unwrap(), "0.1.52");
    assert_eq!(h.get("x-ide-version-code").unwrap(), "20260811");
    assert_eq!(h.get("x-app-version-code").unwrap(), "20260811");
    assert_eq!(h.get("x-ide-version-type").unwrap(), "stable");
    assert_eq!(h.get("x-os-version").unwrap(), "macOS 15.7.4");
    assert_eq!(h.get("x-device-brand").unwrap(), "Apple");
    assert_eq!(h.get("accept").unwrap(), "text/event-stream");
    assert_eq!(h.get("authorization").unwrap(), "Cloud-IDE-JWT jwt-tok");
    assert_eq!(h.get("x-machine-id").unwrap(), "0123456789abcdef0123456789abcdef", "gen0 用原值");
}

#[tokio::test]
async fn sse_events_translate_to_stream_chunks() {
    let (base, _) = spawn().await;
    let mut s = pv(base).stream(&cred(), &route(), &solo_req()).await.unwrap();
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut deltas = Vec::new();
    let mut finish = None;
    while let Some(item) = s.next().await {
        match item.expect("chunk") {
            StreamChunk::Content(t) => text.push_str(&t),
            StreamChunk::Reasoning(t) => reasoning.push_str(&t),
            StreamChunk::ToolCallDelta { index, id, name, arguments } => {
                deltas.push((index, id, name, arguments))
            }
            StreamChunk::Finish { reason, usage } => finish = Some((reason, usage)),
            _ => {}
        }
    }
    assert_eq!(text, "你好");
    assert_eq!(reasoning, "想想");
    // tool_calls 归一：function_call→function、删 namespace/partial_arguments
    assert_eq!(deltas.len(), 1);
    let (index, id, name, args) = deltas.into_iter().next().unwrap();
    assert_eq!(index, 0);
    assert_eq!(id.as_deref(), Some("tc1"));
    assert_eq!(name.as_deref(), Some("bash"));
    assert_eq!(args, "{\"cmd\":\"ls\"}");
    let (reason, usage) = finish.expect("done 必达");
    assert_eq!(reason, "tool_calls");
    assert_eq!(usage.prompt_tokens, 11);
    assert_eq!(usage.completion_tokens, 7);
}

#[tokio::test]
async fn error_event_classified_with_cooldowns() {
    let (base, cap) = spawn().await;
    drop(cap.lock().unwrap());
    // 直接对分类函数断言（SSE error 事件 → classify）。
    // 业务码先于状态码与会话死亡判定（trae-errors.ts:80-126 判定顺序）。
    assert!(matches!(
        classify_trae_error(200, Some(4008), "quota exceeded"),
        ProviderError::RateLimited { retry_after_secs: Some(86_400), .. }
    ));
    assert!(matches!(
        classify_trae_error(200, Some(1005), "plan limit"),
        ProviderError::RateLimited { retry_after_secs: Some(43_200), .. }
    ));
    assert!(matches!(
        classify_trae_error(200, Some(4011), "rate"),
        ProviderError::RateLimited { retry_after_secs: Some(60), .. }
    ));
    assert!(matches!(
        classify_trae_error(200, None, "please login again"),
        ProviderError::Credential(_)
    ));
    // 401 状态码无条件判会话死亡（参考规则 4：标记词只在 401 分支内起作用）
    assert!(matches!(classify_trae_error(401, None, "whatever"), ProviderError::Credential(_)));
    // 含 "4011" 的消息不得被裸 "401" 子串误判成会话死亡（无结构化 code 时）
    assert!(
        matches!(classify_trae_error(400, None, "frequency limit 4011"), ProviderError::BadRequest(_)),
        "4011 字样不能误触发 session-dead"
    );
    // 4001 = 模型不可调用（trae-adapter.ts:556-575/222-224：换号无意义，
    // 不冷却、不可重试）
    match classify_trae_error(200, Some(4001), "We're sorry, the param is invalid.") {
        ProviderError::BadRequest(msg) => {
            assert!(msg.contains("模型不可调用") || msg.contains("not callable"), "4001 语义须指向模型：{msg}")
        }
        other => panic!("4001 → BadRequest（模型不可调用）：{other:?}"),
    }
    let _ = (base, cap);
}

#[test]
fn max_mode_fields_form_complete_strategy_bundle() {
    // Max 模式成套字段（trae.ts:1508-1525）：strategy=max 判定 + 三件套
    // （context_window_size / prompt_max_tokens / max_tokens）缺一不可，
    // 只调大 max_tokens 会被当普通会话按 200K 校验。
    let v = trae_max_mode_fields(TRAE_MAX_CONTEXT_TOKENS, None);
    assert_eq!(v["model_auto_selection"]["strategy"], json!("max"));
    assert_eq!(v["model_auto_selection"]["fallback_to_advance_model"], json!(null));
    assert_eq!(v["model_auto_selection"]["entitlement_id"], json!(null));
    assert_eq!(v["model_selection_strategy"], json!("max"));
    assert_eq!(v["mode_type"], json!(TRAE_MAX_MODE_TYPE));
    assert_eq!(v["context_window_size"], json!(TRAE_MAX_CONTEXT_TOKENS));
    assert_eq!(v["prompt_max_tokens"], json!(TRAE_MAX_PROMPT_TOKENS));
    assert_eq!(v["max_tokens"], json!(TRAE_MAX_MODE_OUTPUT_TOKENS), "缺省输出上限 64K");
    // 远端声明的 Max 窗口与输出上限优先
    let v2 = trae_max_mode_fields(1_000_000, Some(384_000));
    assert_eq!(v2["context_window_size"], json!(1_000_000));
    assert_eq!(v2["max_tokens"], json!(384_000));
    // 非法窗口/输出回退默认（trae.ts:1512/1523 的 >0 判据）
    let v3 = trae_max_mode_fields(0, Some(0));
    assert_eq!(v3["context_window_size"], json!(TRAE_MAX_CONTEXT_TOKENS));
    assert_eq!(v3["max_tokens"], json!(TRAE_MAX_MODE_OUTPUT_TOKENS));
}

#[test]
fn rotating_machine_id_derivation() {
    let base = "0123456789abcdef0123456789abcdef";
    assert_eq!(derive_rotating_machine_id(base, 0), base);
    let g1 = derive_rotating_machine_id(base, 1);
    assert_eq!(g1.len(), 32);
    assert_ne!(g1, base);
    // sha256("{base}#machine1") 前 32 hex（对照参考 trae.ts:1377）
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(format!("{base}#machine1"));
    let expect = format!("{:x}", h.finalize())[..32].to_string();
    assert_eq!(g1, expect);
}

#[test]
fn sse_parser_tolerates_crlf_line_endings() {
    // 真实上游可能用 CRLF 行尾；不剥 \r 会让事件分隔行失效、事件名带 \r
    // （对齐参考 consumeSse 的 line.trim()）。
    let text = "event: output\r\ndata: {\"response\":\"好\"}\r\n\r\nevent: done\r\ndata: {\"finish_reason\":\"stop\"}\r\n\r\n";
    let events = parse_solo_sse(text);
    assert_eq!(events.len(), 2, "{events:?}");
    assert_eq!(events[0].event, "output");
    assert_eq!(events[0].response.as_deref(), Some("好"));
    assert_eq!(events[1].event, "done");
    assert_eq!(events[1].finish_reason.as_deref(), Some("stop"));
}

#[tokio::test]
async fn plain_req_minimal_body_still_valid() {
    let (base, cap) = spawn().await;
    let req: ChatRequest = serde_json::from_value(json!({
        "model": "trae/glm-5.2",
        "messages": [{ "role": "user", "content": "hi" }]
    }))
    .unwrap();
    pv(base).complete(&cred(), &route(), &req).await.unwrap();
    let v = cap.lock().unwrap().body.clone().unwrap();
    assert_eq!(v["config_name"], json!("glm-5.2"));
    assert!(v.get("tools").is_none(), "无 tools 不发键");
    assert!(v.get("tool_choice").is_none());
}
