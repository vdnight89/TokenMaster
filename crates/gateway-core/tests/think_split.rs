//! T1.7 消费层（缝 1 黑盒 + 单元）：
//!
//! 1) think 标签流式拆分：`<think>…</think>` 正文 → reasoning_content，
//!    其后 → content；闭标签跨 chunk 断裂要正确；
//! 2) reasoning 双名归一（上游 reasoning_content / reasoning 字段）；
//! 3) 非流式拆分按 dsh 规则（最后一个闭标签定界，支持裸闭）。
//!
//! 行为来源：spec「路由与协议」+ reference/deepseek-harness-codearts.md sse.ts 规则。

use gateway_core::config::{AuthMode, GatewayConfig};
use gateway_core::provider::{ChunkStream, Provider, ProviderError, StreamChunk};
use gateway_core::registry::{ModelInfo, ProviderCatalog, Registry};
use gateway_core::route::Route;
use gateway_core::{consume, openai::ChatRequest};
use async_trait::async_trait;
use futures::StreamExt;
use std::sync::Arc;

/// 脚本化 Provider：按给定片段发 Content，其余同 Mock。
struct Scripted(Vec<String>);
#[async_trait]
impl Provider for Scripted {
    fn id(&self) -> &str { "mock" }
    fn catalog(&self) -> ProviderCatalog {
        ProviderCatalog { id: "mock".into(), models: vec![ModelInfo { id: "mock-alpha".into() }] }
    }
    async fn complete(&self, _c: &gateway_core::Credential, _r: &Route, _q: &ChatRequest) -> Result<gateway_core::openai::ChatCompletion, ProviderError> {
        unimplemented!("think 拆分测试只走流式")
    }
    async fn stream(&self, _c: &gateway_core::Credential, _r: &Route, _q: &ChatRequest) -> Result<ChunkStream, ProviderError> {
        let pieces = self.0.clone();
        Ok(Box::pin(futures::stream::iter(
            pieces.into_iter().map(|p| Ok(StreamChunk::Content(p))).collect::<Vec<_>>(),
        )
        .chain(futures::stream::once(async {
            Ok(StreamChunk::Finish { reason: "stop".into(), usage: gateway_core::openai::Usage::sum(1, 1) })
        }))))
    }
}

async fn sse_of(pieces: Vec<String>) -> (String, String) {
    let c = GatewayConfig {
        auth: AuthMode::Disabled,
        model_map: vec![],
        registry: Registry::with(ProviderCatalog { id: "mock".into(), models: vec![ModelInfo { id: "mock-alpha".into() }] }),
    };
    let h = gateway_core::server::start_with(c, vec![Arc::new(Scripted(pieces))]).await.unwrap();
    let resp = reqwest::Client::new()
        .post(format!("http://{}/v1/chat/completions", h.addr))
        .json(&serde_json::json!({
            "model": "mock/mock-alpha", "stream": true,
            "messages": [{ "role": "user", "content": "x" }]
        }))
        .send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    h.shutdown().await;
    let mut reasoning = String::new();
    let mut content = String::new();
    for line in body.lines() {
        if let Some(p) = line.strip_prefix("data: ") {
            if p == "[DONE]" { break; }
            let v: serde_json::Value = serde_json::from_str(p).unwrap();
            if let Some(rc) = v["choices"].as_array().and_then(|a| a.first()).and_then(|c| c["delta"]["reasoning_content"].as_str()) {
                reasoning.push_str(rc);
            }
            if let Some(cc) = v["choices"].as_array().and_then(|a| a.first()).and_then(|c| c["delta"]["content"].as_str()) {
                content.push_str(cc);
            }
        }
    }
    (reasoning, content)
}

#[tokio::test]
async fn think_block_is_split_across_chunk_boundaries() {
    // 开闭标签都在 chunk 间断裂：<th | ink>secret</th | ink>answer
    let (r, c) = sse_of(vec![
        "<th".into(), "ink>secret</th".into(), "ink>answer part2".into(),
    ]).await;
    assert_eq!(r, "secret");
    assert_eq!(c, "answer part2");
}

