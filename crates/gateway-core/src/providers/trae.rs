//! trae Provider（TRAE SOLO，缝 2）。
//!
//! 协议要点（对照 reference/deepseek-harness-codearts.md §4.7）：
//! - 鉴权头 `traeSOLOHeaders`：`Authorization: Cloud-IDE-JWT <token>` +
//!   `X-Cloudide-Token`/`X-Ide-Token`（同 token）+ `X-Uid` +
//!   `X-Device-Type: macos` + `Request-Traffic-Type: prod` +
//!   `X-Machine-Id`/`X-Device-Id`（32 hex，登录时一次性生成持久化，
//!   **machine_id 续期绝不可重生成**）。
//! - 登录本地回调 `127.0.0.1:18080`（占用自动回退随机端口），回调参数名
//!   `auth_callback_url`；老流程回传 token（refreshToken/userInfo/userJwt），
//!   新流程 PKCE（code/authCodeInfo，识别后给精确报错）；userInfo 中文
//!   昵称双重编码乱码自动回转；展示名用脱敏手机号。
//! - 余额 `POST /trae/api/v2/pay/ide_user_ent_usage`，body
//!   `{"require_usage":true,"req_source":2}`（缺 require_usage 则 usage
//!   恒 0 余额虚高）；余额 = Σ(credits_limit − credits_amount)。
//! - 失败模式：`4008`（ide_credits 耗尽）与 `1005`（plan 权益不足）→
//!   冷却+换号。
//!
//! ⚠️ **SOLO 推理通道阻塞**（手册概要级、无 wire 样例）：
//! `transformToSOLOBody` 的完整形状（`function:"solo_work_lite"` 通道名、
//! `config_name` 模型映射、`tools[].parameters` 字符串化、
//! `tool_calls.function`→`function_call` 之外的键）、SOLO SSE 事件
//! （output/token_usage/done/error）的 JSON 载具形态、15 通道白名单逐字表、
//! `X-App-Id`/`X-Ide-Version`/`X-OS-Version`/`X-Device-Brand` 具体值——
//! 待用户提供参考实现原文后接线。

use serde_json::{Map, Value};

use crate::provider::{Credential, ProviderError};

pub const DEFAULT_API_BASE: &str = "https://api.trae.cn";
const BALANCE_PATH: &str = "/trae/api/v2/pay/ide_user_ent_usage";
const CALLBACK_PREFERRED_PORT: u16 = 18080;

#[derive(Debug, Clone, PartialEq)]
pub struct TraeBalance {
    /// Σ(credits_limit − credits_amount)
    pub total: u64,
}

fn parse_secret(secret: &str) -> Result<Value, ProviderError> {
    serde_json::from_str::<Value>(secret)
        .map_err(|e| ProviderError::Credential(format!("trae 凭据非 JSON：{e}")))
}

fn secret_str(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

/// percent-decode（%XX → byte；用于双重编码昵称回转）。
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() && b[i + 1].is_ascii_hexdigit() && b[i + 2].is_ascii_hexdigit()
        {
            let hex = |c: u8| (c as char).to_digit(16).unwrap() as u8;
            out.push(hex(b[i + 1]) * 16 + hex(b[i + 2]));
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// userInfo 中文昵称双重编码乱码自动回转：值仍是 %XX 序列时再解一层。
fn fix_double_encoding(s: &str) -> String {
    let b = s.as_bytes();
    let looks_encoded = b.windows(3).any(|w| {
        w[0] == b'%' && w[1].is_ascii_hexdigit() && w[2].is_ascii_hexdigit()
    });
    if looks_encoded {
        percent_decode(s)
    } else {
        s.to_string()
    }
}

fn form_enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub struct TraeProvider {
    api_base: String,
    client: reqwest::Client,
}

impl TraeProvider {
    pub fn new(api_base: String) -> Self {
        Self { api_base, client: reqwest::Client::new() }
    }

    pub fn production() -> Self {
        Self::new(DEFAULT_API_BASE.into())
    }

    /// `traeSOLOHeaders`（推理/IDE 消费共用的鉴权头族）。
    /// 已载明值照抄；`X-App-Id`/`X-Ide-Version`/`X-OS-Version`/
    /// `X-Device-Brand` 的具体值手册未载明，占位待 wire 核对。
    pub fn solo_headers(&self, cred: &Credential) -> reqwest::header::HeaderMap {
        let Ok(v) = parse_secret(&cred.secret) else {
            return reqwest::header::HeaderMap::new();
        };
        let token = secret_str(&v, "token");
        let mut h = reqwest::header::HeaderMap::new();
        let ins = reqwest::header::HeaderValue::from_str;
        let _ = ins(&format!("Cloud-IDE-JWT {token}")).map(|x| h.insert("authorization", x));
        let _ = ins(&token).map(|x| h.insert("x-cloudide-token", x));
        let _ = ins(&token).map(|x| h.insert("x-ide-token", x));
        let _ = ins(&secret_str(&v, "uid")).map(|x| h.insert("x-uid", x));
        // 值待核对（见模块文档）：占位与 macos 身份一致
        let _ = ins("trae-ide").map(|x| h.insert("x-app-id", x));
        let _ = ins("0.5.0").map(|x| h.insert("x-ide-version", x));
        let _ = ins("0").map(|x| h.insert("x-ide-version-code", x));
        let _ = ins("release").map(|x| h.insert("x-ide-version-type", x));
        let _ = ins("macos").map(|x| h.insert("x-device-type", x));
        let _ = ins("14.5").map(|x| h.insert("x-os-version", x));
        let _ = ins("Apple").map(|x| h.insert("x-device-brand", x));
        let _ = ins("prod").map(|x| h.insert("request-traffic-type", x));
        let _ = ins(&secret_str(&v, "machine_id")).map(|x| h.insert("x-machine-id", x));
        let _ = ins(&secret_str(&v, "device_id")).map(|x| h.insert("x-device-id", x));
        h
    }

    /// 余额：Σ(credits_limit − credits_amount)；body 必须带
    /// `require_usage:true`（缺失则 usage 恒 0 余额虚高）。
    pub async fn balance(&self, cred: &Credential) -> Result<TraeBalance, ProviderError> {
        let resp = self
            .client
            .post(format!("{}{}", self.api_base, BALANCE_PATH))
            .headers(self.solo_headers(cred))
            .json(&serde_json::json!({ "require_usage": true, "req_source": 2 }))
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if status != 200 {
            return Err(ProviderError::Upstream(format!(
                "balance http {status}: {}",
                String::from_utf8_lossy(&bytes)
            )));
        }
        let v: Value = serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Upstream(format!("balance 响应非 JSON: {e}")))?;
        let code = v.get("code").and_then(Value::as_i64).unwrap_or(0);
        if code != 0 {
            return Err(ProviderError::Upstream(format!("balance code {code}")));
        }
        let items = v
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| ProviderError::Upstream("balance 响应缺 data 数组".into()))?;
        let total: i64 = items
            .iter()
            .map(|it| {
                let limit = it.get("credits_limit").and_then(Value::as_i64).unwrap_or(0);
                let used = it.get("credits_amount").and_then(Value::as_i64).unwrap_or(0);
                limit - used
            })
            .sum();
        Ok(TraeBalance { total: total.max(0) as u64 })
    }
}

