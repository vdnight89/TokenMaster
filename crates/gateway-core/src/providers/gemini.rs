//! gemini Provider（Google Cloud Code Assist 免费线，Antigravity 客户端身份伪装）。
//!
//! 上游：`POST {base}/v1internal:streamGenerateContent?alt=sse`（SSE 帧）。
//! 协议要点（对照 reference/deepseek-harness-codearts.md §3.6/§4.14）：
//! - 双层信封**每层键字母序**（Go map 序列化语义；serde_json preserve_order 下
//!   显式按序重建，`alphabetize`）。
//! - 五个身份头**写死**（x-machine-id/x-vscode-sessionid 是占位串，不生成随机值）；
//!   流式请求刻意不带 `Accept`；不发 `x-goog-api-key`。
//! - 模型名是准入键：必须带 `-low/-medium/-high/-tiered` 档位后缀，
//!   对外只暴露主名，出站默认补 `-medium`。
//! - system 抽到顶层 `systemInstruction`；assistant 角色 → `model`。
//!
//! project 动态探测（loadCodeAssist）与配额（retrieveUserQuotaSummary，请求体
//! 空对象）见 `detect_project`/`quota_summary`；thoughtSignature 跨轮回填、
//! sessionId 升代自愈在后续切片接入（T4.2c）。

use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::openai::{ChatCompletion, ChatRequest, Usage};
use crate::provider::{ChunkStream, Credential, Provider, ProviderError, StreamChunk};
use crate::registry::{ModelInfo, ProviderCatalog};
use crate::route::Route;
use crate::sse::SseParser;

pub const DEFAULT_BASE: &str = "https://daily-cloudcode-pa.googleapis.com";
const GENERATE_PATH: &str = "/v1internal:streamGenerateContent?alt=sse";
const LOAD_PATH: &str = "/v1internal:loadCodeAssist";
const QUOTA_PATH: &str = "/v1internal:retrieveUserQuotaSummary";
const CLIENT_UA: &str = "antigravity/4.3.0 (cmdc-pak)";
const DEFAULT_EFFORT_SUFFIX: &str = "-medium";

pub struct GeminiProvider {
    base: String,
    /// 探测到空时的兜底 project（`aicode-consumers`）。
    fallback_project: String,
    /// 会话派生 + 升代状态。
    session: std::sync::Mutex<SessionState>,
    client: reqwest::Client,
    /// 探测成功后缓存的 project（None = 尚未探测）。
    resolved_project: std::sync::Mutex<Option<String>>,
    /// thoughtSignature 跨轮状态（生产落盘，测试注入内存）。
    sigs: std::sync::Arc<SigStore>,
}

/// thoughtSignature 跨轮状态（§4.14 要点②）。
/// 精确键 =「工具名 + 按键名升序的紧凑 JSON」（§3.6 `canonicalArgs`，两侧必须
/// 同一序列化）；精确 miss 时按工具名最近一次兜底。生产落盘
/// `~/.tokenmaster/gemini-sigs.json` 独立文件（共享文档整体替换会抹掉未知
/// 字段的教训——签名状态单独成文件，不进账号存储）。
#[derive(Default)]
pub struct SigStore {
    exact: std::sync::Mutex<std::collections::HashMap<String, String>>,
    by_name: std::sync::Mutex<std::collections::HashMap<String, String>>,
    path: Option<std::path::PathBuf>,
}

impl SigStore {
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// 读已有文件恢复（缺失/损坏降级为空表，推理不因此中断）。
    pub fn load_or_create(path: std::path::PathBuf) -> Self {
        let store = Self { path: Some(path.clone()), ..Self::default() };
        let Ok(txt) = std::fs::read_to_string(&path) else { return store };
        let Ok(v) = serde_json::from_str::<Value>(&txt) else { return store };
        if let Some(m) = v.get("exact").and_then(Value::as_object) {
            for (k, sig) in m {
                if let Some(s) = sig.as_str() {
                    store.exact.lock().unwrap().insert(k.clone(), s.to_string());
                }
            }
        }
        if let Some(m) = v.get("by_name").and_then(Value::as_object) {
            for (k, sig) in m {
                if let Some(s) = sig.as_str() {
                    store.by_name.lock().unwrap().insert(k.clone(), s.to_string());
                }
            }
        }
        store
    }

