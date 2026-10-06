//! T2.3 换号重试编排：401/402/403 标失效换号、429 模型级冷却换号、
//! 重试预算 4、BadRequest 立即中止、「全部限流」≠「没有可用账号」。
//! 行为来源：spec「令牌池与调度」+ reference AM determine_retry_strategy_adaptive 语义。

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use gateway_core::config::{AuthMode, GatewayConfig};
use gateway_core::openai::{ChatCompletion, ChatRequest, Usage};
use gateway_core::pool::{PoolEntry, SelectionStrategy, TokenPool};
use gateway_core::provider::{ChunkStream, Provider, ProviderError};
use gateway_core::registry::{ModelInfo, ProviderCatalog, Registry};
use gateway_core::route::Route;
use gateway_core::route::resolve_model;
use gateway_core::{error::error_json, orchestrate, Credential};

fn route() -> gateway_core::route::Route {
    resolve_model("mock/mock-alpha", &[]).unwrap()
}

/// 按 account_id 脚本化行为；统计调用次数。
struct Flaky {
    behavior: HashMap<&'static str, Result<&'static str, ProviderError>>,
    calls: AtomicUsize,
}
impl Flaky {
    fn new(behavior: HashMap<&'static str, Result<&'static str, ProviderError>>) -> Self {
        Self { behavior, calls: AtomicUsize::new(0) }
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}
#[async_trait]
impl Provider for Flaky {
    fn id(&self) -> &str { "mock" }
    fn catalog(&self) -> ProviderCatalog {
        ProviderCatalog { id: "mock".into(), models: vec![ModelInfo { id: "mock-alpha".into() }] }
    }
    async fn complete(&self, cred: &Credential, _r: &Route, _q: &ChatRequest) -> Result<ChatCompletion, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self.behavior.get(cred.account_id.as_str()) {
            Some(Ok(text)) => Ok(ChatCompletion::new("mock/mock-alpha".into(), text.to_string(), Usage::sum(1, 1))),
            Some(Err(e)) => Err(e.clone()),
            None => panic!("未编排的账号: {}", cred.account_id),
        }
    }
    async fn stream(&self, _c: &Credential, _r: &Route, _q: &ChatRequest) -> Result<ChunkStream, ProviderError> {
        unimplemented!()
    }
}

fn rl(secs: u64) -> ProviderError {
    ProviderError::RateLimited { retry_after_secs: Some(secs), msg: "upstream 429".into() }
}

fn pool_of(ids: &[&str]) -> Arc<Mutex<TokenPool>> {
    let mut p = TokenPool::new("mock", SelectionStrategy::ExpireFirst);
    for (i, id) in ids.iter().enumerate() {
        let mut e = PoolEntry::new(id, Some(100 + i as u64 * 10), Some(500));
        e.credential = format!("cred-{id}");
        p.upsert(e);
    }
    Arc::new(Mutex::new(p))
}

fn req() -> ChatRequest {
    serde_json::from_value(serde_json::json!({
        "model": "mock/mock-alpha",
        "messages": [{ "role": "user", "content": "hi" }]
    })).unwrap()
}

#[tokio::test]
async fn rate_limited_account_is_cooled_and_next_account_serves() {
    let provider = Flaky::new(HashMap::from([
        ("a", Err(rl(30))),
        ("b", Ok("来自 b 的回答")),
    ]));
    let pool = pool_of(&["a", "b"]);
    let out = orchestrate::complete_with_retry(&pool, &provider, &route(), &req()).await.unwrap();
    assert_eq!(out.completion.choices[0].message.content, "来自 b 的回答");
    assert_eq!(out.account_id, "b", "归因到实际服务的账号");
    assert_eq!(provider.calls(), 2);
    let p = pool.lock().unwrap();
    assert!(p.entries().iter().find(|e| e.account_id == "a").unwrap()
        .model_limits.contains_key("mock-alpha"), "a 应被记模型级冷却");
}

