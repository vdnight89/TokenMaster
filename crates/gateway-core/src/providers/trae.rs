//! trae Provider（TRAE SOLO，缝 2）。
//!
//! 协议要点（对照参考 deepseek-harness-codearts/src/trae*.ts）：
//! - 鉴权头 `traeSOLOHeaders`（trae.ts:204-237）：`Authorization:
//!   Cloud-IDE-JWT <token>` + `X-Cloudide-Token`/`X-Ide-Token`（同 token）+
//!   `X-Uid` + `X-Device-Type: macos` + `Request-Traffic-Type: prod` +
//!   `X-Machine-Id`/`X-Device-Id`（32 hex，登录时一次性生成持久化，
//!   **machine_id 续期绝不可重生成**）。
//! - 三域分离（trae-product.ts:182-186）：UG（签到/余额）api.trae.cn、
//!   OAuth（ExchangeToken）api.trae.com.cn、推理/模型目录
//!   trae-api-cn.mchost.guru。
//! - 登录本地回调 `127.0.0.1:18080`（占用自动回退随机端口），回调参数名
//!   `auth_callback_url`；老流程回传 token（refreshToken/userInfo/userJwt），
//!   新流程 PKCE（code/authCodeInfo，识别后给精确报错）；userInfo 中文
//!   昵称双重编码乱码自动回转；展示名用脱敏手机号。
//! - 余额 `POST /trae/api/v2/pay/ide_user_ent_usage`（UG 域），body
//!   `{"require_usage":true,"req_source":2}`（缺 require_usage 则 usage
//!   恒 0 余额虚高）；余额 = Σ(credits_limit − credits_amount)，
//!   `user_entitlement_pack_list` 在**顶层**（trae-credits.ts:330）。
//! - 失败模式（trae-errors.ts:80-126 + cooldown-table）：1005 → 12h、
//!   4008 → 24h、4011/429 → 60s、404 → 1h、4001 = 模型不可调用（不换号）、
//!   401/会话失效标记 → 重登。
//! - SOLO 推理（trae.ts:1545-1917）：`transformToSOLOBody` + 自定义 SSE
//!   事件（output/token_usage/done/error）；空响应同账号重发一次；
//!   已收到任何事件绝不重放（trae-adapter.ts:1280-1289/1694-1699）。

use serde_json::{Map, Value};

use crate::provider::{Credential, ProviderError};

/// UG 域（签到/余额，trae-product.ts:184 TRAE_UG_HOST）。
pub const DEFAULT_API_BASE: &str = "https://api.trae.cn";
/// OAuth 域（ExchangeToken/GetUserInfo，trae-product.ts:186 TRAE_OAUTH_HOST
/// —— 与 UG 域**不同源**，混用会打错服务器）。
pub const DEFAULT_OAUTH_BASE: &str = "https://api.trae.com.cn";
/// 推理域（agentHost，trae-product.ts TRAE_AGENT_HOST）。
pub const DEFAULT_AGENT_BASE: &str = "https://trae-api-cn.mchost.guru";
const BALANCE_PATH: &str = "/trae/api/v2/pay/ide_user_ent_usage";
const CHAT_PATH: &str = "/api/agent/v3/llm_utils_chat";
const CALLBACK_PREFERRED_PORT: u16 = 18080;

// ── Max 模式（1M 上下文）常量（trae.ts:1478-1489）──
pub const TRAE_MAX_CONTEXT_TOKENS: u64 = 1_000_000;
/// Max 模式提示词预算（936K：1M 总窗口里留给补全的部分，刻意比总窗口小）。
pub const TRAE_MAX_PROMPT_TOKENS: u64 = 936_000;
/// Max 模式输出上限。
pub const TRAE_MAX_MODE_OUTPUT_TOKENS: u64 = 64_000;
/// Max 模式的 `mode_type` 取值。
pub const TRAE_MAX_MODE_TYPE: u64 = 1;

// 产品常量（参考实现 trae-product.ts:314-337 实测值）
pub const USER_AGENT: &str = "Trae/0.1.52";
pub const APP_ID: &str = "6eefa01c-1036-4c7e-9ca5-d891f63bfcd8";
pub const IDE_VERSION: &str = "0.1.52";
pub const IDE_VERSION_CODE: &str = "20260811";
pub const OS_VERSION: &str = "macOS 15.7.4";
pub const DEVICE_BRAND: &str = "Apple";
/// OAuth clientId（trae-product.ts:321；ExchangeToken body 与登录 URL 共用）。
pub const CLIENT_ID: &str = "en1oxy7wnw8j9n";
/// 登录 URL 的 plugin_version（trae-product.ts:333）。
pub const PLUGIN_VERSION: &str = "2.3.62834";
pub const TRAE_FUNCTION: &str = "solo_work_lite";
pub const TRAE_DEFAULT_MODEL: &str = "glm-5.2";
/// 单次输出额度安全上限（模型上限实测 64000，客户端索要更高会把上游打 4xx）。
pub const TRAE_MAX_COMPLETION_TOKENS: u64 = 64_000;
/// 15 通道白名单（顺序即优先级；参考实现 trae-product.ts resolveChannelList）。
pub const TRAE_CHANNELS: [&str; 15] = [
    "solo_agent",
    "solo_work_lite",
    "solo_agent_remote",
    "solo_work_remote",
    "solo_agent_lite",
    "chat_v3",
    "builder_v3",
    "solo_coder",
    "solo_design_lite",
    "solo_design_remote",
    "git_ai",
    "code_reviewer",
    "code_review_summary",
    "multimodal",
    "system_diagnosis",
];

/// X-Machine-Id 可轮换派生（trae.ts:1377-1383）：gen<=0 用原值，
/// 否则 sha256("{base}#machine{gen}") 前 32 hex。
pub fn derive_rotating_machine_id(base_machine_id: &str, generation: u32) -> String {
    if generation == 0 {
        return base_machine_id.to_string();
    }
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(format!("{base_machine_id}#machine{generation}"));
    let hex = format!("{:x}", h.finalize());
    hex[..32].to_string()
}

/// 构造把远程会话钉到 **Max 模式**（1M 上下文）的成套 wire 字段
/// （trae.ts:1508-1525 `traeMaxModeFields`，对齐
/// Trae2api-cn/trae_remote_client.py:356-397 的 `_max_mode_fields`）。
///
/// 实测要点：
/// - **不能只调大 `max_tokens`**：上游按 `strategy=max` +
///   `model_auto_selection.strategy=max` 判定「这是一个 Max 会话」，缺了它们
///   只会被当成普通会话、按 200K 校验，然后拒绝 1M 的输入；
/// - `context_window_size` / `prompt_max_tokens` / `max_tokens` 三者**成套**
///   下发，远端按它们做准入校验（只发其中一个等于没发）；
/// - 只有远端标了 `display_config.max_mode === true` 的模型才能用；给未标记
///   的模型硬套 Max 参数会被上游拒绝。
///
/// TODO(档位元数据): 接线需要逐模型 `display_config.max_mode` /
/// `context_window_tokens.max` / `model_detail_list[].__max` 的输出上限——
/// 当前 `ModelInfo` 只有 id，`fetch_models` 尚未暴露这些元数据；接入后应在
/// `transform_to_solo_body` 里按模型路由决定是否合并本字段（合并发生在
/// max_tokens 钳制**之后**：Max 会话的输出上限由远端 `__max` 明细声明，
/// 可能高于 64K 安全线，被钳制覆盖会让 Max 请求失去意义——见
/// trae-adapter.ts:1077-1092）。
pub fn trae_max_mode_fields(max_context: u64, output_max: Option<u64>) -> Value {
    let context = if max_context > 0 { max_context } else { TRAE_MAX_CONTEXT_TOKENS };
    serde_json::json!({
        "model_auto_selection": {
            "strategy": "max",
            "fallback_to_advance_model": null,
            "entitlement_id": null,
        },
        "model_selection_strategy": "max",
        "mode_type": TRAE_MAX_MODE_TYPE,
        "context_window_size": context,
        "prompt_max_tokens": TRAE_MAX_PROMPT_TOKENS,
        "max_tokens": match output_max {
            Some(n) if n > 0 => Value::from(n),
            _ => Value::from(TRAE_MAX_MODE_OUTPUT_TOKENS),
        },
    })
}

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
    /// UG 域（签到/余额）：api.trae.cn。
    api_base: String,
    /// OAuth 域（ExchangeToken）：api.trae.com.cn（trae-product.ts:186）。
    oauth_base: String,
    /// 推理域（agentHost）。
    agent_base: String,
    client: reqwest::Client,
    /// 模型目录缓存（fetch_models 成功后填充；catalog() 优先返回缓存，
    /// 未拉取时回退硬编码单模型）。
    models_cache: std::sync::Mutex<Option<std::sync::Arc<Vec<crate::registry::ModelInfo>>>>,
}