    fn key(name: &str, canonical_args: &str) -> String {
        format!("tool:{name}{canonical_args}")
    }

    /// 记录一次「响应中 functionCall (name, args) ↔ 签名」。
    pub fn record(&self, name: &str, args: &Value, sig: &str) {
        let canonical = alphabetize(args).to_string();
        self.record_with_canonical(name, &canonical, sig);
    }

    fn record_with_canonical(&self, name: &str, canonical_args: &str, sig: &str) {
        self.exact
            .lock()
            .unwrap()
            .insert(Self::key(name, canonical_args), sig.to_string());
        self.by_name.lock().unwrap().insert(name.to_string(), sig.to_string());
        if let Some(p) = &self.path {
            let snapshot = serde_json::json!({
                "exact": &*self.exact.lock().unwrap(),
                "by_name": &*self.by_name.lock().unwrap(),
            });
            let _ = std::fs::write(p, snapshot.to_string());
        }
    }

    /// 精确键优先，miss 时按工具名最近一次兜底；从未见过返回 None。
    pub fn lookup(&self, name: &str, args: &Value) -> Option<String> {
        let canonical = alphabetize(args).to_string();
        let key = Self::key(name, &canonical);
        self.exact.lock().unwrap().get(&key).cloned().or_else(|| {
            self.by_name.lock().unwrap().get(name).cloned()
        })
    }
}

/// 会话升代状态：派生因子 `(project, 首条 user 文本, lane)` 的确定性散列，
/// 外加代数计数。服务端按 sessionId 累计 token，1M 超限后该 id **永久 400**，
/// 唯一出路是升代换新 id（一次请求最多一代）。
#[derive(Default)]
struct SessionState {
    base: String,
    gen: u32,
}

fn format_session(s: &SessionState) -> String {
    if s.gen == 0 {
        s.base.clone()
    } else {
        format!("{}-g{}", s.base, s.gen)
    }
}

/// lane 语义参考实现未载明；按上游端点 host 近似（daily/sandbox 两条端点
/// 即两条 lane，端口不参与——同一 host 的测试 stub 不换会话）。
fn host_of(base: &str) -> String {
    base.trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or(base)
        .split(':')
        .next()
        .unwrap_or(base)
        .to_string()
}

/// 上下文超限专属判据（§4.14 要点⑤：句式无 context 字样，
/// harness 认不出，不能靠通用 400 处理）。
pub fn is_context_exceeded(text: &str) -> bool {
    text.contains("The input token count") && text.contains("exceeds")
}

impl GeminiProvider {
    pub fn new(base: String, fallback_project: String) -> Self {
        Self {
            base,
            fallback_project,
            session: std::sync::Mutex::new(SessionState::default()),
            client: reqwest::Client::new(),
            resolved_project: std::sync::Mutex::new(None),
            sigs: std::sync::Arc::new(SigStore::in_memory()),
        }
    }

    /// sessionId 确定性派生：`(project, 首条 user 文本, lane)` 散列，
    /// 跨实例稳定（非随机）——同一会话因子命中服务端同一累计桶。
    pub fn derive_session_id(project: &str, first_user: &str, lane: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(project.as_bytes());
        h.update([0u8]);
        h.update(first_user.as_bytes());
        h.update([0u8]);
        h.update(lane.as_bytes());
        let hex = format!("{:x}", h.finalize());
        format!("sess-{}", &hex[..16])
    }

    fn first_user_text(req: &ChatRequest) -> String {
        req.messages
            .iter()
            .find(|m| m.role == "user")
            .map(|m| m.text())
            .unwrap_or_default()
    }

    /// 会话解析：派生因子变化 → 切换新会话（gen 归零）；返回当前代 sessionId。
    fn current_session_id(&self, project: &str, req: &ChatRequest) -> String {
        let lane = host_of(&self.base);
        let base = Self::derive_session_id(project, &Self::first_user_text(req), &lane);
        let mut s = self.session.lock().unwrap();
        if s.base != base {
            *s = SessionState { base, gen: 0 };
        }
        format_session(&s)
    }

