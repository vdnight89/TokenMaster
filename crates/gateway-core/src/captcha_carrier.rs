//! T6.3 验证码载体：WebView 子窗口按需拉起 + 供应池退避。
//!
//! 设计（ADR-0006）：Tauri 侧开**隐藏 WebView 窗口**加载 zcode.z.ai origin
//! 的载体页，执行 AliyunCaptcha SDK 产出 param。对应 DH 的「桌面内部载体」
//! 方案；外挂 chromium（Chrome/Edge CDP）是兜底。
//!
//! 核心流程：
//! 1. `CaptchaSupplyPool` 维护 param 队列（预产出 + 按需拉起）
//! 2. `claim` 遇 3007 → `acquire_param()` 从池取（池空则触发拉起）
//! 3. 拉起 = 打开隐藏 WebView 加载 `https://zcode.z.ai/` origin 载体页
//! 4. 产出 param 后入池，claim 重试
//! 5. 连续失败退避（30s → 60s → 120s 上限 5 分钟）
//!
//! 本模块是纯 Rust 的池与退避逻辑；WebView 窗口的创建/销毁由 Tauri 壳层
//! （src-tauri）调 `open_carrier_window` 回调注入。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 一个验证码 param（阿里云 captcha 凭据）。
#[derive(Debug, Clone)]
pub struct CaptchaParam {
    pub param: String,
    pub produced_at: Instant,
    pub region: String,
}

/// 验证码供应池（预产出 + 退避）。
pub struct CaptchaSupplyPool {
    /// 就绪的 param 队列（FIFO，先进先用）。
    ready: Mutex<VecDeque<CaptchaParam>>,
    /// 连续失败计数（退避用）。
    consecutive_failures: Mutex<u32>,
    /// 下次允许拉起的时间（退避到期）。
    next_carrier_at: Mutex<Option<Instant>>,
    /// param 有效期（超时丢弃）。
    param_ttl: Duration,
    /// 池容量上限。
    max_pool: usize,
}

impl CaptchaSupplyPool {
    pub fn new() -> Self {
        Self {
            ready: Mutex::new(VecDeque::new()),
            consecutive_failures: Mutex::new(0),
            next_carrier_at: Mutex::new(None),
            param_ttl: Duration::from_secs(120), // 阿里云 captcha param 有效期约 2 分钟
            max_pool: 3,
        }
    }

    /// 入池一个 param（Tauri 载体产出后调用）。
    pub fn push_param(&self, param: &str, region: &str) {
        let mut q = self.ready.lock().unwrap();
        if q.len() < self.max_pool {
            q.push_back(CaptchaParam {
                param: param.to_string(),
                produced_at: Instant::now(),
                region: region.to_string(),
            });
        }
        // 成功产出 → 重置退避
        *self.consecutive_failures.lock().unwrap() = 0;
        *self.next_carrier_at.lock().unwrap() = None;
    }

    /// 从池取一个 param（claim 遇 3007 时调用）。
    /// 过期的 param 自动丢弃。
    pub fn acquire_param(&self) -> Option<CaptchaParam> {
        let mut q = self.ready.lock().unwrap();
        while let Some(front) = q.front() {
            if front.produced_at.elapsed() > self.param_ttl {
                q.pop_front(); // 过期丢弃
                continue;
            }
            return q.pop_front();
        }
        None
    }

    /// 是否需要拉起载体（池空且退避已过）。
    pub fn should_launch_carrier(&self) -> bool {
        if self.acquire_param().is_some() {
            return false; // 池里还有
        }
        let next = self.next_carrier_at.lock().unwrap();
        match *next {
            Some(t) => Instant::now() >= t,
            None => true, // 无退避
        }
    }

    /// 记录一次载体失败（拉起后未产出 param）→ 退避递增。
    /// 30s → 60s → 120s → 300s（上限 5 分钟）。
    pub fn record_carrier_failure(&self) -> Duration {
        let mut fails = self.consecutive_failures.lock().unwrap();
        *fails += 1;
        let backoff = match *fails {
            1 => Duration::from_secs(30),
            2 => Duration::from_secs(60),
            3 => Duration::from_secs(120),
            _ => Duration::from_secs(300),
        };
        *self.next_carrier_at.lock().unwrap() = Some(Instant::now() + backoff);
        backoff
    }

