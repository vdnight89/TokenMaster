//! T4.5b qoder credits/领取/错误码（缝 2：stub 上游）。
//! 行为来源：docs/reference/deepseek-harness-codearts.md §4.5——
//! - 余额 `GET openapi.qoder.sh/sash/api/v2/me/usage`（只需 Bearer+
//!   `Cosy-ClientType`；**余额 = userQuota + addOnQuota 多包累加**，
//!   只读 userQuota 会显示 0）。
//! - 领取 `GET /sash/api/v1/me/campaigns`（**必需成对 machine 头**
//!   `Cosy-MachineToken`/`Cosy-MachineType`，少了只见 VIEW_DETAILS 看不到
//!   可领活动）→ `POST …/{campaignId}/claim`（body 空）；幂等判据是响应体
//!   `replayed:true`（HTTP 仍 200）；只领 `CLAIM_BENEFIT`+`CLAIMABLE`。
//! - 限流：排队码 `10605`（按服务端 retryAfterMs 等待，上限 30 分钟）；
//!   额度码 `110`（Billing daily count exceeded）→ QUOTA_EXCEEDED 不可重试。
//!
//! ⚠️ 手册未载明的 wire 细节（已按合理解释实现并标注）：余额包内剩余字段名
//! （按 `remaining`）、响应包裹层级（按一层 `data`）、campaign 的
//! type/status 字段名、`Cosy-ClientType`/`Cosy-MachineType` 具体值。

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use gateway_core::provider::ProviderError;
use gateway_core::providers::qoder::{map_error_code, ClaimOutcome, QoderProvider};
use gateway_core::Credential;
use serde_json::{json, Value};

#[derive(Default)]
struct Cap {
    usage_headers: Option<HeaderMap>,
    campaigns_headers: Option<HeaderMap>,
    claim_body: Option<String>,
    claim_headers: Option<HeaderMap>,
}

async fn stub_usage(State(cap): State<Arc<Mutex<Cap>>>, h: HeaderMap) -> Response {
    cap.lock().unwrap().usage_headers = Some(h);
    (
        StatusCode::OK,
        Json(json!({
            "data": {
                "userQuota": [ { "remaining": 100 }, { "remaining": 50 } ],
                "addOnQuota": [ { "remaining": 25 } ]
            }
        })),
    )
        .into_response()
}

async fn stub_campaigns(State(cap): State<Arc<Mutex<Cap>>>, h: HeaderMap) -> Response {
    cap.lock().unwrap().campaigns_headers = Some(h);
    (
        StatusCode::OK,
        Json(json!({ "data": [
            { "id": "c-claim", "type": "CLAIM_BENEFIT", "status": "CLAIMABLE", "title": "每日额度" },
            { "id": "c-view", "type": "CLAIM_BENEFIT", "status": "VIEW_DETAILS", "title": "只看不领" },
            { "id": "c-other", "type": "OTHER", "status": "CLAIMABLE", "title": "非领取活动" }
        ]})),
    )
        .into_response()
}

async fn stub_claim(State(cap): State<Arc<Mutex<Cap>>>, h: HeaderMap, body: axum::extract::Request) -> Response {
    let bytes = axum::body::to_bytes(body.into_body(), 1 << 20).await.unwrap();
    {
        let mut c = cap.lock().unwrap();
        c.claim_body = Some(String::from_utf8_lossy(&bytes).to_string());
        c.claim_headers = Some(h);
    }
    (StatusCode::OK, Json(json!({ "replayed": true }))).into_response()
}

async fn spawn() -> (String, Arc<Mutex<Cap>>) {
    let cap = Arc::new(Mutex::new(Cap::default()));
    let app = Router::new()
        .route("/sash/api/v2/me/usage", get(stub_usage))
        .route("/sash/api/v1/me/campaigns", get(stub_campaigns))
        .route("/sash/api/v1/me/campaigns/{id}/claim", post(stub_claim))
        .with_state(cap.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub serve") });
    (format!("http://{addr}"), cap)
}

fn cred() -> Credential {
    Credential {
        account_id: "q1".into(),
        secret: json!({ "access_token": "at-9", "machine_id": "mid-1" }).to_string(),
    }
}

#[tokio::test]
async fn balance_sums_user_and_addon_quota_packages() {
    let (base, cap) = spawn().await;
    let bal = QoderProvider::new(base).balance(&cred()).await.unwrap();
    // (100+50) + 25 = 175；只读 userQuota 会得 150（少 addOnQuota）
    assert_eq!(bal.total, 175);
    let h = cap.lock().unwrap().usage_headers.clone().unwrap();
    assert!(h.get("authorization").unwrap().to_str().unwrap().contains("at-9"));
    assert!(h.contains_key("cosy-client-type"), "余额只需 Bearer+Cosy-ClientType");
}

#[tokio::test]
async fn campaigns_only_claim_benefit_and_claimable_pass_filter() {
    let (base, cap) = spawn().await;
    let list = QoderProvider::new(base).campaigns(&cred()).await.unwrap();
    let ids: Vec<&str> = list.iter().filter_map(|c| c.get("id").and_then(Value::as_str)).collect();
    assert_eq!(ids, vec!["c-claim"], "只领 CLAIM_BENEFIT+CLAIMABLE：{ids:?}");
    let h = cap.lock().unwrap().campaigns_headers.clone().unwrap();
    assert!(h.contains_key("cosy-machinetoken"), "必需成对 machine 头（缺了只见 VIEW_DETAILS）");
    assert!(h.contains_key("cosy-machinetype"));
    assert_eq!(h.get("cosy-machinetoken").unwrap(), "mid-1", "machine token 取凭据 machine_id");
}

#[tokio::test]
async fn claim_posts_empty_body_and_reads_replayed_idempotency() {
    let (base, cap) = spawn().await;
    let p = QoderProvider::new(base);
    // 先拿可领活动
    let list = p.campaigns(&cred()).await.unwrap();
    let id = list[0].get("id").and_then(Value::as_str).unwrap().to_string();
    let out = p.claim(&cred(), &id).await.unwrap();
    assert!(matches!(out, ClaimOutcome::Replayed), "replayed:true 是幂等判据（HTTP 仍 200）");
    let c = cap.lock().unwrap();
    assert_eq!(c.claim_body.as_deref().unwrap_or(""), "", "claim body 为空");
    assert!(c.claim_headers.as_ref().unwrap().contains_key("cosy-machinetoken"));
}

#[test]
fn error_codes_classified_per_reference() {
    // 10605 排队：按服务端 retryAfterMs 等待，上限 30 分钟
    let e = map_error_code("10605", Some(65_000));
    match e {
        Some(ProviderError::RateLimited { retry_after_secs, .. }) => assert_eq!(retry_after_secs, Some(65)),
        other => panic!("10605 应为排队限流：{other:?}"),
    }
    let e2 = map_error_code("10605", Some(9_999_999));
    match e2 {
        Some(ProviderError::RateLimited { retry_after_secs, .. }) => {
            assert_eq!(retry_after_secs, Some(1800), "等待上限 30 分钟")
        }
        other => panic!("{other:?}"),
    }
    // 110 额度：Billing daily count exceeded → 不可重试
    let e3 = map_error_code("110", None);
    assert!(matches!(e3, Some(ProviderError::BadRequest(_))), "{e3:?}");
    // 未知码不臆造
    assert!(map_error_code("99999", None).is_none());
}
