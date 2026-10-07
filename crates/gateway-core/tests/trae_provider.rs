//! T4.4a trae 凭据/登录/余额（缝 2：stub 上游）。
//! 行为来源：docs/reference/deepseek-harness-codearts.md §4.7——
//! - 鉴权头 `traeSOLOHeaders`：`Authorization: Cloud-IDE-JWT <token>` +
//!   `X-Cloudide-Token`/`X-Ide-Token`（同 token）+ `X-Uid` + `X-Device-Type:
//!   macos` + `Request-Traffic-Type: prod` + `X-Machine-Id`/`X-Device-Id`
//!   （32 hex，凭据持久化；machine_id 续期绝不可重生成）。
//! - 登录本地回调 `127.0.0.1:18080`（占用自动回退随机端口），回调参数名
//!   `auth_callback_url`；两套流程都要认——老流程回传 token
//!   （refreshToken/userInfo/userJwt），新流程 PKCE（code/authCodeInfo，
//!   识别后给精确报错）；userInfo 中文昵称双重编码乱码自动回转。
//! - 余额 `POST /trae/api/v2/pay/ide_user_ent_usage`，body
//!   `{"require_usage":true,"req_source":2}`（缺 require_usage 则 usage 恒 0
//!   余额虚高）；余额 = Σ(credits_limit − credits_amount)。
//!
//! ⚠️ SOLO 推理通道（transformToSOLOBody / SOLO SSE 事件形态）手册仅概要级
//! （无 wire 样例），阻塞待用户提供——见 TASKS.md T4.4 备注。

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use gateway_core::providers::trae::{TraeLoginFlow, TraeProvider};
use gateway_core::Credential;
use serde_json::{json, Value};

#[derive(Default)]
struct Cap {
    bal_body: Option<String>,
}

async fn stub_balance(State(cap): State<Arc<Mutex<Cap>>>, body: axum::extract::Request) -> Response {
    let bytes = axum::body::to_bytes(body.into_body(), 1 << 20).await.unwrap();
    cap.lock().unwrap().bal_body = Some(String::from_utf8_lossy(&bytes).to_string());
    (
        StatusCode::OK,
        Json(json!({
            "code": 0,
            "data": {
                "user_entitlement_pack_list": [
                    {
                        "entitlement_base_info": { "quota": { "credits_limit": 100 } },
                        "usage": { "credits_amount": 30 },
                        "expire_time": 1893456000
                    },
                    {
                        "entitlement_base_info": { "quota": { "credits_limit": 50 } },
                        "usage": { "credits_amount": 10 },
                        "expire_time": 1893456001
                    }
                ]
            }
        })),
    )
        .into_response()
}

async fn spawn_balance_stub() -> (String, Arc<Mutex<Cap>>) {
    let cap = Arc::new(Mutex::new(Cap::default()));
    let app = Router::new()
        .route("/trae/api/v2/pay/ide_user_ent_usage", post(stub_balance))
        .with_state(cap.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub serve") });
    (format!("http://{addr}"), cap)
}

fn secret() -> String {
    json!({
        "access_token": "jwt-tok",
        "refresh_token": "rt-1",
        "uid": "u-9527",
        "machine_id": "0123456789abcdef0123456789abcdef",
        "device_id": "fedcba9876543210fedcba9876543210"
    })
    .to_string()
}

fn cred() -> Credential {
    Credential { account_id: "t1".into(), secret: secret() }
}

#[tokio::test]
async fn balance_posts_required_body_and_sums_remaining() {
    let (base, cap) = spawn_balance_stub().await;
    let p = TraeProvider::new(base);
    let bal = p.balance(&cred()).await.unwrap();
    // Σ(credits_limit − credits_amount) = (100−30)+(50−10) = 110
    assert_eq!(bal.total, 110);
    let body = cap.lock().unwrap().bal_body.clone().unwrap();
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["require_usage"], json!(true), "缺 require_usage 则 usage 恒 0 余额虚高");
    assert_eq!(v["req_source"], json!(2));
}

#[tokio::test]
async fn solo_headers_carry_jwt_uid_and_device_identity() {
    let (base, _) = spawn_balance_stub().await;
    let p = TraeProvider::new(base);
    let h = p.solo_headers(&cred());
    assert_eq!(h.get("authorization").unwrap(), "Cloud-IDE-JWT jwt-tok");
    assert_eq!(h.get("x-cloudide-token").unwrap(), "jwt-tok");
    assert_eq!(h.get("x-ide-token").unwrap(), "jwt-tok");
    assert_eq!(h.get("x-uid").unwrap(), "u-9527");
    assert_eq!(h.get("x-device-type").unwrap(), "macos");
    assert_eq!(h.get("request-traffic-type").unwrap(), "prod");
    assert_eq!(h.get("x-machine-id").unwrap(), "0123456789abcdef0123456789abcdef");
    assert_eq!(h.get("x-device-id").unwrap(), "fedcba9876543210fedcba9876543210");
}

#[tokio::test]
async fn old_flow_callback_builds_credential_with_persistent_ids() {
    // 随机端口避免与其他并行测试抢协议默认 18080（start() 保持 18080 优先）
    let flow = TraeLoginFlow::start_with_port(0).await;
    let url = flow.authorize_url("https://login.trae.cn/whatever");
    assert!(
        url.contains("auth_callback_url=http%3A%2F%2F127.0.0.1%3A"),
        "回调参数名必须是 auth_callback_url（回环地址，端口可回退）：{url}"
    );
    // 模拟老流程回调：userJwt/refreshToken/userInfo（userInfo 双重编码中文昵称）
    let cb = url
        .split("auth_callback_url=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .replace("%3A", ":")
        .replace("%2F", "/");
    let user_info = urlencoding_of(r#"{"nickName":"%E5%BC%80%E5%8F%91%E8%80%85","uid":"u-1"}"#);
    let http = reqwest::Client::new();
    let resp = http
        .get(format!("{cb}?userJwt=jwt-1&refreshToken=rt-9&userInfo={user_info}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let cred = flow.wait_credential().await.unwrap();
    let v: Value = serde_json::from_str(&cred.secret).unwrap();
    assert_eq!(v["access_token"], json!("jwt-1"));
    assert_eq!(v["refresh_token"], json!("rt-9"));
    assert_eq!(v["uid"], json!("u-1"), "uid 优先取 userInfo.uid");
    // 中文昵称双重编码乱码自动回转
    assert_eq!(v["nickname"], json!("开发者"), "昵称回转：{}", v["nickname"]);
    // machine_id/device_id 32hex 一次性生成
    assert_eq!(v["machine_id"].as_str().unwrap().len(), 32);
    assert_eq!(v["device_id"].as_str().unwrap().len(), 32);
    assert!(v["machine_id"].as_str().unwrap().chars().all(|c| c.is_ascii_hexdigit()));
}

#[tokio::test]
async fn new_pkce_flow_gives_precise_error() {
    let flow = TraeLoginFlow::start_with_port(0).await;
    let cb = flow.callback_base();
    let http = reqwest::Client::new();
    let resp = http
        .get(format!("{cb}?code=abc&authCodeInfo=%7B%22pkce%22%3Atrue%7D"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "新流程识别后给精确报错");
    let err = flow.wait_credential().await.unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("PKCE") || msg.contains("新流程"), "精确指出新流程：{msg}");
}

/// query 值的 percent-encoding（测试辅助）。
fn urlencoding_of(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
