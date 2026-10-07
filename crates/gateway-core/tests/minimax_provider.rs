//! T4.8 minimax provider（缝 2：stub 上游）。
//! 行为来源：reference §4.11 + minimax-product.ts——
//! - pending 是 HTTP 200 + status:"pending"（不是 OAuth 标准 400）
//! - 签到 timezone_id 是 query 必填（放头回 1406010011 且 HTTP 200）
//! - points 总数 + bonus_points 是其中额外部分**不得相加**
//! - 余额 Σdetails[].remaining_amount（字符串、挡空串、total_count 不是余额）
//! - 推理头：**不需要 anthropic-version**

use std::sync::{Arc, Mutex};

use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use gateway_core::openai::ChatRequest;
use gateway_core::provider::Provider;
use gateway_core::providers::minimax::{MinimaxProvider, MINIMAX_CLIENT_ID, MINIMAX_SCOPE};
use gateway_core::Credential;
use serde_json::{json, Value};

#[derive(Default)]
struct Cap {
    device_body: Mutex<Option<String>>,
    token_bodies: Mutex<Vec<String>>,
    infer_headers: Mutex<Option<HeaderMap>>,
    signin_query: Mutex<Option<String>>,
    credit_query_hits: Mutex<usize>,
}

async fn stub_device_code(State(cap): State<Arc<Cap>>, body: String) -> Response {
    *cap.device_body.lock().unwrap() = Some(body);
    Json(json!({
        "device_code": "dc-123",
        "login_url": "https://account.minimax.cn/oauth-authorize?dc=dc-123"
    }))
    .into_response()
}

async fn stub_token(State(cap): State<Arc<Cap>>, RawQuery(_q): RawQuery, body: String) -> Response {
    cap.token_bodies.lock().unwrap().push(body.clone());
    // refresh 请求直接返回 token（不走 pending 轮询）
    if body.contains("grant_type=refresh_token") {
        return Json(json!({ "access_token": "mm-at", "refresh_token": "mm-rt" })).into_response();
    }
    let n = cap.token_bodies.lock().unwrap().len();
    if n < 3 {
        // pending 是 HTTP 200 + status:"pending"（不是 400）
        return Json(json!({ "status": "pending" })).into_response();
    }
    Json(json!({
        "access_token": "mm-at",
        "refresh_token": "mm-rt",
        "expires_in": 3600
    }))
    .into_response()
}

async fn stub_infer(State(cap): State<Arc<Cap>>, h: HeaderMap, _body: axum::extract::Request) -> Response {
    *cap.infer_headers.lock().unwrap() = Some(h.clone());
    let sse = concat!(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":5}}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"minimax 回复\"}}\n\n",
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":3}}\n\n",
    );
    (StatusCode::OK, [("content-type", "text/event-stream")], sse.to_string()).into_response()
}

async fn stub_signin(State(cap): State<Arc<Cap>>, RawQuery(q): RawQuery) -> Response {
    *cap.signin_query.lock().unwrap() = q;
    Json(json!({
        "base_resp": {"status_code": 0},
        "points": 800,
        "bonus_points": 400,
        "is_today": true,
        "status": 3
    }))
    .into_response()
}

async fn stub_credit(State(cap): State<Arc<Cap>>) -> Response {
    *cap.credit_query_hits.lock().unwrap() += 1;
    // 平铺无 data 键；remaining_amount 是字符串（含空串须挡）
    Json(json!({
        "total_count": 3,
        "details": [
            { "remaining_amount": "500.5" },
            { "remaining_amount": "300" },
            { "remaining_amount": "" }
        ]
    }))
    .into_response()
}

