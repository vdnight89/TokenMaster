//! Mock Provider：网关核心自测用（缝 1 的可控上游）。
//! 行为可预测：回显最后一条用户消息并附路由信息，用量按字符数估算。

use crate::openai::{ChatCompletion, ChatRequest, Usage};
use crate::provider::{ChunkStream, Credential, Provider, ProviderError, StreamChunk};
use crate::registry::{ModelInfo, ProviderCatalog};
use crate::route::Route;
use async_trait::async_trait;
use futures::StreamExt;

#[derive(Clone)]
pub struct MockProvider;

fn approx_tokens(s: &str) -> u64 {
    // 粗略估计：中文约 2 字符/token、英文约 4 字符/token；取保守值
    (s.chars().count() as u64 / 2).max(1)
}

impl MockProvider {
    fn plan(&self, _cred: &Credential, route: &Route, req: &ChatRequest) -> (String, Usage) {
        let last_user = req
            .messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| m.text())
            .unwrap_or_default();
        let content = format!("[mock:{}:{}] {}", route.provider, route.model, last_user);
        let prompt: u64 = req.messages.iter().map(|m| approx_tokens(&m.text())).sum();
        (content.clone(), Usage::sum(prompt, approx_tokens(&content)))
    }
}

#[async_trait]
impl Provider for MockProvider {
    fn id(&self) -> &str {
        "mock"
    }

    fn catalog(&self) -> ProviderCatalog {
        ProviderCatalog {
            id: "mock".into(),
            models: vec![
                ModelInfo { id: "mock-alpha".into() },
                ModelInfo { id: "mock-beta".into() },
            ],
        }
    }

    async fn complete(&self, cred: &Credential, route: &Route, req: &ChatRequest) -> Result<ChatCompletion, ProviderError> {
        let (content, usage) = self.plan(cred, route, req);
        Ok(ChatCompletion::new(route.composite(), content, usage))
    }

    async fn stream(&self, cred: &Credential, route: &Route, req: &ChatRequest) -> Result<ChunkStream, ProviderError> {
        let (content, usage) = self.plan(cred, route, req);
        // 分约 3 段产出，末段前稍作停顿，模拟真实上游节奏。
        let per = content.chars().count().div_ceil(3).max(1);
        let mut pieces: Vec<String> = Vec::new();
        let mut rest: String = content;
        while !rest.is_empty() {
            let cut = rest.char_indices().nth(per).map(|(i, _)| i).unwrap_or(rest.len());
            let (head, tail) = rest.split_at(cut);
            pieces.push(head.to_string());
            rest = tail.to_string();
        }
        let s = futures::stream::iter(vec![Ok::<_, ProviderError>(StreamChunk::Role)])
            .chain(futures::stream::iter(pieces.into_iter().map(|p| Ok(StreamChunk::Content(p)))))
            .chain(futures::stream::once(async move {
                tokio::time::sleep(std::time::Duration::from_millis(15)).await;
                Ok(StreamChunk::Finish { reason: "stop".into(), usage })
            }));
        Ok(Box::pin(s))
    }
}
