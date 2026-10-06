//! zcode（智谱 ZCode/z.ai coding plan）Provider。
//!
//! 上游（start-plan 通道）：`POST {base}/api/v1/zcode-plan/anthropic/v1/messages`
//! ——Anthropic Messages 协议，Bearer JWT（zcodejwttoken）+ 身份头。
//! coding-plan 订阅通道（api.z.ai/api/anthropic，OAuth 换 coding_plan_key）在
//! 登录切片（T4.1b）接入。
//!
//! 错误语义（对照 reference/deepseek-harness-codearts.md zcode 节）：
//! - 401 → Credential（JWT 失效，不可续期，需重登）
//! - 429 → RateLimited（读 Retry-After，缺省 60s）
//! - 3012（身份块准入，HTTP 405）→ RateLimited 30 分钟（账号冷却惩罚）
//! - 3007（验证码）→ RateLimited（验证码载体链路在 T6.3 接入后自动恢复）
//!
//! 3012 要求 system 携带官方身份块（cliPrefix+stable 2898 字符，逐字官方文本）。
//! 真实上游前必须通过 `with_system_prefix` 注入从本机 ZCode 安装提取的块
//! （zcode-pool prompt.rs 的做法）；stub 测试不需要。

use async_trait::async_trait;
use serde_json::Value;

use crate::anthropic::{completion_from_anthropic, openai_to_anthropic_body};
use crate::openai::{ChatCompletion, ChatRequest};
use crate::provider::{ChunkStream, Credential, Provider, ProviderError};
use crate::registry::{ModelInfo, ProviderCatalog};
use crate::route::Route;

pub const DEFAULT_BASE: &str = "https://zcode.z.ai";
pub const DEFAULT_CLIENT_VERSION: &str = "3.14.4";

/// start-plan 通道推理路径。
const MESSAGES_PATH: &str = "/api/v1/zcode-plan/anthropic/v1/messages";

/// 3012 的账号冷却（30 分钟，对照 dsh 消融实测）。
const IDENTITY_COOLDOWN_SECS: u64 = 30 * 60;

pub struct ZcodeProvider {
    base: String,
    client: reqwest::Client,
    version: String,
    device_mid: String,
    system_prefix: Option<String>,
}

impl ZcodeProvider {
    pub fn new(base: String) -> Self {
        Self {
            base,
            client: reqwest::Client::new(),
            version: DEFAULT_CLIENT_VERSION.into(),
            device_mid: format!(
                "{}-{}-{}-{}",
                crate::key::random_id(4),
                crate::key::random_id(2),
                crate::key::random_id(2),
                crate::key::random_id(6)
            ),
            system_prefix: None,
        }
    }

    pub fn production() -> Self {
        Self::new(DEFAULT_BASE.into())
    }

    /// 注入官方身份块（zcode-pool prompt.rs 从本机 ZCode 提取；真实上游必需）。
    pub fn with_system_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.system_prefix = Some(prefix.into());
        self
    }

    fn identity_headers(&self, cred: &Credential) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        let ins = reqwest::header::HeaderValue::from_str;
        let _ = ins(&format!("Bearer {}", cred.secret)).map(|v| h.insert("authorization", v));
        let _ = ins("2023-06-01").map(|v| h.insert("anthropic-version", v));
        let _ = ins(&format!("ZCode/{}", self.version)).map(|v| h.insert("user-agent", v));
        let _ = ins("https://zcode.z.ai").map(|v| h.insert("http-referer", v));
        let _ = ins(&self.version).map(|v| h.insert("x-zcode-app-version", v));
        let _ = ins("stable").map(|v| h.insert("x-release-channel", v));
        let _ = ins("zh-CN").map(|v| h.insert("x-client-language", v));
        let _ = ins("Asia/Shanghai").map(|v| h.insert("x-client-timezone", v));
        let _ = ins(&self.device_mid).map(|v| h.insert("x-device-mid", v));
        let _ = ins("win32").map(|v| h.insert("x-platform", v));
        let _ = ins("windows").map(|v| h.insert("x-os-category", v));
        h
    }

    async fn post_messages(&self, cred: &Credential, route: &Route, req: &ChatRequest) -> Result<ChatCompletion, ProviderError> {
        // 上游要裸模型名（route.model 已剥离 provider 前缀）
        let mut upstream_req = req.clone();
        upstream_req.model = route.model.clone();
        let body = openai_to_anthropic_body(&upstream_req, self.system_prefix.as_deref());
        let resp = self
            .client
            .post(format!("{}{}", self.base, MESSAGES_PATH))
            .headers(self.identity_headers(cred))
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status();
        if status.as_u16() == 200 {
            let v: Value = resp.json().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
            return completion_from_anthropic(&route.composite(), &v)
                .map_err(ProviderError::Upstream);
        }
        let retry_after = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());
        let text = resp.text().await.unwrap_or_default();
        match status.as_u16() {
            401 => Err(ProviderError::Credential(format!("401 jwt rejected: {}", truncate(&text)))),
            429 => Err(ProviderError::RateLimited {
                retry_after_secs: Some(retry_after.unwrap_or(60)),
                msg: truncate(&text),
            }),
            // 3012：官方以 405 + code 3012 表达身份块准入失败（dsh 实测）
            405 if text.contains("3012") => Err(ProviderError::RateLimited {
                retry_after_secs: Some(IDENTITY_COOLDOWN_SECS),
                msg: format!("identity block rejected (3012): {}", truncate(&text)),
            }),
            400 if text.contains("3007") => Err(ProviderError::RateLimited {
                retry_after_secs: Some(60),
                msg: "captcha required (3007)".into(),
            }),
            code => Err(ProviderError::Upstream(format!("http {code}: {}", truncate(&text)))),
        }
    }
}

fn truncate(s: &str) -> String {
    s.chars().take(180).collect()
}

#[async_trait]
impl Provider for ZcodeProvider {
    fn id(&self) -> &str {
        "zcode"
    }

    fn catalog(&self) -> ProviderCatalog {
        ProviderCatalog {
            id: "zcode".into(),
            models: vec![
                ModelInfo { id: "glm-4.7".into() },
                ModelInfo { id: "glm-4.7-air".into() },
                ModelInfo { id: "glm-4.6".into() },
            ],
        }
    }

    async fn complete(&self, cred: &Credential, route: &Route, req: &ChatRequest) -> Result<ChatCompletion, ProviderError> {
        self.post_messages(cred, route, req).await
    }

    async fn stream(&self, _cred: &Credential, _route: &Route, _req: &ChatRequest) -> Result<ChunkStream, ProviderError> {
        // T4.1b：上游 SSE（anthropic 事件流）→ StreamChunk 转换
        Err(ProviderError::Upstream("zcode streaming arrives in T4.1b".into()))
    }
}
