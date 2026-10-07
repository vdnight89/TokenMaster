//! T6.4 签到调度（手动 + 可选自动轮询）。
//!
//! 编排各 provider 的签到/领取端点（M4 已实现各家协议）：
//! - zcode：claim_daily（report_activity → preview → claim）
//! - trae：checkin_claim（claim 响应无数值需补查 status）
//! - minimax：signin_claim（timezone_id query + claim_result 幂等）
//! - qoder：campaigns → claim（replayed:true 幂等）
//! - buddy：checkin（两步 activity-status → daily-checkin）
//!
//! 调度模式：手动触发 + 可选自动轮询（cron 间隔可配）。
//! 设计：CheckinScheduler 持有 provider 池的引用，一次 run_all 遍历全部
//! 账号，失败的标记 last_error 不中断其余。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// 单次签到结果。
#[derive(Debug, Clone, PartialEq)]
pub struct CheckinOutcome {
    pub account_id: String,
    pub provider: String,
    pub success: bool,
    pub already_claimed: bool,
    pub message: String,
}

/// 签到调度器（手动触发；自动轮询由上层 Tauri 定时器或 tokio::spawn 驱动）。
pub struct CheckinScheduler {
    /// provider → 账号凭据列表（由池/Store 提供）。
    accounts: HashMap<String, Vec<crate::provider::Credential>>,
}

impl CheckinScheduler {
    pub fn new() -> Self {
        Self { accounts: HashMap::new() }
    }

    /// 注册某 provider 的账号列表。
    pub fn register(&mut self, provider: &str, creds: Vec<crate::provider::Credential>) {
        self.accounts.insert(provider.to_string(), creds);
    }

    /// 手动触发全部签到（遍历所有注册的 provider × 账号）。
    /// 各家签到方法由调用方注入（因为 provider 实例需要 base URL 等配置）。
    pub async fn run_all<F, Fut>(&self, checkin_fn: F) -> Vec<CheckinOutcome>
    where
        F: Fn(String, crate::provider::Credential) -> Fut,
        Fut: std::future::Future<Output = Result<(bool, String), crate::provider::ProviderError>>,
    {
        let mut results = Vec::new();
        for (provider, creds) in &self.accounts {
            for cred in creds {
                let outcome = match checkin_fn(provider.clone(), cred.clone()).await {
                    Ok((already, msg)) => CheckinOutcome {
                        account_id: cred.account_id.clone(),
                        provider: provider.clone(),
                        success: true,
                        already_claimed: already,
                        message: msg,
                    },
                    Err(e) => CheckinOutcome {
                        account_id: cred.account_id.clone(),
                        provider: provider.clone(),
                        success: false,
                        already_claimed: false,
                        message: e.to_string(),
                    },
                };
                results.push(outcome);
            }
        }
        results
    }

    /// 汇总。
    pub fn summarize(results: &[CheckinOutcome]) -> String {
        let total = results.len();
        let ok = results.iter().filter(|r| r.success).count();
        let already = results.iter().filter(|r| r.already_claimed).count();
        format!("{ok}/{total} 成功（{already} 已领取）")
    }
}

impl Default for CheckinScheduler {
    fn default() -> Self {
        Self::new()
    }
}

/// 自动轮询句柄（上层可 spawn tokio 定时器调 run_all）。
pub struct AutoCheckin {
    interval_secs: u64,
    enabled: Arc<Mutex<bool>>,
}

impl AutoCheckin {
    pub fn new(interval_secs: u64) -> Self {
        Self { interval_secs, enabled: Arc::new(Mutex::new(false)) }
    }

    pub fn interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.interval_secs)
    }

    pub fn is_enabled(&self) -> bool {
        // 锁中毒不致命（bool 标志无 invariant）：恢复数据继续
        *self.enabled.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn set_enabled(&self, on: bool) {
        *self.enabled.lock().unwrap_or_else(|e| e.into_inner()) = on;
    }
}
