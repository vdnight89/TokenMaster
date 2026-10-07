//! T4.2c gemini 工具链（缝 2：stub 上游）。
//! 行为来源：docs/reference/deepseek-harness-codearts.md §3.6——
//! - `tools` 声明 → 信封 `request.tools[].functionDeclarations`；
//!   assistant `tool_calls` → `functionCall` part（args 解析为对象）；
//!   `role:tool` → `functionResponse` part，**name 必须来自对应 tool_use 的 name**
//!   （先扫全消息按 tool_call_id 建映射）。
//! - 响应 `functionCall` part → OpenAI `tool_calls`（arguments 序列化回字符串），
//!   finish_reason=tool_calls（流式为 ToolCallDelta + Finish）。

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use futures::StreamExt;
use gateway_core::openai::ChatRequest;
use gateway_core::provider::{Provider, StreamChunk};
use gateway_core::providers::gemini::GeminiProvider;
use gateway_core::route::Route;
use gateway_core::Credential;
use serde_json::{json, Value};

#[derive(Default)]
struct Cap {
    body: Option<String>,
}

async fn stub_generate(State(cap): State<Arc<Mutex<Cap>>>, body: axum::extract::Request) -> Response {
    let bytes = axum::body::to_bytes(body.into_body(), 8 << 20).await.unwrap();
    cap.lock().unwrap().body = Some(String::from_utf8_lossy(&bytes).to_string());
    let frame1 = json!({
        "candidates": [{"content": {"parts": [
            { "text": "查一下" },
            { "functionCall": { "name": "get_weather", "args": { "city": "北京" } } }
        ], "role": "model"}}]
    });
    let frame2 = json!({
        "candidates": [{"content": {"parts": [], "role": "model"}}],
        "usageMetadata": { "promptTokenCount": 9, "candidatesTokenCount": 4 }
    });
    let sse = format!("data: {frame1}\n\ndata: {frame2}\n\n");
    ([("content-type", "text/event-stream")], sse).into_response()
}