impl TraeProvider {
    pub fn new(api_base: String) -> Self {
        Self::with_agent_base(api_base, DEFAULT_AGENT_BASE.into())
    }

    /// api 域（余额/签到/OAuth）与推理域（agentHost）分离注入。
    /// 测试用单 stub 时 OAuth 与 UG 共用同一基址。
    pub fn with_agent_base(api_base: String, agent_base: String) -> Self {
        let oauth_base = api_base.clone();
        Self {
            api_base,
            oauth_base,
            agent_base,
            client: reqwest::Client::new(),
            models_cache: std::sync::Mutex::new(None),
        }
    }

    pub fn production() -> Self {
        Self {
            api_base: DEFAULT_API_BASE.into(),
            oauth_base: DEFAULT_OAUTH_BASE.into(),
            agent_base: DEFAULT_AGENT_BASE.into(),
            client: reqwest::Client::new(),
            models_cache: std::sync::Mutex::new(None),
        }
    }

    /// `traeSOLOHeaders`（推理/IDE 消费共用的鉴权头族）。
    pub fn solo_headers(&self, cred: &Credential) -> reqwest::header::HeaderMap {
        Self::solo_headers_with(cred, true, 0)
    }

    /// `traeSOLOHeaders`（对照参考实现 trae.ts:204-237，产品常量取
    /// trae-product.ts:316-342 实测值）。
    pub fn solo_headers_with(cred: &Credential, stream: bool, machine_id_generation: u32) -> reqwest::header::HeaderMap {
        let Ok(v) = parse_secret(&cred.secret) else {
            return reqwest::header::HeaderMap::new();
        };
        let token = secret_str(&v, "access_token");
        let mut h = reqwest::header::HeaderMap::new();
        let ins = reqwest::header::HeaderValue::from_str;
        let _ = ins("application/json").map(|x| h.insert("content-type", x));
        let _ = ins(if stream { "text/event-stream" } else { "application/json" })
            .map(|x| h.insert("accept", x));
        let _ = ins(USER_AGENT).map(|x| h.insert("user-agent", x));
        let _ = ins(&format!("Cloud-IDE-JWT {token}")).map(|x| h.insert("authorization", x));
        let _ = ins(&token).map(|x| h.insert("x-cloudide-token", x));
        let _ = ins(&token).map(|x| h.insert("x-ide-token", x));
        let _ = ins(&secret_str(&v, "uid")).map(|x| h.insert("x-uid", x));
        let _ = ins(APP_ID).map(|x| h.insert("x-app-id", x));
        let _ = ins("default").map(|x| h.insert("x-app-version", x));
        let _ = ins(IDE_VERSION).map(|x| h.insert("x-ide-version", x));
        let _ = ins(IDE_VERSION_CODE).map(|x| h.insert("x-ide-version-code", x));
        let _ = ins(IDE_VERSION_CODE).map(|x| h.insert("x-app-version-code", x));
        let _ = ins("stable").map(|x| h.insert("x-ide-version-type", x));
        let _ = ins("macos").map(|x| h.insert("x-device-type", x));
        let _ = ins(OS_VERSION).map(|x| h.insert("x-os-version", x));
        let _ = ins(DEVICE_BRAND).map(|x| h.insert("x-device-brand", x));
        let _ = ins("prod").map(|x| h.insert("request-traffic-type", x));
        let mid = secret_str(&v, "machine_id");
        if !mid.is_empty() {
            let _ = ins(&derive_rotating_machine_id(&mid, machine_id_generation))
                .map(|x| h.insert("x-machine-id", x));
        }
        let did = secret_str(&v, "device_id");
        if !did.is_empty() {
            let _ = ins(&did).map(|x| h.insert("x-device-id", x));
        }
        h
    }

    /// 余额：Σ(credits_limit − credits_amount)；body 必须带
    /// `require_usage:true`（缺失则 usage 恒 0 余额虚高）。
    ///
    /// 响应形状对照参考 trae-credits.ts:330-389（权威）：
    /// `user_entitlement_pack_list` 在**顶层**（不在 `data` 下；参考实现
    /// 直接 `body.user_entitlement_pack_list`）；每条目读
    /// `entitlement_base_info.quota.credits_limit` 与同条目
    /// `usage.credits_amount`；`expire_time` 是**秒级**（展示须 ×1000）。
    /// `credits_limit <= 0` 的包直接跳过（trae-credits.ts:356）——不能让
    /// 0 上限的包用 usage 拉低总额（服务端清空额度后的残留条目）。
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
        // 顶层为权威形状；`data` 包裹形态做宽松回退（两种 wire 只信参考实测的
        // 顶层，回退仅为容错，不改变主路径）。
        let packs = v
            .get("user_entitlement_pack_list")
            .and_then(Value::as_array)
            .or_else(|| v.pointer("/data/user_entitlement_pack_list").and_then(Value::as_array))
            .cloned()
            .unwrap_or_default();
        let total: i64 = packs
            .iter()
            .filter_map(|pack| {
                let limit = pack
                    .pointer("/entitlement_base_info/quota/credits_limit")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                if limit <= 0 {
                    return None; // credits_limit<=0 的包不计入（trae-credits.ts:356）
                }
                let used = pack.pointer("/usage/credits_amount").and_then(Value::as_i64).unwrap_or(0);
                Some(limit.saturating_sub(used))
            })
            .fold(0i64, i64::saturating_add);
        Ok(TraeBalance { total: total.max(0) as u64 })
    }
}

/// 本地回调登录流（老流程 token 透传；新流程 PKCE 精确报错）。
///
/// 对照参考 trae-oauth.ts：回调地址 `http://127.0.0.1:{port}/authorize`
/// （trae.ts:65 TRAE_CALLBACK_PATH；登录页强制回传该路径）；授权 URL 是
/// **完整 17 参数集**（trae-oauth.ts:99-127）——参数缺失时登录页会
/// 「永远停在授权中」（既不跳转也不回传）。
pub struct TraeLoginFlow {
    callback: String,
    /// 登录时一次性生成的 (machine_id, device_id)——URL 与回调凭据共用
    /// 同一份（参考在 buildTraeLoginURL 与凭据组装间共享 session）。
    ids: std::sync::Arc<(String, String)>,
    rx: tokio::sync::oneshot::Receiver<Result<Credential, ProviderError>>,
}