    /// 升代：gen+1 换新 id（服务端按 id 累计，削本地历史无用）。
    fn bump_generation(&self, project: &str, req: &ChatRequest) -> String {
        let lane = host_of(&self.base);
        let base = Self::derive_session_id(project, &Self::first_user_text(req), &lane);
        let mut s = self.session.lock().unwrap();
        if s.base != base {
            *s = SessionState { base, gen: 0 };
        }
        s.gen += 1;
        format_session(&s)
    }

    pub fn production() -> Self {
        let mut p = Self::new(DEFAULT_BASE.into(), "aicode-consumers".into());
        if let Ok(st) = crate::store::Store::open_default() {
            p.sigs = std::sync::Arc::new(SigStore::load_or_create(
                st.root().join("gemini-sigs.json"),
            ));
        }
        p
    }

    /// 注入共享 sigstore（跨 Provider 实例复用签名状态）。
    pub fn with_sig_store(mut self, sigs: std::sync::Arc<SigStore>) -> Self {
        self.sigs = sigs;
        self
    }

    fn identity_headers(&self, cred: &Credential) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        let ins = reqwest::header::HeaderValue::from_str;
        // JSON 凭据取 access_token；裸串回退兼容
        let (at, _) = parse_secret(&cred.secret);
        let _ = ins(&format!("Bearer {at}")).map(|v| h.insert("authorization", v));
        let _ = ins(CLIENT_UA).map(|v| h.insert("user-agent", v));
        let _ = ins("antigravity").map(|v| h.insert("x-client-name", v));
        let _ = ins("4.3.0").map(|v| h.insert("x-client-version", v));
        let _ = ins("cmdc-pak").map(|v| h.insert("x-machine-id", v));
        let _ = ins("proxy").map(|v| h.insert("x-vscode-sessionid", v));
        h
    }

    /// 对外主名 → 上游准入名（无档位后缀时默认 `-medium`）。
    fn wire_model(model: &str) -> String {
        const SUFFIXES: [&str; 4] = ["-low", "-medium", "-high", "-tiered"];
        if SUFFIXES.iter().any(|s| model.ends_with(s)) {
            model.to_string()
        } else {
            format!("{model}{DEFAULT_EFFORT_SUFFIX}")
        }
    }

    async fn load_code_assist(&self, cred: &Credential) -> Result<Value, ProviderError> {
        let resp = self
            .client
            .post(format!("{}{}", self.base, LOAD_PATH))
            .headers(self.identity_headers(cred))
            .json(&Value::Object(Map::new()))
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?.to_vec();
        if status != 200 {
            return Err(map_status_error(status, String::from_utf8_lossy(&bytes).to_string()));
        }
        serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Upstream(format!("loadCodeAssist 响应非 JSON: {e}")))
    }

    /// project 动态探测（每次真探测并更新缓存；GUI/入池可手动刷新）。
    /// 失败 ≠ 探测到空：HTTP/解析失败返回 Err，调用方不得发推理；
    /// 200 但无 `cloudaicompanionProject` 时用兜底值。
    pub async fn detect_project(&self, cred: &Credential) -> Result<String, ProviderError> {
        let v = self.load_code_assist(cred).await?;
        let detected = v.get("cloudaicompanionProject").and_then(Value::as_str).unwrap_or("");
        let project = if detected.is_empty() {
            self.fallback_project.clone()
        } else {
            detected.to_string()
        };
        *self.resolved_project.lock().unwrap() = Some(project.clone());
        Ok(project)
    }

    /// 推理路径：缓存优先，miss 时探测一次。
    async fn resolve_project(&self, cred: &Credential) -> Result<String, ProviderError> {
        if let Some(p) = self.resolved_project.lock().unwrap().clone() {
            return Ok(p);
        }
        self.detect_project(cred).await
    }

    /// 配额摘要（5h/周双窗口百分比，单位 '%' 不参与积分归一）。
    /// 请求体固定空对象 `{}` 且不带 project；响应 schema 参考实现未在手册给出
    /// 完整字段表，原样透传 JSON 供 GUI 接线时以真实响应校准。
    pub async fn quota_summary(&self, cred: &Credential) -> Result<Value, ProviderError> {
        let resp = self
            .client
            .post(format!("{}{}", self.base, QUOTA_PATH))
            .headers(self.identity_headers(cred))
            .json(&Value::Object(Map::new()))
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?.to_vec();
        if status != 200 {
            return Err(map_status_error(status, String::from_utf8_lossy(&bytes).to_string()));
        }
        serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Upstream(format!("quotaSummary 响应非 JSON: {e}")))
    }

    /// 预扫全消息：tool_call_id → function name。`functionResponse` 的 name
    /// 必须来自对应 tool_use（上游按 name 配对），tool 消息自身只有 id。
    fn tool_name_map(req: &ChatRequest) -> std::collections::HashMap<String, String> {
        let mut map = std::collections::HashMap::new();
        for m in &req.messages {
            if let Some(Value::Array(calls)) = &m.tool_calls {
                for c in calls {
                    if let (Some(id), Some(name)) = (
                        c.get("id").and_then(Value::as_str),
                        c.pointer("/function/name").and_then(Value::as_str),
                    ) {
                        map.insert(id.to_string(), name.to_string());
                    }
                }
            }
        }
        map
    }

    fn build_envelope(&self, project: &str, session_id: &str, route: &Route, req: &ChatRequest) -> Result<Value, ProviderError> {
        let name_map = Self::tool_name_map(req);
        let mut contents: Vec<Value> = Vec::new();
        let mut system_parts: Vec<String> = Vec::new();
        for m in &req.messages {
            if m.role == "system" {
                let t = m.text();
                if !t.is_empty() {
                    system_parts.push(t);
                }
                continue;
            }
            let role = if m.role == "assistant" { "model" } else { "user" };
            let mut parts: Vec<Value> = Vec::new();
            let t = m.text();
            // tool 消息的结果只走 functionResponse.response.result，不另发 text part
            if m.role != "tool" && !t.is_empty() {
                parts.push(serde_json::json!({ "text": t }));
            }
            match m.role.as_str() {
                "assistant" => {
                    if let Some(Value::Array(calls)) = &m.tool_calls {
                        for c in calls {
                            let name = c
                                .pointer("/function/name")
                                .and_then(Value::as_str)
                                .ok_or_else(|| ProviderError::BadRequest("tool_call 缺 function.name".into()))?;
                            let args_raw = c
                                .pointer("/function/arguments")
                                .and_then(Value::as_str)
                                .unwrap_or("{}");
                            // args 解析失败明确报错，不伪造 {} 合法外观
                            let args: Value = serde_json::from_str(args_raw)
                                .map_err(|e| ProviderError::BadRequest(format!("tool_call arguments 非 JSON: {e}")))?;
                            let mut fc = Map::new();
                            fc.insert("args".into(), args.clone());
                            fc.insert("name".into(), Value::String(name.to_string()));
                            // 跨轮签名回填：精确键优先，同工具名最近一次兜底
                            if let Some(sig) = self.sigs.lookup(name, &args) {
                                fc.insert("thoughtSignature".into(), Value::String(sig));
                            }
                            parts.push(serde_json::json!({ "functionCall": Value::Object(fc) }));
                        }
                    }
                }
                "tool" => {
                    let id = m.tool_call_id.as_deref().ok_or_else(|| {
                        ProviderError::BadRequest("role:tool 消息缺 tool_call_id".into())
                    })?;
                    let name = name_map.get(id).ok_or_else(|| {
                        ProviderError::BadRequest(format!("tool_call_id {id} 无对应 tool_use"))
                    })?;
                    parts.push(serde_json::json!({
                        "functionResponse": { "name": name, "response": { "result": t } }
                    }));
                }
                _ => {}
            }
            if parts.is_empty() {
                parts.push(serde_json::json!({ "text": t }));
            }
            contents.push(serde_json::json!({ "parts": parts, "role": role }));
        }
        let mut request = Map::new();
        request.insert("contents".into(), Value::Array(contents));
        let mut generation = Map::new();
        if let Some(t) = req.raw.get("temperature") {
            generation.insert("temperature".into(), t.clone());
        }
        request.insert("generationConfig".into(), Value::Object(generation));
        request.insert("sessionId".into(), Value::String(session_id.to_string()));
        if !system_parts.is_empty() {
            request.insert(
                "systemInstruction".into(),
                serde_json::json!({ "parts": [{ "text": system_parts.join("\n\n") }] }),
            );
        }
        // OpenAI tools 声明 → functionDeclarations
        if let Some(Value::Array(tools)) = req.raw.get("tools") {
            let mut decls: Vec<Value> = Vec::new();
            for t in tools {
                let Some(f) = t.get("function") else { continue };
                let mut d = Map::new();
                if let Some(n) = f.get("name") {
                    d.insert("name".into(), n.clone());
                }
                if let Some(p) = f.get("parameters") {
                    d.insert("parameters".into(), p.clone());
                }
                decls.push(Value::Object(d));
            }
            if !decls.is_empty() {
                request.insert(
                    "tools".into(),
                    serde_json::json!([{ "functionDeclarations": decls }]),
                );
            }
        }
        let mut root = Map::new();
        root.insert("model".into(), Value::String(Self::wire_model(&route.model)));
        root.insert("project".into(), Value::String(project.to_string()));
        root.insert("request".into(), Value::Object(request));
        root.insert("requestId".into(), Value::String(crate::key::random_id(10)));
        root.insert("userAgent".into(), Value::String(CLIENT_UA.into()));
        Ok(alphabetize(&Value::Object(root)))
    }

    /// 发送推理请求：返回 (HTTP 状态, 响应体)。
    /// 400 处置升级式：先判上下文超限（升代重试一次，再超限归
    /// `ContextWindowExceeded`），否则带签被拒去签重试一次。
    async fn send(
        &self,
        cred: &Credential,
        route: &Route,
        req: &ChatRequest,
    ) -> Result<(u16, Vec<u8>), ProviderError> {
        // 探测失败不发推理（失败 ≠ 探测到空）
        let project = self.resolve_project(cred).await?;
        let url = format!("{}{}", self.base, GENERATE_PATH);
        let headers = self.identity_headers(cred);
        let post = |body: Value| {
            let mut req = self.client.post(&url);
            req = req.headers(headers.clone()).json(&body);
            async move { req.send().await }
        };
        let sid = self.current_session_id(&project, req);
        let body = self.build_envelope(&project, &sid, route, req)?;
        let resp = post(body.clone()).await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?.to_vec();
        if status == 400 {
            let text = String::from_utf8_lossy(&bytes).to_string();
            if is_context_exceeded(&text) {
                // 升代自愈：服务端按 sessionId 累计，唯一出路换新 id；一次请求最多一代
                let sid2 = self.bump_generation(&project, req);
                let body2 = self.build_envelope(&project, &sid2, route, req)?;
                let resp2 = post(body2).await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
                let status2 = resp2.status().as_u16();
                let bytes2 = resp2.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?.to_vec();
                if status2 == 400 && is_context_exceeded(&String::from_utf8_lossy(&bytes2)) {
                    return Err(ProviderError::ContextWindowExceeded(text));
                }
                return Ok((status2, bytes2));
            }
            // 带签被拒 → 去签重试一次（§4.14 要点②；仅一次，不再循环）
            let stripped = strip_thought_signatures(&body);
            if stripped != body {
                let resp2 = post(stripped).await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
                let status2 = resp2.status().as_u16();
                let bytes2 = resp2.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?.to_vec();
                return Ok((status2, bytes2));
            }
        }
        Ok((status, bytes))
    }
}

