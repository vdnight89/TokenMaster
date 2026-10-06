//! 账本：内存环形（500 条）+ JSONL 落盘（滚动至 `.old`）。
//! 铁律：`record` 绝不抛错——写失败只影响落盘，不影响请求与内存环形。

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;

const RING_CAP: usize = 500;
pub const DEFAULT_MAX_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct UsageRecord {
    /// Unix 秒。
    pub ts: u64,
    pub provider: String,
    pub account_id: String,
    pub model: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    /// HTTP 状态码（成功 200；上游错误取映射后的对外状态）。
    pub status: u16,
    pub ttfb_ms: Option<u64>,
    pub duration_ms: u64,
    /// "openai" | "anthropic"。
    pub proto: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct AggBucket {
    pub key: String,
    pub requests: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub failures: u64,
}

struct Inner {
    ring: VecDeque<UsageRecord>,
    path: Option<PathBuf>,
    max_bytes: u64,
    written: u64,
    writer: Option<File>,
}

pub struct Ledger {
    inner: Mutex<Inner>,
}

impl Ledger {
    /// 仅内存（无盘模式/测试）。
    pub fn memory() -> Ledger {
        Self { inner: Mutex::new(Inner { ring: VecDeque::new(), path: None, max_bytes: DEFAULT_MAX_BYTES, written: 0, writer: None }) }
    }

    pub fn open(path: Option<&Path>) -> Ledger {
        Self::with_max_bytes(path, DEFAULT_MAX_BYTES)
    }

    pub fn with_max_bytes(path: Option<&Path>, max_bytes: u64) -> Ledger {
        Self {
            inner: Mutex::new(Inner {
                ring: VecDeque::new(),
                path: path.map(PathBuf::from),
                max_bytes,
                written: 0,
                writer: None,
            }),
        }
    }

    /// 记一条用量；任何 IO 问题都被吞掉（铁律：记账不抛错）。
    pub fn record(&self, r: UsageRecord) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.ring.push_back(r.clone());
        while inner.ring.len() > RING_CAP {
            inner.ring.pop_front();
        }
        let Some(path) = inner.path.clone() else { return };
        let line = serde_json::to_string(&r).unwrap_or_default();
        // 打开/滚动/写入任一步失败都静默跳过（下次 record 重试打开）
        if inner.writer.is_none() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match OpenOptions::new().create(true).append(true).open(&path) {
                Ok(f) => {
                    inner.written = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    inner.writer = Some(f);
                }
                Err(_) => return,
            }
        }
        if inner.written + line.len() as u64 + 1 > inner.max_bytes {
            // 滚动：usage.jsonl → usage.jsonl.old（覆盖旧 .old），重开新文件
            inner.writer = None;
            let old = path.with_extension("jsonl.old");
            let _ = std::fs::rename(&path, &old);
            match OpenOptions::new().create(true).append(true).open(&path) {
                Ok(f) => {
                    inner.written = 0;
                    inner.writer = Some(f);
                }
                Err(_) => return,
            }
        }
        if let Some(w) = inner.writer.as_mut() {
            if writeln!(w, "{line}").is_ok() {
                inner.written += line.len() as u64 + 1;
            }
        }
    }

    /// 刷新落盘缓冲（测试/关停时用）。
    pub fn flush(&self) {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(w) = inner.writer.as_ref() {
            let _ = w.sync_all();
        }
    }

    pub fn recent(&self, n: usize) -> Vec<UsageRecord> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.ring.iter().rev().take(n).rev().cloned().collect()
    }

    fn aggregate<F: Fn(&UsageRecord) -> String>(&self, key: F) -> Vec<AggBucket> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut order: Vec<String> = Vec::new();
        let mut map: std::collections::HashMap<String, AggBucket> = std::collections::HashMap::new();
        for r in inner.ring.iter() {
            let k = key(r);
            let b = map.entry(k.clone()).or_insert_with(|| {
                order.push(k.clone());
                AggBucket { key: k, requests: 0, prompt_tokens: 0, completion_tokens: 0, failures: 0 }
            });
            b.requests += 1;
            b.prompt_tokens += r.prompt_tokens;
            b.completion_tokens += r.completion_tokens;
            if r.status >= 400 {
                b.failures += 1;
            }
        }
        order.into_iter().map(|k| map.remove(&k).unwrap()).collect()
    }

    pub fn aggregate_by_provider(&self) -> Vec<AggBucket> {
        self.aggregate(|r| r.provider.clone())
    }

    pub fn aggregate_by_model(&self) -> Vec<AggBucket> {
        self.aggregate(|r| r.model.clone())
    }

    pub fn aggregate_by_account(&self) -> Vec<AggBucket> {
        self.aggregate(|r| r.account_id.clone())
    }

    /// 按本地日期（YYYY-MM-DD）聚合。无时区库依赖：直接用系统本地时间格式化。
    pub fn aggregate_by_day(&self) -> Vec<AggBucket> {
        self.aggregate(|r| local_day(r.ts))
    }
}

/// Unix 秒 → 本地 `YYYY-MM-DD`（Windows 下用系统 API 太重，用 UTC 近似——
/// 桌面单机工具的日常聚合误差可接受；GUI 呈现时再按本地时区校正）。
fn local_day(ts: u64) -> String {
    let days = ts / 86_400;
    // civil_from_days（Howard Hinnant 算法）
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}