/// login_trace_id（trae-oauth.ts:71-76）：machine_id+device_id 拼接串的
/// 尾部 16 字符——回调反查 pending 的唯一凭据。
fn machine_trace_id(machine_id: &str, device_id: &str) -> String {
    let joined = format!("{machine_id}{device_id}");
    let n = joined.len();
    if n >= 16 {
        joined[n - 16..].to_string()
    } else {
        format!("{joined:0>16}")
    }
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
        let callback = format!("http://127.0.0.1:{port}/authorize");
        let (tx, rx) = tokio::sync::oneshot::channel();
        let tx = std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));
        // machine_id/device_id 32hex 一次性生成（URL 下发 + 凭据持久化共用；
        // 续期不可重生成）
        let ids = std::sync::Arc::new((
            crate::key::random_id(16),
            crate::key::random_id(16),
        ));
        let ids_for_handler = ids.clone();
        let app = axum::Router::new().route(
            "/authorize",
            axum::routing::get(
                move |axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>| {
                    let tx = tx.clone();
                    let ids = ids_for_handler.clone();
                    async move {
                        let send = |res: Result<Credential, ProviderError>| {
                            if let Some(tx) = tx.lock().unwrap_or_else(|e| e.into_inner()).take() {
                                let _ = tx.send(res);
                            }
                        };
                        // userJwt 是 URL 编码的 JSON {Token, RefreshToken}
                        // （trae-oauth.ts:291-292）；兼容裸 token 旧形态。
                        let (jwt_token, jwt_refresh) = match q.get("userJwt").map(String::as_str) {
                            Some(s) if s.starts_with('{') => {
                                match serde_json::from_str::<Value>(s) {
                                    Ok(v) => (
                                        v.get("Token").and_then(Value::as_str).unwrap_or("").to_string(),
                                        v.get("RefreshToken").and_then(Value::as_str).unwrap_or("").to_string(),
                                    ),
                                    Err(_) => (String::new(), String::new()),
                                }
                            }
                            Some(s) => (s.to_string(), String::new()),
                            None => (String::new(), String::new()),
                        };
                        // refreshToken：query 优先，缺失回退 userJwt.RefreshToken
                        // （login.sh:165-166 / trae-oauth.ts:294-295）。
                        let refresh = q
                            .get("refreshToken")
                            .cloned()
                            .filter(|s| !s.is_empty())
                            .or_else(|| (!jwt_refresh.is_empty()).then(|| jwt_refresh.clone()));
                        let is_new_flow = q.contains_key("code") || q.contains_key("authCodeInfo");
                        if let Some(refresh) = refresh {
                            // 老流程：token 直接回传（参考分支 1 会立即 ExchangeToken
                            // 换新；此处存回调 token + refresh_token，续期由
                            // exchange_refresh 承接，语义等价）
                            let mut m = Map::new();
                            m.insert("access_token".into(), Value::String(jwt_token.clone()));
                            m.insert("refresh_token".into(), Value::String(refresh));
                            if let Some(ui) = q.get("userInfo").and_then(|s| serde_json::from_str::<Value>(s).ok()) {
                                // 权威字段名 UserID/ScreenName/TenantID
                                // （trae-oauth.ts:286-289），兼容 uid/nickName 旧形态。
                                let uid = ui
                                    .get("UserID")
                                    .or_else(|| ui.get("uid"))
                                    .and_then(Value::as_str)
                                    .unwrap_or_default();
                                if !uid.is_empty() {
                                    m.insert("uid".into(), Value::String(uid.to_string()));
                                }
                                let nick = ui
                                    .get("ScreenName")
                                    .or_else(|| ui.get("nickName"))
                                    .or_else(|| ui.get("nickname"))
                                    .and_then(Value::as_str)
                                    .unwrap_or_default();
                                m.insert("nickname".into(), Value::String(fix_double_encoding(nick)));
                                if let Some(tenant) = ui.get("TenantID").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                                    m.insert("enterprise_id".into(), Value::String(tenant.to_string()));
                                }
                            }
                            let (machine_id, device_id) = &*ids;
                            m.insert("machine_id".into(), Value::String(machine_id.clone()));
                            m.insert("device_id".into(), Value::String(device_id.clone()));
                            send(Ok(Credential {
                                account_id: String::new(),
                                secret: Value::Object(m).to_string(),
                            }));
                            return (axum::http::StatusCode::OK, "TokenMaster: trae 登录完成，可关闭此页");
                        }
                        if is_new_flow {
                            send(Err(ProviderError::BadRequest(
                                "trae 新流程（PKCE code/authCodeInfo）暂不支持：请改用老流程（refreshToken/userJwt 回传）重新登录".into(),
                            )));
                            return (axum::http::StatusCode::BAD_REQUEST, "unsupported new PKCE flow");
                        }
                        (axum::http::StatusCode::BAD_REQUEST, "missing refreshToken/userJwt")
                    }
                },
            ),
        );
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("callback serve");
        });
        Self { callback, ids, rx }
    }

    /// 授权 URL：**完整 17 参数集**（trae-oauth.ts:105-126）。回调地址参数名
    /// 必须是 `auth_callback_url`（不是 callback_url/redirect_uri）；缺其余
    /// 参数登录页会停在授权中（auth_from/login_channel/auth_type/redirect
    /// 决定走本地回传分支，login_trace_id 是回调反查凭据，x_* 是客户端
    /// 形态伪装）。
    pub fn authorize_url(&self, login_page: &str) -> String {
        let (machine_id, device_id) = &*self.ids;
        let trace = machine_trace_id(machine_id, device_id);
        format!(
            "{login_page}\
             ?login_version=1\
             &auth_from=solo\
             &login_channel=native_ide\
             &plugin_version={PLUGIN_VERSION}\
             &auth_type=local\
             &client_id={CLIENT_ID}\
             &redirect=0\
             &login_trace_id={trace}\
             &auth_callback_url={}\
             &machine_id={machine_id}\
             &device_id={device_id}\
             &x_device_id={device_id}\
             &x_machine_id={machine_id}\
             &x_device_brand=PC\
             &x_device_type=PC\
             &x_os_version=1.0\
             &x_app_version={IDE_VERSION}\
             &x_app_type=stable",
            form_enc(&self.callback)
        )
    }

    pub fn callback_base(&self) -> &str {
        &self.callback
    }

    /// 登录时生成的设备身份（供调用方随凭据一并持久化展示）。
    pub fn device_identity(&self) -> (String, String) {
        (self.ids.0.clone(), self.ids.1.clone()) // (machine_id, device_id)
    }

    pub async fn wait_credential(self) -> Result<Credential, ProviderError> {
        self.rx.await.map_err(|_| ProviderError::Upstream("登录回调通道关闭".into()))?
    }
}


// ───────────────── SOLO 推理通道（T4.4b，复刻参考实现 trae.ts） ─────────────────

use crate::openai::{ChatCompletion, ChatRequest, Usage};
use crate::provider::{ChunkStream, Provider, StreamChunk};
use crate::route::Route;
use async_trait::async_trait;