#[tokio::test]
async fn credential_error_marks_dead_and_switches() {
    let provider = Flaky::new(HashMap::from([
        ("a", Err(ProviderError::Credential("401 invalid".into()))),
        ("b", Ok("ok-b")),
    ]));
    let pool = pool_of(&["a", "b"]);
    let out = orchestrate::complete_with_retry(&pool, &provider, &route(), &req()).await.unwrap();
    assert_eq!(out.completion.choices[0].message.content, "ok-b");
    let p = pool.lock().unwrap();
    assert!(p.entries().iter().find(|e| e.account_id == "a").unwrap().dead);
}

#[tokio::test]
async fn bad_request_aborts_without_retry() {
    let provider = Flaky::new(HashMap::from([
        ("a", Err(ProviderError::BadRequest("bad".into()))),
    ]));
    let pool = pool_of(&["a", "b"]);
    let err = orchestrate::complete_with_retry(&pool, &provider, &route(), &req()).await.unwrap_err();
    assert_eq!(error_json(&err)["error"]["code"], "invalid_request_error");
    assert_eq!(provider.calls(), 1, "BadRequest 不换号");
}

#[tokio::test]
async fn all_rate_limited_yields_429_shape() {
    let provider = Flaky::new(HashMap::from([
        ("a", Err(rl(30))),
        ("b", Err(rl(60))),
    ]));
    let pool = pool_of(&["a", "b"]);
    let err = orchestrate::complete_with_retry(&pool, &provider, &route(), &req()).await.unwrap_err();
    assert_eq!(error_json(&err)["error"]["code"], "rate_limited", "全部限流 → 429 语义");
    assert_eq!(provider.calls(), 2);
}

#[tokio::test]
async fn empty_pool_is_no_available_account_not_rate_limited() {
    let provider = Flaky::new(HashMap::new());
    let pool = Arc::new(Mutex::new(TokenPool::new("mock", SelectionStrategy::ExpireFirst)));
    let err = orchestrate::complete_with_retry(&pool, &provider, &route(), &req()).await.unwrap_err();
    let body = error_json(&err);
    assert_eq!(body["error"]["code"], "no_available_account");
}

#[tokio::test]
async fn upstream_errors_exhaust_accounts_then_surface_last_error() {
    let provider = Flaky::new(HashMap::from([
        ("a", Err(ProviderError::Upstream("conn reset".into()))),
        ("b", Err(ProviderError::Upstream("timeout".into()))),
    ]));
    let pool = pool_of(&["a", "b"]);
    let err = orchestrate::complete_with_retry(&pool, &provider, &route(), &req()).await.unwrap_err();
    let body = error_json(&err);
    assert_eq!(body["error"]["code"], "retry_exhausted");
    assert!(body["error"]["message"].as_str().unwrap().contains("timeout"), "带出最后错误");
    assert_eq!(provider.calls(), 2);
}

/* ---- 缝 1 黑盒：网关 + 池编排端到端 ---- */

#[tokio::test]
async fn gateway_serves_from_next_account_after_429() {
    let provider = Arc::new(Flaky::new(HashMap::from([
        ("a", Err(rl(30))),
        ("b", Ok("端到端来自 b")),
    ])));
    let cfg = GatewayConfig {
        auth: AuthMode::Disabled,
        model_map: Vec::new(),
        registry: Registry::with(ProviderCatalog { id: "mock".into(), models: vec![ModelInfo { id: "mock-alpha".into() }] }),
    };
    let pool = pool_of(&["a", "b"]);
    let h = gateway_core::server::start_pooled(cfg, provider.clone(), pool).await.unwrap();
    let first = reqwest::Client::new()
        .post(format!("http://{}/v1/chat/completions", h.addr))
        .json(&serde_json::json!({ "model": "mock/mock-alpha", "messages": [{ "role": "user", "content": "x" }] }))
        .send().await.unwrap();
    assert_eq!(first.status(), 200);
    let body: serde_json::Value = first.json().await.unwrap();
    assert_eq!(body["choices"][0]["message"]["content"], "端到端来自 b");
    h.shutdown().await;
}
