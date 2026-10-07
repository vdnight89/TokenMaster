//! T4.5b qoder credits/领取/错误码（缝 2：stub 上游）。
//! 行为来源：deepseek-harness-codearts/src/qoder-credits.ts（2026-09-19/21
//! 真实抓包）——
//! - 余额 `GET /sash/api/v2/me/usage`：响应 `{displayMode, qoderUsage:{
//!   userQuota:{total,used,remaining}, addOnQuota:{…}, dedicatedResourcePackages:[…]}}`；
//!   **余额 = userQuota + addOnQuota + 专用包剩余累加**（只读 userQuota 会
//!   显示 0）；remaining 优先、缺失按 max(0,total-used)；企业版无额度数字；
//!   一个包都没有 = 查不到 ≠ 余额 0。
//! - 活动 `GET /sash/api/v1/me/campaigns`：顶层 `campaigns[]`，字段
//!   `campaignId`/`actionType`/`claimStatus`；只领 `CLAIM_BENEFIT`+`CLAIMABLE`。
//!   **必需成对 machine 头**（消融实验 :202-218：只带 ClientType:'10' 时仅
//!   1 条 VIEW_DETAILS）→ `POST …/{campaignId}/claim`（body 空）；幂等判据
//!   是响应体 `replayed:true`（HTTP 仍 200）；`status` 在场但非 CLAIMED →
//!   领取未成功。
//! - machine 头值：凭据可选 `machine_token`/`machine_type`（对应 IDE
//!   machine_token.json 的 {token,type}，qoder-machine.ts:47-53）优先，
//!   缺失回退 machine_id/占位（回退值未经 wire 验证）。
//! - 推理错误分型（qoder-adapter.ts:745-897）：10605 排队（两形态）、409
//!   重发一次、105/401 续期（每请求一次）、110 不可重试。

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use gateway_core::provider::ProviderError;
use gateway_core::providers::qoder::{
    map_error_code, map_infer_error_code, ClaimOutcome, InferErrorAction, QoderProvider,
};
use gateway_core::Credential;
use serde_json::{json, Value};

#[derive(Default)]
struct Cap {
    usage_headers: Option<HeaderMap>,
    campaigns_headers: Option<HeaderMap>,
    claim_body: Option<String>,
    claim_headers: Option<HeaderMap>,
    /// claim 响应体（默认 replayed:true 的幂等重放形态）
    claim_resp: Mutex<Value>,
}

async fn stub_usage(State(cap): State<Arc<Mutex<Cap>>>, h: HeaderMap) -> Response {
    cap.lock().unwrap().usage_headers = Some(h);
    (
        StatusCode::OK,
        // 形状按 2026-09-19 实测（qoder-credits.ts:18-25）
        Json(json!({
            "displayMode": "qoder",
            "qoderUsage": {
                "userType": "personal_standard",
                "userQuota":  { "total": 0, "used": 0, "remaining": 0, "unit": "credits" },
                "addOnQuota": { "total": 300, "used": 100, "remaining": 200 },
                "dedicatedResourcePackages": [ { "id": "d1", "remaining": 40 } ]
            }
        })),
    )
        .into_response()
}

async fn stub_campaigns(State(cap): State<Arc<Mutex<Cap>>>, h: HeaderMap) -> Response {
    cap.lock().unwrap().campaigns_headers = Some(h);
    (
        StatusCode::OK,
        Json(json!({ "showCampaign": true, "claimable": true, "campaigns": [
            { "campaignId": "c-claim", "actionType": "CLAIM_BENEFIT", "claimStatus": "CLAIMABLE",
              "benefit": { "kind": "CREDITS", "amount": 100 } },
            { "campaignId": "c-view", "actionType": "CLAIM_BENEFIT", "claimStatus": "VIEW_DETAILS" },
            { "campaignId": "c-other", "actionType": "OTHER", "claimStatus": "CLAIMABLE" },
            { "campaignId": "c-done", "actionType": "CLAIM_BENEFIT", "claimStatus": "CLAIMED" }
        ]})),
    )
        .into_response()
}