/// 递归移除全部 `thoughtSignature` 键（去签重试用）。
fn strip_thought_signatures(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut out = Map::new();
            for (k, val) in m {
                if k == "thoughtSignature" {
                    continue;
                }
                out.insert(k.clone(), strip_thought_signatures(val));
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(strip_thought_signatures).collect()),
        other => other.clone(),
    }
}

/// 递归按键字母序重建对象（serde_json preserve_order 下即输出字母序）。
pub fn alphabetize(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let mut out = Map::new();
            for k in keys {
                out.insert(k.clone(), alphabetize(&m[k]));
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(alphabetize).collect()),
        other => other.clone(),
    }
}

/// HTTP 状态 → ProviderError（公开供单测）。
pub fn map_status_error(status: u16, msg: String) -> ProviderError {
    match status {
        401 => ProviderError::Credential(format!("401 token rejected: {msg}")),
        429 => ProviderError::RateLimited { retry_after_secs: Some(60), msg: format!("429 quota: {msg}") },
        404 => ProviderError::BadRequest(format!("404 model not admitted (needs effort suffix): {msg}")),
        code => ProviderError::Upstream(format!("http {code}: {msg}")),
    }
}

/// 一帧 SSE data 的聚合结果。
struct FrameData {
    text: String,
    /// functionCall 调用 (name, args JSON 串)
    calls: Vec<(String, String)>,
    /// 签名事件 (工具名, canonical args, signature)——与同消息后续 functionCall 配对
    sig_events: Vec<(String, String, String)>,
    usage: Option<Usage>,
}