/// OpenAI → SOLO 请求体（transformToSOLOBody，trae.ts:1545-1577；
/// 消息序列化对照 trae-adapter.ts:406-529 serializeTraeMessages）：
/// stream 恒 true；function=通道名；model→config_name+model 双字段
/// （`__` 后缀消除）；content 字符串→`[{type:"text"}]`；assistant
/// tool_calls function→function_call（无 name 剔除、全剔删键）；
/// tool_choice 归一；tools.parameters 对象→JSON 字符串；max_tokens 钳 64000。
/// **不写 `reasoning_content` 键**：参考序列化层对输入消息只发
/// role/content/tool_calls/tool_call_id，思考内容不回传上游。
pub fn transform_to_solo_body(route: &Route, req: &ChatRequest) -> Value {
    let mut m = req.raw.clone();
    m.remove("stream");

    let mut msgs: Vec<Value> = Vec::with_capacity(req.messages.len());
    for msg in &req.messages {
        let mut o = Map::new();
        o.insert("role".into(), Value::String(msg.role.clone()));
        match &msg.content {
            Value::String(t) => {
                o.insert("content".into(), serde_json::json!([{ "type": "text", "text": t }]));
            }
            Value::Array(a) => {
                o.insert("content".into(), Value::Array(a.clone()));
            }
            _ => {} // null/缺省 → 跳过键（纯 tool_calls assistant）
        }
        // 注意：msg.reasoning_content 刻意不序列化（参考 serializeTraeMessages
        // / transformSOLOMessage 均不写该键；把思考回传上游会污染上下文）。
        if let Some(id) = &msg.tool_call_id {
            o.insert("tool_call_id".into(), Value::String(id.clone()));
        }
        if msg.role == "assistant" {
            if let Some(Value::Array(calls)) = &msg.tool_calls {
                let mut kept: Vec<Value> = Vec::new();
                for c in calls {
                    let mut t = c.clone();
                    if let Some(f) = t.as_object_mut().and_then(|obj| obj.remove("function")) {
                        if let Some(obj) = t.as_object_mut() {
                            obj.insert("function_call".into(), f);
                        }
                    }
                    let name = t.pointer("/function_call/name").and_then(Value::as_str).unwrap_or("").trim();
                    if name.is_empty() {
                        continue;
                    }
                    kept.push(t);
                }
                if !kept.is_empty() {
                    o.insert("tool_calls".into(), Value::Array(kept));
                }
            }
        }
        msgs.push(Value::Object(o));
    }
    m.insert("messages".into(), Value::Array(msgs));

    // model → config_name + model（双字段同值；__dev 后缀消除）
    let base_model = route.model.split("__").next().unwrap_or(&route.model);
    let config_name = if base_model.is_empty() { TRAE_DEFAULT_MODEL } else { base_model };
    m.insert("config_name".into(), Value::String(config_name.to_string()));
    m.insert("model".into(), Value::String(config_name.to_string()));
    m.insert("stream".into(), Value::Bool(true));
    m.insert("function".into(), Value::String(TRAE_FUNCTION.into()));
    if let Some(mt) = m.get("max_tokens").and_then(Value::as_u64) {
        m.insert("max_tokens".into(), Value::from(mt.min(TRAE_MAX_COMPLETION_TOKENS)));
    }
    normalize_solo_tool_choice(&mut m);
    normalize_solo_tools(&mut m);
    Value::Object(m)
}

/// tool_choice 归一（trae.ts:1631-1690）："none"/{type:none} → 删 tool_choice
/// 并删 tools；{type:auto/required} → 字符串；{type:function} → name 字符串
/// （无 name 回退 auto）；其他类型删除。
fn normalize_solo_tool_choice(body: &mut Map<String, Value>) {
    let Some(tc) = body.get("tool_choice").cloned() else { return };
    match tc {
        Value::String(s) => {
            if s.trim().eq_ignore_ascii_case("none") {
                body.remove("tool_choice");
                body.remove("tools");
                body.remove("functions");
            }
        }
        Value::Object(v) => {
            let typ = v
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_lowercase();
            match typ.as_str() {
                "none" => {
                    body.remove("tool_choice");
                    body.remove("tools");
                    body.remove("functions");
                }
                "auto" | "required" => {
                    body.insert("tool_choice".into(), Value::String(typ));
                }
                "function" => {
                    let name = v
                        .get("function")
                        .and_then(|f| f.get("name"))
                        .and_then(Value::as_str)
                        .or_else(|| v.get("name").and_then(Value::as_str))
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    let choice = if name.is_empty() { "auto".to_string() } else { name };
                    body.insert("tool_choice".into(), Value::String(choice));
                }
                _ => {
                    body.remove("tool_choice");
                }
            }
        }
        _ => {
            body.remove("tool_choice");
        }
    }
}

/// tools.parameters 序列化（trae.ts:1696-1725）：SOLO 要求 string 类型。
fn normalize_solo_tools(body: &mut Map<String, Value>) {
    let Some(Value::Array(raw)) = body.get("tools").cloned() else { return };
    if raw.is_empty() {
        return;
    }
    let mut out: Vec<Value> = Vec::new();
    for item in raw {
        let Some(obj) = item.as_object() else { continue };
        let mut t = obj.clone();
        let Some(fnv) = t.get("function").cloned() else { continue };
        let Some(f) = fnv.as_object() else {
            out.push(Value::Object(t));
            continue;
        };
        let mut f2 = f.clone();
        if let Some(p) = f2.get("parameters") {
            if p.is_object() {
                f2.insert("parameters".into(), Value::String(p.to_string()));
            }
        }
        t.insert("function".into(), Value::Object(f2));
        out.push(Value::Object(t));
    }
    if out.is_empty() {
        body.remove("tools");
    } else {
        body.insert("tools".into(), Value::Array(out));
    }
}

/// SOLO SSE 解析后的单事件（parseTraeSSELine，trae.ts:1743-1783）。
#[derive(Debug, Clone, Default)]
pub struct SoloEvent {
    pub event: String,
    pub response: Option<String>,
    pub reasoning: Option<String>,
    pub tool_calls: Option<Value>,
    pub usage: Option<Usage>,
    pub finish_reason: Option<String>,
    /// (code, message)
    pub error: Option<(Option<i64>, String)>,
}

/// 解析整条 SSE 文本：`event:`/`data:` 行配对，空行分发。
/// 逐行先 trim（对齐参考 consumeSse 的 `line.trim()` / aggregateTraeSSE 的
/// `line.trimEnd()`）：真实上游可能用 CRLF 行尾，不剥 `\r` 会让事件分隔行
/// 失效、事件名带 `\r`。
pub fn parse_solo_sse(text: &str) -> Vec<SoloEvent> {
    let mut out: Vec<SoloEvent> = Vec::new();
    let mut ev_name = String::new();
    let mut data = String::new();
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            push_event(&mut out, &ev_name, &data);
            ev_name.clear();
            data.clear();
            continue;
        }
        if let Some(v) = line.strip_prefix("event:") {
            ev_name = v.to_string();
        } else if let Some(v) = line.strip_prefix("data:") {
            data.push_str(v.trim_start());
        }
    }
    push_event(&mut out, &ev_name, &data);
    out
}

fn push_event(out: &mut Vec<SoloEvent>, ev: &str, d: &str) {
    let event = ev.trim().to_string();
    if event.is_empty() && d.is_empty() {
        return;
    }
    if d.is_empty() {
        out.push(SoloEvent { event, ..Default::default() });
        return;
    }
    let Ok(raw) = serde_json::from_str::<Value>(d) else {
        out.push(SoloEvent { event, ..Default::default() });
        return;
    };
    let mut e = SoloEvent { event: event.clone(), ..Default::default() };
    match event.as_str() {
        "output" => {
            if let Some(r) = raw.get("response").and_then(Value::as_str) {
                e.response = Some(r.to_string());
            }
            if let Some(r) = raw.get("reasoning_content").and_then(Value::as_str) {
                e.reasoning = Some(r.to_string());
            }
            if let Some(tc) = raw.get("tool_calls") {
                if tc.is_array() {
                    e.tool_calls = Some(normalize_solo_tool_calls(tc));
                }
            }
        }
        "token_usage" => {
            e.usage = Some(Usage::sum(
                raw.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0),
                raw.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0),
            ));
        }
        "done" => {
            if let Some(fr) = raw.get("finish_reason").and_then(Value::as_str) {
                e.finish_reason = Some(fr.to_string());
            }
        }
        "error" => {
            e.error = Some((
                raw.get("code").and_then(Value::as_i64),
                raw.get("message").and_then(Value::as_str).unwrap_or_default().to_string(),
            ));
        }
        _ => {}
    }
    out.push(e);
}