async fn spawn() -> (String, Arc<Cap>) {
    let cap = Arc::new(Cap::default());
    let app = Router::new()
        .route("/oauth2/device/code", post(stub_device_code))
        .route("/oauth2/token", post(stub_token))
        .route("/mavis/api/v1/llm/v1/messages", post(stub_infer))
        .route("/minimax-cloud/api/v1/signin/status", get(stub_signin))
        .route("/minimax-cloud/api/v1/credit/details", get(stub_credit))
        .with_state(cap.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub") });
    (format!("http://{addr}"), cap)
}

fn pv(base: &str) -> MinimaxProvider {
    MinimaxProvider::new(base.to_string(), base.to_string())
}

fn cred() -> Credential {
    Credential {
        account_id: "mm1".into(),
        secret: json!({ "access_token": "mm-at", "refresh_token": "mm-rt" }).to_string(),
    }
}

fn req() -> ChatRequest {
    serde_json::from_value(json!({
        "model": "minimax/MiniMax-M3",
        "messages": [{ "role": "user", "content": "hi" }]
    }))
    .unwrap()
}

fn route() -> gateway_core::route::Route {
    gateway_core::route::Route { provider: "minimax".into(), model: "MiniMax-M3".into() }
}

#[tokio::test]
async fn device_login_pending_is_200_then_token() {
    let (base, cap) = spawn().await;
    let p = pv(&base);
    let (dc, _url) = p.device_code_start().await.unwrap();
    assert_eq!(dc, "dc-123");
    // device_code 请求体含 client_id/scope
    let dev_body = cap.device_body.lock().unwrap().clone().unwrap();
    assert!(dev_body.contains(MINIMAX_CLIENT_ID), "{dev_body}");
    assert!(dev_body.contains(MINIMAX_SCOPE), "{dev_body}");

    // 轮询：前两次 pending（HTTP 200 + status:"pending"），第三次成功
    assert!(p.poll_token(&dc).await.unwrap().is_none(), "第 1 次 pending");
    assert!(p.poll_token(&dc).await.unwrap().is_none(), "第 2 次 pending");
    let cred = p.poll_token(&dc).await.unwrap().unwrap();
    let v: Value = serde_json::from_str(&cred.secret).unwrap();
    assert_eq!(v["access_token"], json!("mm-at"));
}

#[tokio::test]
async fn signin_timezone_id_in_query() {
    let (base, cap) = spawn().await;
    let signin = pv(&base).signin_status(&cred()).await.unwrap();
    // points=800 总数、bonus_points=400 是其中额外部分——**不得相加**
    assert_eq!(signin.points, 800);
    assert_eq!(signin.bonus_points, 400);
    assert!(signin.already_claimed, "is_today && status==3");
    // timezone_id 必须在 query 里
    let q = cap.signin_query.lock().unwrap().clone().unwrap();
    assert!(q.contains("timezone_id="), "timezone_id 在 query：{q}");
}

#[tokio::test]
async fn balance_sums_remaining_amount_strings() {
    let (base, _) = spawn().await;
    let bal = pv(&base).balance(&cred()).await.unwrap();
    // "500.5" + "300" + ""（挡空串）= 800（u64 取整）
    assert_eq!(bal.total_remaining, 800, "Σremaining_amount（字符串挡空串），total_count 不是余额");
}

#[tokio::test]
async fn infer_headers_no_anthropic_version() {
    let (base, cap) = spawn().await;
    let out = pv(&base).complete(&cred(), &route(), &req()).await.unwrap();
    assert!(out.choices[0].message.content.contains("minimax 回复"));
    let h = cap.infer_headers.lock().unwrap().clone().unwrap();
    assert!(h.get("authorization").is_some());
    assert!(
        h.get("anthropic-version").is_none(),
        "minimax 不需要 anthropic-version 头（实测）"
    );
    assert_eq!(h.get("accept").unwrap(), "text/event-stream");
}

#[tokio::test]
async fn refresh_rotates_token() {
    let (base, _) = spawn().await;
    // 第 4 次 token 请求（前 3 次被 device login 用了）——stub 已 queue 满
    // 直接调 refresh（走同一路径）
    let out = pv(&base).refresh(&cred()).await.unwrap();
    let v: Value = serde_json::from_str(&out.secret).unwrap();
    assert_eq!(v["access_token"], json!("mm-at"), "刷新成功");
}
