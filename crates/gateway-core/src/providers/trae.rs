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
/// 推理域（agentHost，trae-product.ts TRAE_AGENT_HOST）。
pub const DEFAULT_AGENT_BASE: &str = "https://trae-api-cn.mchost.guru";
const BALANCE_PATH: &str = "/trae/api/v2/pay/ide_user_ent_usage";
const CHAT_PATH: &str = "/api/agent/v3/llm_utils_chat";
const CALLBACK_PREFERRED_PORT: u16 = 18080;

// 产品常量（参考实现 trae-product.ts:316-342 实测值）
pub const USER_AGENT: &str = "Trae/0.1.52";
pub const APP_ID: &str = "6eefa01c-1036-4c7e-9ca5-d891f63bfcd8";
pub const IDE_VERSION: &str = "0.1.52";
pub const IDE_VERSION_CODE: &str = "20260811";
pub const OS_VERSION: &str = "macOS 15.7.4";
pub const DEVICE_BRAND: &str = "Apple";
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
    agent_base: String,
    client: reqwest::Client,
}

impl TraeProvider {
    pub fn new(api_base: String) -> Self {
        Self::with_agent_base(api_base, DEFAULT_AGENT_BASE.into())
    }

    /// api 域（余额/签到）与推理域（agentHost）分离注入。
    pub fn with_agent_base(api_base: String, agent_base: String) -> Self {
        Self { api_base, agent_base, client: reqwest::Client::new() }
    }

    pub fn production() -> Self {
        Self::new(DEFAULT_API_BASE.into())
    }

