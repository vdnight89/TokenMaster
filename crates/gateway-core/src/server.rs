//! 网关 HTTP 服务：axum 路由 + 鉴权中间件 + 优雅停机。
//! 缝 1 的被测边界——测试进程内 `start_with()` 起真实监听。

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::extract::rejection::JsonRejection;
use axum::extract::{Request, State};
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, HeaderName};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::StreamExt;
use serde_json::{json, Value};

use crate::config::{AuthMode, GatewayConfig};
use crate::error::ApiError;
use crate::openai::{ChatRequest, Usage};
use crate::provider::{Credential, Provider, ProviderError, StreamChunk};
use crate::route::resolve_model;

#[derive(Clone)]
pub struct AppState {
    pub config: GatewayConfig,
    pub providers: HashMap<String, Arc<dyn Provider>>,
    /// 每 Provider 一张令牌池；未配置的 Provider 走无池直连。
    pub pools: HashMap<String, Arc<std::sync::Mutex<crate::pool::TokenPool>>>,
    /// 用量账本（未配置则不记账）。
    pub ledger: Option<Arc<crate::ledger::Ledger>>,
}

pub struct GatewayHandle {
    pub addr: SocketAddr,
    shutdown: tokio::sync::watch::Sender<bool>,
}

impl GatewayHandle {
    /// 优雅停机：等待进行中的请求收尾。
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// 在回环随机端口启动网关（无 Provider 注册；鉴权/模型目录类测试用）。
pub async fn start(config: GatewayConfig) -> std::io::Result<GatewayHandle> {
    start_with(config, Vec::new()).await
}

/// 在回环随机端口启动网关并注册 Provider，返回句柄（缝 1 测试入口）。
pub async fn start_with(
    config: GatewayConfig,
    providers: Vec<Arc<dyn Provider>>,
) -> std::io::Result<GatewayHandle> {
    start_full(config, providers, HashMap::new()).await
}

/// 启动网关：单 Provider + 其令牌池（池化编排路径的测试入口）。
pub async fn start_pooled(
    config: GatewayConfig,
    provider: Arc<dyn Provider>,
    pool: Arc<std::sync::Mutex<crate::pool::TokenPool>>,
) -> std::io::Result<GatewayHandle> {
    let id = provider.id().to_string();
    start_full(config, vec![provider], HashMap::from([(id, pool)])).await
}

pub async fn start_full(
    config: GatewayConfig,
    providers: Vec<Arc<dyn Provider>>,
    pools: HashMap<String, Arc<std::sync::Mutex<crate::pool::TokenPool>>>,
) -> std::io::Result<GatewayHandle> {
    start_with_ledger(config, providers, pools, None).await
}

/// 启动网关并挂账本（缝 1 记账行为的测试入口）。
pub async fn start_with_ledger(
    config: GatewayConfig,
    providers: Vec<Arc<dyn Provider>>,
    pools: HashMap<String, Arc<std::sync::Mutex<crate::pool::TokenPool>>>,
    ledger: Option<Arc<crate::ledger::Ledger>>,
) -> std::io::Result<GatewayHandle> {
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let port = config.port.unwrap_or(0);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let addr = listener.local_addr()?;
    let providers: HashMap<String, Arc<dyn Provider>> =
        providers.into_iter().map(|p| (p.id().to_string(), p)).collect();
    let state = AppState { config, providers, pools, ledger };
    let app = router(state);
    tokio::spawn(async move {
        let serve = axum::serve(listener, app).with_graceful_shutdown(async move {
            let mut rx = shutdown_rx;
            while rx.changed().await.is_ok() {
                if *rx.borrow() {
                    break;
                }
            }
        });
        let _ = serve.await; // 日志系统接入后换成 tracing
    });
    Ok(GatewayHandle { addr, shutdown: shutdown_tx })
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/models", get(list_models))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/messages", post(anthropic_messages))
        .route("/health", get(health))
        .layer(middleware::from_fn_with_state(state.clone(), auth))
        .with_state(state)
}

async fn health() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}

async fn list_models(State(state): State<AppState>) -> impl IntoResponse {
    let data: Vec<serde_json::Value> = state
        .config
        .registry
        .all_models()
        .into_iter()
        .map(|(provider, model)| {
            json!({ "id": format!("{provider}/{model}"), "object": "model", "owned_by": provider })
        })
        .collect();
    Json(json!({ "object": "list", "data": data }))
}

/// 路由解析失败时仍要记账：用请求原文构造一个伪路由（provider 取前缀或 unknown）。
fn fake_route(model: &str) -> crate::route::Route {
    match model.split_once('/') {
        Some((p, m)) if !p.is_empty() && !m.is_empty() => {
            crate::route::Route { provider: p.to_string(), model: m.to_string() }
        }
        _ => crate::route::Route { provider: "unknown".into(), model: model.to_string() },
    }
}

