//! T4.10 workbuddy（复用 buddy，换 workbuddy.ai 配置）。
//! 验证常量差异：endpoint、domain、productCode、platform 与 buddy 不同。

use gateway_core::provider::Provider;
use gateway_core::providers::workbuddy::{
    WorkbuddyProvider, WORKBUDDY_API_BASE, WORKBUDDY_DOMAIN, WORKBUDDY_PLATFORM,
    WORKBUDDY_PLUGIN_VERSION, WORKBUDDY_PRODUCT_CODE,
};
use gateway_core::providers::buddy::BUDDY_API_BASE;

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
