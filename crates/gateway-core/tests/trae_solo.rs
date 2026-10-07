//! T4.4b trae SOLO 推理通道（缝 2：stub 上游）。
//! 逐条对照参考实现（deepseek-harness-codearts/src/trae.ts）：
//! - `transformToSOLOBody`（:1545）：stream 恒 true、function=solo_work_lite、
//!   model→config_name+model 双字段（`__dev` 后缀消除）、content 字符串→
//!   `[{type:"text"}]`、assistant tool_calls function→function_call（无 name
//!   剔除、全剔删键）、tool_choice 归一（none 删 tools）、tools.parameters
//!   对象→JSON 字符串、max_tokens 钳 64000。
//! - `traeSOLOHeaders`（:204）：产品常量实测值 + X-Machine-Id 可轮换派生。
//! - `parseTraeSSELine`（:1743）：event/data 行；output{response,
//!   reasoning_content,tool_calls}、token_usage{prompt_tokens,
//!   completion_tokens}、done{finish_reason}、error{code,message}；
//!   tool_calls 归一 function_call→function、删 namespace/partial_arguments。
//! - 错误分类（trae-errors.ts + cooldown-table）：4008→24h、1005→12h、
//!   4011→60s、会话死亡标记→重登。

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
    classify_trae_error, derive_rotating_machine_id, TraeProvider,
    TRAE_MAX_COMPLETION_TOKENS,
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
            { "role": "assistant", "content": null, "tool_calls": [
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
    // 直接对分类函数断言（SSE error 事件 → classify）
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
    let _ = (base, cap);
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