/// 记账：成功与失败都记（spec 用户故事 23）。
fn record_usage(
    state: &AppState,
    route: &crate::route::Route,
    account_id: &str,
    status: u16,
    usage: Option<&crate::openai::Usage>,
    proto: &'static str,
    started: std::time::Instant,
) {
    let Some(ledger) = state.ledger.clone() else { return };
    let (p, c) = usage.map(|u| (u.prompt_tokens, u.completion_tokens)).unwrap_or((0, 0));
    ledger.record(crate::ledger::UsageRecord {
        ts: crate::openai::now_ts(),
        provider: route.provider.clone(),
        account_id: account_id.to_string(),
        model: route.model.clone(),
        prompt_tokens: p,
        completion_tokens: c,
        status,
        ttfb_ms: None,
        duration_ms: started.elapsed().as_millis() as u64,
        proto,
    });
}

async fn chat_completions(
    State(state): State<AppState>,
    payload: Result<Json<ChatRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let started = std::time::Instant::now();
    let Json(req) = payload.map_err(|_| ApiError::Message("invalid or missing JSON body".into()))?;
    let route = match resolve_model(&req.model, &state.config.model_map) {
        Ok(r) => r,
        Err(e) => {
            record_usage(&state, &fake_route(&req.model), "n/a", crate::error::status_u16(&e), None, "openai", started);
            return Err(e);
        }
    };
    let provider = match state.providers.get(&route.provider) {
        Some(p) => p.clone(),
        None => {
            let e = ApiError::ModelNotFound;
            record_usage(&state, &route, "n/a", crate::error::status_u16(&e), None, "openai", started);
            return Err(e);
        }
    };
    if let Some(pool) = state.pools.get(&route.provider) {
        let dispatched =
            match crate::orchestrate::complete_with_retry(pool, provider.as_ref(), &route, &req).await {
                Ok(d) => {
                    record_usage(&state, &route, &d.account_id, 200, Some(&d.completion.usage), "openai", started);
                    d
                }
                Err(e) => {
                    record_usage(&state, &route, "n/a", crate::error::status_u16(&e), None, "openai", started);
                    return Err(e);
                }
            };
        return Ok(Json(serde_json::to_value(dispatched.completion).expect("completion serializes")).into_response());
    }
    if req.stream {
        let chunk_stream = provider
            .stream(&Credential::direct(), &route, &req)
            .await
            .map_err(provider_error)?;
        let split = crate::consume::think_split(chunk_stream);
        Ok(openai_sse(split, route.composite()).into_response())
    } else {
        let completion = provider
            .complete(&Credential::direct(), &route, &req)
            .await
            .map_err(provider_error)?;
        record_usage(&state, &route, "direct", 200, Some(&completion.usage), "openai", started);
        Ok(Json(serde_json::to_value(completion).expect("completion serializes")).into_response())
    }
}

/// Provider 增量流 → OpenAI `chat.completion.chunk` SSE。
/// 结尾：finish chunk → 空 choices 的 usage chunk → `[DONE]`。
fn openai_sse(
    chunk_stream: crate::provider::ChunkStream,
    model: String,
) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let id = format!("chatcmpl-{}", crate::key::random_id(12));
    let created = crate::openai::now_ts();
    let (tail_id, tail_model, tail_created) = (id.clone(), model.clone(), created);
    let base = move |delta: serde_json::Value, finish: Option<&str>| {
        json!({
            "id": id, "object": "chat.completion.chunk", "created": created, "model": model,
            "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }]
        })
    };
    let usage_cell: Arc<Mutex<Option<Usage>>> = Arc::new(Mutex::new(None));
    let cell = usage_cell.clone();
    let mapped = chunk_stream.map(move |item| {
        let ev = match item {
            Ok(StreamChunk::Role) => base(json!({ "role": "assistant", "content": "" }), None),
            Ok(StreamChunk::Content(s)) => base(json!({ "content": s }), None),
            Ok(StreamChunk::Reasoning(s)) => base(json!({ "reasoning_content": s }), None),
            Ok(StreamChunk::ToolCallDelta { index, id, name, arguments }) => {
                let mut tc = json!({ "index": index, "function": { "arguments": arguments } });
                if let Some(i) = id {
                    tc["id"] = json!(i);
                }
                if let Some(n) = name {
                    tc["function"]["name"] = json!(n);
                }
                base(json!({ "tool_calls": [tc] }), None)
            }
            Ok(StreamChunk::Finish { reason, usage }) => {
                *cell.lock().unwrap_or_else(|p| p.into_inner()) = Some(usage);
                base(json!({}), Some(&reason))
            }
            Err(e) => base(json!({ "refusal": e.to_string() }), Some("error")),
        };
        Ok::<_, Infallible>(Event::default().data(serde_json::to_string(&ev).expect("chunk serializes")))
    });
    let tail_cell = usage_cell;
    let tail = futures::stream::once(async move {
        let usage = tail_cell.lock().unwrap_or_else(|p| p.into_inner()).unwrap_or_default();
        let usage_chunk = json!({
            "id": tail_id, "object": "chat.completion.chunk", "created": tail_created, "model": tail_model,
            "choices": [], "usage": usage
        });
        futures::stream::iter(vec![
            Ok::<_, Infallible>(Event::default().data(serde_json::to_string(&usage_chunk).expect("usage serializes"))),
            Ok(Event::default().data("[DONE]")),
        ])
    })
    .flatten();
    Sse::new(mapped.chain(tail)).keep_alive(KeepAlive::default())
}