async fn spawn() -> (String, Arc<Mutex<Cap>>) {
    let cap = Arc::new(Mutex::new(Cap::default()));
    let app = Router::new()
        .route("/v1internal:streamGenerateContent", post(stub_generate))
        .route("/v1internal:loadCodeAssist", post(|| async {
            (StatusCode::OK, Json(json!({ "cloudaicompanionProject": "proj-1" })))
        }))
        .with_state(cap.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub serve") });
    (format!("http://{addr}"), cap)
}

fn pv(base: String) -> GeminiProvider {
    GeminiProvider::new(base, "aicode-consumers".into())
}

fn cred() -> Credential {
    Credential { account_id: "g1".into(), secret: "oauth-token".into() }
}

fn route() -> Route {
    Route { provider: "gemini".into(), model: "gemini-3.8-flash".into() }
}

fn tool_req() -> ChatRequest {
    serde_json::from_value(json!({
        "model": "gemini/gemini-3.8-flash",
        "tools": [{
            "type": "function",
            "function": {
                "name": "get_weather",
                "parameters": { "type": "object", "properties": { "city": { "type": "string" } }, "required": ["city"] }
            }
        }],
        "messages": [
            { "role": "user", "content": "天气怎么样" },
            { "role": "assistant", "content": "", "tool_calls": [{
                "id": "call_1", "type": "function",
                "function": { "name": "get_weather", "arguments": "{\"city\":\"北京\"}" }
            }]},
            { "role": "tool", "tool_call_id": "call_1", "content": "晴 25 度" }
        ]
    }))
    .unwrap()
}

#[tokio::test]
async fn tools_and_tool_history_map_into_envelope() {
    let (base, cap) = spawn().await;
    pv(base).complete(&cred(), &route(), &tool_req()).await.unwrap();
    let raw = cap.lock().unwrap().body.clone().unwrap();
    let v: Value = serde_json::from_str(&raw).unwrap();

    // tools 声明 → functionDeclarations
    let decls = &v["request"]["tools"][0]["functionDeclarations"];
    assert_eq!(decls[0]["name"], json!("get_weather"));
    assert_eq!(decls[0]["parameters"]["properties"]["city"]["type"], json!("string"));

    // contents: user / model(functionCall) / user(functionResponse)
    let contents = v["request"]["contents"].as_array().unwrap();
    assert_eq!(contents[0]["role"], json!("user"));
    let fc = &contents[1]["parts"][0]["functionCall"];
    assert_eq!(contents[1]["role"], json!("model"));
    assert_eq!(fc["name"], json!("get_weather"));
    assert_eq!(fc["args"], json!({ "city": "北京" }), "args 必须是对象");
    let fr = &contents[2]["parts"][0]["functionResponse"];
    assert_eq!(contents[2]["role"], json!("user"));
    assert_eq!(fr["name"], json!("get_weather"), "name 须来自 tool_call_id 映射，不能臆造");
    assert_eq!(fr["response"]["content"], json!("晴 25 度"), "response 用 content 键（gemini-messages.ts:246-250）");
}

#[tokio::test]
async fn response_function_call_becomes_openai_tool_calls() {
    let (base, _) = spawn().await;
    let out = pv(base).complete(&cred(), &route(), &tool_req()).await.unwrap();
    assert!(out.choices[0].message.content.contains("查一下"));
    let tc = out.choices[0]
        .message
        .tool_calls
        .as_ref()
        .expect("非流式完成必须带 tool_calls");
    assert_eq!(tc[0]["function"]["name"], json!("get_weather"));
    assert_eq!(tc[0]["function"]["arguments"], json!("{\"city\":\"北京\"}"));
    assert_eq!(tc[0]["type"], json!("function"));
    assert!(tc[0]["id"].as_str().unwrap().starts_with("call_"));
    assert_eq!(out.choices[0].finish_reason.as_deref(), Some("tool_calls"));
    assert_eq!(out.usage.prompt_tokens, 9);
}

#[tokio::test]
async fn stream_function_call_becomes_tool_call_delta() {
    let (base, _) = spawn().await;
    let mut s = pv(base).stream(&cred(), &route(), &tool_req()).await.unwrap();
    let mut text = String::new();
    let mut deltas = Vec::new();
    let mut finish = None;
    while let Some(item) = s.next().await {
        match item.expect("chunk") {
            StreamChunk::Content(t) => text.push_str(&t),
            StreamChunk::ToolCallDelta { index, id, name, arguments } => {
                deltas.push((index, id, name, arguments))
            }
            StreamChunk::Finish { reason, usage } => finish = Some((reason, usage)),
            _ => {}
        }
    }
    assert_eq!(text, "查一下");
    assert_eq!(deltas.len(), 1);
    let (index, id, name, args) = deltas.into_iter().next().unwrap();
    assert_eq!(index, 0);
    assert!(id.expect("首片须带 id").starts_with("call_"));
    assert_eq!(name.as_deref(), Some("get_weather"));
    assert_eq!(args, "{\"city\":\"北京\"}");
    let (reason, usage) = finish.expect("must finish");
    assert_eq!(reason, "tool_calls");
    assert_eq!(usage.prompt_tokens, 9);
}

/// 有 tools 就**恒发** toolConfig（gemini-messages.ts:332-333/360-375：参考实现
/// 无 toolChoice 字段时也发默认 AUTO，不做条件省略）；命名强制时 ANY +
/// allowedFunctionNames。
#[tokio::test]
async fn tool_config_always_present_when_tools_declared() {
    let (base, cap) = spawn().await;
    pv(base).complete(&cred(), &route(), &tool_req()).await.unwrap();
    let raw = cap.lock().unwrap().body.clone().unwrap();
    let v: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        v["request"]["toolConfig"]["functionCallingConfig"]["mode"],
        json!("AUTO"),
        "无 tool_choice 也须发默认 AUTO 的 toolConfig"
    );
}

#[tokio::test]
async fn named_tool_choice_maps_to_any_with_allowed_function_names() {
    let (base, cap) = spawn().await;
    let mut req = tool_req();
    req.raw.insert(
        "tool_choice".into(),
        json!({ "type": "function", "function": { "name": "get_weather" } }),
    );
    pv(base).complete(&cred(), &route(), &req).await.unwrap();
    let raw = cap.lock().unwrap().body.clone().unwrap();
    let v: Value = serde_json::from_str(&raw).unwrap();
    let fc = &v["request"]["toolConfig"]["functionCallingConfig"];
    assert_eq!(fc["mode"], json!("ANY"));
    assert_eq!(fc["allowedFunctionNames"], json!(["get_weather"]));
}