    /// 当前池大小（可读的 param 数，含过期判定）。
    pub fn ready_count(&self) -> usize {
        let q = self.ready.lock().unwrap();
        q.iter().filter(|p| p.produced_at.elapsed() <= self.param_ttl).count()
    }
}

impl Default for CaptchaSupplyPool {
    fn default() -> Self {
        Self::new()
    }
}

/// 载体窗口控制（由 Tauri 壳注入实现）。
pub struct CarrierController {
    pool: Arc<CaptchaSupplyPool>,
    /// Tauri 壳层的「打开载体窗口」回调（产出 param 后调 pool.push_param）。
    #[allow(clippy::type_complexity)]
    open_window: Box<dyn Fn() + Send + Sync>,
}

impl CarrierController {
    pub fn new(pool: Arc<CaptchaSupplyPool>, open_window: Box<dyn Fn() + Send + Sync>) -> Self {
        Self { pool, open_window }
    }

    /// claim 遇 3007 → 调此方法。
    /// 池有 param 直接返回；否则检查退避并触发窗口拉起。
    /// 返回 Some(param) = 可用；None = 需要用户在载体窗口完成验证。
    pub fn on_captcha_required(&self) -> Option<CaptchaParam> {
        if let Some(p) = self.pool.acquire_param() {
            return Some(p);
        }
        if self.pool.should_launch_carrier() {
            (self.open_window)();
        }
        None // 池空且已触发拉起（或退避中）——本轮 claim 放弃，等下次
    }

    /// 载体窗口产出 param 后调此方法（由 Tauri WebView 的 JS bridge 调）。
    pub fn on_param_produced(&self, param: &str, region: &str) {
        self.pool.push_param(param, region);
    }

    /// 载体窗口失败（超时/用户关闭）→ 退避。
    pub fn on_carrier_failed(&self) -> Duration {
        self.pool.record_carrier_failure()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_push_and_acquire() {
        let pool = CaptchaSupplyPool::new();
        pool.push_param("cap-abc-123", "cn");
        assert_eq!(pool.ready_count(), 1);
        let p = pool.acquire_param().unwrap();
        assert_eq!(p.param, "cap-abc-123");
        assert_eq!(p.region, "cn");
        assert_eq!(pool.ready_count(), 0);
        assert!(pool.acquire_param().is_none());
    }

    #[test]
    fn pool_capacity_limit() {
        let pool = CaptchaSupplyPool::new();
        for i in 0..5 {
            pool.push_param(&format!("cap-{i}"), "cn");
        }
        assert_eq!(pool.ready_count(), 3, "max_pool=3");
    }

    #[test]
    fn backoff_sequence() {
        let pool = CaptchaSupplyPool::new();
        assert_eq!(pool.record_carrier_failure(), Duration::from_secs(30));
        assert!(!pool.should_launch_carrier(), "退避中不拉起");
        assert_eq!(pool.record_carrier_failure(), Duration::from_secs(60));
        assert_eq!(pool.record_carrier_failure(), Duration::from_secs(120));
        assert_eq!(pool.record_carrier_failure(), Duration::from_secs(300));
        assert_eq!(pool.record_carrier_failure(), Duration::from_secs(300), "上限 5 分钟");
        // 成功后重置退避
        pool.push_param("ok", "cn");
        assert_eq!(pool.ready_count(), 1, "param 入池");
        assert!(pool.acquire_param().is_some(), "取出 param");
        // 池空但退避已重置 → 可拉起
        assert!(pool.should_launch_carrier(), "成功后无退避");
    }

    #[test]
    fn carrier_controller_flow() {
        let pool = Arc::new(CaptchaSupplyPool::new());
        let launched = Arc::new(Mutex::new(false));
        let l2 = launched.clone();
        let ctrl = CarrierController::new(pool.clone(), Box::new(move || {
            *l2.lock().unwrap() = true;
        }));

        // 池空 → 触发拉起 → 返回 None
        assert!(ctrl.on_captcha_required().is_none());
        assert!(*launched.lock().unwrap());

        // 产出 param 后可取
        ctrl.on_param_produced("param-xyz", "cn");
        let p = ctrl.on_captcha_required().unwrap();
        assert_eq!(p.param, "param-xyz");
    }
}
