//! T4.2c thoughtSignature 跨轮回填（缝 2：stub 上游）。
//! 行为来源：gemini-messages.ts:651-657 / gemini-sigstore.ts——
//! - 响应中**只取 functionCall part 自身的** `thoughtSignature`（思考分片上的
//!   签名回传时被上游忽略，存了纯属噪音），按「工具名 + 按键名升序的紧凑
//!   JSON」为键存入独立 sigstore；
//! - 请求侧构造 functionCall part 时查表回填；精确 miss 时按工具名最近一次
//!   兜底；从未见过 → 不带签名；带签请求被拒 → 去签重试一次；
//! - sigstore 上限 2000 条淘汰最旧一半；落盘 tmp+rename 原子写。

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use futures::StreamExt;
use gateway_core::openai::ChatRequest;
use gateway_core::provider::{Provider, StreamChunk};
use gateway_core::providers::gemini::{GeminiProvider, SigStore, SIG_MAX_ENTRIES};
use gateway_core::route::Route;
use gateway_core::Credential;
use serde_json::{json, Value};

#[derive(Default)]
struct Cap {
    gen_count: usize,
    last_body: String,
    /// 每次推理请求是否带 thoughtSignature（按时间序）
    had_sigs: Vec<bool>,
}

async fn stub_generate(State(cap): State<Arc<Mutex<Cap>>>, body: axum::extract::Request) -> Response {
    let bytes = axum::body::to_bytes(body.into_body(), 8 << 20).await.unwrap();
    let raw = String::from_utf8_lossy(&bytes).to_string();
    let had_sig = raw.contains("thoughtSignature");
    {
        let mut c = cap.lock().unwrap();
        c.gen_count += 1;
        c.last_body = raw.clone();
        c.had_sigs.push(had_sig);
    }
    // 带签一律 400（模拟上游拒签）；无签正常回 functionCall 帧
    if had_sig {
        return (StatusCode::BAD_REQUEST, "signature rejected").into_response();
    }
    // 签名挂在 functionCall part 自身；前面的裸 thoughtSignature part 是噪音
    // （思考分片签名），不得入缓存（gemini-messages.ts:683-686）
    let frame = json!({
        "candidates": [{"content": {"parts": [
            { "thought": true, "text": "思考中", "thoughtSignature": "sig-noise" },
            { "functionCall": { "name": "get_weather", "args": { "city": "北京" } },
              "thoughtSignature": "sig-abc" }
        ], "role": "model"}}]
    });
    let sse = format!("data: {frame}\n\n");
    ([("content-type", "text/event-stream")], sse).into_response()
}