/// SOLO tool_calls 归一（normalizeTraeToolCalls，trae.ts:1786-1805）：
/// function_call→function；删 namespace/partial_arguments。
fn normalize_solo_tool_calls(calls: &Value) -> Value {
    let Some(arr) = calls.as_array() else { return calls.clone() };
    Value::Array(
        arr.iter()
            .map(|c| {
                let Some(obj) = c.as_object() else { return c.clone() };
                let mut o = obj.clone();
                if let Some(fc) = o.remove("function_call") {
                    if let Some(f) = fc.as_object() {
                        let mut f2 = f.clone();
                        f2.remove("namespace");
                        f2.remove("partial_arguments");
                        o.insert("function".into(), Value::Object(f2));
                    }
                } else if let Some(Value::Object(f)) = o.get("function") {
                    let mut f2 = f.clone();
                    f2.remove("namespace");
                    f2.remove("partial_arguments");
                    o.insert("function".into(), Value::Object(f2));
                }
                Value::Object(o)
            })
            .collect(),
    )
}

/// trae 错误分类（trae-errors.ts:80-126 Classify + trae-cooldown-table.ts）。
/// 判定顺序与参考一致：**业务码先于状态码**——
/// 1. 1005 → hard-plan，冷却 12h（参考要求 body 同时含 "plan" 关键词；这里
///    拿到的是已解析的结构化 code，比子串匹配可靠，不再复刻关键词条件）；
/// 2. 4008 → quota-exceeded（ide_credits 耗尽），冷却 24h（**先于 4011**：
///    两者可能同时出现，让较轻的 4011 抢先命中会让耗尽账号 60s 后被重试）；
/// 3. 4011 → soft-rate，冷却 60s；
/// 4. 4001 → 模型不可调用（trae-adapter.ts:556-575 实测：只由
///    `is_custom_model === true` 或模型/通道不匹配触发，与参数格式无关；
///    换号无意义——换账号也是同一模型被拒，见 isRotatableStreamError 注释）
///    → BadRequest（不可重试、不冷却）；
/// 5. HTTP 401 → session-dead（重登）；错误体带失效标记且无结构化 code 时
///    同样判死（流内 200+`event:error` 场景）；
/// 6. 429 → 60s；404 → 1h；5xx → Upstream；其余 4xx → BadRequest。
pub fn classify_trae_error(status: u16, code: Option<i64>, message: &str) -> ProviderError {
    match code {
        Some(1005) => {
            return ProviderError::RateLimited {
                retry_after_secs: Some(12 * 3600),
                msg: format!("plan limit (1005): {message}"),
            }
        }
        Some(4008) => {
            return ProviderError::RateLimited {
                retry_after_secs: Some(24 * 3600),
                msg: format!("ide_credits exhausted (4008): {message}"),
            }
        }
        Some(4011) => {
            return ProviderError::RateLimited {
                retry_after_secs: Some(60),
                msg: format!("rate limited (4011): {message}"),
            }
        }
        Some(4001) => {
            return ProviderError::BadRequest(format!(
                "model not callable (4001): {message} —— 该模型通常是「仅可见但不可调用」\
                 的自定义模型（需先在 TRAE IDE 内绑定供应商）或不在当前通道的目录中，\
                 请改用模型列表中的其它模型"
            ));
        }
        _ => {}
    }
    // 会话死亡：401 状态码（参考规则 4，标记词在参考里也只在 401 分支内起作用）；
    // 结构化 code 缺失时才做标记词兜底（流内 error 事件可能只有 message）。
    // 注意不检查裸 "401" 子串——"4011" 消息会被它误判成会话死亡。
    let m = message.to_lowercase();
    let session_dead = status == 401
        || (code.is_none()
            && (m.contains("login")
                || m.contains("token 失效")
                || m.contains("token invalid")
                || m.contains("session")
                || m.contains("unauthorized")));
    if session_dead {
        return ProviderError::Credential(format!("session dead: {message}"));
    }
    match status {
        429 => ProviderError::RateLimited { retry_after_secs: Some(60), msg: message.into() },
        404 => ProviderError::RateLimited { retry_after_secs: Some(3600), msg: format!("not found: {message}") },
        s if (500..=599).contains(&s) => ProviderError::Upstream(format!("http {s}: {message}")),
        s if (400..=499).contains(&s) => ProviderError::BadRequest(format!("http {s}: {message}")),
        _ => ProviderError::Upstream(message.into()),
    }
}

struct SoloAgg {
    text: String,
    reasoning: String,
    tool_calls: Vec<(u64, Option<String>, Option<String>, String)>,
    usage: Usage,
    finish_reason: Option<String>,
}

/// 聚合 SOLO 事件为单次结果（对照 Go 端 Aggregate / trae.ts:1860-1917）。
///
/// 流内 `event:error`：参考实现（trae-adapter.ts:1611-1647）在**未产出任何
/// 内容**且错误可换号（1005/4008/4011）时才换号重发；已产出内容则如实抛错、
/// **绝不重放**（防重复计费/重复执行工具）。本实现是缓冲模式（整包读回后
/// 一次聚合）——遇 error 事件直接 Err 在语义上等价：空响应重试已在
/// `chat_events` 里以「零可解析事件」为闸（任何事件、含 metadata，都不重放），
/// 因此 error 事件到达此处时必然已「收到过事件」，Err 即参考的「如实抛错」
/// 分支；错误发生前已缓冲的增量随 Err 一并丢弃，与参考流式路径抛错时
/// 已发射 chunk 由上层终止的行为一致。
fn aggregate(events: Vec<SoloEvent>, status: u16) -> Result<SoloAgg, ProviderError> {
    let mut agg = SoloAgg {
        text: String::new(),
        reasoning: String::new(),
        tool_calls: Vec::new(),
        usage: Usage::default(),
        finish_reason: None,
    };
    for ev in events {
        if let Some(r) = ev.response {
            agg.text.push_str(&r);
        }
        if let Some(r) = ev.reasoning {
            agg.reasoning.push_str(&r);
        }
        if let Some(tcs) = ev.tool_calls {
            if let Some(arr) = tcs.as_array() {
                for (i, c) in arr.iter().enumerate() {
                    // index 优先用上游 wire 值（consumeSse 的 wireIndex，
                    // trae-adapter.ts:1543），缺失时退化为条目序号——跨事件
                    // 的同名 index 会由网关下游按 index 合并。
                    agg.tool_calls.push((
                        c.get("index").and_then(Value::as_u64).unwrap_or(i as u64),
                        c.get("id").and_then(Value::as_str).map(str::to_string),
                        c.pointer("/function/name").and_then(Value::as_str).map(str::to_string),
                        c.pointer("/function/arguments")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    ));
                }
            }
        }
        if let Some(u) = ev.usage {
            agg.usage = u;
        }
        if let Some(fr) = ev.finish_reason {
            agg.finish_reason = Some(fr);
        }
        if let Some((code, msg)) = ev.error {
            return Err(classify_trae_error(status, code, &msg));
        }
    }
    Ok(agg)
}