    /// `traeSOLOHeaders`（推理/IDE 消费共用的鉴权头族）。
    /// 已载明值照抄；`X-App-Id`/`X-Ide-Version`/`X-OS-Version`/
    /// `X-Device-Brand` 的具体值手册未载明，占位待 wire 核对。
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
        // 形状对照参考 trae-credits.ts:330-389：
        // data.user_entitlement_pack_list[].entitlement_base_info.quota.credits_limit
        // 与同条目 usage.credits_amount、条目级 expire_time（秒）。
        let packs = v
            .pointer("/data/user_entitlement_pack_list")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let total: i64 = packs
            .iter()
            .map(|pack| {
                let limit = pack
                    .pointer("/entitlement_base_info/quota/credits_limit")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let used = pack.pointer("/usage/credits_amount").and_then(Value::as_i64).unwrap_or(0);
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
                            m.insert("access_token".into(), Value::String(jwt));
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
                                m.insert("nickname".into(), Value::String(fix_double_encoding(nick)));
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


// ───────────────── SOLO 推理通道（T4.4b，复刻参考实现 trae.ts） ─────────────────

use crate::openai::{ChatCompletion, ChatRequest, Usage};
use crate::provider::{ChunkStream, Provider, StreamChunk};
use crate::route::Route;
use async_trait::async_trait;

/// OpenAI → SOLO 请求体（transformToSOLOBody，trae.ts:1545-1577）：
/// stream 恒 true；function=通道名；model→config_name+model 双字段
/// （`__` 后缀消除）；content 字符串→`[{type:"text"}]`；assistant
/// tool_calls function→function_call（无 name 剔除、全剔删键）；
/// tool_choice 归一；tools.parameters 对象→JSON 字符串；max_tokens 钳 64000。
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
        if let Some(r) = &msg.reasoning_content {
            o.insert("reasoning_content".into(), Value::String(r.clone()));
        }
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
            let typ = v.get("type").and_then(Value::as_str).unwrap_or("").to_lowercase();
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
pub fn parse_solo_sse(text: &str) -> Vec<SoloEvent> {
    let mut out: Vec<SoloEvent> = Vec::new();
    let mut ev_name = String::new();
    let mut data = String::new();
    for line in text.lines() {
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

/// trae 错误分类（trae-errors.ts + cooldown-table.ts）：
/// 会话死亡标记 → 重登；4008 → 24h；1005 → 12h；4011/429 → 60s；
/// 404 → 1h；5xx → Upstream；其他 4xx → BadRequest。
pub fn classify_trae_error(status: u16, code: Option<i64>, message: &str) -> ProviderError {
    let m = message.to_lowercase();
    let session_dead = status == 401
        || m.contains("login")
        || m.contains("token 失效")
        || m.contains("token invalid")
        || m.contains("session")
        || m.contains("unauthorized")
        || m.contains("401");
    if session_dead {
        return ProviderError::Credential(format!("session dead: {message}"));
    }
    match code {
        Some(4008) => ProviderError::RateLimited {
            retry_after_secs: Some(24 * 3600),
            msg: format!("ide_credits exhausted (4008): {message}"),
        },
        Some(1005) => ProviderError::RateLimited {
            retry_after_secs: Some(12 * 3600),
            msg: format!("plan limit (1005): {message}"),
        },
        Some(4011) => ProviderError::RateLimited {
            retry_after_secs: Some(60),
            msg: format!("rate limited (4011): {message}"),
        },
        _ => match status {
            429 => ProviderError::RateLimited { retry_after_secs: Some(60), msg: message.into() },
            404 => ProviderError::RateLimited { retry_after_secs: Some(3600), msg: format!("not found: {message}") },
            s if (500..=599).contains(&s) => ProviderError::Upstream(format!("http {s}: {message}")),
            s if (400..=499).contains(&s) => ProviderError::BadRequest(format!("http {s}: {message}")),
            _ => ProviderError::Upstream(message.into()),
        },
    }
}

struct SoloAgg {
    text: String,
    reasoning: String,
    tool_calls: Vec<(u64, Option<String>, Option<String>, String)>,
    usage: Usage,
    finish_reason: Option<String>,
}

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
                    agg.tool_calls.push((
                        i as u64,
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

#[async_trait]
impl Provider for TraeProvider {
    fn id(&self) -> &str {
        "trae"
    }

    async fn refresh(&self, cred: &Credential) -> Result<Credential, ProviderError> {
        self.exchange_refresh(cred).await
    }

    fn catalog(&self) -> crate::registry::ProviderCatalog {
        crate::registry::ProviderCatalog {
            id: "trae".into(),
            models: vec![crate::registry::ModelInfo { id: "glm-5.2".into() }],
        }
    }

    async fn complete(
        &self,
        cred: &Credential,
        route: &Route,
        req: &ChatRequest,
    ) -> Result<ChatCompletion, ProviderError> {
        // 空响应（200 零可解析事件，含 metadata）→ 同账号重发一次；
        // 已收到任何事件则绝不重放（trae-adapter.ts:1280-1289/1694-1699）
        let (mut status, mut body) = send_chat(self, cred, route, req).await?;
        if status == 200 && parse_solo_sse(&body).is_empty() {
            let (s2, b2) = send_chat(self, cred, route, req).await?;
            status = s2;
            body = b2;
        }
        if status != 200 {
            return Err(classify_trae_error(status, None, &body));
        }
        let agg = aggregate(parse_solo_sse(&body), status)?;
        let Some(reason) = agg.finish_reason else {
            // 无 done 事件 = 截断，不伪造完成
            return Err(ProviderError::Upstream("no done event (truncated stream)".into()));
        };
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
        let (mut status, mut body) = send_chat(self, cred, route, req).await?;
        if status == 200 && parse_solo_sse(&body).is_empty() {
            let (s2, b2) = send_chat(self, cred, route, req).await?;
            status = s2;
            body = b2;
        }
        if status != 200 {
            return Err(classify_trae_error(status, None, &body));
        }
        let agg = aggregate(parse_solo_sse(&body), status)?;
        let Some(reason) = agg.finish_reason else {
            return Err(ProviderError::Upstream("no done event (truncated stream)".into()));
        };
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

/// 签到 Vscode-Sessionid：派生 64hex（trae.ts:342-345）。
fn derive_session_id_hex(uid: &str) -> String {
    seeded_stream(uid, "session", 32).iter().map(|b| format!("{b:02x}")).collect()
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
    /// 「该设备已签到」拦截）。
    fn checkin_headers(&self, cred: &Credential) -> Result<reqwest::header::HeaderMap, ProviderError> {
        let v = parse_secret(&cred.secret)?;
        let uid = secret_str(&v, "uid");
        let token = secret_str(&v, "access_token");
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
        let _ = ins(&derive_session_id_hex(&uid)).map(|x| h.insert("vscode-sessionid", x));
        Ok(h)
    }

    /// ExchangeToken 续期（trae-auth.ts:378-432）：refreshToken 每次轮换
    /// 旧值即刻失效**必须立即回写**；expires_at 归一毫秒字符串；
    /// machine_id/device_id/uid/nickname 完全不动。401/403 或 2xx 无
    /// accessToken 为终态（凭据失效时上游回 HTML 错误页，先读文本再 parse）。
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
            .post(format!("{}{}", self.api_base, EXCHANGE_PATH))
            .headers(h)
            .json(&serde_json::json!({
                "ClientID": "en1oxy7wnw8j9n",
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
        let result = parsed.get("Result").cloned().unwrap_or(parsed);
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
        // expires_at 毫秒字符串归一（trae.ts:619-629 三态）
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let expire_at = result
            .get("TokenExpireAt")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let duration = result
            .get("TokenExpireDuration")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let expires_ms = if expire_at > 1_000_000_000_000 {
            expire_at
        } else if expire_at > 1_000_000_000 {
            expire_at * 1000
        } else if duration > 0 {
            now_ms + duration * 1000
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
    /// 模型目录（trae-auth.ts:676-711 + trae.ts:1211-1268）：非流式头；
    /// 白名单通道整组过滤 + 条目三重过滤（usage==chat_completion、
    /// config_switch!=false、!is_invisible_to_user）；同 config_name
    /// 多通道**后覆盖前**（参考规则 3；规则 1/2 的档位择优在接入档位
    /// 元数据时补）。
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
        Ok(out)
    }

    /// 签到状态：{checked_in, credits, streak_days}。
    pub async fn checkin_status(&self, cred: &Credential) -> Result<CheckinOutcome, ProviderError> {
        let v = self.ug_post(cred, CHECKIN_STATUS_PATH).await?;
        Ok(CheckinOutcome {
            checked_in: v.get("checked_in").and_then(Value::as_bool).unwrap_or(false),
            credits: v.get("credits").and_then(Value::as_u64),
            streak_days: v.get("streak_days").and_then(Value::as_u64),
        })
    }

    /// 签到领取：claim 响应只有 {"code":0} **不含积分数**——须补查
    /// status 取真实 credits/streak_days。9074（人数过多，code 可为字符串）
    /// 冷却 300s **不换设备**。
    pub async fn checkin_claim(&self, cred: &Credential) -> Result<CheckinOutcome, ProviderError> {
        let v = self.ug_post(cred, CHECKIN_CLAIM_PATH).await?;
        // code 兼容数字与字符串两种形态
        let code = match v.get("code") {
            Some(Value::Number(n)) => n.as_i64(),
            Some(Value::String(s)) => s.parse::<i64>().ok(),
            _ => None,
        };
        match code {
            Some(0) => {}
            Some(9074) => {
                return Err(ProviderError::RateLimited {
                    retry_after_secs: Some(300),
                    msg: "checkin too many people (9074)：不换设备，冷却后重试".into(),
                });
            }
            Some(other) => {
                return Err(ProviderError::Upstream(format!("checkin code {other}")));
            }
            None => {}
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

