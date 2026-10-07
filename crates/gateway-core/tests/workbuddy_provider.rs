//! T4.10 workbuddy（复用 buddy，换 workbuddy.ai 配置）。
//! 验证常量差异：endpoint、domain、productCode、platform 与 buddy 不同；
//! 并验证请求头真正带国际版产品档（`X-Product-Code: workbuddy`——
//! reference §4.3：差异收敛在 product.ts，复用 buddy-adapter 不等于
//! 复用中国版产品常量）。

use std::sync::{Arc, Mutex};

use axum::http::HeaderMap;
use axum::routing::post;
use axum::Router;
use gateway_core::openai::ChatRequest;
use gateway_core::provider::Provider;
use gateway_core::providers::buddy::BUDDY_API_BASE;
use gateway_core::providers::workbuddy::{
    WorkbuddyProvider, WORKBUDDY_API_BASE, WORKBUDDY_DOMAIN, WORKBUDDY_PLATFORM,
    WORKBUDDY_PLUGIN_VERSION, WORKBUDDY_PRODUCT_CODE,
};
use gateway_core::Credential;
use serde_json::json;

#[test]
fn constants_match_reference() {
    assert_eq!(WORKBUDDY_API_BASE, "https://www.workbuddy.ai");
    assert_eq!(WORKBUDDY_DOMAIN, "www.workbuddy.ai");
    assert_eq!(WORKBUDDY_PRODUCT_CODE, "workbuddy");
    assert_eq!(WORKBUDDY_PLATFORM, "workbuddy-ai");
    assert_eq!(WORKBUDDY_PLUGIN_VERSION, "5.5.2");
    // 确认与中国版不同
    assert_ne!(WORKBUDDY_API_BASE, BUDDY_API_BASE);
    assert_ne!(WORKBUDDY_DOMAIN, "codebuddy.cn");
}

#[tokio::test]
async fn inherits_buddy_behavior() {
    // WorkbuddyProvider 复用 BuddyProvider——验证 inner 可用
    let p = WorkbuddyProvider::production();
    assert_eq!(p.inner().id(), "buddy", "复用 buddy 实现");
}

/// 国际版请求头带 workbuddy 产品档（非中国版 codebuddy 常量）。
#[tokio::test]
async fn workbuddy_headers_carry_international_product_profile() {
    let captured: Arc<Mutex<Option<HeaderMap>>> = Arc::new(Mutex::new(None));
    let c = captured.clone();
    let app = Router::new().route(
        "/v2/chat/completions",
        post(move |h: HeaderMap, _b: axum::extract::Request| {
            let c = c.clone();
            async move {
                *c.lock().unwrap() = Some(h);
                axum::response::IntoResponse::into_response((axum::http::StatusCode::OK, "ok"))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub") });

    let cred = Credential {
        account_id: "wb1".into(),
        secret: json!({ "access_token": "wb-at", "refresh_token": "wb-rt", "domain": "codebuddy.cn" })
            .to_string(),
    };
    let req: ChatRequest = serde_json::from_value(json!({
        "model": "workbuddy/claude-sonnet-4-6",
        "messages": [{ "role": "user", "content": "hi" }]
    }))
    .unwrap();
    let route = gateway_core::route::Route { provider: "workbuddy".into(), model: "claude-sonnet-4-6".into() };
    let _ = WorkbuddyProvider::with_base(format!("http://{addr}"))
        .complete(&cred, &route, &req)
        .await;

    let h = captured.lock().unwrap().clone().unwrap();
    assert_eq!(h.get("x-product-code").unwrap(), WORKBUDDY_PRODUCT_CODE, "国际版产品码");
    assert_eq!(h.get("x-product").unwrap(), "WorkBuddy");
    assert_eq!(h.get("x-ide-version").unwrap(), WORKBUDDY_PLUGIN_VERSION, "pluginVersion 5.5.2");
    // 产品级 domain 覆盖凭据级（|| 语义）——凭据里的中国域名不得泄漏到国际版请求
    assert_eq!(h.get("x-domain").unwrap(), WORKBUDDY_DOMAIN, "X-Domain 产品优先");
}
