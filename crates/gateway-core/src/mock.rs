//! Mock Provider：网关核心自测用（缝 1 的可控上游）。
//! 行为可预测：回显最后一条用户消息并附路由信息，用量按字符数估算。

use crate::openai::{ChatCompletion, ChatRequest, Usage};
use crate::provider::{Provider, ProviderError};
use crate::registry::{ModelInfo, ProviderCatalog};
use crate::route::Route;
use async_trait::async_trait;

#[derive(Clone)]
pub struct MockProvider;

fn approx_tokens(s: &str) -> u64 {
    // 粗略估计：ASCII ~4 字符/token，中文 ~1.5 字符/token；取保守值
    (s.chars().count() as u64 / 2).max(1)
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

    async fn complete(&self, route: &Route, req: &ChatRequest) -> Result<ChatCompletion, ProviderError> {
        let last_user = req
            .messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| m.text())
            .unwrap_or_default();
        let content = format!("[mock:{}:{}] {}", route.provider, route.model, last_user);
        let prompt: u64 = req.messages.iter().map(|m| approx_tokens(&m.text())).sum();
        let usage = Usage::sum(prompt, approx_tokens(&content));
        Ok(ChatCompletion::new(route.composite(), content, usage))
    }
}