/// 从 HTTP 错误体提取结构化业务码（顶层 `code`，数字或数字字符串形态）。
/// 参考分类的输入是**响应体原文**（classifyTraeError 对 body 做 1005/4008/
/// 4011 子串判定、业务码先于状态码，trae-errors.ts:80-126）；流内 error 事件
/// 走结构化 code，HTTP 路径在这里从 JSON 体里取同一字段。
fn business_code_from_body(body: &str) -> Option<i64> {
    let v: Value = serde_json::from_str(body).ok()?;
    match v.get("code") {
        Some(Value::Number(n)) => n.as_i64(),
        Some(Value::String(s)) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

async fn send_chat(
    provider: &TraeProvider,
    cred: &Credential,
    route: &Route,
    req: &ChatRequest,
) -> Result<(u16, String), ProviderError> {
    let body = transform_to_solo_body(route, req);
    let resp = provider
        .client
        .post(format!("{}{}", provider.agent_base, CHAT_PATH))
        .headers(TraeProvider::solo_headers_with(cred, true, 0))
        .json(&body)
        .send()
        .await
        .map_err(|e| ProviderError::Upstream(e.to_string()))?;
    let status = resp.status().as_u16();
    let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
    Ok((status, String::from_utf8_lossy(&bytes).to_string()))
}

impl TraeProvider {
    /// 发送 chat 并取回事件序列（complete/stream 共用）。
    ///
    /// 空响应（HTTP 200 但零可解析事件，含 metadata）→ 同账号重发**一次**；
    /// 已收到任何事件则绝不重放（trae-adapter.ts:1280-1289/1694-1699：
    /// 重放会让上游重复计费并可能重复执行工具）。
    async fn chat_events(
        &self,
        cred: &Credential,
        route: &Route,
        req: &ChatRequest,
    ) -> Result<Vec<SoloEvent>, ProviderError> {
        let (status, body) = send_chat(self, cred, route, req).await?;
        if status == 200 {
            let events = parse_solo_sse(&body);
            if !events.is_empty() {
                return Ok(events);
            }
            let (s2, b2) = send_chat(self, cred, route, req).await?;
            if s2 == 200 {
                return Ok(parse_solo_sse(&b2));
            }
            return Err(classify_trae_error(s2, business_code_from_body(&b2), &b2));
        }
        Err(classify_trae_error(status, business_code_from_body(&body), &body))
    }

    /// 聚合出带 finish 的完整结果（无 done 事件 = 截断，不伪造完成）。
    async fn chat_aggregate(
        &self,
        cred: &Credential,
        route: &Route,
        req: &ChatRequest,
    ) -> Result<SoloAgg, ProviderError> {
        let agg = aggregate(self.chat_events(cred, route, req).await?, 200)?;
        if agg.finish_reason.is_none() {
            return Err(ProviderError::Upstream("no done event (truncated stream)".into()));
        }
        Ok(agg)
    }
}

#[async_trait]
impl Provider for TraeProvider {
    fn id(&self) -> &str {
        "trae"
    }

    async fn refresh(&self, cred: &Credential) -> Result<Credential, ProviderError> {
        self.exchange_refresh(cred).await
    }

    fn catalog(&self) -> crate::registry::ProviderCatalog {
        // fetch_models 成功过 → 返回远端目录缓存（参考 TraeAdapter 用
        // remoteModels 缓存目录，trae-auth.ts:642-652 含 30s TTL）；未拉取时
        // 回退硬编码单模型（保证 catalog 无凭据也能同步返回）。
        let cached = self
            .models_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .unwrap_or_else(|| std::sync::Arc::new(vec![crate::registry::ModelInfo { id: TRAE_DEFAULT_MODEL.into() }]));
        crate::registry::ProviderCatalog { id: "trae".into(), models: (*cached).clone() }
    }

    async fn complete(
        &self,
        cred: &Credential,
        route: &Route,
        req: &ChatRequest,
    ) -> Result<ChatCompletion, ProviderError> {
        let agg = self.chat_aggregate(cred, route, req).await?;
        let reason = agg.finish_reason.unwrap_or_else(|| "stop".into());
        let mut out = ChatCompletion::new(route.composite(), agg.text, agg.usage);
        if !agg.tool_calls.is_empty() {
            let calls: Vec<Value> = agg
                .tool_calls
                .iter()
                .map(|(_, id, name, args)| {
                    serde_json::json!({
                        "id": id.clone().unwrap_or_default(),
                        "type": "function",
                        "function": { "name": name.clone().unwrap_or_default(), "arguments": args }
                    })
                })
                .collect();
            out.choices[0].message.tool_calls = Some(Value::Array(calls));
        }
        if reason != "stop" {
            out.choices[0].finish_reason = Some(reason);
        }
        Ok(out)
    }

    async fn stream(
        &self,
        cred: &Credential,
        route: &Route,
        req: &ChatRequest,
    ) -> Result<ChunkStream, ProviderError> {
        let agg = self.chat_aggregate(cred, route, req).await?;
        let reason = agg.finish_reason.unwrap_or_else(|| "stop".into());
        let mut queue: std::collections::VecDeque<Result<StreamChunk, ProviderError>> =
            std::collections::VecDeque::new();
        queue.push_back(Ok(StreamChunk::Role));
        if !agg.reasoning.is_empty() {
            queue.push_back(Ok(StreamChunk::Reasoning(agg.reasoning)));
        }
        if !agg.text.is_empty() {
            queue.push_back(Ok(StreamChunk::Content(agg.text)));
        }
        for (index, id, name, args) in agg.tool_calls {
            queue.push_back(Ok(StreamChunk::ToolCallDelta { index, id, name, arguments: args }));
        }
        queue.push_back(Ok(StreamChunk::Finish { reason, usage: agg.usage }));
        Ok(Box::pin(futures::stream::iter(queue)))
    }
}

// ───────────── T4.19：签到设备派生 / ExchangeToken 续期 ─────────────

/// seeded 派生流（trae.ts:353-373）：sha256("{salt}:{uid}" ++ counterBE32)
/// 串联取 nbytes 字节——同 uid 恒定、跨 uid 互异。
fn seeded_stream(uid: &str, salt: &str, nbytes: usize) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    let prefix = format!("{salt}:{uid}");
    let mut out: Vec<u8> = Vec::with_capacity(nbytes);
    let mut counter: u32 = 0;
    while out.len() < nbytes {
        let mut h = Sha256::new();
        h.update(prefix.as_bytes());
        h.update(counter.to_be_bytes());
        out.extend_from_slice(&h.finalize());
        counter += 1;
    }
    out.truncate(nbytes);
    out
}

/// 签到 X-Device-Id：15 位数字（每字节 %10，trae.ts:378-381）。
pub fn derive_checkin_device_id(uid: &str) -> String {
    seeded_stream(uid, "devid", 15)
        .into_iter()
        .map(|b| char::from(b'0' + b % 10))
        .collect()
}

/// 签到 X-Market-User-ID：派生 UUIDv4（trae.ts:386-391 语义：version/variant 位改写）。
fn derive_market_user_id(uid: &str) -> String {
    let mut bs = seeded_stream(uid, "market", 16);
    bs[6] = (bs[6] & 0x0F) | 0x40;
    bs[8] = (bs[8] & 0x3F) | 0x80;
    let hex: String = bs.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

/// 签到 Vscode-Sessionid：派生 64hex（trae.ts:342-345；盐是 **`sess`**，
/// 不是 `session`——逐字对照参考 seededStream(userId, 'sess', 32)）。
fn derive_session_id_hex(uid: &str) -> String {
    seeded_stream(uid, "sess", 32).iter().map(|b| format!("{b:02x}")).collect()
}

/// 随机 UUIDv4（X-Request-Id 用，trae.ts:386-391 uuidV4 的等价实现：
/// 版本/变体位改写为 RFC 4122 形态）。
fn random_uuid_v4() -> String {
    let hex = crate::key::random_id(16);
    let bs: Vec<u8> = (0..16)
        .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap_or(0))
        .collect();
    let mut bs = bs;
    bs[6] = (bs[6] & 0x0F) | 0x40;
    bs[8] = (bs[8] & 0x3F) | 0x80;
    let h: String = bs.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

const CHECKIN_STATUS_PATH: &str = "/trae/api/v2/ug/checkin_credits/status";
const CHECKIN_CLAIM_PATH: &str = "/trae/api/v2/ug/checkin_credits/claim";
const EXCHANGE_PATH: &str = "/cloudide/api/v3/trae/oauth/ExchangeToken";

#[derive(Debug, Clone, PartialEq)]
pub struct CheckinOutcome {
    pub checked_in: bool,
    pub credits: Option<u64>,
    pub streak_days: Option<u64>,
}

impl TraeProvider {
    /// 签到头族（traeCheckinHeaders，trae.ts:281-314）：设备身份按 uid
    /// **确定性派生**（同账号稳定、跨账号互异——同天共用 device_id 会被
    /// 「该设备已签到」拦截）；每请求独立 X-Request-Id / X-Tt-Trace-Id。
    /// 参考还带 `Accept-Encoding: gzip, deflate`——本网关 reqwest 未启用
    /// gzip 解压特性，声明了却不解压会拿到乱码，故刻意不发。
    fn checkin_headers(&self, cred: &Credential) -> Result<reqwest::header::HeaderMap, ProviderError> {
        let v = parse_secret(&cred.secret)?;
        let uid = secret_str(&v, "uid");
        if uid.is_empty() {
            // 设备身份全由 uid 派生：空 uid 会让所有账号共享同一台"设备"，
            // 直接被「该设备已签到」拦截——对齐参考 postJson 的精确报错
            return Err(ProviderError::Credential(
                "trae 凭据缺 uid，无法派生签到设备身份".into(),
            ));
        }
        let token = secret_str(&v, "access_token");
        let trace_id = format!("00-{}-01", crate::key::random_id(8));
        let mut h = reqwest::header::HeaderMap::new();
        let ins = reqwest::header::HeaderValue::from_str;
        let _ = ins("application/json").map(|x| h.insert("content-type", x));
        let _ = ins("*/*").map(|x| h.insert("accept", x));
        let _ = ins("zh-CN").map(|x| h.insert("accept-language", x));
        let _ = ins("VSCode 1.107.1 (TRAE SOLO CN)").map(|x| h.insert("user-agent", x));
        let _ = ins(&format!("Cloud-IDE-JWT {token}")).map(|x| h.insert("authorization", x));
        let _ = ins("VSCode 1.107.1").map(|x| h.insert("x-market-client-id", x));
        let _ = ins(&derive_market_user_id(&uid)).map(|x| h.insert("x-market-user-id", x));
        let _ = ins("CN").map(|x| h.insert("x-user-region", x));
        let _ = ins(&derive_checkin_device_id(&uid)).map(|x| h.insert("x-device-id", x));
        let _ = ins("3").map(|x| h.insert("x-lgw-req-sdk-type", x));
        let _ = ins("stable_cn").map(|x| h.insert("package-type", x));
        let _ = ins("787976").map(|x| h.insert("x-lscbd-aid", x));
        let _ = ins("windows").map(|x| h.insert("x-lscbd-platform", x));
        let _ = ins(IDE_VERSION).map(|x| h.insert("app-version", x));
        let _ = ins(&trace_id).map(|x| h.insert("x-tt-trace-id", x));
        let _ = ins(&derive_session_id_hex(&uid)).map(|x| h.insert("vscode-sessionid", x));
        let _ = ins(&random_uuid_v4()).map(|x| h.insert("x-request-id", x));
        let _ = ins("empty").map(|x| h.insert("sec-fetch-dest", x));
        let _ = ins("no-cors").map(|x| h.insert("sec-fetch-mode", x));
        let _ = ins("none").map(|x| h.insert("sec-fetch-site", x));
        Ok(h)
    }

    /// ExchangeToken 续期（trae-auth.ts:378-432）：refreshToken 每次轮换
    /// 旧值即刻失效**必须立即回写**；expires_at 归一毫秒字符串；
    /// machine_id/device_id/uid/nickname 完全不动。401/403 或 2xx 无
    /// accessToken 为终态（凭据失效时上游回 HTML 错误页，先读文本再 parse）。
    /// 端点在 **OAuth 域**（api.trae.com.cn，trae-product.ts:186），与 UG 域
    /// （api.trae.cn）不同源。
    pub async fn exchange_refresh(&self, cred: &Credential) -> Result<Credential, ProviderError> {
        let v = parse_secret(&cred.secret)?;
        let rt = secret_str(&v, "refresh_token");
        if rt.is_empty() {
            return Err(ProviderError::Credential("trae 凭据无 refresh_token".into()));
        }
        let mut h = reqwest::header::HeaderMap::new();
        let ins = reqwest::header::HeaderValue::from_str;
        let _ = ins("application/json").map(|x| h.insert("content-type", x));
        let _ = ins("application/json").map(|x| h.insert("accept", x));
        let _ = ins(USER_AGENT).map(|x| h.insert("user-agent", x));
        let resp = self
            .client
            .post(format!("{}{}", self.oauth_base, EXCHANGE_PATH))
            .headers(h)
            .json(&serde_json::json!({
                "ClientID": CLIENT_ID,
                "RefreshToken": rt,
                "ClientSecret": "-",
                "UserID": ""
            }))
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        if status == 401 || status == 403 {
            return Err(ProviderError::Credential(format!(
                "ExchangeToken http {status}: {}",
                text.chars().take(120).collect::<String>()
            )));
        }
        if status != 200 {
            return Err(ProviderError::Upstream(format!("ExchangeToken http {status}")));
        }
        let parsed: Value = serde_json::from_str(&text)
            .map_err(|_| ProviderError::Credential("ExchangeToken 响应非 JSON（疑似登录失效 HTML 页）".into()))?;
        // Result/result 双键（parseTraeExchangeResponse 的 data.Result ?? data.result）
        let result = parsed
            .get("Result")
            .or_else(|| parsed.get("result"))
            .cloned()
            .unwrap_or(parsed);
        let g = |names: [&str; 3]| -> Option<String> {
            for n in names {
                if let Some(s) = result.get(n).and_then(Value::as_str).filter(|s| !s.is_empty()) {
                    return Some(s.to_string());
                }
            }
            None
        };
        let Some(new_token) = g(["Token", "token", "accessToken"]) else {
            return Err(ProviderError::Credential("ExchangeToken 2xx 但无 accessToken（终态）".into()));
        };
        // rt 轮换：新值为空保留旧值
        let new_rt = g(["RefreshToken", "refreshToken", "refresh_token"]).unwrap_or_else(|| rt.clone());
        // expires_at 毫秒字符串归一（trae.ts:619-629 applyTraeRefresh 三态：
        // 毫秒直写 / 秒 ×1000 / 相对秒数 now+duration）
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let read_num = |names: [&str; 2]| -> u64 {
            for n in names {
                match result.get(n) {
                    Some(Value::Number(x)) => {
                        if let Some(u) = x.as_u64() {
                            return u;
                        }
                    }
                    // 兼容字符串形态的数字（readNumberField 的口径）
                    Some(Value::String(s)) => {
                        if let Ok(u) = s.trim().parse::<u64>() {
                            return u;
                        }
                    }
                    _ => {}
                }
            }
            0
        };
        let expire_at = read_num(["TokenExpireAt", "tokenExpireAt"]);
        let duration = read_num(["TokenExpireDuration", "tokenExpireDuration"]);
        let expires_ms = if expire_at > 1_000_000_000_000 {
            expire_at
        } else if expire_at > 0 {
            expire_at.saturating_mul(1000)
        } else if duration > 0 {
            now_ms.saturating_add(duration.saturating_mul(1000))
        } else {
            0
        };
        let mut out = v.clone();
        let Some(obj) = out.as_object_mut() else {
            return Err(ProviderError::Credential("trae 凭据非 JSON".into()));
        };
        obj.insert("access_token".into(), Value::String(new_token));
        obj.insert("refresh_token".into(), Value::String(new_rt));
        if expires_ms > 0 {
            obj.insert("expires_at".into(), Value::String(expires_ms.to_string()));
        } else {
            // 三态都拿不到 → 按「未知」处理，删掉旧 token 遗留的过期时刻
            //（参考写 ''/JWT exp 兜底，语义同为未知；沿用已失效旧 token 的
            // expires_at 会让续期后的过期判断基于错误数据）
            obj.remove("expires_at");
        }
        Ok(Credential { secret: out.to_string(), ..cred.clone() })
    }
}

// ───────────── T4.19：签到 / 模型目录 / Provider::refresh ─────────────

const MODELS_PATH: &str = "/api/ide/v1/batch_get_detail_param";
/// functions 必须传**全部 22 个**（trae-auth.ts:677-686；只传聊天通道会
/// 导致响应错位）。
const ALL_FUNCTIONS: [&str; 22] = [
    "ui_builder_v2", "solo_coder", "chat_v3", "solo_builder",
    "builder_v3", "builder", "chat", "inline_chat", "git_ai",
    "custom_agent_generation", "utils", "code_reviewer",
    "code_review_summary", "solo_agent", "solo_agent_remote",
    "solo_work_remote", "solo_agent_lite", "solo_work_lite",
    "solo_design_lite", "solo_design_remote", "multimodal",
    "system_diagnosis",
];

impl TraeProvider {
    /// 模型目录（trae-auth.ts:676-711 + trae.ts:1205-1272）：非流式头；
    /// 白名单通道整组过滤 + 条目三重过滤（usage==chat_completion、
    /// config_switch!=false、!is_invisible_to_user）；同 config_name
    /// 多通道**后覆盖前**（参考规则 3；规则 1/2 的档位择优——空档位不
    /// 覆盖有档位、同有档位取白名单靠前者——需要 `reasoning_effort_config`
    /// 元数据参与择优且当前 `ModelInfo` 只有 id、择优结果不可观测，
    /// 接入档位元数据时再补）。成功后写入 catalog 缓存。
    pub async fn fetch_models(
        &self,
        cred: &Credential,
    ) -> Result<Vec<crate::registry::ModelInfo>, ProviderError> {
        let resp = self
            .client
            .post(format!("{}{}", self.agent_base, MODELS_PATH))
            .headers(Self::solo_headers_with(cred, false, 0))
            .json(&serde_json::json!({
                "functions": ALL_FUNCTIONS,
                "agent_type": "",
                "current_config_info": { "config_name": "", "is_custom_model": false },
                "mode_type": 0,
                "access_type": 0,
                "ab_force_vids": "",
                "ab_autotest_advanced_mode": 0,
                "show_custom_model": true,
            }))
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if status != 200 {
            return Err(ProviderError::Upstream(format!("models http {status}")));
        }
        let v: Value = serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Upstream(format!("models 响应非 JSON: {e}")))?;
        let groups = v
            .get("function_configs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut seen: std::collections::HashMap<String, ()> = std::collections::HashMap::new();
        let mut out: Vec<crate::registry::ModelInfo> = Vec::new();
        for g in groups {
            let func = g.get("function").and_then(Value::as_str).unwrap_or("");
            if !TRAE_CHANNELS.contains(&func) {
                continue; // 非白名单通道整组丢弃（发出去必被拒：4023/4001/3003）
            }
            for item in g.get("config_info_list").and_then(Value::as_array).cloned().unwrap_or_default() {
                let usage_ok = item.get("usage").and_then(Value::as_str) == Some("chat_completion");
                let switch_ok = item.get("config_switch").and_then(Value::as_bool) != Some(false);
                let visible_ok = item.get("is_invisible_to_user").and_then(Value::as_bool) != Some(true);
                let Some(name) = item.get("config_name").and_then(Value::as_str).filter(|n| !n.is_empty()) else { continue };
                if !usage_ok || !switch_ok || !visible_ok {
                    continue;
                }
                if seen.insert(name.to_string(), ()).is_none() {
                    out.push(crate::registry::ModelInfo { id: name.to_string() });
                }
            }
        }
        // catalog() 优先返回远端目录（参考 TraeAdapter.remoteModels 缓存）
        *self.models_cache.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::sync::Arc::new(out.clone()));
        Ok(out)
    }

    /// 签到状态：{checked_in, credits, streak_days}。
    /// 业务码非 0 视为失败（对照 fetchTraeCheckinStatus 的
    /// `readClaimCode(body); if (code !== 0) return null`）。
    pub async fn checkin_status(&self, cred: &Credential) -> Result<CheckinOutcome, ProviderError> {
        let v = self.ug_post(cred, CHECKIN_STATUS_PATH).await?;
        if let Some(code) = read_ug_code(&v) {
            if code != 0 {
                return Err(ProviderError::Upstream(format!("checkin status code {code}")));
            }
        }
        Ok(CheckinOutcome {
            checked_in: v.get("checked_in").and_then(Value::as_bool).unwrap_or(false),
            credits: v.get("credits").and_then(Value::as_u64),
            streak_days: v.get("streak_days").and_then(Value::as_u64),
        })
    }

    /// 签到领取：claim 响应只有 {"code":0} **不含积分数**——须补查
    /// status 取真实 credits/streak_days。9074（人数过多，code 可为字符串）
    /// 冷却 300s **不换设备**；1005 → PlanLimit 冷却 12h；其余业务码 →
    /// BusinessError 冷却 300s（classifyTraeCheckinError，
    /// trae-credits.ts:74-100）。
    pub async fn checkin_claim(&self, cred: &Credential) -> Result<CheckinOutcome, ProviderError> {
        let v = self.ug_post(cred, CHECKIN_CLAIM_PATH).await?;
        match read_ug_code(&v) {
            Some(0) | None => {}
            Some(9074) => {
                return Err(ProviderError::RateLimited {
                    retry_after_secs: Some(300),
                    msg: "checkin too many people (9074)：不换设备，冷却后重试".into(),
                });
            }
            Some(1005) => {
                return Err(ProviderError::RateLimited {
                    retry_after_secs: Some(12 * 3600),
                    msg: "checkin plan limit (1005)".into(),
                });
            }
            Some(other) => {
                return Err(ProviderError::RateLimited {
                    retry_after_secs: Some(300),
                    msg: format!("checkin business code {other}"),
                });
            }
        }
        // 幂等补查（claim 已签返回 code:0；真实数值在 status）
        self.checkin_status(cred).await
    }

    async fn ug_post(&self, cred: &Credential, path: &str) -> Result<Value, ProviderError> {
        let resp = self
            .client
            .post(format!("{}{}", self.api_base, path))
            .headers(self.checkin_headers(cred)?)
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if status != 200 {
            return Err(ProviderError::Upstream(format!("ug http {status}")));
        }
        serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Upstream(format!("ug 响应非 JSON: {e}")))
    }
}

/// 读取 UG（签到）业务码，兼容数字与字符串形态（readClaimCode，
/// trae-credits.ts:127-132：后端在部分网关上以字符串 `"9074"` 返回）。
/// 键缺失视为成功（0）；存在但解析不出数字按业务失败（-1）。
fn read_ug_code(v: &Value) -> Option<i64> {
    match v.get("code") {
        Some(Value::Number(n)) => n.as_i64(),
        Some(Value::String(s)) => s.trim().parse::<i64>().ok().or(Some(-1)),
        _ => None,
    }
}

