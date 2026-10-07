//! T4.4a trae 凭据/登录/余额（缝 2：stub 上游）。
//! 行为来源：参考源码 deepseek-harness-codearts/src/trae.ts / trae-credits.ts——
//! - 鉴权头 `traeSOLOHeaders`：`Authorization: Cloud-IDE-JWT <token>` +
//!   `X-Cloudide-Token`/`X-Ide-Token`（同 token）+ `X-Uid` + `X-Device-Type:
//!   macos` + `Request-Traffic-Type: prod` + `X-Machine-Id`/`X-Device-Id`
//!   （32 hex，凭据持久化；machine_id 续期绝不可重生成）。
//! - 登录本地回调 `127.0.0.1:18080`（占用自动回退随机端口），回调参数名
//!   `auth_callback_url`；两套流程都要认——老流程回传 token
//!   （refreshToken/userInfo/userJwt），新流程 PKCE（code/authCodeInfo，
//!   识别后给精确报错）；userInfo 中文昵称双重编码乱码自动回转。
//! - 余额 `POST /trae/api/v2/pay/ide_user_ent_usage`，body
//!   `{"require_usage":true,"req_source":2}`（缺 require_usage 则 usage
//!   恒 0 余额虚高）；余额 = Σ(credits_limit − credits_amount)，
//!   `user_entitlement_pack_list` 在**顶层**（trae-credits.ts:330 权威）；
//!   credits_limit<=0 的包跳过（trae-credits.ts:356）。

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use gateway_core::providers::trae::{TraeLoginFlow, TraeProvider};
use gateway_core::Credential;
use serde_json::{json, Value};

#[derive(Default)]
struct Cap {
    bal_body: Option<String>,
    /// 余额响应（可切换形状）。
    bal_resp: Option<String>,
}

