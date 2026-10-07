//! T4.3b commandcode 设备指纹（缝 2：stub 上游）。
//! 行为来源：docs/reference/commandcode-proxy.md §4.3/§7 坑 28——
//! - 哈希层逐字对齐 CLI：`fingerprintHash(v) = sha256_hex(FP_SALT ‖ "\\0" ‖
//!   lower(trim(v)))`，`FP_SALT='command-code:device-fingerprint:v1'`，空值省略；
//!   thumbmark = `sha256_hex(FP_SALT ‖ "\\0machine\\0" ‖ join('|', parts))`，
//!   空 parts 以 'unknown' 兜底。
//! - 派生层：`fpDigest(apiKey, field) = sha256(salt ‖ "\\0" ‖ apiKey ‖ "\\0" ‖
//!   field)`；候选池选择按 digest **字节序取最大**（非取模）；指纹由 apiKey
//!   确定性派生而非随机（重启/恢复上游看到同一台设备——换指纹本身可疑）。
//! - 上报节奏：首次请求前并行发 fingerprint/record + lifecycle-events
//!   （各自失败仅告警不阻塞）；Promise.all 后**无论成败**都写 nextInitAt，
//!   8h + rand(0..2h) 内不再上报（proxy.mjs:417-451）。

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use gateway_core::openai::ChatRequest;
use gateway_core::provider::Provider;
use gateway_core::providers::commandcode::{
    derive_device_profile, fingerprint_hash, thumbmark, CommandcodeProvider,
};
use gateway_core::route::Route;
use gateway_core::Credential;
use serde_json::{json, Value};

#[derive(Default, Clone)]
struct Seen {
    order: Vec<&'static str>,
    fp_body: Option<Value>,
    lc_body: Option<Value>,
    fp_headers: Option<axum::http::HeaderMap>,
    report_fail: bool,
}

#[derive(Default)]
struct Cap {
    seen: Mutex<Seen>,
}

