//! workbuddy Provider（WorkBuddy 国际版，T4.10）。
//!
//! 与 buddy（中国版）共用全部实现逻辑，仅换产品常量
//! （对照参考 product.ts:459-495 的 `WORKBUDDY`）：
//! - endpoint/apiDomain = `www.workbuddy.ai`
//! - platform = `workbuddy-ai`、productCode = `workbuddy`
//! - pluginVersion = `5.5.2`
//! - **无成长中心**（claimBase/webBase 是占位值）
//! - UA 国际版 `WorkBuddy/5.5.2 WorkBuddy AI/5.5.2 CLI/5.5.2`

use async_trait::async_trait;
use crate::openai::{ChatCompletion, ChatRequest};
use crate::provider::{ChunkStream, Credential, Provider, ProviderError};
use crate::registry::ProviderCatalog;
use crate::route::Route;
use crate::providers::buddy::BuddyProvider;

pub const WORKBUDDY_API_BASE: &str = "https://www.workbuddy.ai";
pub const WORKBUDDY_DOMAIN: &str = "www.workbuddy.ai";
pub const WORKBUDDY_PRODUCT_CODE: &str = "workbuddy";
pub const WORKBUDDY_PLATFORM: &str = "workbuddy-ai";
pub const WORKBUDDY_PLUGIN_VERSION: &str = "5.5.2";

/// 复用 BuddyProvider（同协议同实现，仅换常量）。
pub struct WorkbuddyProvider {
    inner: BuddyProvider,
}

impl WorkbuddyProvider {
    pub fn production() -> Self {
        Self {
            inner: BuddyProvider::new(WORKBUDDY_API_BASE.into())
                .with_domain(WORKBUDDY_DOMAIN.into())
                .with_product(WORKBUDDY_PRODUCT_CODE, "WorkBuddy", WORKBUDDY_PLUGIN_VERSION),
        }
    }

    /// 测试注入。
    pub fn with_base(base: String) -> Self {
        Self {
            inner: BuddyProvider::new(base)
                .with_domain(WORKBUDDY_DOMAIN.into())
                .with_product(WORKBUDDY_PRODUCT_CODE, "WorkBuddy", WORKBUDDY_PLUGIN_VERSION),
        }
    }

    pub fn inner(&self) -> &BuddyProvider {
        &self.inner
    }
}

#[async_trait]
impl Provider for WorkbuddyProvider {
    fn id(&self) -> &str {
        "workbuddy"
    }

    fn catalog(&self) -> ProviderCatalog {
        self.inner.catalog()
    }

    async fn refresh(&self, cred: &Credential) -> Result<Credential, ProviderError> {
        BuddyProvider::refresh(&self.inner, cred).await
    }

    async fn complete(&self, cred: &Credential, route: &Route, req: &ChatRequest) -> Result<ChatCompletion, ProviderError> {
        self.inner.complete(cred, route, req).await
    }

    async fn stream(&self, cred: &Credential, route: &Route, req: &ChatRequest) -> Result<ChunkStream, ProviderError> {
        self.inner.stream(cred, route, req).await
    }
}
