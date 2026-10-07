//! qodercn Provider（Qoder 中国版，T4.6）。
//!
//! 与国际版（qoder.rs）共用全部实现逻辑，仅换产品常量
//! （对照参考 qoder-product.ts:584-660 的 `QODER_CN`）：
//! - auth=qoder.cn、openapi=openapi.qoder.com.cn、加密推理=gateway.qoder.com.cn
//! - client_id `732aef47-…`（与国际版完全不同，用错授权后报参数无效）
//! - session_type CN=`qoder_work`（国际版=qodercli）
//! - Cosy-ClientType 同 `'10'`、UA 同 `qoder` 前缀

use crate::providers::qoder::{
    QoderDeviceSession, QoderOAuth, QoderProvider,
};
use crate::provider::{Credential, ProviderError};

pub const QODERCN_AUTH_BASE: &str = "https://qoder.cn";
pub const QODERCN_OPENAPI_BASE: &str = "https://openapi.qoder.com.cn";
pub const QODERCN_ENCRYPTED_INFER_BASE: &str = "https://gateway.qoder.com.cn";
/// CN asar `Vpe.authClientIds.prod`——与国际版**完全不同**
/// （qoder-product.ts:606-608；用错的症状是授权页 302 正常、点授权后报参数无效）。
pub const QODERCN_CLIENT_ID: &str = "732aef47-9cf2-46a2-95fe-4cebb5d0d1fa";
/// CN session_type（qoder-wasm.ts:199——国际版是 `qodercli`）。
pub const QODERCN_SESSION_TYPE: &str = "qoder_work";

/// CN 版授权 URL（auth base 换 qoder.cn，client_id 换 CN 值）。
pub fn build_qodercn_auth_url(sess: &QoderDeviceSession) -> String {
    format!(
        "{}/device/selectAccounts?challenge={}&challenge_method=S256&nonce={}&machine_id={}&client_id={}",
        QODERCN_AUTH_BASE, sess.challenge, sess.nonce, sess.machine_id, QODERCN_CLIENT_ID
    )
}

pub struct QodercnProvider {
    inner: QoderProvider,
    oauth: QoderOAuth,
}

impl QodercnProvider {
    pub fn production() -> Self {
        Self::with_openapi_base(QODERCN_OPENAPI_BASE.into())
    }

    pub fn with_openapi_base(openapi: String) -> Self {
        Self {
            inner: QoderProvider::new(openapi.clone()),
            oauth: QoderOAuth::new_for_test(openapi),
        }
    }

    /// CN 设备码登录（走 CN 端点，凭据同国际版形态）。
    pub async fn login_with_interval(
        &self,
        interval: std::time::Duration,
    ) -> Result<Credential, ProviderError> {
        let sess = self.oauth.create_device_session();
        self.oauth.poll_until_token(&sess, interval).await
    }

    /// 余额（透传到 qoder 实现——请求路径/Cosy 头与产品无关）。
    pub async fn balance(
        &self,
        cred: &Credential,
    ) -> Result<crate::providers::qoder::QoderBalance, ProviderError> {
        self.inner.balance(cred).await
    }

    /// 活动（透传）。
    pub async fn campaigns(&self, cred: &Credential) -> Result<Vec<serde_json::Value>, ProviderError> {
        self.inner.campaigns(cred).await
    }

    /// 领取（透传）。
    pub async fn claim(
        &self,
        cred: &Credential,
        campaign_id: &str,
    ) -> Result<crate::providers::qoder::ClaimOutcome, ProviderError> {
        self.inner.claim(cred, campaign_id).await
    }
}

#[cfg(feature = "wasm")]
mod cn_infer {
    use super::*;
    use serde_json::Value;
    use crate::providers::qoder::build_infer_payload;
    use crate::openai::{ChatCompletion, ChatRequest};
    use crate::provider::{ChunkStream, Provider, ProviderError, StreamChunk};
    use crate::registry::{ModelInfo, ProviderCatalog};
    use crate::route::Route;
    use crate::sse::SseParser;
    use async_trait::async_trait;

    const COSY_VERSION: &str = "1.1.49";
    const CN_SESSION_TYPE: &str = "qoder_work";

    struct Agg {
        text: String,
        finish: Option<(String, crate::openai::Usage)>,
    }

