//! M3 缝 1：网关请求被记入账本（成功与失败，账号归因）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use gateway_core::config::{AuthMode, GatewayConfig};
use gateway_core::ledger::{Ledger, UsageRecord};
use gateway_core::openai::{ChatCompletion, ChatRequest, Usage};
use gateway_core::pool::{PoolEntry, SelectionStrategy, TokenPool};
use gateway_core::provider::{ChunkStream, Credential, Provider, ProviderError};
use gateway_core::registry::{ModelInfo, ProviderCatalog, Registry};
use gateway_core::route::Route;

struct Pv;
#[async_trait]
impl Provider for Pv {
    fn id(&self) -> &str { "mock" }
    fn catalog(&self) -> ProviderCatalog {
        ProviderCatalog { id: "mock".into(), models: vec![ModelInfo { id: "mock-alpha".into() }] }
    }
    async fn complete(&self, cred: &Credential, r: &Route, _q: &ChatRequest) -> Result<ChatCompletion, ProviderError> {
        if cred.account_id == "bad" {
            return Err(ProviderError::RateLimited { retry_after_secs: Some(30), msg: "429".into() });
        }
        Ok(ChatCompletion::new(r.composite(), format!("hi-{}", cred.account_id), Usage::sum(10, 5)))
    }
    async fn stream(&self, _c: &Credential, _r: &Route, _q: &ChatRequest) -> Result<ChunkStream, ProviderError> {
        unimplemented!()
    }
}

fn body(model: &str) -> serde_json::Value {
    serde_json::json!({ "model": model, "messages": [{ "role": "user", "content": "x" }] })
}

#[tokio::test]
async fn successful_and_failed_requests_are_ledgered_with_account_attribution() {
    let ledger = Arc::new(Ledger::memory());
    let cfg = GatewayConfig {
        auth: AuthMode::Disabled,
        model_map: Vec::new(),
        registry: Registry::with(ProviderCatalog { id: "mock".into(), models: vec![ModelInfo { id: "mock-alpha".into() }] }),
    };
    let mut pool = TokenPool::new("mock", SelectionStrategy::ExpireFirst);
    for (id, cred) in [("bad", "c1"), ("good", "c2")] {
        let mut e = PoolEntry::new(id, None, None);
        e.credential = cred.into();
        pool.upsert(e);
    }
    let pools = HashMap::from([("mock".to_string(), Arc::new(Mutex::new(pool)))]);
    let h = gateway_core::server::start_with_ledger(cfg, vec![Arc::new(Pv)], pools, Some(ledger.clone()))
        .await
        .unwrap();
    let client = reqwest::Client::new();

    // 成功：bad 429 → good 服务
    let ok = client
        .post(format!("http://{}/v1/chat/completions", h.addr))
        .json(&body("mock/mock-alpha"))
        .send().await.unwrap();
    assert_eq!(ok.status(), 200);

    // 失败：不存在的 provider
    let miss = client
        .post(format!("http://{}/v1/chat/completions", h.addr))
        .json(&body("nope/ghost"))
        .send().await.unwrap();
    assert_eq!(miss.status(), 404);
    h.shutdown().await;

    let recent = ledger.recent(10);
    assert_eq!(recent.len(), 2, "成功与失败都入账");
    let success: &UsageRecord = recent.iter().find(|r| r.status == 200).unwrap();
    assert_eq!(success.account_id, "good", "归因到换号后实际服务的账号");
    assert_eq!(success.provider, "mock");
    assert_eq!(success.model, "mock-alpha");
    assert_eq!(success.prompt_tokens, 10);
    assert_eq!(success.completion_tokens, 5);
    let failure: &UsageRecord = recent.iter().find(|r| r.status == 404).unwrap();
    assert_eq!(failure.model, "ghost");
    assert_eq!(failure.provider, "nope");
}