async fn stub_claim(State(cap): State<Arc<Mutex<Cap>>>, h: HeaderMap, body: axum::extract::Request) -> Response {
    let bytes = axum::body::to_bytes(body.into_body(), 1 << 20).await.unwrap();
    let resp = {
        let mut c = cap.lock().unwrap();
        c.claim_body = Some(String::from_utf8_lossy(&bytes).to_string());
        c.claim_headers = Some(h);
        let out = c.claim_resp.lock().unwrap().clone();
        out
    };
    (StatusCode::OK, Json(resp)).into_response()
}

async fn spawn() -> (String, Arc<Mutex<Cap>>) {
    let cap = Arc::new(Mutex::new(Cap::default()));
    *cap.lock().unwrap().claim_resp.lock().unwrap() = json!({ "replayed": true });
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
async fn balance_sums_all_quota_packages() {
    let (base, cap) = spawn().await;
    let bal = QoderProvider::new(base).balance(&cred()).await.unwrap();
    // userQuota(0) + addOnQuota(200) + 专用包(40) = 240；只读 userQuota 会得 0
    assert_eq!(bal.total, 240);
    let h = cap.lock().unwrap().usage_headers.clone().unwrap();
    assert!(h.get("authorization").unwrap().to_str().unwrap().contains("at-9"));
    assert!(h.contains_key("cosy-client-type"), "余额只需 Bearer+Cosy-ClientType");
    assert_eq!(h.get("cosy-client-type").unwrap(), "10", "实测值 '10'（qoder-product.ts:454-455）");
}

#[tokio::test]
async fn balance_falls_back_to_total_minus_used() {
    // remaining 缺失按 max(0, total-used)（qoder-credits.ts:283-291）
    let cap = Arc::new(Mutex::new(Cap::default()));
    let app = Router::new()
        .route(
            "/sash/api/v2/me/usage",
            get(|| async {
                Json(json!({ "displayMode": "qoder", "qoderUsage": {
                    "userQuota": { "total": 120, "used": 20 }
                }}))
            }),
        )
        .with_state(cap.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub") });
    let bal = QoderProvider::new(format!("http://{addr}")).balance(&cred()).await.unwrap();
    assert_eq!(bal.total, 100);
}

#[tokio::test]
async fn balance_unparseable_shape_is_error_not_zero() {
    // 一个包都解析不出 → 查不到 ≠ 余额 0（qoder-credits.ts:378-380）
    let cap = Arc::new(Mutex::new(Cap::default()));
    let app = Router::new()
        .route(
            "/sash/api/v2/me/usage",
            get(|| async { Json(json!({ "displayMode": "qoder", "qoderUsage": {} })) }),
        )
        .with_state(cap.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub") });
    let err = QoderProvider::new(format!("http://{addr}"))
        .balance(&cred())
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::Upstream(_)), "{err:?}");
}

#[tokio::test]
async fn campaigns_only_claim_benefit_and_claimable_pass_filter() {
    let (base, cap) = spawn().await;
    let list = QoderProvider::new(base).campaigns(&cred()).await.unwrap();
    let ids: Vec<&str> = list.iter().filter_map(|c| c.get("campaignId").and_then(Value::as_str)).collect();
    assert_eq!(ids, vec!["c-claim"], "只领 CLAIM_BENEFIT+CLAIMABLE：{ids:?}");
    let c = cap.lock().unwrap();
    let h = c.campaigns_headers.clone().unwrap();
    assert!(h.contains_key("cosy-machinetoken"), "必需成对 machine 头（缺了只见 VIEW_DETAILS）");
    assert!(h.contains_key("cosy-machinetype"));
    assert_eq!(h.get("cosy-machinetoken").unwrap(), "mid-1", "缺失 machine_token 时回退 machine_id");
    assert_eq!(h.get("cosy-machinetype").unwrap(), "pc", "缺失 machine_type 时回退占位值（未验证）");
}

#[tokio::test]
async fn machine_headers_prefer_credential_machine_token_fields() {
    // 凭据显式 machine_token/machine_type（对应 IDE machine_token.json 的
    // {token,type}，qoder-machine.ts:47-53）优先于 machine_id/占位回退
    let (base, cap) = spawn().await;
    let cred = Credential {
        account_id: "q1".into(),
        secret: json!({
            "access_token": "at-9",
            "machine_id": "mid-1",
            "machine_token": "mtok-live",
            "machine_type": "mtype-live"
        })
        .to_string(),
    };
    let _ = QoderProvider::new(base).campaigns(&cred).await.unwrap();
    let h = cap.lock().unwrap().campaigns_headers.clone().unwrap();
    assert_eq!(h.get("cosy-machinetoken").unwrap(), "mtok-live");
    assert_eq!(h.get("cosy-machinetype").unwrap(), "mtype-live");
}

#[tokio::test]
async fn claim_posts_empty_body_and_reads_replayed_idempotency() {
    let (base, cap) = spawn().await;
    let p = QoderProvider::new(base);
    // 先拿可领活动
    let list = p.campaigns(&cred()).await.unwrap();
    let id = list[0].get("campaignId").and_then(Value::as_str).unwrap().to_string();
    let out = p.claim(&cred(), &id).await.unwrap();
    assert!(matches!(out, ClaimOutcome::Replayed), "replayed:true 是幂等判据（HTTP 仍 200）");
    let c = cap.lock().unwrap();
    assert_eq!(c.claim_body.as_deref().unwrap_or(""), "", "claim body 为空（content-length:0）");
    assert!(c.claim_headers.as_ref().unwrap().contains_key("cosy-machinetoken"));
}

#[tokio::test]
async fn claim_success_and_non_claimed_status() {
    let (base, cap) = spawn().await;
    let p = QoderProvider::new(base);
    let id = p.campaigns(&cred()).await.unwrap()[0]
        .get("campaignId")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();
    // status:CLAIMED + replayed:false → 领取成功
    *cap.lock().unwrap().claim_resp.lock().unwrap() =
        json!({ "status": "CLAIMED", "replayed": false, "benefit": { "amount": 100 } });
    assert!(matches!(p.claim(&cred(), &id).await.unwrap(), ClaimOutcome::Claimed));
    // status 在场但非 CLAIMED → 领取未成功（qoder-credits.ts:637-639）
    *cap.lock().unwrap().claim_resp.lock().unwrap() = json!({ "status": "PENDING" });
    let err = p.claim(&cred(), &id).await.unwrap_err();
    assert!(matches!(err, ProviderError::Upstream(_)), "{err:?}");
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

#[test]
fn infer_error_actions_follow_reference_decision_table() {
    use InferErrorAction as A;
    // 10605 两形态都归排队（qoder-adapter.ts:783-789）：HTTP 403 + JSON body，
    // 或 HTTP 200 + SSE 内嵌 {code:"10605"} 帧
    for status in [403u16, 200] {
        assert_eq!(
            map_infer_error_code(status, Some("10605"), Some(30_000)),
            Some(A::Queued { retry_after_ms: Some(30_000) }),
            "status={status}"
        );
    }
    // 排队延迟上限 30 分钟
    assert_eq!(
        map_infer_error_code(403, Some("10605"), Some(9_999_999)),
        Some(A::Queued { retry_after_ms: Some(1_800_000) })
    );
    // 拿不到服务端延迟 → None 延迟（调用方保守短退避）
    assert_eq!(
        map_infer_error_code(200, Some("10605"), None),
        Some(A::Queued { retry_after_ms: None })
    );
    // 409 duplicate_request：凭据没问题，不刷新，重发一次（:834-838）
    assert_eq!(map_infer_error_code(409, None, None), Some(A::DuplicateResend));
    // 105 auth_error / 401：续期后重试（每请求至多一次，:820-830）
    assert_eq!(map_infer_error_code(200, Some("105"), None), Some(A::RefreshAuth));
    assert_eq!(map_infer_error_code(401, None, None), Some(A::RefreshAuth));
    // 403 非排队非额度同样按认证失败续期（adapter :793-831 的实际分支）
    assert_eq!(map_infer_error_code(403, None, None), Some(A::RefreshAuth));
    // 110：当天额度耗尽，不可重试（应切号/报 QUOTA_EXCEEDED）
    assert_eq!(map_infer_error_code(200, Some("110"), None), Some(A::QuotaExceeded));
    // 未知码 + 无关状态：不臆造
    assert_eq!(map_infer_error_code(500, Some("99999"), None), None);
    assert_eq!(map_infer_error_code(200, None, None), None);
}