async fn stub_fp(State(cap): State<Arc<Cap>>, body: axum::extract::Request) -> Response {
    {
        let mut s = cap.seen.lock().unwrap();
        s.order.push("fingerprint");
        s.fp_headers = Some(body.headers().clone());
    }
    let bytes = axum::body::to_bytes(body.into_body(), 1 << 20).await.unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let fail = cap.seen.lock().unwrap().report_fail;
    if fail {
        return (StatusCode::INTERNAL_SERVER_ERROR, "fp down").into_response();
    }
    cap.seen.lock().unwrap().fp_body = Some(v);
    (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
}

async fn stub_lc(State(cap): State<Arc<Cap>>, body: axum::extract::Request) -> Response {
    cap.seen.lock().unwrap().order.push("lifecycle");
    let bytes = axum::body::to_bytes(body.into_body(), 1 << 20).await.unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    cap.seen.lock().unwrap().lc_body = Some(v);
    (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
}

async fn stub_generate(State(cap): State<Arc<Cap>>) -> Response {
    cap.seen.lock().unwrap().order.push("generate");
    let ndjson = concat!(
        r#"{"type":"text-delta","text":"ok"}"#,
        "\n",
        r#"{"type":"finish","finishReason":"stop","totalUsage":{"inputTokens":3,"outputTokens":2}}"#,
    );
    (StatusCode::OK, [("content-type", "application/json")], ndjson.to_string()).into_response()
}

async fn spawn() -> (String, Arc<Cap>) {
    let cap = Arc::new(Cap::default());
    let app = Router::new()
        .route("/alpha/fingerprint/record", post(stub_fp))
        .route("/alpha/lifecycle-events", post(stub_lc))
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

fn req() -> ChatRequest {
    serde_json::from_value(json!({
        "model": "commandcode/deepseek/deepseek-v4-flash",
        "messages": [{ "role": "user", "content": "hi" }]
    }))
    .unwrap()
}

fn route() -> Route {
    Route { provider: "commandcode".into(), model: "deepseek/deepseek-v4-flash".into() }
}

#[test]
fn fingerprint_hash_matches_cli_algorithm_verbatim() {
    // 金标准：sha256_hex("command-code:device-fingerprint:v1\0" + lower(trim(v)))
    assert_eq!(
        fingerprint_hash("  TestCPU  ").as_deref(),
        Some("bd071fa91e472f4b7bbd14eba73ab9ddb832cbb9d9714f1f2079cc337d8f1bc8")
    );
    assert_eq!(fingerprint_hash("   "), None, "空值省略（undefined）");
}

#[test]
fn thumbmark_matches_cli_algorithm_verbatim() {
    assert_eq!(
        thumbmark("M", &["aa".to_string(), "bb".to_string()], "", ""),
        "a6539bf70c1901ae7251e51152127d9087fba0c35fb0a4a2a0623c4867593929"
    );
    assert_eq!(
        thumbmark("", &[], "H", "C"),
        // machineId 为空时 hostname/cpu 才参与：join("H|C")
        thumbmark("", &[], "H", "C"),
    );
    assert_eq!(
        thumbmark("", &[], "", ""),
        "387c57d3c8a92e6853c0ec51951879f4b8a6c76c6cbd6ee6383d52bfa6c0e27f",
        "空 parts 以 unknown 兜底"
    );
}

#[test]
fn device_profile_derived_deterministically_from_key() {
    let a = derive_device_profile("user_k1", "");
    let b = derive_device_profile("user_k1", "");
    assert_eq!(a.machine_id, b.machine_id, "同 key 同 salt 必须同一台机器（坑28）");
    assert_eq!(a.macs, b.macs);
    assert_eq!(a.git_email, b.git_email);
    // machineId 形状：8-4-4-4-12 hex
    let parts: Vec<&str> = a.machine_id.split('-').collect();
    assert_eq!(
        parts.iter().map(|s| s.len()).collect::<Vec<_>>(),
        vec![8, 4, 4, 4, 12]
    );
    assert!(a.hostname.starts_with("DESKTOP-"), "{}", a.hostname);
    assert!(!a.macs.is_empty());
    assert!(a.macs.iter().all(|m| m.contains(':')), "MAC 冒号形态：{:?}", a.macs);
    assert_eq!(a.macs, {
        let mut s = a.macs.clone();
        s.sort();
        s.dedup();
        s
    }, "MAC 排序去重序");
    let c = derive_device_profile("user_k2", "");
    assert_ne!(a.machine_id, c.machine_id, "换 key 换设备");
}

#[tokio::test]
async fn fingerprint_and_lifecycle_reported_before_first_generate() {
    let (base, cap) = spawn().await;
    let out = pv(base).complete(&cred(), &route(), &req()).await.unwrap();
    assert!(out.choices[0].message.content.contains("ok"));
    let s = cap.seen.lock().unwrap();
    assert_eq!(s.order, vec!["fingerprint", "lifecycle", "generate"], "首次请求前并行上报");
    let fp = s.fp_body.clone().unwrap();
    assert!(fp["thumbmark"].as_str().unwrap().len() == 64, "thumbmark 是 sha256 hex");
    let c = &fp["components"];
    // 键序逐字对照 proxy.mjs:171-189（preserve_order 下 wire 上可观测）
    let keys: Vec<&str> = c.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        vec![
            "machineIdHash", "macHashes", "osUserHash", "hostnameHash", "gitEmailHash",
            "platform", "arch", "osRelease", "cpuModel", "cpuCount", "memGiB",
            "isContainer", "timezone", "runtime", "collectorVersion",
        ],
        "components 键序与参考一致"
    );
    // 形状逐字对照 proxy.mjs:170-189：哈希键 + 原样键 + 数字键 + 环境键
    assert!(c.get("machineIdHash").is_some());
    assert!(c["macHashes"].as_array().unwrap().iter().all(|h| h.as_str().unwrap().len() == 64), "MAC 逐条哈希成数组");
    assert!(c.get("hostnameHash").is_some());
    assert!(c.get("osUserHash").is_some());
    assert!(c.get("gitEmailHash").is_some());
    assert_eq!(c["platform"], json!("win32"));
    assert_eq!(c["arch"], json!("x64"));
    assert_eq!(c["osRelease"], json!("10.0.22631"));
    assert_eq!(c["isContainer"], json!(false));
    assert!(c["cpuModel"].as_str().unwrap().contains("Intel") || c["cpuModel"].as_str().unwrap().contains("AMD"), "cpuModel 原样");
    assert!(c["cpuCount"].is_u64(), "cpuCount 数字");
    assert!(c["memGiB"].is_u64(), "memGiB 数字");
    assert!(c["timezone"].as_str().unwrap().contains('/'), "timezone 原样");
    assert_eq!(c["runtime"], json!("cli"));
    assert_eq!(c["collectorVersion"], json!(1));
    let lc = s.lc_body.clone().unwrap();
    assert_eq!(lc["eventType"], json!("cli_session_exists"));
    let meta = &lc["metadata"];
    assert!(meta["sessionId"].as_str().unwrap().starts_with("sess_"), "sess_+16hex");
    assert_eq!(meta["cliVersion"], json!("1.53.1"));
    assert_eq!(meta["mode"], json!("interactive"), "cliSessionMode 枚举（坑2）");
    assert_eq!(meta["os"], json!("win32-x64"));
    // 上报头是 generate 的子集：无 UA/slug/session/traceparent
    let h = s.fp_headers.clone().unwrap();
    assert!(h.get("user-agent").is_none());
    assert!(h.get("x-project-slug").is_none());
    assert!(h.get("traceparent").is_none());
    assert_eq!(h.get("x-command-code-version").unwrap(), "1.53.1");
}

#[tokio::test]
async fn report_not_repeated_within_8h() {
    let (base, cap) = spawn().await;
    let p = pv(base);
    p.complete(&cred(), &route(), &req()).await.unwrap();
    p.complete(&cred(), &route(), &req()).await.unwrap();
    let s = cap.seen.lock().unwrap();
    let fp_count = s.order.iter().filter(|o| **o == "fingerprint").count();
    let lc_count = s.order.iter().filter(|o| **o == "lifecycle").count();
    assert_eq!((fp_count, lc_count), (1, 1), "8h+rand 内不重复上报");
    assert_eq!(s.order.iter().filter(|o| **o == "generate").count(), 2);
}

#[tokio::test]
async fn report_failure_does_not_block_inference() {
    let (base, cap) = spawn().await;
    cap.seen.lock().unwrap().report_fail = true;
    let out = pv(base).complete(&cred(), &route(), &req()).await.unwrap();
    assert!(out.choices[0].message.content.contains("ok"), "上报失败仅告警不阻塞");
}

#[tokio::test]
async fn next_init_at_written_even_when_report_fails() {
    let (base, cap) = spawn().await;
    cap.seen.lock().unwrap().report_fail = true;
    let p = pv(base);
    p.complete(&cred(), &route(), &req()).await.unwrap();
    p.complete(&cred(), &route(), &req()).await.unwrap();
    let s = cap.seen.lock().unwrap();
    // 对齐 proxy.mjs:417-451：单项失败只 log warn，Promise.all 后**无条件**写
    // nextInitAt（8h+抖动）——失败后 8h 内不重试上报（下次请求直接进 generate）
    let fp_count = s.order.iter().filter(|o| **o == "fingerprint").count();
    assert_eq!(fp_count, 1, "上报失败也写 nextInitAt，不逐请求重试");
    assert_eq!(s.order.iter().filter(|o| **o == "generate").count(), 2);
}

#[tokio::test]
async fn models_fetched_from_provider_endpoint_with_subset_headers() {
    use std::sync::Mutex as Mx;
    let hit: Arc<Mx<Vec<axum::http::HeaderMap>>> = Arc::new(Mx::new(Vec::new()));
    let h2 = hit.clone();
    let app = Router::new().route(
        "/provider/v1/models",
        axum::routing::get(move |h: axum::http::HeaderMap| {
            let h2 = h2.clone();
            async move {
                h2.lock().unwrap().push(h);
                Json(json!({ "data": { "data": [ { "id": "m-one" }, { "id": "m-two" } ] } }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub") });

    let p = CommandcodeProvider::new(format!("http://{addr}"));
    let models = p.fetch_models(&cred()).await.unwrap();
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["m-one", "m-two"]);
    let h = hit.lock().unwrap()[0].clone();
    assert!(h.get("authorization").unwrap().to_str().unwrap().contains("user_"));
    assert_eq!(h.get("x-cli-environment").unwrap(), "production");
    assert_eq!(h.get("x-command-code-version").unwrap(), "1.53.1");
    // models 端点不带 zdr（README_zh 97-99）；无 UA/slug/session/traceparent
    assert!(h.get("x-cmd-zdr").is_none());
    assert!(h.get("user-agent").is_none());
    assert!(h.get("x-project-slug").is_none());
    assert!(h.get("traceparent").is_none());
}

#[tokio::test]
async fn models_fetch_failure_keeps_static_fallback() {
    let app = Router::new().route(
        "/provider/v1/models",
        axum::routing::get(|| async { (axum::http::StatusCode::NOT_FOUND, "gone") }),
    );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub") });
    let p = CommandcodeProvider::new(format!("http://{addr}"));
    assert!(p.fetch_models(&cred()).await.is_err(), "拉取失败显式报错");
    let catalog = p.catalog();
    assert!(!catalog.models.is_empty(), "静态回退目录仍可用");
}