/// 本地回调登录流（老流程 token 透传；新流程 PKCE 精确报错）。
pub struct TraeLoginFlow {
    callback: String,
    rx: tokio::sync::oneshot::Receiver<Result<Credential, ProviderError>>,
}

impl TraeLoginFlow {
    /// 回调 listener：优先 `127.0.0.1:18080`，占用自动回退随机端口。
    pub async fn start() -> Self {
        Self::start_with_port(CALLBACK_PREFERRED_PORT).await
    }

    /// 注入首选端口（测试用随机端口避免并行竞争；0 = 纯随机）。
    pub async fn start_with_port(preferred: u16) -> Self {
        let listener = match tokio::net::TcpListener::bind(("127.0.0.1", preferred)).await {
            Ok(l) => l,
            Err(_) => tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.expect("bind callback listener"),
        };
        let port = listener.local_addr().unwrap().port();
        let callback = format!("http://127.0.0.1:{port}");
        let (tx, rx) = tokio::sync::oneshot::channel();
        let tx = std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));
        let app = axum::Router::new().route(
            "/",
            axum::routing::get(
                move |axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>| {
                    let tx = tx.clone();
                    async move {
                        let jwt = q.get("userJwt").cloned();
                        let refresh = q.get("refreshToken").cloned();
                        let user_info = q.get("userInfo").cloned();
                        let is_new_flow = q.contains_key("code") || q.contains_key("authCodeInfo");
                        let send = |res: Result<Credential, ProviderError>| {
                            if let Some(tx) = tx.lock().unwrap().take() {
                                let _ = tx.send(res);
                            }
                        };
                        if let (Some(jwt), Some(refresh)) = (jwt, refresh) {
                            // 老流程：token 直接回传
                            let mut m = Map::new();
                            m.insert("token".into(), Value::String(jwt));
                            m.insert("refresh_token".into(), Value::String(refresh));
                            if let Some(ui) = user_info.as_deref().and_then(|s| serde_json::from_str::<Value>(s).ok()) {
                                if let Some(uid) = ui.get("uid").and_then(Value::as_str) {
                                    m.insert("uid".into(), Value::String(uid.to_string()));
                                }
                                let nick = ui
                                    .get("nickName")
                                    .or_else(|| ui.get("nickname"))
                                    .and_then(Value::as_str)
                                    .unwrap_or_default();
                                m.insert("screen_name".into(), Value::String(fix_double_encoding(nick)));
                            }
                            // machine_id/device_id 32hex 一次性生成（续期不可重生成）
                            m.insert("machine_id".into(), Value::String(crate::key::random_id(16)));
                            m.insert("device_id".into(), Value::String(crate::key::random_id(16)));
                            send(Ok(Credential {
                                account_id: String::new(),
                                secret: Value::Object(m).to_string(),
                            }));
                            return (axum::http::StatusCode::OK, "TokenMaster: trae 登录完成，可关闭此页");
                        }
                        if is_new_flow {
                            send(Err(ProviderError::BadRequest(
                                "trae 新流程（PKCE code/authCodeInfo）暂不支持：请改用老流程（userJwt/refreshToken 回传）重新登录".into(),
                            )));
                            return (axum::http::StatusCode::BAD_REQUEST, "unsupported new PKCE flow");
                        }
                        (axum::http::StatusCode::BAD_REQUEST, "missing userJwt/refreshToken")
                    }
                },
            ),
        );
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("callback serve");
        });
        Self { callback, rx }
    }

    /// 授权 URL：回调参数名必须是 `auth_callback_url`。
    pub fn authorize_url(&self, login_page: &str) -> String {
        format!("{login_page}?auth_callback_url={}", form_enc(&self.callback))
    }

    pub fn callback_base(&self) -> &str {
        &self.callback
    }

    pub async fn wait_credential(self) -> Result<Credential, ProviderError> {
        self.rx.await.map_err(|_| ProviderError::Upstream("登录回调通道关闭".into()))?
    }
}
