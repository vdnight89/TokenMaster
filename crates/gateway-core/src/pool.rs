//! 令牌池：候选过滤（启用 + 未冷却 + 该模型未被限流）→ 选号策略
//! （先到期优先 / 剩余额度最多）。限流状态按「账号 × 模型」记录，
//! 另有账号级全局冷却与停用/失效标记。
//! 「全部限流」与「没有可用账号」必须可区分（spec 用户故事 13）。

use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionStrategy {
    /// 先到期优先：额度/凭据最快作废的先用。
    ExpireFirst,
    /// 剩余额度最多优先。
    MostQuotaLeft,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shortage {
    /// 池里有账号，但对该模型全部处于限流冷却。
    AllRateLimited,
    /// 池为空，或全部停用/失效。
    NoAccounts,
}

#[derive(Debug, Clone)]
pub struct PoolEntry {
    pub account_id: String,
    /// 账号凭据（内存明文；由 Store 解密注入）。
    pub credential: String,
    /// 凭据/额度到期时间戳（秒）；None 表示未知（排在已知之后）。
    pub expires_at: Option<u64>,
    /// 剩余额度（策略 MostQuotaLeft 用）；None 表示未知。
    pub quota_left: Option<u64>,
    pub disabled: bool,
    pub dead: bool,
    /// 账号级全局冷却截止（429 无模型信息时）。
    pub cooldown_until: Option<u64>,
    /// 模型级限流冷却：model → until。
    pub model_limits: HashMap<String, u64>,
}

impl PoolEntry {
    pub fn new(account_id: &str, expires_at: Option<u64>, quota_left: Option<u64>) -> Self {
        Self {
            account_id: account_id.into(),
            credential: String::new(),
            expires_at,
            quota_left,
            disabled: false,
            dead: false,
            cooldown_until: None,
            model_limits: HashMap::new(),
        }
    }

    fn available(&self, model: &str, now: u64) -> bool {
        if self.disabled || self.dead {
            return false;
        }
        if let Some(until) = self.cooldown_until {
            if now < until {
                return false;
            }
        }
        if let Some(until) = self.model_limits.get(model) {
            if now < *until {
                return false;
            }
        }
        true
    }
}

#[derive(Debug)]
pub struct TokenPool {
    provider: String,
    strategy: SelectionStrategy,
    entries: Vec<PoolEntry>,
}

impl TokenPool {
    pub fn new(provider: &str, strategy: SelectionStrategy) -> Self {
        Self { provider: provider.into(), strategy, entries: Vec::new() }
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn set_strategy(&mut self, s: SelectionStrategy) {
        self.strategy = s;
    }

    pub fn strategy(&self) -> SelectionStrategy {
        self.strategy
    }

    /// 插入或按 account_id 更新条目。
    pub fn upsert(&mut self, e: PoolEntry) {
        match self.entries.iter().position(|x| x.account_id == e.account_id) {
            Some(i) => self.entries[i] = e,
            None => self.entries.push(e),
        }
    }

    pub fn entries(&self) -> &[PoolEntry] {
        &self.entries
    }

    /// 可变访问（刷新调度回写凭据等内部维护用）。
    pub fn entries_mut(&mut self) -> &mut [PoolEntry] {
        &mut self.entries
    }

    /// 选号：过滤候选 → 排除 tried → 按策略取最优。
    pub fn pick(&self, model: &str, now: u64, tried: &HashSet<String>) -> Option<&PoolEntry> {
        let mut best: Option<&PoolEntry> = None;
        for e in &self.entries {
            if !e.available(model, now) || tried.contains(&e.account_id) {
                continue;
            }
            best = Some(match best {
                None => e,
                Some(b) => better(e, b, self.strategy),
            });
        }
        best
    }

    /// 账号级全局冷却（429 未指明模型时）。
    pub fn mark_rate_limited(&mut self, account_id: &str, until: u64) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.account_id == account_id) {
            e.cooldown_until = Some(until.max(e.cooldown_until.unwrap_or(0)));
        }
    }

    /// 模型级限流冷却：该账号此模型在 until 前不可选，其他模型不受影响。
    pub fn mark_model_rate_limited(&mut self, account_id: &str, model: &str, until: u64) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.account_id == account_id) {
            let slot = e.model_limits.entry(model.to_string()).or_insert(0);
            *slot = (*slot).max(until);
        }
    }

    pub fn mark_disabled(&mut self, account_id: &str) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.account_id == account_id) {
            e.disabled = true;
        }
    }

    pub fn mark_dead(&mut self, account_id: &str) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.account_id == account_id) {
            e.dead = true;
        }
    }

    pub fn revive(&mut self, account_id: &str) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.account_id == account_id) {
            e.dead = false;
            e.cooldown_until = None;
            e.model_limits.clear();
        }
    }

    /// pick 返回 None 时的原因归类。
    pub fn shortage_reason(&self, model: &str, now: u64) -> Shortage {
        if self.entries.is_empty() {
            return Shortage::NoAccounts;
        }
        let has_alive = self.entries.iter().any(|e| !e.disabled && !e.dead);
        if !has_alive {
            return Shortage::NoAccounts;
        }
        let any_limited = self.entries.iter().any(|e| {
            !e.disabled && !e.dead && !e.available(model, now)
        });
        if any_limited {
            Shortage::AllRateLimited
        } else {
            // 有活账号、无冷却，但 pick 失败 → 只能是全部被 tried 排除
            Shortage::NoAccounts
        }
    }
}

fn better<'a>(a: &'a PoolEntry, b: &'a PoolEntry, strategy: SelectionStrategy) -> &'a PoolEntry {
    match strategy {
        SelectionStrategy::ExpireFirst => match (a.expires_at, b.expires_at) {
            (Some(x), Some(y)) => if x <= y { a } else { b },
            (Some(_), None) => a,
            (None, _) => b,
        },
        SelectionStrategy::MostQuotaLeft => match (a.quota_left, b.quota_left) {
            (Some(x), Some(y)) => if x >= y { a } else { b },
            (Some(_), None) => a,
            (None, _) => b,
        },
    }
}