fn parse_frame(v: &Value) -> FrameData {
    let parts = v.pointer("/candidates/0/content/parts").and_then(Value::as_array);
    let text: String = parts
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<String>()
        })
        .unwrap_or_default();
    // 签名配对：thinking part 的 thoughtSignature 搬到同消息后续 functionCall 上
    let mut pending_sigs: std::collections::VecDeque<String> = parts
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p.get("thoughtSignature").and_then(Value::as_str).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let mut calls = Vec::new();
    let mut sig_events = Vec::new();
    if let Some(parts) = parts {
        for p in parts {
            let Some(fc) = p.get("functionCall").and_then(Value::as_object) else { continue };
            let name = fc.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
            let args = fc.get("args").cloned().unwrap_or(Value::Object(Map::new()));
            if let Some(sig) = pending_sigs.pop_front() {
                sig_events.push((name.clone(), alphabetize(&args).to_string(), sig));
            }
            calls.push((name, args.to_string()));
        }
    }
    let usage = v.get("usageMetadata").map(|u| Usage {
        prompt_tokens: u.get("promptTokenCount").and_then(Value::as_u64).unwrap_or(0),
        completion_tokens: u.get("candidatesTokenCount").and_then(Value::as_u64).unwrap_or(0),
        total_tokens: u.get("promptTokenCount").and_then(Value::as_u64).unwrap_or(0)
            + u.get("candidatesTokenCount").and_then(Value::as_u64).unwrap_or(0),
    });
    FrameData { text, calls, sig_events, usage }
}

