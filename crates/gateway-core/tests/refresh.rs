//! T2.5 刷新调度：铁律「续期不看 enabled」；不可续期只探测；
//! Credential 失败标失效。行为来源：reference/deepseek-harness-codearts.md
//! refresh-scheduler（30min 周期/启动即刷/入池补刷——周期行为由 start() 承担，
//! 此处测 tick 语义）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use gateway_core::openai::{ChatCompletion, ChatRequest};
use gateway_core::pool::{PoolEntry, SelectionStrategy, TokenPool};
use gateway_core::provider::{ChunkStream, Credential, Provider, ProviderError};
use gateway_core::refresh::RefreshScheduler;
use gateway_core::route::Route;
use gateway_core::registry::{ModelInfo, ProviderCatalog};

enum RefreshOutcome {
    Rotate,
    CredentialFail,
    NotRenewable,
}

struct Refresher(HashMap<&'static str, RefreshOutcome>);

#[async_trait]
impl Provider for Refresher {
    fn id(&self) -> &str { "mock" }
    fn catalog(&self) -> ProviderCatalog {
        ProviderCatalog { id: "mock".into(), models: vec![ModelInfo { id: "mock-alpha".into() }] }
    }
    async fn complete(&self, _c: &Credential, _r: &Route, _q: &ChatRequest) -> Result<ChatCompletion, ProviderError> {
        unimplemented!()
    }
    async fn stream(&self, _c: &Credential, _r: &Route, _q: &ChatRequest) -> Result<ChunkStream, ProviderError> {
        unimplemented!()
    }
    async fn refresh(&self, cred: &Credential) -> Result<Credential, ProviderError> {
        match self.0.get(cred.account_id.as_str()) {
            Some(RefreshOutcome::Rotate) => {
                Ok(Credential { account_id: cred.account_id.clone(), secret: format!("rot-{}", cred.account_id) })
            }
            Some(RefreshOutcome::CredentialFail) => Err(ProviderError::Credential("expired".into())),
            Some(RefreshOutcome::NotRenewable) => Err(ProviderError::BadRequest("not renewable".into())),
            None => panic!("未编排账号"),
        }
    }
}

fn pool_with(entries: Vec<(&str, &str, bool)>) -> Arc<Mutex<TokenPool>> {
    let mut p = TokenPool::new("mock", SelectionStrategy::ExpireFirst);
    for (id, cred, enabled) in entries {
        let mut e = PoolEntry::new(id, None, None);
        e.credential = cred.into();
        e.disabled = !enabled;
        p.upsert(e);
    }
    Arc::new(Mutex::new(p))
}

fn entry_secret(pool: &Arc<Mutex<TokenPool>>, id: &str) -> String {
    pool.lock()
        .unwrap()
        .entries()
        .iter()
        .find(|e| e.account_id == id)
        .unwrap()
        .credential
        .clone()
}

#[tokio::test]
async fn refresh_covers_disabled_accounts_too() {
    let provider = Arc::new(Refresher(HashMap::from([("a", RefreshOutcome::Rotate), ("b", RefreshOutcome::Rotate)])));
    let pool = pool_with(vec![("a", "old-a", true), ("b", "old-b", false)]);
    let s = RefreshScheduler::new(pool.clone(), provider);
    let (ok, fail) = s.tick().await;
    assert_eq!((ok, fail), (2, 0), "停用账号也要续期（铁律）");
    assert_eq!(entry_secret(&pool, "a"), "rot-a");
    assert_eq!(entry_secret(&pool, "b"), "rot-b");
}

#[tokio::test]
async fn credential_failure_marks_dead_but_others_continue() {
    let provider = Arc::new(Refresher(HashMap::from([
        ("a", RefreshOutcome::CredentialFail),
        ("b", RefreshOutcome::Rotate),
    ])));
    let pool = pool_with(vec![("a", "old-a", true), ("b", "old-b", true)]);
    let s = RefreshScheduler::new(pool.clone(), provider);
    let (ok, fail) = s.tick().await;
    assert_eq!((ok, fail), (1, 1));
    assert!(pool.lock().unwrap().entries().iter().find(|e| e.account_id == "a").unwrap().dead);
    assert_eq!(entry_secret(&pool, "b"), "rot-b");
}

#[tokio::test]
async fn not_renewable_is_probed_without_failure_count() {
    let provider = Arc::new(Refresher(HashMap::from([("a", RefreshOutcome::NotRenewable)])));
    let pool = pool_with(vec![("a", "static-jwt", true)]);
    let s = RefreshScheduler::new(pool.clone(), provider);
    let (ok, fail) = s.tick().await;
    assert_eq!((ok, fail), (0, 0), "不可续期不算成功也不算失败");
    assert_eq!(entry_secret(&pool, "a"), "static-jwt");
    assert!(!pool.lock().unwrap().entries()[0].dead);
}
