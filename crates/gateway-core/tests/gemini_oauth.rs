//! T4.2e gemini OAuth（缝 2：stub 上游）。
//! 行为来源：docs/reference/deepseek-harness-codearts.md §4.14——
//! - OAuth：`accounts.google.com/o/oauth2/v2/auth` + `oauth2.googleapis.com/token`，
//!   client 是上游 CloudCode 客户端的公开 client（env `CMDC_PAK_GOOGLE_CLIENT_ID`
//!   可覆盖；**具体值与六项 scope 逐字清单手册未载明，生产默认值待用户确认**）。
//! - **refresh_token 会轮换，刷新后必须立即回写**（响应无新 rt 时保留旧值）。
//! - 凭据 secret 为 JSON（access_token/refresh_token/expires_at）；
//!   推理 Authorization 取其中的 access_token。
//! - 登录流：本地回环回调收 code → token 端点交换。

use gateway_core::provider::{Provider, ProviderError};
use gateway_core::providers::gemini::{GeminiOAuth, GeminiProvider};
use gateway_core::Credential;
use serde_json::{json, Value};

async fn spawn_token_stub() -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let _s2 = seen.clone();
    let app = axum::Router::new()
        .route(
            "/token",
            axum::routing::post(move |State(_s2): State<std::sync::Arc<std::sync::Mutex<Vec<String>>>>, body: String| {
                async move {
                    _s2.lock().unwrap().push(body);
                    // 默认响应：交换成功
                    (
                        StatusCode::OK,
                        axum::Json(json!({
                            "access_token": "at-1",
                            "refresh_token": "rt-1",
                            "expires_in": 3600
                        })),
                    )
                        .into_response()
                }
            }),
        )
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub") });
    (format!("http://{addr}/token"), seen)
}

fn oauth(token_url: String) -> GeminiOAuth {
    GeminiOAuth::new(
        "https://accounts.google.com/o/oauth2/v2/auth".into(),
        token_url,
        "client-x".into(),
        vec!["cloud-platform".into(), "cclog".into()],
    )
}

#[tokio::test]
async fn exchange_code_posts_standard_grant_and_builds_credential() {
    let (token_url, seen) = spawn_token_stub().await;
    let cred = oauth(token_url)
        .exchange_code("code-xyz", "http://127.0.0.1:45678")
        .await
        .unwrap();
    let form = seen.lock().unwrap()[0].clone();
    for expect in [
        "grant_type=authorization_code",
        "code=code-xyz",
        "client_id=client-x",
        "redirect_uri=http%3A%2F%2F127.0.0.1%3A45678",
    ] {
        assert!(form.contains(expect), "交换请求缺 {expect}：{form}");
    }
    let v: Value = serde_json::from_str(&cred.secret).unwrap();
    assert_eq!(v["access_token"], json!("at-1"));
    assert_eq!(v["refresh_token"], json!("rt-1"));
    assert!(v["expires_at"].is_u64(), "expires_at 必须落绝对时间戳");
}

#[tokio::test]
async fn refresh_rotates_refresh_token_immediately() {
    let (token_url, seen) = spawn_token_stub().await;
    // 第二次（refresh）响应轮换新 rt
    // 先手工构造带旧 rt 的凭据
    let secret = json!({ "access_token": "at-0", "refresh_token": "rt-old", "expires_at": 0 }).to_string();
    let cred = oauth(token_url)
        .refresh(&Credential { account_id: "g1".into(), secret })
        .await
        .unwrap();
    let form = seen.lock().unwrap()[0].clone();
    assert!(form.contains("grant_type=refresh_token"), "{form}");
    assert!(form.contains("refresh_token=rt-old"), "{form}");
    let v: Value = serde_json::from_str(&cred.secret).unwrap();
    // stub 恒返回 rt-1：轮换语义=响应里的 rt 立即覆盖
    assert_eq!(v["refresh_token"], json!("rt-1"), "轮换后必须立即回写新 rt");
    assert_eq!(v["access_token"], json!("at-1"));
}

