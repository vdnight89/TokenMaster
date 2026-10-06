//! Provider 统一接口。M4 起每家上游实现本 trait；
//! 网关只面向 trait 编程，池化/重试在 trait 之外编排。

use std::pin::Pin;

use crate::openai::{ChatCompletion, ChatRequest, Usage};
use crate::registry::ProviderCatalog;
use crate::route::Route;
use async_trait::async_trait;
use futures::Stream;

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// 账号/凭据问题——编排层应换号重试（401/402/403 语义）。
    #[error("credential rejected: {0}")]
    Credential(String),
    /// 上游限流——编排层按 Retry-After 冷却后换号（429 语义）。
    #[error("rate limited: retry after {retry_after_secs:?}s: {msg}")]
    RateLimited { retry_after_secs: Option<u64>, msg: String },
    /// 请求本身的问题，换号也无济于事。
    #[error("invalid request: {0}")]
    BadRequest(String),
    /// 网络/上游故障，可换号或退避重试。
    #[error("upstream failure: {0}")]
    Upstream(String),
}

/// 上游流式增量。网关把它翻译成 OpenAI chunk；
/// Reasoning 在 T1.7 消费层归一为 `reasoning_content`。
#[derive(Debug, Clone)]
pub enum StreamChunk {
    Role,
    Content(String),
    Reasoning(String),
    Finish { reason: String, usage: Usage },
}

pub type ChunkStream = Pin<Box<dyn Stream<Item = Result<StreamChunk, ProviderError>> + Send>>;

#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &str;
    fn catalog(&self) -> ProviderCatalog;

    /// 非流式补全。
    async fn complete(&self, route: &Route, req: &ChatRequest) -> Result<ChatCompletion, ProviderError>;

    /// 流式补全：返回增量流；流自然结束前应产出 `Finish`。
    async fn stream(&self, route: &Route, req: &ChatRequest) -> Result<ChunkStream, ProviderError>;
}