/// 聚合的 functionCall 列表 → OpenAI tool_calls 形态。
fn openai_tool_calls(calls: &[(String, String)]) -> Value {
    Value::Array(
        calls
            .iter()
            .map(|(name, args)| {
                serde_json::json!({
                    "id": format!("call_{}", crate::key::random_id(10)),
                    "type": "function",
                    "function": { "name": name, "arguments": args }
                })
            })
            .collect(),
    )
}

#[async_trait]
impl Provider for GeminiProvider {
    fn id(&self) -> &str {
        "gemini"
    }

    async fn refresh(&self, cred: &Credential) -> Result<Credential, ProviderError> {
        GeminiOAuth::production().refresh(cred).await
    }

    fn catalog(&self) -> ProviderCatalog {
        ProviderCatalog {
            id: "gemini".into(),
            models: vec![
                ModelInfo { id: "gemini-3-pro".into() },
                ModelInfo { id: "gemini-3-flash".into() },
            ],
        }
    }

    async fn complete(&self, cred: &Credential, route: &Route, req: &ChatRequest) -> Result<ChatCompletion, ProviderError> {
        let (status, bytes) = self.send(cred, route, req).await?;
        if status != 200 {
            return Err(map_status_error(status, String::from_utf8_lossy(&bytes).to_string()));
        }
        let mut parser = SseParser::new();
        parser.feed(&bytes);
        parser.finalize();
        let mut text = String::new();
        let mut calls: Vec<(String, String)> = Vec::new();
        let mut usage = Usage::default();
        while let Some(data) = parser.next_data() {
            let Ok(v) = serde_json::from_str::<Value>(&data) else { continue };
            let fd = parse_frame(&v);
            text.push_str(&fd.text);
            calls.extend(fd.calls);
            for (name, canonical, sig) in fd.sig_events {
                self.sigs.record_with_canonical(&name, &canonical, &sig);
            }
            if let Some(u2) = fd.usage {
                usage = u2;
            }
        }
        let mut out = ChatCompletion::new(route.composite(), text, usage);
        if !calls.is_empty() {
            out.choices[0].message.tool_calls = Some(openai_tool_calls(&calls));
            out.choices[0].finish_reason = Some("tool_calls".into());
        }
        Ok(out)
    }