async fn spawn() -> (String, Arc<Mutex<Cap>>) {
    let cap = Arc::new(Mutex::new(Cap::default()));
    let app = Router::new()
        .route("/v1internal:streamGenerateContent", post(stub_generate))
        .route("/v1internal:loadCodeAssist", post(|| async { Json(json!({})) }))
        .with_state(cap.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub serve") });
    (format!("http://{addr}"), cap)
}

fn pv(base: String, sigs: Arc<SigStore>) -> GeminiProvider {
    GeminiProvider::new(base, "aicode-consumers".into()).with_sig_store(sigs)
}

fn cred() -> Credential {
    Credential { account_id: "g1".into(), secret: "oauth-token".into() }
}

fn route() -> Route {
    Route { provider: "gemini".into(), model: "gemini-3.8-flash".into() }
}

/// 历史里带 get_weather functionCall 的请求；args_city 控制规范化参数。
fn req_with_args(city: &str) -> ChatRequest {
    serde_json::from_value(json!({
        "model": "gemini/gemini-3.8-flash",
        "messages": [
            { "role": "user", "content": "天气怎么样" },
            { "role": "assistant", "content": "", "tool_calls": [{
                "id": "call_1", "type": "function",
                "function": { "name": "get_weather", "arguments": format!("{{\"city\":\"{city}\"}}") }
            }]},
            { "role": "tool", "tool_call_id": "call_1", "content": "晴" }
        ]
    }))
    .unwrap()
}

async fn drain(p: &GeminiProvider, req: &ChatRequest) {
    let mut s = p.stream(&cred(), &route(), req).await.unwrap();
    while let Some(item) = s.next().await {
        if let StreamChunk::Finish { .. } = item.expect("chunk") {
            break;
        }
    }
}

#[tokio::test]
async fn signature_recorded_from_function_call_part_only_and_replayed() {
    let (base, cap) = spawn().await;
    let sigs = Arc::new(SigStore::in_memory());
    let p = pv(base, sigs.clone());
    // 第一轮：响应 functionCall part 带签名 → 记录；思考分片的噪音签名不得入缓存
    drain(&p, &req_with_args("北京")).await;
    assert_eq!(sigs.len(), 1, "只存 functionCall part 自身的签名（噪音签名不入缓存）");
    // 第二轮：请求侧 functionCall 应带 thoughtSignature → 400 → 去签重试一次成功
    let out = p.complete(&cred(), &route(), &req_with_args("北京")).await.unwrap();
    assert!(out.choices[0].message.tool_calls.is_some());
    let c = cap.lock().unwrap();
    assert_eq!(
        c.had_sigs,
        vec![false, true, false],
        "轮1无签记录；轮2先带签被拒，再去签重试"
    );
}

#[tokio::test]
async fn unknown_args_falls_back_to_latest_signature_of_same_tool() {
    let (base, cap) = spawn().await;
    let p = pv(base, Arc::new(SigStore::in_memory()));
    drain(&p, &req_with_args("北京")).await;
    // 精确键 (get_weather,{city:北京}) miss（args 换上海）→ 按工具名兜底回填最近签名
    let _ = p.complete(&cred(), &route(), &req_with_args("上海")).await;
    let c = cap.lock().unwrap();
    assert_eq!(
        c.had_sigs,
        vec![false, true, false],
        "args 变更后仍须按工具名兜底带签（再被拒去签重试）"
    );
    // 重试后的最终 body 无签名键
    let v: Value = serde_json::from_str(&c.last_body).unwrap();
    let fc = &v["request"]["contents"][1]["parts"][0]["functionCall"];
    assert!(fc.get("thoughtSignature").is_none());
}

#[tokio::test]
async fn no_signature_key_when_never_seen() {
    let (base, cap) = spawn().await;
    let p = pv(base, Arc::new(SigStore::in_memory()));
    // 不先发过任何记录请求：直接第二轮
    let _ = p.complete(&cred(), &route(), &req_with_args("北京")).await;
    let c = cap.lock().unwrap();
    let v: Value = serde_json::from_str(&c.last_body).unwrap();
    let fc = &v["request"]["contents"][1]["parts"][0]["functionCall"];
    assert!(fc.get("thoughtSignature").is_none(), "从未见过签名不得臆造");
    assert_eq!(c.gen_count, 1, "无签请求不该被 400 拒，也不该重试");
    assert_eq!(c.had_sigs, vec![false]);
}

fn tempfile_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("tm-gemini-sigs-{tag}-{}", gateway_core::key::random_id(6)));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[tokio::test]
async fn sigstore_caps_at_2000_evicts_oldest_half_and_persists_atomically() {
    let dir = tempfile_dir("cap");
    let path = dir.join("gemini-sigs.json");
    let store = SigStore::load_or_create(path.clone());
    // 灌 2001 条：第 0 条用独立工具名（否则按名兜底会掩盖精确键 miss），
    // 第 2001 条触发淘汰最旧一半（gemini-sigstore.ts:175/185-192）
    store.record("solo_old", &json!({ "n": 0 }), "sig-old");
    for i in 1..=SIG_MAX_ENTRIES {
        store.record(
            "get_weather",
            &json!({ "city": format!("city-{i}") }),
            &format!("sig-{i}"),
        );
    }
    assert_eq!(store.len(), SIG_MAX_ENTRIES - 999, "2001 条淘汰 1000 条最旧的，剩 1001");
    // 最新一条精确键命中；最旧（独立工具名）精确键与按名兜底双双落空
    assert!(store.lookup("get_weather", &json!({ "city": "city-2000" })).is_some());
    assert!(store.lookup("solo_old", &json!({ "n": 0 })).is_none(), "最旧条目应已被淘汰");

    // 落盘：请求结束显式冲刷（dirty 标记语义）；原子写（无残留 .tmp），
    // 形状可被 load_or_create 复原
    store.flush();
    let raw = std::fs::read_to_string(&path).unwrap();
    let v: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(v.as_object().unwrap().len(), SIG_MAX_ENTRIES - 999);
    assert!(!dir.join("gemini-sigs.json.tmp").exists(), "tmp 文件应已被 rename 吃掉");
    let reloaded = SigStore::load_or_create(path);
    assert_eq!(reloaded.len(), SIG_MAX_ENTRIES - 999, "重载后条目数一致");
    assert!(reloaded.lookup("get_weather", &json!({ "city": "city-2000" })).is_some());
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn sigstore_latest_by_name_uses_insertion_order_and_overwrite_moves_to_end() {
    let store = SigStore::in_memory();
    // 同工具不同 args：按名兜底取最近一次写入（插入序，非时间戳）
    store.record("tool_a", &json!({ "n": 1 }), "sig-a1");
    store.record("tool_a", &json!({ "n": 2 }), "sig-a2");
    store.record("tool_b", &json!({ "n": 3 }), "sig-b1");
    // 精确键各自命中
    assert_eq!(store.lookup("tool_a", &json!({ "n": 1 })).as_deref(), Some("sig-a1"));
    // 未知 args 按名兜底 → tool_a 最近一次是 sig-a2
    assert_eq!(
        store.lookup("tool_a", &json!({ "n": 999 })).as_deref(),
        Some("sig-a2"),
        "按名兜底取插入序最后一个"
    );
    assert_eq!(store.lookup("tool_b", &json!({ "n": 999 })).as_deref(), Some("sig-b1"));
    // 空签名丢弃
    store.record("tool_c", &json!({ "n": 1 }), "   ");
    assert!(store.lookup("tool_c", &json!({ "n": 999 })).is_none());
    // 同键覆盖：值更新且挪到末尾（Map 插入序语义）
    store.record("tool_a", &json!({ "n": 2 }), "sig-a2-new");
    assert_eq!(store.lookup("tool_a", &json!({ "n": 2 })).as_deref(), Some("sig-a2-new"));
    assert_eq!(store.lookup("tool_a", &json!({ "n": 999 })).as_deref(), Some("sig-a2-new"));
}
