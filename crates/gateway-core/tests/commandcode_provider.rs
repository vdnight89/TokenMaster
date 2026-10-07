//! T4.3a commandcode 推理通道（缝 2：stub 上游）。
//! 行为来源：docs/reference/commandcode-proxy.md §4.1/4.2/4.4/§7——
//! - `POST {base}/alpha/generate`，响应 200 + NDJSON（每行一个 JSON 事件）；
//! - 私有信封固定键序 `config, memory, taste, skills, permissionMode,
//!   threadId, mode, params`（promptCache 从不生成）；params.stream 恒 true；
//! - 凭据必须是 `user_` 形状，`Authorization: Bearer` 原样透传；
//! - 请求头：cli UA / x-command-code-version 1.53.1 / x-cli-environment
//!   production / x-project-slug（slugify(projectDir)） / x-session-id /
//!   traceparent（W3C 00-32hex-16hex-01）；
//! - 无 system 时发 `[{type:'text',text:' '}]` 占位（issue #17）；
//! - assistant 内容块次序强制 [reasoning, text, tool-call]（坑 6）；
//! - 流尽无 finish 绝不伪造完成（坑 11）；error 事件状态采纳链
//!   message `<NNN>` 前缀 > statusCode > 502（坑 12）。

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use futures::StreamExt;
use gateway_core::openai::ChatRequest;
use gateway_core::provider::{Provider, ProviderError, StreamChunk};
use gateway_core::providers::commandcode::{map_finish_reason, map_status_error, slugify, CommandcodeProvider};
use gateway_core::route::Route;
use gateway_core::Credential;
use serde_json::{json, Value};

#[derive(Default)]
struct Cap {
    body: Option<String>,
    headers: Option<HeaderMap>,
    /// NDJSON 响应行（可配置；默认 happy path）
    ndjson: Option<String>,
}

async fn stub_generate(State(cap): State<Arc<Mutex<Cap>>>, headers: HeaderMap, body: axum::extract::Request) -> Response {
    let bytes = axum::body::to_bytes(body.into_body(), 8 << 20).await.unwrap();
    let mut c = cap.lock().unwrap();
    c.body = Some(String::from_utf8_lossy(&bytes).to_string());
    c.headers = Some(headers.clone());
    let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("");
    if !auth.contains("user_") {
        return (StatusCode::UNAUTHORIZED, "invalid key").into_response();
    }
    let ndjson = c.ndjson.clone().unwrap_or_else(|| {
        [
            r#"{"type":"start"}"#,
            r#"{"type":"text-delta","text":"你"}"#,
            r#"{"type":"reasoning-delta","text":"想一想"}"#,
            r#"{"type":"text-delta","text":"好"}"#,
            r#"{"type":"finish-step","finishReason":"stop","usage":{"inputTokens":11,"outputTokens":5}}"#,
            r#"{"type":"finish","finishReason":"tool-calls","totalUsage":{"inputTokens":11,"cachedInputTokens":3,"outputTokens":7}}"#,
        ]
        .join("\n")
    });
    (StatusCode::OK, [("content-type", "application/json")], ndjson).into_response()
}

