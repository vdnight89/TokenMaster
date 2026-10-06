//! 刷新调度（T2.5）：周期 30 分钟 + 启动即一轮 + 新账号入池补刷。
//! 铁律：续期不看账号 enabled（dsh refresh-scheduler 语义）——
//! 停用只影响选号，不影响续期；不可续期 provider 只探测。
//! Credential 失败 → 标记失效（等用户重新登录）；成功 → 回写新凭据。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::pool::TokenPool;
use crate::provider::{Credential, Provider, ProviderError};

pub const REFRESH_INTERVAL: Duration = Duration::from_secs(30 * 60);

pub struct RefreshScheduler {
    pool: Arc<Mutex<TokenPool>>,
    provider: Arc<dyn Provider>,
    interval: Duration,
}

impl RefreshScheduler {
    pub fn new(pool: Arc<Mutex<TokenPool>>, provider: Arc<dyn Provider>) -> Self {
        Self { pool, provider, interval: REFRESH_INTERVAL }
    }

    /// 执行一轮刷新，返回（成功数，失败数）。
    pub async fn tick(&self) -> (usize, usize) {
        let entries: Vec<(String, String)> = {
            let p = self.pool.lock().unwrap_or_else(|e| e.into_inner());
            p.entries()
                .iter()
                .map(|e| (e.account_id.clone(), e.credential.clone()))
                .collect()
        };
        let mut ok = 0;
        let mut fail = 0;
        for (account_id, secret) in entries {
            let cred = Credential { account_id: account_id.clone(), secret };
            match self.provider.refresh(&cred).await {
                Ok(new_cred) => {
                    ok += 1;
                    let mut p = self.pool.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(e) = p.entries_mut().iter_mut().find(|e| e.account_id == account_id) {
                        e.credential = new_cred.secret;
                    }
                }
                Err(ProviderError::BadRequest(_)) => {
                    // 不可续期：探测即止（loomy 会话 / zcode 静态 JWT）
                }
                Err(_) => {
                    fail += 1;
                    let mut p = self.pool.lock().unwrap_or_else(|e| e.into_inner());
                    p.mark_dead(&account_id);
                }
            }
        }
        (ok, fail)
    }

    /// 启动后台循环：立即一轮，之后按 interval 周期执行。
    pub fn start(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(self.interval);
            ticker.tick().await; // interval 的首个 tick 立即返回
            loop {
                ticker.tick().await;
                let _ = self.tick().await;
            }
        })
    }
}