    async fn stream(&self, cred: &Credential, route: &Route, req: &ChatRequest) -> Result<ChunkStream, ProviderError> {
        let (status, bytes) = self.send(cred, route, req).await?;
        if status != 200 {
            return Err(map_status_error(status, String::from_utf8_lossy(&bytes).to_string()));
        }
        let mut parser = SseParser::new();
        parser.feed(&bytes);
        parser.finalize();
        let mut queue: std::collections::VecDeque<Result<StreamChunk, ProviderError>> =
            std::collections::VecDeque::new();
        queue.push_back(Ok(StreamChunk::Role));
        let mut usage = Usage::default();
        let mut saw_tool_call = false;
        while let Some(data) = parser.next_data() {
            let Ok(v) = serde_json::from_str::<Value>(&data) else { continue };
            let fd = parse_frame(&v);
            if !fd.text.is_empty() {
                queue.push_back(Ok(StreamChunk::Content(fd.text)));
            }
            for (index, (name, args)) in fd.calls.into_iter().enumerate() {
                saw_tool_call = true;
                queue.push_back(Ok(StreamChunk::ToolCallDelta {
                    index: index as u64,
                    id: Some(format!("call_{}", crate::key::random_id(10))),
                    name: Some(name),
                    arguments: args,
                }));
            }
            for (name, canonical, sig) in fd.sig_events {
                self.sigs.record_with_canonical(&name, &canonical, &sig);
            }
            if let Some(u2) = fd.usage {
                usage = u2;
            }
        }
        // 上游一次性返回帧流；帧尽即完成
        let reason = if saw_tool_call { "tool_calls" } else { "stop" };
        queue.push_back(Ok(StreamChunk::Finish { reason: reason.into(), usage }));
        Ok(Box::pin(futures::stream::iter(queue)))
    }
}

/// Gemini OAuth（§4.14）：授权 `accounts.google.com/o/oauth2/v2/auth` +
/// token `oauth2.googleapis.com/token`；**refresh_token 会轮换，刷新后必须
/// 立即回写**（响应未带新 rt 时保留旧值）。
///
/// 凭据 secret 形态：JSON `{access_token, refresh_token, expires_at}`；
/// 推理 Authorization 取其中 access_token（裸串回退兼容）。
///
/// ⚠️ 生产默认值待确认（手册未载明）：client 的公开 client_id 具体值（参考
/// 实现经 env `CMDC_PAK_GOOGLE_CLIENT_ID` 可覆盖）与六项 scope 逐字清单
/// （`cloud-platform`+`cclog`+`experimentsandconfigs` 等）。
#[derive(Clone)]
pub struct GeminiOAuth {
    auth_url: String,
    token_url: String,
    client_id: String,
    scopes: Vec<String>,
    client: reqwest::Client,
}

/// 解析凭据 secret：JSON 形态取 (access_token, Some(refresh_token))；
/// 非 JSON 按裸 access_token 回退（历史/手工凭据兼容）。
pub(crate) fn parse_secret(secret: &str) -> (String, Option<String>) {
    if let Ok(v) = serde_json::from_str::<Value>(secret) {
        if let Some(at) = v.get("access_token").and_then(Value::as_str) {
            let rt = v.get("refresh_token").and_then(Value::as_str).map(str::to_string);
            return (at.to_string(), rt);
        }
    }
    (secret.to_string(), None)
}

