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
    /// 按次响应（第 n 次请求取第 n 条；耗尽回退 ndjson/默认）
    responses: Vec<String>,
    /// 强制 HTTP 状态（非 200 场景；优先于 responses/ndjson）
    force_status: Option<u16>,
    hits: usize,
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
    let nth = c.hits;
    c.hits += 1;
    if let Some(code) = c.force_status {
        return (StatusCode::from_u16(code).unwrap(), "forced").into_response();
    }
    let ndjson = (nth < c.responses.len())
        .then(|| c.responses[nth].clone())
        .or_else(|| c.ndjson.clone())
        .unwrap_or_else(|| {
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
            r#"{"type":"error","error":{"message":"<429> boom rate limited","code":"RATE_LIMITED"}}"#.to_string(),
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

#[tokio::test]
async fn truncated_stream_retried_before_first_byte() {
    let (base, cap) = spawn().await;
    {
        let mut c = cap.lock().unwrap();
        // 第一发：200 但无 finish（截断）；第二发：正常（默认 happy path）
        c.responses = vec![r#"{"type":"text-delta","text":"半"}"#.to_string()];
    }
    let out = pv(base).complete(&cred(), &route(), &plain_req()).await.unwrap();
    assert!(out.choices[0].message.content.contains("好"), "重试后拿到完整回复");
    assert_eq!(cap.lock().unwrap().hits, 2, "截断在未吐字前重试一次");
}

#[test]
fn status_adoption_only_leading_angle_bracket_code() {
    use gateway_core::providers::commandcode::status_from_parts_pub;
    // 中间的 429 不再被误认（proxy.mjs:1027-1030）
    assert_eq!(status_from_parts_pub("boom 429 too many", None), 502);
    assert_eq!(status_from_parts_pub("<429> too many", None), 429);
    assert_eq!(status_from_parts_pub("err", Some(403)), 403);
    // `^<(\d{3})>` 锚定串首：前导空白不容忍（对齐正则锚点）
    assert_eq!(status_from_parts_pub(" <429> too many", None), 502);
}

// ───────────────────── 消息映射细节（对照 proxy.mjs:566-628） ─────────────────────

#[tokio::test]
async fn assistant_array_content_reasoning_block_passthrough() {
    let (base, cap) = spawn().await;
    let req: ChatRequest = serde_json::from_value(json!({
        "model": "commandcode/deepseek/deepseek-v4-flash",
        "messages": [
            { "role": "user", "content": "hi" },
            // 客户端把 reasoning 放在 content 数组里（无 reasoning_content 字段）→ 透传
            { "role": "assistant", "content": [
                { "type": "reasoning", "text": "想了想" },
                { "type": "text", "text": "答了" }
            ]}
        ]
    }))
    .unwrap();
    pv(base).complete(&cred(), &route(), &req).await.unwrap();
    let msgs = sent_body(&cap)["params"]["messages"].as_array().unwrap().clone();
    let blocks = msgs[1]["content"].as_array().unwrap().clone();
    let types: Vec<&str> = blocks.iter().map(|b| b["type"].as_str().unwrap()).collect();
    assert_eq!(types, vec!["reasoning", "text"], "reasoning 块透传且在最前（proxy.mjs:601-604）");
    assert_eq!(blocks[0]["text"], json!("想了想"));

    // 有 reasoning_content 字段时数组里的 reasoning 块不重复透传
    let (base2, cap2) = spawn().await;
    let req2: ChatRequest = serde_json::from_value(json!({
        "model": "commandcode/deepseek/deepseek-v4-flash",
        "messages": [
            { "role": "user", "content": "hi" },
            { "role": "assistant", "reasoning_content": "字段版", "content": [
                { "type": "reasoning", "text": "数组版" },
                { "type": "text", "text": "答" }
            ]}
        ]
    }))
    .unwrap();
    pv(base2).complete(&cred(), &route(), &req2).await.unwrap();
    let blocks2 = sent_body(&cap2)["params"]["messages"][1]["content"].as_array().unwrap().clone();
    let types2: Vec<&str> = blocks2.iter().map(|b| b["type"].as_str().unwrap()).collect();
    assert_eq!(types2, vec!["reasoning", "text"], "字段优先，数组 reasoning 不重复");
    assert_eq!(blocks2[0]["text"], json!("字段版"));
}

#[tokio::test]
async fn user_array_falsy_part_filtered_and_http_image_url_no_mime() {
    let (base, cap) = spawn().await;
    let req: ChatRequest = serde_json::from_value(json!({
        "model": "commandcode/deepseek/deepseek-v4-flash",
        "messages": [{
            "role": "user",
            "content": [
                Value::Null,
                { "type": "text", "text": "看图" },
                { "type": "image_url", "image_url": { "url": "https://example.com/cat.png" } }
            ]
        }]
    }))
    .unwrap();
    pv(base).complete(&cred(), &route(), &req).await.unwrap();
    let blocks = sent_body(&cap)["params"]["messages"][0]["content"].as_array().unwrap().clone();
    assert_eq!(blocks.len(), 2, "null 块被 filter(Boolean) 丢弃（proxy.mjs:582）");
    // 非 dataURL：原样透传，不带 mimeType（proxy.mjs:576-580）
    assert_eq!(blocks[1]["type"], json!("image"));
    assert_eq!(blocks[1]["image"], json!("https://example.com/cat.png"));
    assert!(blocks[1].get("mimeType").is_none(), "mimeType 仅 dataURL 形态下发");
}

#[tokio::test]
async fn assistant_tool_arguments_non_string_shapes() {
    let (base, cap) = spawn().await;
    let req: ChatRequest = serde_json::from_value(json!({
        "model": "commandcode/deepseek/deepseek-v4-flash",
        "messages": [
            { "role": "user", "content": "go" },
            { "role": "assistant", "tool_calls": [
                // arguments 已是对象 → 原样（proxy.mjs:614-616）
                { "id": "c1", "type": "function", "function": { "name": "f", "arguments": { "k": 1 } } },
                // arguments 为 null（假值）→ {}
                { "id": "c2", "type": "function", "function": { "name": "g", "arguments": null } },
                // 缺 id → toolCallId 整键省略（JS undefined）
                { "type": "function", "function": { "name": "h", "arguments": "{}" } }
            ]}
        ]
    }))
    .unwrap();
    pv(base).complete(&cred(), &route(), &req).await.unwrap();
    let blocks = sent_body(&cap)["params"]["messages"][1]["content"].as_array().unwrap().clone();
    assert_eq!(blocks[0]["input"], json!({ "k": 1 }), "对象 arguments 原样不折空对象");
    assert_eq!(blocks[1]["input"], json!({}), "null（假值）arguments 折空对象");
    assert!(blocks[2].get("toolCallId").is_none(), "缺 id 时 toolCallId 省键（proxy.mjs:611）");
    assert_eq!(blocks[2]["toolName"], json!("h"), "toolName 恒下发（空串语义）");
}

#[tokio::test]
async fn tool_result_array_content_joins_text_blocks_with_newline() {
    let (base, cap) = spawn().await;
    let req: ChatRequest = serde_json::from_value(json!({
        "model": "commandcode/deepseek/deepseek-v4-flash",
        "messages": [
            { "role": "user", "content": "go" },
            { "role": "assistant", "tool_calls": [
                { "id": "t1", "type": "function", "function": { "name": "run", "arguments": "{}" } }
            ]},
            { "role": "tool", "tool_call_id": "t1", "content": [
                { "type": "text", "text": "line1" },
                { "type": "image_url", "image_url": { "url": "data:image/png;base64,x" } },
                { "type": "text", "text": "line2" }
            ]}
        ]
    }))
    .unwrap();
    pv(base).complete(&cred(), &route(), &req).await.unwrap();
    let tr = &sent_body(&cap)["params"]["messages"][2]["content"][0];
    // toWireToolOutputValue：只取 text 块、'\n' 拼接（proxy.mjs:724-730）
    assert_eq!(tr["output"]["value"], json!("line1\nline2"));
}

#[tokio::test]
async fn tool_choice_wire_shapes() {
    let (base, cap) = spawn().await;
    let req: ChatRequest = serde_json::from_value(json!({
        "model": "commandcode/deepseek/deepseek-v4-flash",
        "tool_choice": "required",
        "messages": [{ "role": "user", "content": "hi" }]
    }))
    .unwrap();
    pv(base).complete(&cred(), &route(), &req).await.unwrap();
    assert_eq!(
        sent_body(&cap)["params"]["tool_choice"],
        json!({ "type": "any" }),
        "字符串 → {{type: mapped}}（proxy.mjs:698-700）"
    );

    let (base2, cap2) = spawn().await;
    let req2: ChatRequest = serde_json::from_value(json!({
        "model": "commandcode/deepseek/deepseek-v4-flash",
        "tool_choice": "banana",
        "messages": [{ "role": "user", "content": "hi" }]
    }))
    .unwrap();
    pv(base2).complete(&cred(), &route(), &req2).await.unwrap();
    assert_eq!(sent_body(&cap2)["params"]["tool_choice"], json!({ "type": "auto" }), "未知字符串折 auto");

    let (base3, cap3) = spawn().await;
    let req3: ChatRequest = serde_json::from_value(json!({
        "model": "commandcode/deepseek/deepseek-v4-flash",
        "tool_choice": { "type": "function", "function": { "name": "get_weather" } },
        "messages": [{ "role": "user", "content": "hi" }]
    }))
    .unwrap();
    pv(base3).complete(&cred(), &route(), &req3).await.unwrap();
    assert_eq!(
        sent_body(&cap3)["params"]["tool_choice"],
        json!({ "type": "tool", "name": "get_weather" })
    );
}

#[tokio::test]
async fn max_tokens_zero_falls_back_to_default() {
    let (base, cap) = spawn().await;
    let req: ChatRequest = serde_json::from_value(json!({
        "model": "commandcode/deepseek/deepseek-v4-flash",
        "max_tokens": 0,
        "messages": [{ "role": "user", "content": "hi" }]
    }))
    .unwrap();
    pv(base).complete(&cred(), &route(), &req).await.unwrap();
    // `max_tokens || 64000`：0 是 JS 假值 → 默认（proxy.mjs:668）
    assert_eq!(sent_body(&cap)["params"]["max_tokens"], json!(64000));
}

// ───────────────────── threadId / system 断点（信封层） ─────────────────────

#[tokio::test]
async fn thread_id_omitted_for_non_uuid_session() {
    let (base, cap) = spawn().await;
    let req: ChatRequest = serde_json::from_value(json!({
        "model": "commandcode/deepseek/deepseek-v4-flash",
        "prompt_cache_key": "my-custom-session-key",
        "messages": [{ "role": "user", "content": "hi" }]
    }))
    .unwrap();
    pv(base).complete(&cred(), &route(), &req).await.unwrap();
    let v = sent_body(&cap);
    assert!(
        v.get("threadId").is_none(),
        "threadId 仅 UUID 形态注入，否则整键省略（proxy.mjs:1307-1315）"
    );
    // x-session-id 头不受 UUID 限制，仍携带采信的 session 标识（≥8 字符）
    let h = cap.lock().unwrap().headers.clone().unwrap();
    assert_eq!(h.get("x-session-id").unwrap().to_str().unwrap(), "my-custom-session-key");

    // UUID 形态的 prompt_cache_key → 直接作为 threadId
    let (base2, cap2) = spawn().await;
    let req2: ChatRequest = serde_json::from_value(json!({
        "model": "commandcode/deepseek/deepseek-v4-flash",
        "prompt_cache_key": "123e4567-e89b-12d3-a456-426614174000",
        "messages": [{ "role": "user", "content": "hi" }]
    }))
    .unwrap();
    pv(base2).complete(&cred(), &route(), &req2).await.unwrap();
    assert_eq!(
        sent_body(&cap2)["threadId"],
        json!("123e4567-e89b-12d3-a456-426614174000")
    );
}

#[tokio::test]
async fn system_blocks_normalized_and_empty_text_with_cache_control_kept() {
    let (base, cap) = spawn().await;
    let req: ChatRequest = serde_json::from_value(json!({
        "model": "commandcode/deepseek/deepseek-v4-flash",
        "messages": [
            { "role": "system", "content": [
                { "type": "text", "text": "规则A" },
                // content 键兜底（proxy.mjs:539 text ?? content）
                { "type": "text", "content": "规则B" },
                // 空 text + cache_control 的块保留
                { "type": "text", "text": "", "cache_control": { "type": "ephemeral" } },
                // 空 text 无 cache_control → 跳过
                { "type": "text", "text": "" }
            ]},
            { "role": "user", "content": "hi" }
        ]
    }))
    .unwrap();
    pv(base).complete(&cred(), &route(), &req).await.unwrap();
    let system = sent_body(&cap)["params"]["system"].as_array().unwrap().clone();
    assert_eq!(system.len(), 3, "空 text 无断点的块被跳过");
    assert_eq!(system[0]["text"], json!("规则A\n"), "非最后一块补 \\n");
    assert_eq!(system[1]["text"], json!("规则B\n"), "content 键兜底提取（同样补 \\n）");
    assert_eq!(system[2]["text"], json!(""));
    assert_eq!(system[2]["cache_control"], json!({ "type": "ephemeral" }), "断点保留");
}

#[tokio::test]
async fn prompt_cache_key_breakpoint_skipped_when_message_marked() {
    let (base, cap) = spawn().await;
    let req: ChatRequest = serde_json::from_value(json!({
        "model": "commandcode/deepseek/deepseek-v4-flash",
        "prompt_cache_key": "pk-1234567890",
        "messages": [
            { "role": "system", "content": "你是助手" },
            { "role": "user", "content": [
                { "type": "text", "text": "hi", "cache_control": { "type": "ephemeral" } }
            ]}
        ]
    }))
    .unwrap();
    pv(base).complete(&cred(), &route(), &req).await.unwrap();
    let v = sent_body(&cap);
    // 消息块已带断点 → 不再往 system 最后一块补（hasCacheMarker 看全量消息，proxy.mjs:634-638）
    assert!(v["params"]["system"][0].get("cache_control").is_none());
    // 断点原样保留在消息块上
    assert_eq!(
        v["params"]["messages"][0]["content"][0]["cache_control"],
        json!({ "type": "ephemeral" })
    );

    // 无任何断点标记 + prompt_cache_key → system 最后一块补 ephemeral（<8 字符也触发：非空即可）
    let (base2, cap2) = spawn().await;
    let req2: ChatRequest = serde_json::from_value(json!({
        "model": "commandcode/deepseek/deepseek-v4-flash",
        "prompt_cache_key": "short",
        "messages": [
            { "role": "system", "content": "你是助手" },
            { "role": "user", "content": "hi" }
        ]
    }))
    .unwrap();
    pv(base2).complete(&cred(), &route(), &req2).await.unwrap();
    assert_eq!(
        sent_body(&cap2)["params"]["system"][0]["cache_control"],
        json!({ "type": "ephemeral" }),
        "prompt_cache_key 非空即触发断点（无长度门槛，proxy.mjs:639-641）"
    );
}

// ───────────────────── NDJSON 终态与重试口径 ─────────────────────

#[tokio::test]
async fn tool_call_event_missing_id_gets_fallback() {
    let (base, cap) = spawn().await;
    cap.lock().unwrap().ndjson = Some(
        [
            r#"{"type":"tool-call","toolName":"bash","input":{"cmd":"ls"}}"#.to_string(),
            r#"{"type":"tool-call","toolCallId":"","toolName":"sh","input":null}"#.to_string(),
            r#"{"type":"finish","finishReason":"stop","totalUsage":{"inputTokens":3,"outputTokens":2}}"#.to_string(),
        ]
        .join("\n"),
    );
    let mut s = pv(base).stream(&cred(), &route(), &plain_req()).await.unwrap();
    let mut ids = Vec::new();
    let mut args = Vec::new();
    while let Some(item) = s.next().await {
        if let StreamChunk::ToolCallDelta { id, arguments, .. } = item.expect("chunk") {
            ids.push(id.unwrap());
            args.push(arguments);
        }
    }
    // 兜底 call_{ms}_{index}（proxy.mjs:795）；空串同样视为缺省
    assert!(ids[0].starts_with("call_") && ids[0].matches('_').count() == 2, "{} 应为 call_ms_index 形态", ids[0]);
    assert!(ids[1].starts_with("call_") && ids[1].ends_with("_1"), "第二只 index=1：{}", ids[1]);
    assert_ne!(ids[0], ids[1]);
    assert_eq!(args[1], "{}", "input 假值（null）折空对象");
}

#[tokio::test]
async fn error_event_message_falls_back_to_top_level() {
    let (base, cap) = spawn().await;
    // 无 error 对象、message 在顶层（proxy.mjs:1019 采纳链）
    cap.lock().unwrap().ndjson = Some(
        [
            r#"{"type":"text-delta","text":"x"}"#.to_string(),
            r#"{"type":"error","message":"<503> upstream容量不足"}"#.to_string(),
            r#"{"type":"finish","finishReason":"stop","totalUsage":{"inputTokens":3,"outputTokens":2}}"#.to_string(),
        ]
        .join("\n"),
    );
    let err = pv(base).complete(&cred(), &route(), &plain_req()).await.unwrap_err();
    match err {
        ProviderError::Upstream(msg) => assert!(msg.contains("503"), "{msg}"),
        other => panic!("503 → Upstream：{other:?}"),
    }

    // error.message 的 <NNN> 前缀 > error.statusCode（坑 12）
    let (base2, cap2) = spawn().await;
    cap2.lock().unwrap().ndjson = Some(
        r#"{"type":"error","message":"顶层","error":{"message":"<429> 限流","statusCode":500}}"#.to_string(),
    );
    let err2 = pv(base2).complete(&cred(), &route(), &plain_req()).await.unwrap_err();
    match err2 {
        ProviderError::RateLimited { retry_after_secs, msg } => {
            assert_eq!(retry_after_secs, Some(30));
            assert!(msg.contains("限流"), "error.message 优先：{msg}");
        }
        other => panic!("应为 RateLimited：{other:?}"),
    }
}

#[tokio::test]
async fn http_500_status_not_retried() {
    let (base, cap) = spawn().await;
    cap.lock().unwrap().force_status = Some(500);
    let err = pv(base).complete(&cred(), &route(), &plain_req()).await.unwrap_err();
    assert!(matches!(err, ProviderError::Upstream(_)), "{err:?}");
    // HTTP 状态错是确定性响应，不重试（对齐参考 !ok 直接下发）
    assert_eq!(cap.lock().unwrap().hits, 1, "500 只打一发");
}

#[tokio::test]
async fn upstream_error_finish_reason_retried_before_first_byte() {
    let (base, cap) = spawn().await;
    {
        let mut c = cap.lock().unwrap();
        // 第一发：finish 带 network-error（provider 报连接失败）→ 可重试 502 语义
        c.responses = vec![
            [
                r#"{"type":"text-delta","text":"半"}"#,
                r#"{"type":"finish","finishReason":"network-error"}"#,
            ]
            .join("\n"),
        ];
    }
    let out = pv(base).complete(&cred(), &route(), &plain_req()).await.unwrap();
    assert!(out.choices[0].message.content.contains("好"));
    assert_eq!(cap.lock().unwrap().hits, 2, "upstream-error finishReason 与截断同等重试");
}

#[tokio::test]
async fn finish_without_reason_defaults_to_stop() {
    let (base, cap) = spawn().await;
    cap.lock().unwrap().ndjson = Some(
        [
            r#"{"type":"text-delta","text":"ok"}"#.to_string(),
            r#"{"type":"finish","totalUsage":{"inputTokens":3,"outputTokens":2}}"#.to_string(),
        ]
        .join("\n"),
    );
    let mut s = pv(base).stream(&cred(), &route(), &plain_req()).await.unwrap();
    let mut finish = None;
    while let Some(item) = s.next().await {
        if let StreamChunk::Finish { reason, .. } = item.expect("chunk") {
            finish = Some(reason);
        }
    }
    // finish 缺 finishReason → 'stop'（proxy.mjs:821 || 'stop'）
    assert_eq!(finish.as_deref(), Some("stop"));
}

#[tokio::test]
async fn catalog_fallback_has_26_models() {
    let (base, _) = spawn().await;
    let catalog = pv(base).catalog();
    assert_eq!(catalog.models.len(), 26, "MODELS 回退表 26 款（proxy.mjs:457-494）");
    let ids: Vec<&str> = catalog.models.iter().map(|m| m.id.as_str()).collect();
    assert!(ids.contains(&"deepseek/deepseek-v4-flash"));
    assert!(ids.contains(&"claude-sonnet-4-6"));
    assert!(ids.contains(&"google/gemini-3.1-flash-lite"));
}

#[test]
fn finish_reason_normalizes_trim_and_lowercase() {
    // mapFinishReason：trim + lowercase，空值 → stop（proxy.mjs:930-932）
    assert_eq!(map_finish_reason(""), "stop");
    assert_eq!(map_finish_reason("   "), "stop");
    assert_eq!(map_finish_reason(" Tool-Calls "), "tool_calls");
    assert_eq!(map_finish_reason("MAX_TOKENS"), "length");
    assert_eq!(map_finish_reason("Pause_Turn"), "pause_turn", "未知/保留值小写化后原样");
}

#[tokio::test]
async fn tools_wire_format_bare_fallback_and_defaults() {
    let (base, cap) = spawn().await;
    let req: ChatRequest = serde_json::from_value(json!({
        "model": "commandcode/deepseek/deepseek-v4-flash",
        "tools": [
            // 标准 OpenAI 形态（别名只作用于声明）
            { "type": "function", "function": { "name": "bash_output", "description": "跑", "parameters": { "type": "object" } } },
            // 裸工具形态：无 function 包装，回退顶层键（proxy.mjs:678-684）
            { "name": "裸工具", "input_schema": { "type": "object", "properties": {} } },
            // 缺 description/schema → 默认值补齐，三键恒下发
            { "type": "function", "function": { "name": "极简" } }
        ],
        "messages": [{ "role": "user", "content": "hi" }]
    }))
    .unwrap();
    pv(base).complete(&cred(), &route(), &req).await.unwrap();
    let tools = sent_body(&cap)["params"]["tools"].as_array().unwrap().clone();
    assert_eq!(tools.len(), 3);
    assert_eq!(tools[0]["name"], json!("shell_output"), "tools 声明走别名表（proxy.mjs:714-721）");
    assert_eq!(tools[0]["description"], json!("跑"));
    assert_eq!(tools[0].get("type"), None, "CC 线格无 type 字段");
    assert_eq!(tools[1]["name"], json!("裸工具"), "裸形态回退顶层 name");
    assert_eq!(tools[1]["input_schema"], json!({ "type": "object", "properties": {} }), "裸形态回退 input_schema");
    assert_eq!(tools[1]["description"], json!(""), "缺 description 补空串");
    assert_eq!(tools[2]["description"], json!(""));
    assert_eq!(tools[2]["input_schema"], json!({ "type": "object", "properties": {} }), "缺 schema 补空对象骨架");
}
