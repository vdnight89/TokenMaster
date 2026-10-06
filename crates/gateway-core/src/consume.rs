//! 消费层：上游文本增量 → 网关 StreamChunk 的归一化处理。
//!
//! think 标签规则（对照 docs/reference/deepseek-harness-codearts.md sse.ts）：
//! - 流式：嗅探开标签 `<think>`（容忍跨 chunk 断裂），闭标签前的正文作为
//!   reasoning 增量、其后作为 content 增量；无标签则原样透传。
//!   裸闭标签（无开标签）在流式下无法零延迟判定，交给非流式路径。
//! - 非流式 `split_think`：按**最后一个** `</think>` 定界，支持裸闭标签；
//!   只有开标签没有闭标签 → 整段视为思考。

use futures::Stream;
use serde_json::Value;
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::provider::{ChunkStream, ProviderError, StreamChunk};

const OPEN: &str = "<think>";
const CLOSE: &str = "</think>";

/// 非流式拆分：返回 (reasoning, content)。
pub fn split_think(text: &str) -> (String, String) {
    if let Some(i) = text.rfind(CLOSE) {
        let head = &text[..i];
        let reasoning = head.strip_prefix(OPEN).unwrap_or(head);
        return (reasoning.to_string(), text[i + CLOSE.len()..].to_string());
    }
    if let Some(rest) = text.strip_prefix(OPEN) {
        return (rest.to_string(), String::new());
    }
    (String::new(), text.to_string())
}

/// 上游 chunk 的思考字段双名归一：`reasoning_content` 优先于 `reasoning`。
pub fn normalize_reasoning_field(chunk: &Value) -> Option<String> {
    let delta = chunk.get("delta")?;
    if let Some(rc) = delta.get("reasoning_content").and_then(Value::as_str) {
        return Some(rc.to_string());
    }
    delta.get("reasoning").and_then(Value::as_str).map(str::to_string)
}

/// `s` 的最长相干后缀：它是 `tag` 的真前缀（可能在未来 chunk 补全成 tag）。
fn held_tag_tail(s: &str, tag: &str) -> usize {
    let b = s.as_bytes();
    let t = tag.as_bytes();
    let max = s.len().min(tag.len() - 1);
    for n in (1..=max).rev() {
        if b[b.len() - n..] == t[..n] {
            return n;
        }
    }
    0
}

/// 流式 think 拆分适配器：包在 Provider 的 ChunkStream 外。
pub fn think_split(inner: ChunkStream) -> ChunkStream {
    Box::pin(ThinkSplit {
        inner,
        mode: Mode::Sniff,
        buf: String::new(),
        out: Vec::new(),
        flushed: false,
    })
}

#[derive(PartialEq)]
enum Mode {
    /// 尚未判定是否是 think 块（还在积累开标签可能的碎片）。
    Sniff,
    Reasoning,
    Content,
}

struct ThinkSplit {
    inner: ChunkStream,
    mode: Mode,
    buf: String,
    out: Vec<Result<StreamChunk, ProviderError>>,
    flushed: bool,
}

impl Stream for ThinkSplit {
    type Item = Result<StreamChunk, ProviderError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if let Some(item) = this.out.pop() {
                return Poll::Ready(Some(item));
            }
            if this.flushed {
                return Poll::Ready(None);
            }
            match Pin::new(&mut this.inner).poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => {
                    this.flush();
                    continue;
                }
                Poll::Ready(Some(item)) => {
                    this.handle(item);
                }
            }
        }
    }
}

impl ThinkSplit {
    fn handle(&mut self, item: Result<StreamChunk, ProviderError>) {
        match item {
            Ok(StreamChunk::Content(s)) => {
                self.buf.push_str(&s);
                self.drain();
            }
            Ok(StreamChunk::Finish { reason, usage }) => {
                self.flush();
                self.out.push(Ok(StreamChunk::Finish { reason, usage }));
            }
            other => {
                // Role/Reasoning 增量与错误原样通过（Reasoning 已是归一结果，不二次处理）。
                self.out.push(other);
            }
        }
    }

    /// 处理 buf 中可判定的部分；不能判定的留在 buf 里等下一个 chunk。
    fn drain(&mut self) {
        match self.mode {
            Mode::Sniff => {
                let lead = self.buf.len() - self.buf.trim_start().len();
                let rest = &self.buf[lead..];
                if rest.is_empty() {
                    return; // 只有空白，继续等
                }
                if rest.len() < OPEN.len() && OPEN.as_bytes().starts_with(rest.as_bytes()) {
                    return; // 可能是未补全的开标签碎片
                }
                if rest.starts_with(OPEN) {
                    // 开标签出现：前导空白 + 标签丢弃，进入思考段
                    self.buf = rest.strip_prefix(OPEN).unwrap_or("").to_string();
                    self.mode = Mode::Reasoning;
                    self.drain();
                } else {
                    self.mode = Mode::Content;
                    self.drain();
                }
            }
            Mode::Reasoning => {
                if let Some(i) = self.buf.find(CLOSE) {
                    let head = self.buf[..i].to_string();
                    if !head.is_empty() {
                        self.out.push(Ok(StreamChunk::Reasoning(head)));
                    }
                    self.buf = self.buf[i + CLOSE.len()..].to_string();
                    self.mode = Mode::Content;
                    self.drain();
                } else {
                    let hold = held_tag_tail(&self.buf, CLOSE);
                    let cut = self.buf.len() - hold;
                    if cut > 0 {
                        let head = self.buf[..cut].to_string();
                        self.buf = self.buf[cut..].to_string();
                        self.out.push(Ok(StreamChunk::Reasoning(head)));
                    }
                }
            }
            Mode::Content => {
                let s = std::mem::take(&mut self.buf);
                if !s.is_empty() {
                    self.out.push(Ok(StreamChunk::Content(s)));
                }
            }
        }
    }

    /// 流结束（或 Finish）时清空缓冲。
    fn flush(&mut self) {
        if self.buf.is_empty() {
            self.flushed = true;
            return;
        }
        let s = std::mem::take(&mut self.buf);
        let chunk = match self.mode {
            Mode::Reasoning => StreamChunk::Reasoning(s),
            _ => StreamChunk::Content(s),
        };
        self.out.push(Ok(chunk));
        self.flushed = true;
    }
}