    fn consume(v: &Value, agg: &mut Agg) {
        if let Some(t) = v.pointer("/choices/0/delta/content").and_then(Value::as_str) {
            agg.text.push_str(t);
        }
        if let Some(u) = v.get("usage") {
            let usage = crate::openai::Usage::sum(
                u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0),
                u.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0),
            );
            agg.finish = Some(("stop".into(), usage));
        }
    }

    #[async_trait]
    impl Provider for QodercnProvider {
        fn id(&self) -> &str {
            "qodercn"
        }

        fn catalog(&self) -> ProviderCatalog {
            ProviderCatalog {
                id: "qodercn".into(),
                models: vec![ModelInfo { id: "qfmodel".into() }],
            }
        }

        async fn complete(&self, cred: &Credential, route: &Route, req: &ChatRequest) -> Result<ChatCompletion, ProviderError> {
            let (status, body) = self.send_cn(cred, route, req).await?;
            if status != 200 {
                return Err(ProviderError::Upstream(format!("qodercn http {status}")));
            }
            let mut agg = Agg { text: String::new(), finish: None };
            let mut parser = SseParser::new();
            parser.feed(body.as_bytes());
            parser.finalize();
            while let Some(data) = parser.next_data() {
                if data.trim() == "[DONE]" { break; }
                if let Ok(v) = serde_json::from_str::<Value>(&data) {
                    consume(&v, &mut agg);
                }
            }
            let (_, usage) = agg.finish.unwrap_or_default();
            Ok(ChatCompletion::new(route.composite(), agg.text, usage))
        }

        async fn stream(&self, cred: &Credential, route: &Route, req: &ChatRequest) -> Result<ChunkStream, ProviderError> {
            let (status, body) = self.send_cn(cred, route, req).await?;
            if status != 200 {
                return Err(ProviderError::Upstream(format!("qodercn http {status}")));
            }
            let mut agg = Agg { text: String::new(), finish: None };
            let mut parser = SseParser::new();
            parser.feed(body.as_bytes());
            parser.finalize();
            while let Some(data) = parser.next_data() {
                if data.trim() == "[DONE]" { break; }
                if let Ok(v) = serde_json::from_str::<Value>(&data) {
                    consume(&v, &mut agg);
                }
            }
            let (reason, usage) = agg.finish.unwrap_or_else(|| ("stop".into(), Default::default()));
            let mut queue: std::collections::VecDeque<Result<StreamChunk, ProviderError>> =
                std::collections::VecDeque::new();
            queue.push_back(Ok(StreamChunk::Role));
            if !agg.text.is_empty() {
                queue.push_back(Ok(StreamChunk::Content(agg.text)));
            }
            queue.push_back(Ok(StreamChunk::Finish { reason, usage }));
            Ok(Box::pin(futures::stream::iter(queue)))
        }
    }

    impl QodercnProvider {
        async fn send_cn(&self, cred: &Credential, route: &Route, req: &ChatRequest) -> Result<(u16, String), ProviderError> {
            let secret = cred.secret.clone();
            let model = route.model.clone();
            let payload = build_infer_payload(route, req).to_string();
            let infer_base = crate::providers::qodercn::QODERCN_ENCRYPTED_INFER_BASE.to_string();
            std::thread::spawn(move || -> Result<(u16, String), ProviderError> {
                let mut wasm = crate::providers::qoder_wasm::QoderWasm::new()
                    .map_err(|e| ProviderError::Upstream(e.to_string()))?;
                let v: Value = serde_json::from_str(&secret)
                    .map_err(|e| ProviderError::Credential(format!("qodercn credential not JSON: {e}")))?;
                let uid = v.get("uid").and_then(Value::as_str).unwrap_or_default().to_string();
                let token = v.get("security_oauth_token").or_else(|| v.get("access_token"))
                    .and_then(Value::as_str).unwrap_or_default().to_string();
                if uid.is_empty() || token.is_empty() {
                    return Err(ProviderError::Credential("qodercn missing uid/token".into()));
                }
                let machine = v.get("machine_id").and_then(Value::as_str).unwrap_or_default().to_string();
                let fields = wasm.generate_runtime_auth_fields(&uid, &token)
                    .map_err(|e| ProviderError::Upstream(e.to_string()))?;
                let user_info = serde_json::json!({
                    "uid": uid, "encrypt_user_info": fields.encrypt_user_info, "key": fields.key,
                    "organization_id": "", "organization_tags": [], "data_policy_agreed": false,
                }).to_string();
                let meta = r#"{"client_type":"5","business_product":"cli","business_type":"agent","scene":"assistant"}"#;
                let ctx = wasm.context_new(&machine, COSY_VERSION, &user_info, meta)
                    .map_err(|e| ProviderError::Upstream(e.to_string()))?;
                let host = infer_base.trim_start_matches("https://").to_string();
                let infer = wasm.prepare_infer_request(ctx, &host, &payload, &model, "system")
                    .map_err(|e| ProviderError::Upstream(e.to_string()))?;
                let mut h = reqwest::header::HeaderMap::new();
                for (k, val) in &infer.headers {
                    if let (Ok(hk), Ok(hv)) = (
                        reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                        reqwest::header::HeaderValue::from_str(val),
                    ) { h.insert(hk, hv); }
                }
                let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()
                    .map_err(|e| ProviderError::Upstream(e.to_string()))?;
                rt.block_on(async {
                    let resp = reqwest::Client::new()
                        .post(&infer.url).headers(h)
                        .header("accept", "text/event-stream")
                        .body(infer.body.clone())
                        .send().await
                        .map_err(|e| ProviderError::Upstream(e.to_string()))?;
                    let status = resp.status().as_u16();
                    let text = resp.text().await.unwrap_or_default();
                    Ok((status, text))
                })
            })
            .join()
            .map_err(|_| ProviderError::Upstream("wasm thread panic".into()))?
        }
    }
}