async fn spawn() -> (String, Arc<Mutex<Cap>>) {
    let cap = Arc::new(Mutex::new(Cap::default()));
    let app = Router::new()
        .route("/alpha/generate", post(stub_generate))
        .with_state(cap.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub serve") });
    (format!("http://{addr}"), cap)
}

fn pv(base: String) -> CommandcodeProvider {
    CommandcodeProvider::new(base)
}

fn cred() -> Credential {
    Credential { account_id: "cc1".into(), secret: "user_abc123".into() }
}

fn route() -> Route {
    Route { provider: "commandcode".into(), model: "deepseek/deepseek-v4-flash".into() }
}

fn plain_req() -> ChatRequest {
    serde_json::from_value(json!({
        "model": "commandcode/deepseek/deepseek-v4-flash",
        "messages": [{ "role": "user", "content": "打招呼" }]
    }))
    .unwrap()
}

fn sent_body(cap: &Arc<Mutex<Cap>>) -> Value {
    let raw = cap.lock().unwrap().body.clone().unwrap();
    serde_json::from_str(&raw).unwrap()
}

#[tokio::test]
async fn envelope_fixed_key_order_and_headers() {
    let (base, cap) = spawn().await;
    pv(base).complete(&cred(), &route(), &plain_req()).await.unwrap();
    let v = sent_body(&cap);
    let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        vec!["config", "memory", "taste", "skills", "permissionMode", "threadId", "mode", "params"],
        "信封键序固定（promptCache 从不生成）：{keys:?}"
    );
    assert_eq!(v["mode"], json!("agent"));
    assert_eq!(v["memory"], Value::Null, "skills/memory 发 null 不是空串（坑5）");
    assert!(v["threadId"].as_str().unwrap().len() == 36, "threadId=UUID 形态");
    let cfg = &v["config"];
    assert_eq!(cfg["environment"], json!("win32"));
    assert!(cfg["workingDir"].as_str().unwrap().contains("projects"));
    assert!(cfg["date"].as_str().unwrap().len() == 10);
    assert_eq!(cfg["isGitRepo"], json!(false));

    let h = cap.lock().unwrap().headers.clone().unwrap();
    assert_eq!(h.get("user-agent").unwrap(), "cli");
    assert_eq!(h.get("x-command-code-version").unwrap(), "1.53.1");
    assert_eq!(h.get("x-cli-environment").unwrap(), "production");
    assert_eq!(h.get("x-taste-learning").unwrap(), "false");
    assert_eq!(
        h.get("x-project-slug").and_then(|v| v.to_str().ok()),
        Some("c-users-dev-projects-app"),
        "slug 由 projectDir 现算（坑22）"
    );
    assert!(h.get("x-session-id").is_some());
    let tp = h.get("traceparent").unwrap().to_str().unwrap();
    let parts: Vec<&str> = tp.split('-').collect();
    assert_eq!(parts.len(), 4);
    assert_eq!(parts[0], "00");
    assert_eq!(parts[1].len(), 32);
    assert_eq!(parts[2].len(), 16);
    assert_eq!(parts[3], "01");
    assert_eq!(
        h.get("authorization").unwrap().to_str().unwrap(),
        "Bearer user_abc123",
        "user_ key 原样透传"
    );
}

#[tokio::test]
async fn params_mapping_system_placeholder_and_clamps() {
    let (base, cap) = spawn().await;
    let req: ChatRequest = serde_json::from_value(json!({
        "model": "commandcode/deepseek/deepseek-v4-flash",
        "max_tokens": 300000,
        "temperature": 0.3,
        "reasoning_effort": "high",
        "messages": [{ "role": "user", "content": "hi" }]
    }))
    .unwrap();
    pv(base).complete(&cred(), &route(), &req).await.unwrap();
    let p = &sent_body(&cap)["params"];
    assert_eq!(p["max_tokens"], json!(200000), "钳制上限 200000");
    assert_eq!(p["stream"], json!(true), "CC API 只有流式（坑30）");
    assert_eq!(p["temperature"], json!(0.3));
    assert_eq!(p["reasoning_effort"], json!("high"));
    assert_eq!(p["system"], json!([{ "type": "text", "text": " " }]), "无 system 发空格占位（坑4）");
    assert_eq!(p["tools"], json!([]), "tools 恒下发空数组（坑5）");
}

