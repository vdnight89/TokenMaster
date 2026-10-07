//! Provider 统一接口。M4 起每家上游实现本 trait；
//! 网关只面向 trait 编程，池化/重试在 trait 之外编排。

use std::pin::Pin;

use crate::openai::{ChatCompletion, ChatRequest, Usage};
use crate::registry::ProviderCatalog;
use crate::route::Route;
use async_trait::async_trait;
use futures::Stream;

#[derive(Debug, Clone, thiserror::Error)]
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
    /// 上下文超限（gemini 句式 "The input token count (N) exceeds…" 无 context
    /// 字样，须专属判据）。确定性失败：换号无用，客户端应触发自动压缩。
    #[error("context window exceeded: {0}")]
    ContextWindowExceeded(String),
}

/// 上游流式增量。网关把它翻译成 OpenAI chunk；
/// Reasoning 在 T1.7 消费层归一为 `reasoning_content`。
#[derive(Debug, Clone)]
pub enum StreamChunk {
    Role,
    Content(String),
    Reasoning(String),
    /// OpenAI 形态的工具调用增量分片（index 稳定；id/name 仅首片携带）。
    ToolCallDelta { index: u64, id: Option<String>, name: Option<String>, arguments: String },
    Finish { reason: String, usage: Usage },
}

pub type ChunkStream = Pin<Box<dyn Stream<Item = Result<StreamChunk, ProviderError>> + Send>>;

/// 一次调用的账号凭据：编排层从池中选出账号后注入。
#[derive(Debug, Clone)]
pub struct Credential {
    pub account_id: String,
    pub secret: String,
}

impl Credential {
    /// 无池直连模式（内部直调/单账号 provider）。
    pub fn direct() -> Self {
        Self { account_id: "direct".into(), secret: String::new() }
    }
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &str;
    fn catalog(&self) -> ProviderCatalog;

    /// 非流式补全。
    async fn complete(
        &self,
        cred: &Credential,
        route: &Route,
        req: &ChatRequest,
    ) -> Result<ChatCompletion, ProviderError>;

    /// 流式补全：返回增量流；流自然结束前应产出 `Finish`。
    async fn stream(
        &self,
        cred: &Credential,
        route: &Route,
        req: &ChatRequest,
    ) -> Result<ChunkStream, ProviderError>;

    /// 凭据续期；不可续期的 provider 保持默认（返回 BadRequest），
    /// 调度器对这类账号只探测不更新。
    async fn refresh(&self, _cred: &Credential) -> Result<Credential, ProviderError> {
        Err(ProviderError::BadRequest("provider does not support refresh".into()))
    }
}
