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
    session_id: String,
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

impl GeminiProvider {
    pub fn new(base: String, fallback_project: String) -> Self {
        Self {
            base,
            fallback_project,
            session_id: format!("sess-{}", crate::key::random_id(8)),
            client: reqwest::Client::new(),
            resolved_project: std::sync::Mutex::new(None),
            sigs: std::sync::Arc::new(SigStore::in_memory()),
        }
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
        let _ = ins(&format!("Bearer {}", cred.secret)).map(|v| h.insert("authorization", v));
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
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
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
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
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

    fn build_envelope(&self, project: &str, route: &Route, req: &ChatRequest) -> Result<Value, ProviderError> {
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
        request.insert("sessionId".into(), Value::String(self.session_id.clone()));
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

    async fn send(
        &self,
        cred: &Credential,
        route: &Route,
        req: &ChatRequest,
    ) -> Result<reqwest::Response, ProviderError> {
        // 探测失败不发推理（失败 ≠ 探测到空）
        let project = self.resolve_project(cred).await?;
        let body = self.build_envelope(&project, route, req)?;
        let url = format!("{}{}", self.base, GENERATE_PATH);
        let headers = self.identity_headers(cred);
        let mut resp = self
            .client
            .post(&url)
            .headers(headers.clone())
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        // 带签被 400 拒 → 去签重试一次（§4.14 要点②；仅一次，不再循环）
        if resp.status().as_u16() == 400 {
            let stripped = strip_thought_signatures(&body);
            if stripped != body {
                resp = self
                    .client
                    .post(&url)
                    .headers(headers)
                    .json(&stripped)
                    .send()
                    .await
                    .map_err(|e| ProviderError::Upstream(e.to_string()))?;
            }
        }
        Ok(resp)
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
        let resp = self.send(cred, route, req).await?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
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
        let resp = self.send(cred, route, req).await?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
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