#[tokio::test]
async fn default_max_tokens_and_system_blocks() {
    let (base, cap) = spawn().await;
    let req: ChatRequest = serde_json::from_value(json!({
        "model": "commandcode/deepseek/deepseek-v4-flash",
        "messages": [
            { "role": "system", "content": "你是助手" },
            { "role": "user", "content": "hi" }
        ]
    }))
    .unwrap();
    pv(base).complete(&cred(), &route(), &req).await.unwrap();
    let p = &sent_body(&cap)["params"];
    assert_eq!(p["max_tokens"], json!(64000), "缺省 64000");
    assert_eq!(p["system"], json!([{ "type": "text", "text": "你是助手" }]));
    // system 消息不进 messages
    let msgs = p["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["role"], json!("user"));
}

#[tokio::test]
async fn assistant_block_order_and_tool_result_name_lookup() {
    let (base, cap) = spawn().await;
    let req: ChatRequest = serde_json::from_value(json!({
        "model": "commandcode/deepseek/deepseek-v4-flash",
        "messages": [
            { "role": "user", "content": "查天气" },
            { "role": "assistant", "content": "想想", "reasoning_content": "先查天气", "tool_calls": [{
                "id": "call_1", "type": "function",
                "function": { "name": "bash_output", "arguments": "{\"city\":\"北京\"}" }
            }]},
            { "role": "tool", "tool_call_id": "call_1", "content": "晴" }
        ]
    }))
    .unwrap();
    pv(base).complete(&cred(), &route(), &req).await.unwrap();
    let msgs = sent_body(&cap)["params"]["messages"].as_array().unwrap().clone();
    // assistant 内容块次序强制 [reasoning, text, tool-call]（坑6）
    let blocks = msgs[1]["content"].as_array().unwrap().clone();
    let types: Vec<&str> = blocks.iter().map(|b| b["type"].as_str().unwrap()).collect();
    assert_eq!(types, vec!["reasoning", "text", "tool-call"], "次序错会被上游拒绝");
    assert_eq!(blocks[0]["text"], json!("先查天气"));
    assert_eq!(blocks[2]["toolCallId"], json!("call_1"));
    assert_eq!(blocks[2]["toolName"], json!("bash_output"), "消息体 toolName 用原始名（别名只在 tools 声明做）");
    assert_eq!(blocks[2]["input"], json!({ "city": "北京" }), "arguments 解析为对象");
    // tool 消息 → tool-result，name 由 id 反查
    let tr = &msgs[2]["content"][0];
    assert_eq!(msgs[2]["role"], json!("tool"));
    assert_eq!(tr["type"], json!("tool-result"));
    assert_eq!(tr["toolName"], json!("bash_output"), "toolName 从 assistant 反查（原始名）");
    assert_eq!(tr["output"], json!({ "type": "text", "value": "晴" }));
}

#[tokio::test]
async fn image_url_maps_to_cc_image_block() {
    let (base, cap) = spawn().await;
    let req: ChatRequest = serde_json::from_value(json!({
        "model": "commandcode/deepseek/deepseek-v4-flash",
        "messages": [{
            "role": "user",
            "content": [
                { "type": "text", "text": "看图" },
                { "type": "image_url", "image_url": { "url": "data:image/png;base64,QUJD" } }
            ]
        }]
    }))
    .unwrap();
    pv(base).complete(&cred(), &route(), &req).await.unwrap();
    let blocks = sent_body(&cap)["params"]["messages"][0]["content"].as_array().unwrap().clone();
    assert_eq!(blocks[0]["type"], json!("text"));
    assert_eq!(blocks[1]["type"], json!("image"));
    assert_eq!(blocks[1]["image"], json!("data:image/png;base64,QUJD"));
    assert_eq!(blocks[1]["mimeType"], json!("image/png"));
}

#[tokio::test]
async fn ndjson_stream_translates_to_chunks() {
    let (base, _) = spawn().await;
    let mut s = pv(base).stream(&cred(), &route(), &plain_req()).await.unwrap();
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
    assert_eq!(reasoning, "想一想");
    // 默认 stub 无 tool-call 事件：delta 为空
    assert!(deltas.is_empty());
    let (reason, usage) = finish.expect("finish 必达");
    assert_eq!(reason, "tool_calls", "连字符 tool-calls 必须规范化（坑10）");
    assert_eq!(usage.prompt_tokens, 11);
    assert_eq!(usage.completion_tokens, 7);
}

#[tokio::test]
async fn tool_call_event_stringifies_input() {
    let (base, cap) = spawn().await;
    cap.lock().unwrap().ndjson = Some(
        [
            r#"{"type":"tool-call","toolCallId":"tc1","toolName":"bash","input":{"cmd":"ls"}}"#.to_string(),
            r#"{"type":"finish","finishReason":"stop","totalUsage":{"inputTokens":3,"outputTokens":2}}"#.to_string(),
        ]
        .join("\n"),
    );
    let mut s = pv(base).stream(&cred(), &route(), &plain_req()).await.unwrap();
    let mut deltas = Vec::new();
    while let Some(item) = s.next().await {
        if let StreamChunk::ToolCallDelta { index, id, name, arguments } = item.expect("chunk") {
            deltas.push((index, id, name, arguments));
        }
    }
    assert_eq!(deltas.len(), 1);
    let (index, id, name, args) = deltas.into_iter().next().unwrap();
    assert_eq!(index, 0);
    assert_eq!(id.as_deref(), Some("tc1"));
    assert_eq!(name.as_deref(), Some("bash"));
    assert_eq!(args, "{\"cmd\":\"ls\"}", "input 对象统一 stringify");
}

#[tokio::test]
async fn missing_finish_is_truncation_not_forged_completion() {
    let (base, cap) = spawn().await;
    cap.lock().unwrap().ndjson = Some(
        [
            r#"{"type":"text-delta","text":"半截"}"#.to_string(),
            r#"{"type":"error","error":{"message":"boom 429 too many","code":"RATE_LIMITED"}}"#.to_string(),
        ]
        .join("\n"),
    );
    let err = pv(base).complete(&cred(), &route(), &plain_req()).await.unwrap_err();
    match err {
        ProviderError::RateLimited { retry_after_secs, msg } => {
            assert_eq!(retry_after_secs, Some(30), "message <NNN> 前缀 > statusCode（坑12）");
            assert!(msg.contains("429"));
        }
        other => panic!("应为 RateLimited：{other:?}"),
    }
}

#[tokio::test]
async fn zero_output_maps_to_rate_limit() {
    let (base, cap) = spawn().await;
    cap.lock().unwrap().ndjson = Some(
        r#"{"type":"finish","finishReason":"stop","totalUsage":{"inputTokens":9,"outputTokens":0}}"#.to_string(),
    );
    let err = pv(base).complete(&cred(), &route(), &plain_req()).await.unwrap_err();
    assert!(
        matches!(err, ProviderError::RateLimited { retry_after_secs: Some(10), .. }),
        "零输出 → 429 rate_limit retry 10：{err:?}"
    );
}

#[tokio::test]
async fn finish_step_alone_completes_with_usage_fallback() {
    let (base, cap) = spawn().await;
    cap.lock().unwrap().ndjson = Some(
        [
            r#"{"type":"text-delta","text":"ok"}"#.to_string(),
            r#"{"type":"finish-step","finishReason":"stop","usage":{"inputTokens":5,"outputTokens":2}}"#.to_string(),
        ]
        .join("
"),
    );
    let out = pv(base).complete(&cred(), &route(), &plain_req()).await.unwrap();
    // 只收到 finish-step 也算正常完成，usage 回退用它的 usage
    assert!(out.choices[0].message.content.contains("ok"));
    assert_eq!(out.usage.prompt_tokens, 5);
    assert_eq!(out.usage.completion_tokens, 2);
}

#[tokio::test]
async fn invalid_key_shape_rejected_before_send() {
    let (base, cap) = spawn().await;
    let err = pv(base)
        .complete(&Credential { account_id: "cc1".into(), secret: "sk-not-user".into() }, &route(), &plain_req())
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::Credential(_)), "{err:?}");
    assert!(cap.lock().unwrap().body.is_none(), "非 user_ 形状不得发请求");
}

#[tokio::test]
async fn status_map_and_finish_reason_pure_functions() {
    assert!(matches!(map_status_error(402, "x".into()), ProviderError::RateLimited { .. }), "付费失败按限流");
    assert!(matches!(map_status_error(401, "x".into()), ProviderError::Credential(_)));
    assert!(matches!(map_status_error(500, "x".into()), ProviderError::Upstream(_)));
    assert_eq!(map_finish_reason("tool-calls"), "tool_calls");
    assert_eq!(map_finish_reason("tool_use"), "tool_calls");
    assert_eq!(map_finish_reason("max_output_tokens"), "length");
    assert_eq!(map_finish_reason("model_context_window_exceeded"), "length");
    assert_eq!(map_finish_reason("pause_turn"), "pause_turn", "原样保留不折（proxy.mjs:926）");
    assert_eq!(map_finish_reason("whatever-new"), "whatever-new", "未知值原样不折 stop");
    assert_eq!(slugify(r"C:\Users\dev\projects\app"), "c-users-dev-projects-app");
    assert_eq!(slugify("!!!"), "root", "空折叠兜底 root");
}
