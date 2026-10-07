//! T4.20a qoder 设备码登录/续期/Cosy 头修正（缝 2：stub 上游）。
//! 行为来源（参考源码行号）：
//! - 设备码登录没有"从 API 拿 code"这一步：nonce/PKCE verifier/
//!   machine_id 全本地生成；challenge = base64url(sha256(verifier))
//!   去掉 padding（qoder.ts:63-67）。
//! - 授权 URL `GET {auth}/device/selectAccounts?challenge&challenge_method=
//!   S256&nonce&machine_id&client_id`（clientId e883ade2-…；prod 用错
//!   client_id 回调报"参数无效"）。
//! - 轮询 **GET** `{openapi}/api/v1/deviceToken/poll?nonce&verifier&
//!   challenge_method=S256`：404=未授权**继续轮询**；2xx 无 token 继续；
//!   其他非 2xx 立即抛错。
//! - 成功解析：token 取 `token`|`device_token`|`access_token` 三键；
//!   **user_id → uid（加密推理必需）**；凭据 `security_oauth_token` 与
//!   `access_token` 双写同值 + machine_id 随凭据持久化。
//! - refresh：POST `/api/v1/deviceToken/refresh` body {refresh_token,
//!   machine_id}；终态=401/403 或 200 无 token；回写保留 machine_id/uid。
//! - Cosy 头修正：ClientType 实测值 '10'（qoder-product.ts:454-455）。

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use gateway_core::providers::qoder::{
    build_qoder_auth_url, build_qoder_poll_url, QoderOAuth,
};
use gateway_core::Credential;
use serde_json::{json, Value};

#[derive(Default)]
struct Cap {
    poll_hits: Mutex<Vec<String>>, // query 串
    refresh_body: Mutex<Option<String>>,
    refresh_headers: Mutex<Option<axum::http::HeaderMap>>,
    usage_headers: Mutex<Option<axum::http::HeaderMap>>,
    /// 前两次 poll 404，第三次成功
    polls_before_ok: Mutex<usize>,
}

async fn stub_poll(State(cap): State<Arc<Cap>>, axum::extract::RawQuery(q): axum::extract::RawQuery) -> Response {
    let q = q.unwrap_or_default();
    cap.poll_hits.lock().unwrap().push(q.clone());
    let n = cap.poll_hits.lock().unwrap().len();
    let threshold = *cap.polls_before_ok.lock().unwrap();
    if n <= threshold {
        return (axum::http::StatusCode::NOT_FOUND, Json(json!({ "errorCode": "NotFound" }))).into_response();
    }
    Json(json!({
        "token": "at-login",
        "refresh_token": "rt-1",
        "expires_at": 1893456000i64,
        "user_id": "u-777",
        "user_name": "qoder-user"
    })).into_response()
}

async fn stub_refresh(State(cap): State<Arc<Cap>>, headers: axum::http::HeaderMap, req: axum::extract::Request) -> Response {
    let bytes = axum::body::to_bytes(req.into_body(), 1 << 20).await.unwrap();
    *cap.refresh_body.lock().unwrap() = Some(String::from_utf8_lossy(&bytes).to_string());
    *cap.refresh_headers.lock().unwrap() = Some(headers);
    Json(json!({ "device_token": "at-refresh", "expires_at": 1893456000i64 }))
        .into_response()
}

async fn stub_usage(State(cap): State<Arc<Cap>>, headers: axum::http::HeaderMap) -> Response {
    *cap.usage_headers.lock().unwrap() = Some(headers);
    Json(json!({ "data": { "userQuota": [{ "remaining": 5 }], "addOnQuota": [] } })).into_response()
}

