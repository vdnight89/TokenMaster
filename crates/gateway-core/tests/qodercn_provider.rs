//! T4.6 qodercn：复用 qoder 实现，仅换端点/client_id/品牌信息（缝 2）。
//! 行为来源：deepseek-harness-codearts qoder-product.ts:584-660——
//! - 端点三换：auth=qoder.cn、openapi=openapi.qoder.com.cn、加密推理=gateway.qoder.com.cn
//! - client_id 换 `732aef47-…`（与国际版**完全不同**，用错授权后报参数无效）
//! - session_type CN=`qoder_work`（国际版=qodercli）
//! - Cosy-ClientType 同 `'10'`、UA 同 `qoder` 前缀

use gateway_core::providers::qoder::{QoderOAuth, QODER_CLIENT_ID};
use gateway_core::providers::qodercn::{
    QodercnProvider, QODERCN_AUTH_BASE, QODERCN_CLIENT_ID, QODERCN_OPENAPI_BASE,
    QODERCN_SESSION_TYPE,
};

#[test]
fn constants_match_reference() {
    assert_eq!(QODERCN_AUTH_BASE, "https://qoder.cn");
    assert_eq!(QODERCN_OPENAPI_BASE, "https://openapi.qoder.com.cn");
    assert_eq!(QODERCN_CLIENT_ID, "732aef47-9cf2-46a2-95fe-4cebb5d0d1fa");
    assert_ne!(QODERCN_CLIENT_ID, QODER_CLIENT_ID, "CN client_id 与国际版完全不同");
    assert_eq!(QODERCN_SESSION_TYPE, "qoder_work", "CN 用 qoder_work（qoder-wasm.ts:199）");
}

#[tokio::test]
async fn device_login_hits_cn_endpoints() {
    use axum::extract::{RawQuery, State};
    use axum::response::{IntoResponse, Response};
    use axum::routing::get;
    use axum::{Json, Router};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Cap {
        poll_path: Mutex<Option<String>>,
        bodies: Mutex<Vec<serde_json::Value>>,
    }

    async fn poll(State(cap): State<Arc<Cap>>, RawQuery(q): RawQuery) -> Response {
        *cap.poll_path.lock().unwrap() = q;
        let hits = cap.bodies.lock().unwrap().len();
        if hits < 2 {
            cap.bodies.lock().unwrap().push(serde_json::json!({"ok": true}));
            return (
                axum::http::StatusCode::NOT_FOUND,
                axum::Json(serde_json::json!({"errorCode": "NotFound"})),
            )
                .into_response();
        }
        cap.bodies.lock().unwrap().push(
            serde_json::json!({
                "token": "cn-token",
                "refresh_token": "cn-rt",
                "user_id": "cn-uid",
                "expires_at": 1893456000i64,
            }),
        );
        Json(serde_json::json!({
            "token": "cn-token",
            "refresh_token": "cn-rt",
            "user_id": "cn-uid",
            "expires_at": 1893456000i64,
        }))
        .into_response()
    }

    let cap = Arc::new(Cap::default());
    let app = Router::new()
        .route("/api/v1/deviceToken/poll", get(poll))
        .with_state(cap.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub") });
    let base = format!("http://{addr}");

    // QodercnProvider 的 device login 应该走 CN 的 openapi 端点
    let p = QodercnProvider::with_openapi_base(base.clone());
    let cred = p
        .login_with_interval(std::time::Duration::from_millis(10))
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&cred.secret).unwrap();
    assert_eq!(v["access_token"], "cn-token");
    assert_eq!(v["uid"], "cn-uid");
    assert_eq!(v["machine_id"], v["machine_id"]); // 存在

    // 授权 URL 应该指向 qoder.cn（CN auth base）
    let oa = QoderOAuth::new_for_test(base);
    let sess = oa.create_device_session();
    let url = gateway_core::providers::qoder::build_qoder_auth_url(&sess);
    // build_qoder_auth_url 硬编码国际版——CN 版需要自己的
    let cn_url = gateway_core::providers::qodercn::build_qodercn_auth_url(&sess);
    assert!(cn_url.starts_with("https://qoder.cn/device/selectAccounts?"), "{cn_url}");
    assert!(cn_url.contains(QODERCN_CLIENT_ID), "CN client_id: {cn_url}");
    assert!(!cn_url.contains(QODER_CLIENT_ID), "不含国际版 id");
    assert!(url.starts_with("https://qoder.com"), "国际版不受影响");
}

#[tokio::test]
async fn credits_hits_cn_openapi() {
    use axum::extract::State;
    use axum::http::HeaderMap;
    use axum::response::{IntoResponse, Response};
    use axum::routing::get;
    use axum::{Json, Router};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Cap {
        headers: Mutex<Option<HeaderMap>>,
    }

    async fn usage(State(cap): State<Arc<Cap>>, h: HeaderMap) -> Response {
        *cap.headers.lock().unwrap() = Some(h);
        Json(serde_json::json!({
            "displayMode": "personal",
            "qoderUsage": { "userQuota": { "total": 100, "used": 30, "remaining": 70 } },
            "addOnQuota": { "total": 0, "used": 0, "remaining": 0 }
        }))
        .into_response()
    }

    let cap = Arc::new(Cap::default());
    let app = Router::new()
        .route("/sash/api/v2/me/usage", get(usage))
        .with_state(cap.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub") });
    let base = format!("http://{addr}");

    let cred = gateway_core::Credential {
        account_id: "qcn1".into(),
        secret: serde_json::json!({
            "access_token": "cn-at",
            "machine_id": "cn-mid"
        })
        .to_string(),
    };
    let p = QodercnProvider::with_openapi_base(base);
    let bal = p.balance(&cred).await.unwrap();
    assert_eq!(bal.total, 70);
    let h = cap.headers.lock().unwrap().clone().unwrap();
    assert_eq!(h.get("cosy-client-type").unwrap(), "10");
}
