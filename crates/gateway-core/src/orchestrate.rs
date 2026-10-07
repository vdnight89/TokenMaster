//! 编排层：池选号 + 失败分类换号重试（预算 4 次）。
//!
//! 分类语义（spec「令牌池与调度」+ AM 自适应重试）：
//! - Credential(401/402/403)：账号标记失效，换下一个；
//! - RateLimited(429)：账号×模型记冷却（Retry-After 缺省 60s），换下一个；
//! - Upstream：直接换下一个（请求级快切，避免全池锁死）；
//! - BadRequest：请求本身的问题，立即中止不换号。
//!
//! 首轮选号失败时区分「全部限流」(429) 与「没有可用账号」(503)。

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use crate::error::ApiError;
use crate::openai::{now_ts, ChatCompletion, ChatRequest};
use crate::pool::{Shortage, TokenPool};
use crate::provider::{Credential, Provider, ProviderError};
use crate::route::Route;

pub const MAX_ATTEMPTS: usize = 4;
const DEFAULT_RATE_LIMIT_SECS: u64 = 60;

/// 编排成功结果：补全内容 + 实际服务的账号（账本归因用）。
#[derive(Debug)]
pub struct Dispatched {
    pub completion: ChatCompletion,
    pub account_id: String,
}

/// 非流式：带换号重试的补全（route 由调用方解析，便于沿用映射表）。
pub async fn complete_with_retry(
    pool: &Arc<Mutex<TokenPool>>,
    provider: &dyn Provider,
    route: &Route,
    req: &ChatRequest,
) -> Result<Dispatched, ApiError> {
    let model = route.model.clone();
    let mut tried: HashSet<String> = HashSet::new();
    let mut last_err: Option<ProviderError> = None;

    for _ in 0..MAX_ATTEMPTS {
        let now = now_ts();
        let picked = {
            let p = pool.lock().unwrap_or_else(|e| e.into_inner());
            p.pick(&model, now, &tried).map(|e| (e.account_id.clone(), e.credential.clone()))
        };
        let Some((account_id, secret)) = picked else {
            if tried.is_empty() {
                let p = pool.lock().unwrap_or_else(|e| e.into_inner());
                return Err(match p.shortage_reason(&model, now) {
                    Shortage::AllRateLimited => ApiError::RateLimited {
                        retry_after_secs: None,
                        msg: format!("all accounts of {} are rate-limited for {model}", p.provider()),
                    },
                    Shortage::NoAccounts => ApiError::NoAvailableAccount { provider: p.provider().to_string() },
                });
            }
            break; // 账号用尽，带最后错误出循环
        };
        let cred = Credential { account_id: account_id.clone(), secret };
        tried.insert(account_id.clone());
        match provider.complete(&cred, route, req).await {
            Ok(completion) => return Ok(Dispatched { completion, account_id }),
            Err(ProviderError::BadRequest(_)) => {
                return Err(ApiError::Message("request rejected by upstream".into()));
            }
            // 上下文超限是请求本身的确定性失败：换号无用，直接终态（客户端触发压缩）
            Err(ProviderError::ContextWindowExceeded(msg)) => {
                return Err(ApiError::Upstream { status: 400, code: "context_window_exceeded".into(), msg });
            }
            Err(e @ ProviderError::RateLimited { retry_after_secs, .. }) => {
                let until = now + retry_after_secs.unwrap_or(DEFAULT_RATE_LIMIT_SECS);
                pool.lock()
                    .unwrap_or_else(|e2| e2.into_inner())
                    .mark_model_rate_limited(&account_id, &model, until);
                last_err = Some(e);
            }
            Err(e @ ProviderError::Credential(_)) => {
                pool.lock().unwrap_or_else(|e2| e2.into_inner()).mark_dead(&account_id);
                last_err = Some(e);
            }
            Err(e @ ProviderError::Upstream(_)) => {
                last_err = Some(e); // 请求级快切：直接换号
            }
        }
    }
    Err(match last_err {
        Some(ProviderError::RateLimited { retry_after_secs, msg }) => {
            ApiError::RateLimited { retry_after_secs, msg }
        }
        Some(e) => ApiError::RetryExhausted(e.to_string()),
        None => ApiError::NoAvailableAccount { provider: pool.lock().unwrap_or_else(|e| e.into_inner()).provider().to_string() },
    })
}
