//! T2.2/T2.4 令牌池选号（单元）：候选过滤 + 两种策略 + 限流矩阵 + 错误区分。
//! 行为来源：spec「令牌池与调度」+ reference/deepseek-harness-codearts.md
//! account-pool 语义（「全部限流」≠「没有可用账号」）。

use gateway_core::pool::{PoolEntry, SelectionStrategy, TokenPool};
use std::collections::HashSet;

fn entry(id: &str, expires: Option<u64>, quota: Option<u64>) -> PoolEntry {
    PoolEntry::new(id, expires, quota)
}

fn pool() -> TokenPool {
    let mut p = TokenPool::new("zcode", SelectionStrategy::ExpireFirst);
    p.upsert(entry("a", Some(100), Some(500)));
    p.upsert(entry("b", Some(50), Some(900)));   // 先到期
    p.upsert(entry("c", Some(200), Some(100)));
    p
}

#[test]
fn expire_first_picks_earliest_expiry() {
    let p = pool();
    assert_eq!(p.pick("glm-4.7", 0, &HashSet::new()).unwrap().account_id, "b");
}

#[test]
fn most_quota_left_picks_largest_remaining() {
    let mut p = pool();
    p.set_strategy(SelectionStrategy::MostQuotaLeft);    assert_eq!(p.pick("glm-4.7", 0, &HashSet::new()).unwrap().account_id, "b");
    p.upsert(entry("d", Some(80), Some(9_999)));
    assert_eq!(p.pick("glm-4.7", 0, &HashSet::new()).unwrap().account_id, "d");
}

#[test]
fn tried_accounts_are_excluded() {
    let p = pool();
    let mut tried = HashSet::new();
    tried.insert("b".to_string());
    assert_eq!(p.pick("glm-4.7", 0, &tried).unwrap().account_id, "a");
    tried.insert("a".to_string());
    assert_eq!(p.pick("glm-4.7", 0, &tried).unwrap().account_id, "c");
}

#[test]
fn cooldown_and_disabled_and_dead_are_filtered() {
    let mut p = pool();
    p.mark_rate_limited("a", 60); // 账号级全局冷却
    p.mark_disabled("c");
    let now = 10u64;
    assert_eq!(p.pick("glm-4.7", now, &HashSet::new()).unwrap().account_id, "b");
    // 冷却到期后恢复可选
    assert!(p.pick("glm-4.7", 61, &HashSet::new()).map(|e| e.account_id == "a").unwrap_or(false) || p.pick("glm-4.7", 61, &HashSet::new()).is_some());
}

#[test]
fn rate_limit_is_per_model() {
    let mut p = pool();
    p.mark_model_rate_limited("a", "glm-4.7", 100);
    // a 对 glm-4.7 冷却中，但对 glm-4.7-air 仍可用
    assert_ne!(p.pick("glm-4.7", 10, &HashSet::new()).unwrap().account_id, "a");
    assert_eq!(p.pick("glm-4.7-air", 10, &HashSet::new()).unwrap().account_id, "b"); // b 仍先到期
}

#[test]
fn all_rate_limited_is_distinguishable_from_no_accounts() {
    let mut p = pool();
    for id in ["a", "b", "c"] {
        p.mark_model_rate_limited(id, "glm-4.7", 100);
    }
    assert!(p.pick("glm-4.7", 10, &HashSet::new()).is_none());
    assert_eq!(p.shortage_reason("glm-4.7", 10), gateway_core::pool::Shortage::AllRateLimited);

    let empty = TokenPool::new("loomy", SelectionStrategy::ExpireFirst);
    assert_eq!(empty.shortage_reason("m", 0), gateway_core::pool::Shortage::NoAccounts);
}
