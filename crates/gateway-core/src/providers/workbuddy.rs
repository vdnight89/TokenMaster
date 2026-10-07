//! workbuddy Provider（WorkBuddy 国际版，T4.10）。
//!
//! 与 buddy（中国版）共用全部实现逻辑，仅换产品常量
//! （对照参考 product.ts:459-495 的 `WORKBUDDY`）：
//! - endpoint/apiDomain = `www.workbuddy.ai`
//! - platform = `workbuddy-ai`、productCode = `workbuddy`
//! - pluginVersion = `5.5.2`
//! - **无成长中心**（claimBase/webBase 是占位值）
//! - UA 国际版 `WorkBuddy/5.5.2 WorkBuddy AI/5.5.2 CLI/5.5.2`

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
                .with_domain(WORKBUDDY_DOMAIN.into()),
        }
    }

    /// 测试注入。
    pub fn with_base(base: String) -> Self {
        Self {
            inner: BuddyProvider::new(base).with_domain(WORKBUDDY_DOMAIN.into()),
        }
    }

    pub fn inner(&self) -> &BuddyProvider {
        &self.inner
    }
}