fn form_enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn now_ts_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl GeminiOAuth {
    pub fn new(auth_url: String, token_url: String, client_id: String, scopes: Vec<String>) -> Self {
        Self { auth_url, token_url, client_id, scopes, client: reqwest::Client::new() }
    }

    /// 生产配置：Google 固定端点；client_id 经 `CMDC_PAK_GOOGLE_CLIENT_ID`
    /// 注入（未配置时 refresh 会被上游以凭据错误拒绝）。
    pub fn production() -> Self {
        Self::new(
            "https://accounts.google.com/o/oauth2/v2/auth".into(),
            "https://oauth2.googleapis.com/token".into(),
            std::env::var("CMDC_PAK_GOOGLE_CLIENT_ID").unwrap_or_default(),
            Vec::new(), // 六项 scope 逐字清单待用户提供（见类型文档）
        )
    }

    /// 授权 URL（response_type=code + access_type=offline 换 refresh_token；
    /// prompt=consent 保证离线授权下发）。
    pub fn login_url(&self, redirect_uri: &str, state: &str) -> String {
        format!(
            "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&state={}&access_type=offline&prompt=consent",
            self.auth_url,
            form_enc(&self.client_id),
            form_enc(redirect_uri),
            form_enc(&self.scopes.join(" ")),
            form_enc(state),
        )
    }

    async fn post_token(&self, form: &[(&str, &str)]) -> Result<Value, ProviderError> {
        let resp = self
            .client
            .post(&self.token_url)
            .form(form)
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if status != 200 {
            return Err(ProviderError::Credential(format!(
                "oauth token endpoint {status}: {}",
                String::from_utf8_lossy(&bytes)
            )));
        }
        serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Upstream(format!("token 响应非 JSON: {e}")))
    }

    fn credential_from(&self, v: &Value, fallback_rt: Option<&str>, account_id: &str) -> Result<Credential, ProviderError> {
        let at = v
            .get("access_token")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Upstream("token 响应缺 access_token".into()))?;
        // 轮换语义：响应带新 refresh_token 立即回写；未带保留旧值
        let rt = v
            .get("refresh_token")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| fallback_rt.map(str::to_string));
        let expires_at = now_ts_secs() + v.get("expires_in").and_then(Value::as_u64).unwrap_or(3600);
        let secret = serde_json::json!({
            "access_token": at,
            "refresh_token": rt,
            "expires_at": expires_at,
        })
        .to_string();
        Ok(Credential { account_id: account_id.into(), secret })
    }

    /// 本地回环回调登录：随机端口起 listener，返回 (授权 URL, 凭据接收端)。
    /// 浏览器完成授权后 Google 重定向到 `http://127.0.0.1:{port}?code=…&state=…`，
    /// 校验 state（防 CSRF）→ 交换 token → oneshot 回传 Credential。
    pub async fn start_login(
        &self,
    ) -> (String, tokio::sync::oneshot::Receiver<Result<Credential, ProviderError>>) {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind loopback callback");
        let port = listener.local_addr().unwrap().port();
        let redirect_uri = format!("http://127.0.0.1:{port}");
        let state = crate::key::random_id(12);
        let url = self.login_url(&redirect_uri, &state);
        let (tx, rx) = tokio::sync::oneshot::channel();
        let tx = std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));
        let this = self.clone();
        let expected_state = state.clone();
        let app = axum::Router::new().route(
            "/",
            axum::routing::get(
                move |axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>| {
                    let tx = tx.clone();
                    let this = this.clone();
                    let expected_state = expected_state.clone();
                    async move {
                        let (code, got_state) = (query.get("code"), query.get("state"));
                        let (Some(code), Some(got_state)) = (code, got_state) else {
                            return (axum::http::StatusCode::BAD_REQUEST, "missing code/state");
                        };
                        if got_state != &expected_state {
                            return (axum::http::StatusCode::FORBIDDEN, "state mismatch");
                        }
                        // redirect_uri 与授权时一致（同端口）
                        let res = this.exchange_code(code, &format!("http://127.0.0.1:{port}")).await;
                        if let Some(tx) = tx.lock().unwrap().take() {
                            let _ = tx.send(res);
                        }
                        (axum::http::StatusCode::OK, "TokenMaster: 登录完成，可关闭此页")
                    }
                },
            ),
        );
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("callback serve");
        });
        (url, rx)
    }

    /// 授权码交换（account_id 由调用方落库时分配，此处沿用传入值）。
    pub async fn exchange_code(&self, code: &str, redirect_uri: &str) -> Result<Credential, ProviderError> {
        let v = self
            .post_token(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("client_id", self.client_id.as_str()),
                ("redirect_uri", redirect_uri),
            ])
            .await?;
        self.credential_from(&v, None, "")
    }

    /// 刷新：refresh_token 轮换立即回写（§4.14）。
    pub async fn refresh(&self, cred: &Credential) -> Result<Credential, ProviderError> {
        let (_, Some(rt)) = parse_secret(&cred.secret) else {
            return Err(ProviderError::Credential(
                "gemini 凭据无 refresh_token（裸串或 JSON 缺字段），需重新登录".into(),
            ));
        };
        let v = self
            .post_token(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", rt.as_str()),
                ("client_id", self.client_id.as_str()),
            ])
            .await?;
        self.credential_from(&v, Some(&rt), &cred.account_id)
    }
}