async fn stub_balance(State(cap): State<Arc<Mutex<Cap>>>, body: axum::extract::Request) -> Response {
    let bytes = axum::body::to_bytes(body.into_body(), 1 << 20).await.unwrap();
    let bal_resp = {
        let mut c = cap.lock().unwrap();
        c.bal_body = Some(String::from_utf8_lossy(&bytes).to_string());
        c.bal_resp.clone()
    };
    // 权威形状（trae-credits.ts:330）：user_entitlement_pack_list 在顶层。
    let default_resp = json!({
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
    })
    .to_string();
    let resp = bal_resp.unwrap_or(default_resp);
    (StatusCode::OK, [("content-type", "application/json")], resp).into_response()
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
async fn balance_skips_zero_limit_packs() {
    // credits_limit<=0 的包不计入（trae-credits.ts:356）——不能让 0 上限的
    // 残留条目用 usage 拉低总额。
    let (base, cap) = spawn_balance_stub().await;
    cap.lock().unwrap().bal_resp = Some(
        json!({
            "user_entitlement_pack_list": [
                { "entitlement_base_info": { "quota": { "credits_limit": 100 } }, "usage": { "credits_amount": 40 } },
                { "entitlement_base_info": { "quota": { "credits_limit": 0 } }, "usage": { "credits_amount": 999 } }
            ]
        })
        .to_string(),
    );
    let bal = TraeProvider::new(base).balance(&cred()).await.unwrap();
    assert_eq!(bal.total, 60, "0 上限包的 usage 不得拉低余额");
}

#[tokio::test]
async fn balance_accepts_data_wrapped_shape_as_fallback() {
    // 宽松回退：data 包裹形态也能解析（权威是顶层，trae-credits.ts:330）。
    let (base, cap) = spawn_balance_stub().await;
    cap.lock().unwrap().bal_resp = Some(
        json!({
            "code": 0,
            "data": {
                "user_entitlement_pack_list": [
                    { "entitlement_base_info": { "quota": { "credits_limit": 80 } }, "usage": { "credits_amount": 30 } }
                ]
            }
        })
        .to_string(),
    );
    let bal = TraeProvider::new(base).balance(&cred()).await.unwrap();
    assert_eq!(bal.total, 50);
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
    let url = flow.authorize_url("https://www.trae.cn/authorization");
    assert!(
        url.contains("auth_callback_url=http%3A%2F%2F127.0.0.1%3A"),
        "回调参数名必须是 auth_callback_url（回环地址，端口可回退）：{url}"
    );
    // 完整 17 参数集（trae-oauth.ts:105-126）：缺参登录页会「停在授权中」。
    for param in [
        "login_version=1",
        "auth_from=solo",
        "login_channel=native_ide",
        "auth_type=local",
        "client_id=en1oxy7wnw8j9n",
        "redirect=0",
        "login_trace_id=",
        "machine_id=",
        "device_id=",
        "x_device_id=",
        "x_machine_id=",
        "x_device_brand=PC",
        "x_device_type=PC",
        "x_os_version=1.0",
        "x_app_version=0.1.52",
        "x_app_type=stable",
        "plugin_version=2.3.62834",
    ] {
        assert!(url.contains(param), "授权 URL 缺参数 {param}：{url}");
    }
    let url_machine_id = url.split("machine_id=").nth(1).unwrap().split('&').next().unwrap().to_string();
    // 模拟老流程回调：userJwt（裸 token）/refreshToken/userInfo（权威字段名
    // UserID/ScreenName/TenantID；ScreenName 双重编码中文昵称）
    let cb = url
        .split("auth_callback_url=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .replace("%3A", ":")
        .replace("%2F", "/");
    assert!(cb.ends_with("/authorize"), "回调路径 /authorize（trae.ts:65）：{cb}");
    let user_info = urlencoding_of(r#"{"UserID":"u-1","ScreenName":"%E5%BC%80%E5%8F%91%E8%80%85","TenantID":"ten-1"}"#);
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
    assert_eq!(v["uid"], json!("u-1"), "uid 优先取 userInfo.UserID");
    // 中文昵称双重编码乱码自动回转
    assert_eq!(v["nickname"], json!("开发者"), "昵称回转：{}", v["nickname"]);
    assert_eq!(v["enterprise_id"], json!("ten-1"), "回调字段名是 TenantID");
    // machine_id/device_id 32hex 一次性生成，且与授权 URL 下发的是同一份
    assert_eq!(v["machine_id"], json!(url_machine_id), "URL 与凭据共用同一 machine_id");
    assert_eq!(v["machine_id"].as_str().unwrap().len(), 32);
    assert_eq!(v["device_id"].as_str().unwrap().len(), 32);
    assert!(v["machine_id"].as_str().unwrap().chars().all(|c| c.is_ascii_hexdigit()));
}

#[tokio::test]
async fn old_flow_accepts_json_userjwt_and_jwt_refresh_fallback() {
    // userJwt 的权威形态是 JSON {Token, RefreshToken}（trae-oauth.ts:291-292）；
    // query 缺 refreshToken 时回退 userJwt.RefreshToken（trae-oauth.ts:294-295）。
    let flow = TraeLoginFlow::start_with_port(0).await;
    let cb = flow.callback_base();
    let user_jwt = urlencoding_of(r#"{"Token":"jwt-2","RefreshToken":"rt-from-jwt"}"#);
    let http = reqwest::Client::new();
    let resp = http
        .get(format!("{cb}?userJwt={user_jwt}&userInfo=%7B%22UserID%22%3A%22u-2%22%7D"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let cred = flow.wait_credential().await.unwrap();
    let v: Value = serde_json::from_str(&cred.secret).unwrap();
    assert_eq!(v["access_token"], json!("jwt-2"), "access 取 userJwt.Token");
    assert_eq!(v["refresh_token"], json!("rt-from-jwt"), "refresh 回退 userJwt.RefreshToken");
    assert_eq!(v["uid"], json!("u-2"));
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