/// Anthropic 协议面：请求转换 → 同一条 Provider 管线 → 响应/SSE 转换。
async fn anthropic_messages(
    State(state): State<AppState>,
    payload: Result<Json<crate::anthropic::AnthropicRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(areq) = payload.map_err(|_| ApiError::Message("invalid or missing JSON body".into()))?;
    let stream = areq.stream;
    let req = areq.into_chat_request();
    let route = resolve_model(&req.model, &state.config.model_map)?;
    let provider = state.providers.get(&route.provider).ok_or(ApiError::ModelNotFound)?.clone();
    if stream {
        let chunk_stream = provider
            .stream(&Credential::direct(), &route, &req)
            .await
            .map_err(provider_error)?;
        let split = crate::consume::think_split(chunk_stream);
        Ok(anthropic_sse(split, route.composite(), &req).into_response())
    } else {
        let completion = provider
            .complete(&Credential::direct(), &route, &req)
            .await
            .map_err(provider_error)?;
        // 记账（与 OpenAI 面同一口径）
        if let Some(ledger) = &state.ledger {
            ledger.record(crate::ledger::UsageRecord {
                ts: now_secs(),
                provider: route.provider.clone(),
                account_id: "direct".into(),
                model: route.model.clone(),
                prompt_tokens: completion.usage.prompt_tokens,
                completion_tokens: completion.usage.completion_tokens,
                status: 200,
                ttfb_ms: None,
                duration_ms: 0,
                proto: "anthropic",
            });
        }
        Ok(Json(crate::anthropic::message_from_completion(&completion)).into_response())
    }
}

/// StreamChunk 流 → anthropic SSE 事件序列。
fn anthropic_sse(
    chunk_stream: crate::provider::ChunkStream,
    model: String,
    req: &crate::openai::ChatRequest,
) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let input_tokens: u64 = req
        .messages
        .iter()
        .map(|m| (m.text().chars().count() as u64 / 2).max(1))
        .sum();
    let mut builder = crate::anthropic::AnthropicEventBuilder::new(model.clone(), input_tokens);
    let start = builder.message_start();
    let head = futures::stream::iter(vec![Ok::<_, Infallible>(
        Event::default().event("message_start").data(serde_json::to_string(&start).expect("event serializes")),
    )]);
    let mapped = chunk_stream.flat_map(move |item| {
        let events: Vec<Value> = match item {
            Ok(crate::provider::StreamChunk::Role) => vec![],
            Ok(crate::provider::StreamChunk::Content(s)) => builder.on_content(&s),
            Ok(crate::provider::StreamChunk::Reasoning(s)) => builder.on_reasoning(&s),
            Ok(crate::provider::StreamChunk::Finish { reason, usage }) => builder.on_finish(&reason, &usage),
            Ok(crate::provider::StreamChunk::ToolCallDelta { .. }) => vec![], // 工具块在 M4 接入时补
            Err(_) => vec![],
        };
        futures::stream::iter(
            events
                .into_iter()
                .map(|v| {
                    let name = v["type"].as_str().unwrap_or("event").to_string();
                    let payload = serde_json::to_string(&v).expect("event serializes");
                    Ok::<_, Infallible>(Event::default().event(name).data(payload))
                })
                .collect::<Vec<_>>(),
        )
    });
    Sse::new(head.chain(mapped)).keep_alive(KeepAlive::default())
}

/// Provider 故障映射为对外错误。M2 编排层在此之前完成换号重试；
/// 到达这里的都是重试耗尽后的终态。
fn provider_error(e: ProviderError) -> ApiError {
    match e {
        ProviderError::RateLimited { retry_after_secs, msg } => {
            ApiError::RateLimited { retry_after_secs, msg }
        }
        ProviderError::Credential(msg) => ApiError::Upstream { status: 401, code: "provider_credentials_rejected".into(), msg },
        ProviderError::BadRequest(msg) => ApiError::Message(msg),
        ProviderError::Upstream(msg) => ApiError::Upstream { status: 502, code: "upstream_error".into(), msg },
        ProviderError::ContextWindowExceeded(msg) => ApiError::Upstream { status: 400, code: "context_window_exceeded".into(), msg },
    }
}

/// 鉴权：Required 模式下接受 `Authorization: Bearer <key>` 或 `x-api-key: <key>`。
async fn auth(
    State(state): State<AppState>,
    headers: HeaderMap,
    req: Request,
    next: Next,
) -> Response {
    let required = match &state.config.auth {
        AuthMode::Disabled => return next.run(req).await,
        AuthMode::Required(k) => k.clone(),
    };
    let bearer = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let api_key = headers
        .get(HeaderName::from_static("x-api-key"))
        .and_then(|v| v.to_str().ok());
    let ok = bearer == Some(required.as_str()) || api_key == Some(required.as_str());
    if ok {
        next.run(req).await
    } else {
        ApiError::InvalidApiKey.into_response()
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