#[tokio::test]
async fn content_without_think_tags_passes_through_unchanged() {
    let (r, c) = sse_of(vec!["普通 < 字符".into(), "与文本 1<2".into()]).await;
    assert_eq!(r, "");
    assert_eq!(c, "普通 < 字符与文本 1<2");
}

#[tokio::test]
async fn reasoning_chunks_from_provider_pass_through_untouched() {
    // Provider 已给出 Reasoning 增量（上游有独立思考字段时），消费层不得二次处理
    let c = GatewayConfig {
        auth: AuthMode::Disabled,
        model_map: vec![],
        registry: Registry::with(ProviderCatalog { id: "mock".into(), models: vec![ModelInfo { id: "mock-alpha".into() }] }),
    };
    let s: Vec<Result<StreamChunk, ProviderError>> = vec![
        Ok(StreamChunk::Role),
        Ok(StreamChunk::Reasoning("原始思考".into())),
        Ok(StreamChunk::Content("<无标签正文".into())),
        Ok(StreamChunk::Finish { reason: "stop".into(), usage: gateway_core::openai::Usage::sum(1, 1) }),
    ];
    struct R(Vec<Result<StreamChunk, ProviderError>>);
    #[async_trait]
    impl Provider for R {
        fn id(&self) -> &str { "mock" }
        fn catalog(&self) -> ProviderCatalog { ProviderCatalog { id: "mock".into(), models: vec![ModelInfo { id: "mock-alpha".into() }] } }
        async fn complete(&self, _c: &gateway_core::Credential, _r: &Route, _q: &ChatRequest) -> Result<gateway_core::openai::ChatCompletion, ProviderError> { unimplemented!() }
        async fn stream(&self, _c: &gateway_core::Credential, _r: &Route, _q: &ChatRequest) -> Result<ChunkStream, ProviderError> {
            Ok(Box::pin(futures::stream::iter(self.0.clone())))
        }
    }
    let h = gateway_core::server::start_with(c, vec![Arc::new(R(s))]).await.unwrap();
    let resp = reqwest::Client::new()
        .post(format!("http://{}/v1/chat/completions", h.addr))
        .json(&serde_json::json!({ "model": "mock/mock-alpha", "stream": true, "messages": [{ "role": "user", "content": "x" }] }))
        .send().await.unwrap();
    let body = resp.text().await.unwrap();
    h.shutdown().await;
    assert!(body.contains("\"reasoning_content\":\"原始思考\""));
    assert!(body.contains("<无标签正文"));
}

/* ---- 非流式拆分（dsh 规则：最后一个闭标签定界，支持裸闭） ---- */

#[test]
fn non_stream_split_uses_last_close_tag() {
    let (r, c) = consume::split_think("a</think>b</think>c");
    assert_eq!(r, "a</think>b");
    assert_eq!(c, "c");
}

#[test]
fn non_stream_bare_close_without_open_splits() {
    let (r, c) = consume::split_think("只思考一半</think>结论");
    assert_eq!(r, "只思考一半");
    assert_eq!(c, "结论");
}

#[test]
fn non_stream_no_tags_returns_all_content() {
    let (r, c) = consume::split_think("平平无奇");
    assert_eq!(r, "");
    assert_eq!(c, "平平无奇");
}

#[test]
fn non_stream_unclosed_open_tag_is_all_reasoning() {
    let (r, c) = consume::split_think("<think>想了很多");
    assert_eq!(r, "想了很多");
    assert_eq!(c, "");
}

#[test]
fn reasoning_dual_field_names_are_normalized() {
    use serde_json::json;
    let only_content = json!({ "delta": { "content": "hi" } });
    assert_eq!(consume::normalize_reasoning_field(&only_content), None);
    let rc = json!({ "delta": { "reasoning_content": "A" } });
    assert_eq!(consume::normalize_reasoning_field(&rc).as_deref(), Some("A"));
    let plain = json!({ "delta": { "reasoning": "B" } });
    assert_eq!(consume::normalize_reasoning_field(&plain).as_deref(), Some("B"));
    let both = json!({ "delta": { "reasoning_content": "优先", "reasoning": "忽略" } });
    assert_eq!(consume::normalize_reasoning_field(&both).as_deref(), Some("优先"));
}