#[tokio::test]
async fn refresh_without_new_rt_keeps_old() {
    // 响应不含 refresh_token：保留旧值（部分实现不轮换）
    let seen: std::sync::Arc<std::sync::Mutex<Vec<String>>> = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let app = axum::Router::new().route(
        "/token",
        axum::routing::post(|| async {
            axum::Json(json!({ "access_token": "at-2", "expires_in": 3600 }))
        }),
    );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub") });
    let secret = json!({ "access_token": "at-1", "refresh_token": "rt-keep", "expires_at": 0 }).to_string();
    let cred = GeminiOAuth::new(
        "https://a".into(),
        format!("http://{addr}/token"),
        "client-x".into(),
        vec![],
    )
    .refresh(&Credential { account_id: "g1".into(), secret })
    .await
    .unwrap();
    let v: Value = serde_json::from_str(&cred.secret).unwrap();
    assert_eq!(v["refresh_token"], json!("rt-keep"));
    assert_eq!(v["access_token"], json!("at-2"));
    let _ = seen; // 占位避免未用告警
}

#[tokio::test]
async fn login_url_carries_auth_params() {
    let url = oauth("http://unused".into()).login_url("http://127.0.0.1:45678", "st-9");
    assert!(url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"), "{url}");
    for expect in [
        "client_id=client-x",
        "redirect_uri=http%3A%2F%2F127.0.0.1%3A45678",
        "response_type=code",
        "scope=cloud-platform+cclog",
        "state=st-9",
        "access_type=offline",
    ] {
        assert!(url.contains(expect), "授权 URL 缺 {expect}：{url}");
    }
}

#[tokio::test]
async fn refresh_error_without_rt_in_secret() {
    let (token_url, _) = spawn_token_stub().await;
    let err = oauth(token_url)
        .refresh(&Credential { account_id: "g1".into(), secret: "bare-token".into() })
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::Credential(_)), "{err:?}");
}

#[tokio::test]
async fn identity_headers_use_access_token_from_json_secret() {
    use axum::extract::State;
    use axum::response::IntoResponse;
    let hdr = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let _h2 = hdr.clone();
    let app = axum::Router::new()
        .route(
            "/v1internal:streamGenerateContent",
            axum::routing::post(move |State(_h2): State<std::sync::Arc<std::sync::Mutex<String>>>, h: axum::http::HeaderMap| async move {
                *_h2.lock().unwrap() =
                    h.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
                ([("content-type", "text/event-stream")], "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"ok\"}]}}]}\n\n").into_response()
            }),
        )
        .route("/v1internal:loadCodeAssist", axum::routing::post(|| async { axum::Json(json!({})) }))
        .with_state(hdr.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub") });

    let secret = json!({ "access_token": "at-9", "refresh_token": "rt-9", "expires_at": 0 }).to_string();
    let p = GeminiProvider::new(format!("http://{addr}"), "aicode-consumers".into());
    let out = p
        .complete(
            &Credential { account_id: "g1".into(), secret },
            &gateway_core::route::Route { provider: "gemini".into(), model: "gemini-3-pro".into() },
            &serde_json::from_value(json!({
                "model": "gemini/gemini-3-pro",
                "messages": [{ "role": "user", "content": "hi" }]
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(out.choices[0].message.content.contains("ok"));
    assert_eq!(*hdr.lock().unwrap(), "Bearer at-9");
}

#[tokio::test]
async fn callback_login_flow_end_to_end() {
    let (token_url, _) = spawn_token_stub().await;
    let oa = oauth(token_url);
    let (url, rx) = oa.start_login().await;
    // 从授权 URL 提取 redirect_uri（回环端口）与 state，模拟浏览器重定向
    let redirect = url.split("redirect_uri=").nth(1).unwrap().split('&').next().unwrap();
    let callback = redirect.replace("%3A", ":").replace("%2F", "/");
    let state = url.split("state=").nth(1).unwrap().split('&').next().unwrap().to_string();
    let http = reqwest::Client::new();
    let resp = http
        .get(format!("{callback}?code=code-e2e&state={state}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let cred = rx.await.unwrap().unwrap();
    let v: Value = serde_json::from_str(&cred.secret).unwrap();
    assert_eq!(v["access_token"], json!("at-1"));
    assert_eq!(v["refresh_token"], json!("rt-1"));
}

#[tokio::test]
async fn callback_rejects_state_mismatch() {
    let (token_url, seen) = spawn_token_stub().await;
    let oa = oauth(token_url);
    let (url, rx) = oa.start_login().await;
    let redirect = url.split("redirect_uri=").nth(1).unwrap().split('&').next().unwrap();
    let callback = redirect.replace("%3A", ":").replace("%2F", "/");
    let http = reqwest::Client::new();
    let resp = http
        .get(format!("{callback}?code=evil&state=forged"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403, "state 不匹配必须拒绝（CSRF）");
    assert!(seen.lock().unwrap().is_empty(), "state 不匹配不得发起 token 交换");
    drop(rx);
}