/// 孤儿 tool 结果（tool_call_id 查无对应 tool_use）整块丢弃，不打死整轮
/// （gemini-messages.ts:240-245：上游按 name 配对，空 name 会 400）。
#[tokio::test]
async fn orphan_tool_result_dropped_not_fatal() {
    let (base, cap) = spawn().await;
    let req: ChatRequest = serde_json::from_value(json!({
        "model": "gemini/gemini-3.8-flash",
        "tools": [{
            "type": "function",
            "function": { "name": "get_weather", "parameters": { "type": "object", "properties": {} } }
        }],
        "messages": [
            { "role": "user", "content": "天气怎么样" },
            { "role": "tool", "tool_call_id": "call_unknown", "content": "孤儿结果" }
        ]
    }))
    .unwrap();
    let out = pv(base).complete(&cred(), &route(), &req).await.unwrap();
    assert!(out.choices[0].message.content.contains("查一下"));
    let raw = cap.lock().unwrap().body.clone().unwrap();
    let v: Value = serde_json::from_str(&raw).unwrap();
    let contents = v["request"]["contents"].as_array().unwrap();
    assert_eq!(contents.len(), 1, "孤儿 tool 消息整条跳过：{raw}");
    assert_eq!(contents[0]["role"], json!("user"));
}

/// schema 清洗（gemini.ts:615-681）：type 数组滤 'null' 收敛 + nullable；
/// 含非字符串枚举整删；白名单外键（additionalProperties/$schema）整键删除。
#[tokio::test]
async fn schema_type_array_and_enum_sanitized() {
    let (base, cap) = spawn().await;
    let req: ChatRequest = serde_json::from_value(json!({
        "model": "gemini/gemini-3.8-flash",
        "tools": [{
            "type": "function",
            "function": {
                "name": "mixed_tool",
                "parameters": {
                    "type": "object",
                    "$schema": "http://json-schema.org/draft-07/schema#",
                    "additionalProperties": false,
                    "properties": {
                        "maybe": { "type": ["string", "null"], "description": "可空" },
                        "num": { "type": ["null", "integer"] },
                        "kind": { "enum": [1, "a", "b"] }
                    },
                    "required": ["maybe"]
                }
            }
        }],
        "messages": [{ "role": "user", "content": "跑工具" }]
    }))
    .unwrap();
    pv(base).complete(&cred(), &route(), &req).await.unwrap();
    let raw = cap.lock().unwrap().body.clone().unwrap();
    let v: Value = serde_json::from_str(&raw).unwrap();
    let params = &v["request"]["tools"][0]["functionDeclarations"][0]["parameters"];
    assert!(params.get("$schema").is_none(), "白名单外键必须整键删除");
    assert!(params.get("additionalProperties").is_none(), "白名单外键必须整键删除");
    let props = &params["properties"];
    // ["string","null"] → type: string + nullable: true（滤 null 取首个非空）
    assert_eq!(props["maybe"]["type"], json!("string"));
    assert_eq!(props["maybe"]["nullable"], json!(true));
    // ["null","integer"] → null 在首位也滤掉，type 取 integer
    assert_eq!(props["num"]["type"], json!("integer"));
    assert_eq!(props["num"]["nullable"], json!(true));
    // enum 含非字符串值（1）→ 整删
    assert!(props["kind"].get("enum").is_none(), "非字符串枚举必须整删");
    assert_eq!(params["required"], json!(["maybe"]));
}

/// assistant tool_call 的 args 解析失败退化为 {}（gemini-messages.ts:405-416：
/// 不编造参数外观，也不因畸形历史打死整轮）。
#[tokio::test]
async fn malformed_tool_call_args_degrade_to_empty_object() {
    let (base, cap) = spawn().await;
    let req: ChatRequest = serde_json::from_value(json!({
        "model": "gemini/gemini-3.8-flash",
        "messages": [
            { "role": "user", "content": "天气怎么样" },
            { "role": "assistant", "content": "", "tool_calls": [{
                "id": "call_1", "type": "function",
                "function": { "name": "get_weather", "arguments": "{not-json" }
            }]},
            { "role": "tool", "tool_call_id": "call_1", "content": "晴" }
        ]
    }))
    .unwrap();
    let out = pv(base).complete(&cred(), &route(), &req).await.unwrap();
    assert!(out.choices[0].message.content.contains("查一下"), "畸形历史不得打死整轮");
    let raw = cap.lock().unwrap().body.clone().unwrap();
    let v: Value = serde_json::from_str(&raw).unwrap();
    let fc = &v["request"]["contents"][1]["parts"][0]["functionCall"];
    assert_eq!(fc["args"], json!({}), "解析失败退化为空对象");
}