async fn spawn() -> (String, Arc<Cap>) {
    let cap = Arc::new(Cap::default());
    *cap.polls_before_ok.lock().unwrap() = 2;
    let app = Router::new()
        .route("/api/v1/deviceToken/poll", get(stub_poll))
        .route("/api/v1/deviceToken/refresh", post(stub_refresh))
        .route("/sash/api/v2/me/usage", get(stub_usage))
        .with_state(cap.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub") });
    (format!("http://{addr}"), cap)
}

fn oauth(openapi: String) -> QoderOAuth {
    QoderOAuth::new_for_test(openapi)
}

#[tokio::test]
async fn device_login_polls_404_then_builds_credential() {
    let (openapi, cap) = spawn().await;
    let oa = oauth(openapi.clone());
    // 本地会话
    let sess = oa.create_device_session();
    let url = build_qoder_auth_url(&sess);
    assert!(
        url.starts_with("https://qoder.com/device/selectAccounts?"),
        "授权 URL：{url}"
    );
    for expect in ["challenge_method=S256", "nonce=", "machine_id=", "client_id=e883ade2-e6e3-4d6d-adf7-f92ceff5fdcb"] {
        assert!(url.contains(expect), "授权 URL 缺 {expect}：{url}");
    }
    // challenge 是 base64url(sha256(verifier)) 无 padding（43 字符）
    assert!(!sess.challenge.contains('=') && !sess.challenge.contains('+') && !sess.challenge.contains('/'));
    assert!(sess.verifier.len() >= 43 && sess.verifier.len() <= 128, "verifier 长度 43..128：{}", sess.verifier.len());

    // 轮询：前两次 404 继续轮询，第三次成功
    let cred = oa.poll_until_token(&sess, std::time::Duration::from_millis(10)).await.unwrap();
    let v: Value = serde_json::from_str(&cred.secret).unwrap();
    assert_eq!(v["access_token"], json!("at-login"));
    assert_eq!(v["security_oauth_token"], json!("at-login"), "双写同值");
    assert_eq!(v["refresh_token"], json!("rt-1"));
    assert_eq!(v["uid"], json!("u-777"), "user_id → uid（加密推理必需）");
    assert_eq!(v["machine_id"], json!(sess.machine_id), "machine_id 随凭据持久化");
    assert_eq!(cap.poll_hits.lock().unwrap().len(), 3, "404 继续轮询");

    let first_query = cap.poll_hits.lock().unwrap()[0].clone();
    assert!(first_query.contains("nonce=") && first_query.contains("verifier=") && first_query.contains("challenge_method=S256"), "{first_query}");
    let _ = build_qoder_poll_url(&sess);
}

#[tokio::test]
async fn refresh_keeps_identity_and_rotates_token() {
    let (openapi, cap) = spawn().await;
    let old = Credential {
        account_id: "q1".into(),
        secret: json!({
            "access_token": "at-old",
            "security_oauth_token": "at-old",
            "refresh_token": "rt-old",
            "machine_id": "mid-1",
            "uid": "u-777",
            "nickname": "nick"
        })
        .to_string(),
    };
    let out = oauth(openapi).refresh(&old).await.unwrap();
    let v: Value = serde_json::from_str(&out.secret).unwrap();
    assert_eq!(v["access_token"], json!("at-refresh"), "device_token 也认");
    assert_eq!(v["machine_id"], json!("mid-1"), "续期保留 machine_id");
    assert_eq!(v["uid"], json!("u-777"), "续期保留 uid");
    assert_eq!(v["nickname"], json!("nick"));
    let body = cap.refresh_body.lock().unwrap().clone().unwrap();
    assert!(body.contains("\"refresh_token\":\"rt-old\"") && body.contains("\"machine_id\":\"mid-1\""), "{body}");
    let h = cap.refresh_headers.lock().unwrap().clone().unwrap();
    assert!(h.get("user-agent").unwrap().to_str().unwrap().starts_with("qoder/"), "UA qoder/1.0.0");
}

#[tokio::test]
async fn cosy_client_type_is_ten() {
    let (openapi, cap) = spawn().await;
    let cred = Credential {
        account_id: "q1".into(),
        secret: json!({ "access_token": "at-9", "machine_id": "mid" }).to_string(),
    };
    gateway_core::providers::qoder::QoderProvider::new(openapi)
        .balance(&cred)
        .await
        .unwrap();
    let h = cap.usage_headers.lock().unwrap().clone().unwrap();
    assert_eq!(h.get("cosy-client-type").unwrap(), "10", "实测值 '10'（qoder-product.ts:454）");
}
