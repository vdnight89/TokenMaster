//! commandcode Provider（CLI 私有协议，缝 2）。
//!
//! 上游：`POST {base}/alpha/generate`，响应 200 + NDJSON（每行一个 JSON 事件）。
//! 协议要点（对照 docs/reference/commandcode-proxy.md §4/§7）：
//! - 私有信封固定键序 `config, memory, taste, skills, permissionMode,
//!   threadId, mode, params`（promptCache 从不生成；threadId 仅 UUID 形态注入）；
//!   `params.stream` 恒 true（CC API 只有流式，非流式是代理自己缓冲）。
//! - 凭据必须是 `user_[a-zA-Z0-9_-]+` 形状，Bearer 原样透传全部端点。
//! - 无 system 时发 `[{type:'text',text:' '}]` 占位，阻止上游注入 ~7.5K
//!   token 默认提示词（issue #17）。
//! - assistant 内容块次序强制 `[reasoning, text, tool-call]`（thinking 模式
//!   校验 reasoning 随历史带回，丢弃/乱序被上游拒绝）。
//! - 流尽无 finish 绝不伪造完成（截断=可重试错误）；error 事件状态采纳链
//!   `message <NNN> 前缀 > error.statusCode > 502`。
//! - 零输出（无 text/reasoning/tool-call 且 finish 到达）→ 429 rate_limit。
//!
//! 设备指纹（fpDigest 派生+哈希层+上报节奏）在 T4.3b 接入；
//! `/provider/v1/models` 动态目录在 T4.3c 接入。

use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::openai::{ChatCompletion, ChatRequest, Usage};
use crate::provider::{ChunkStream, Credential, Provider, ProviderError, StreamChunk};
use crate::registry::{ModelInfo, ProviderCatalog};
use crate::route::Route;

pub const DEFAULT_BASE: &str = "https://api.commandcode.ai";
const GENERATE_PATH: &str = "/alpha/generate";
pub const CC_VERSION: &str = "1.53.1";
const DEFAULT_PROJECT_DIR: &str = r"C:\Users\dev\projects\app";
const DEFAULT_MAX_TOKENS: u64 = 64000;
const MAX_TOKENS_CAP: u64 = 200000;

/// 工具名别名表（toWireToolName）。手册 §4.2 只逐字载明
/// `bash_output→shell_output` 一条，其余 3 条原文未载明——拿到原文后补齐。
fn wire_tool_name(n: &str) -> &str {
    match n {
        "bash_output" => "shell_output",
        other => other,
    }
}

/// 凭据形状：`user_[a-zA-Z0-9_-]+`（非此形状上游 401）。
fn is_user_key(s: &str) -> bool {
    let rest = match s.strip_prefix("user_") {
        Some(r) => r,
        None => return false,
    };
    !rest.is_empty()
        && rest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// slugifyProjectPath：lowercase → 非字母数字折叠 `-` → 去首尾，空则 `root`。
pub fn slugify(path: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in path.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() { "root".into() } else { trimmed }
}

/// finishReason 规范化（§4.4.5；未知值原样返回，宁可露出不谎报 stop）。
pub fn map_finish_reason(r: &str) -> String {
    match r {
        "tool-calls" | "tool_calls" | "tool_use" => "tool_calls".into(),
        "length" | "max_tokens" | "max_output_tokens" | "model_context_window_exceeded" => "length".into(),
        // OpenAI 无对应枚举；折 stop 是谎报完成（§7 坑 9）
        "pause_turn" => "length".into(),
        other => other.into(),
    }
}

/// `network-error` / `connection_error` / `upstream-error` 家族（正则
/// `^(?:network|connection|upstream)[-_\s]?error$`）→ 可重试上游错误。
fn is_upstream_error_reason(r: &str) -> bool {
    let lower = r.to_lowercase();
    for p in ["network", "connection", "upstream"] {
        let Some(rest) = lower.strip_prefix(p) else { continue };
        let rest = rest.trim_start_matches(['-', '_', ' ', '\t']);
        if rest == "error" {
            return true;
        }
    }
    false
}

/// CC_STATUS_MAP（§4.4.6）：402 付费失败按限流；429 带 retry 30。
pub fn map_status_error(status: u16, msg: String) -> ProviderError {
    match status {
        400 | 404 | 422 => ProviderError::BadRequest(format!("http {status}: {msg}")),
        401 | 403 => ProviderError::Credential(format!("http {status}: {msg}")),
        402 => ProviderError::RateLimited { retry_after_secs: None, msg: format!("payment required: {msg}") },
        429 => ProviderError::RateLimited { retry_after_secs: Some(30), msg },
        code => ProviderError::Upstream(format!("http {code}: {msg}")),
    }
}

/// error 事件状态采纳链（§7 坑 12）：message 里的三位状态码 > error.statusCode > 502。
fn status_from_parts(message: &str, status_code: Option<u16>) -> u16 {
    for tok in message.split(|c: char| !c.is_ascii_digit()) {
        if tok.len() == 3 {
            if let Ok(n) = tok.parse::<u16>() {
                if (100..=599).contains(&n) {
                    return n;
                }
            }
        }
    }
    status_code.unwrap_or(502)
}

/// UTC 日期 YYYY-MM-DD（信封 config.date；Howard Hinnant civil 算法）。
fn today_str() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86400) as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

fn now_ts_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn random_uuid() -> String {
    let hex = crate::key::random_id(16);
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

/// CC usage → 统一口径；outputTokens 为 0 时 input 一并归零（反误计费）。
fn cc_usage(v: &Value) -> Usage {
    let input = v.get("inputTokens").and_then(Value::as_u64).unwrap_or(0);
    let output = v.get("outputTokens").and_then(Value::as_u64).unwrap_or(0);
    if output == 0 {
        return Usage::default();
    }
    Usage::sum(input, output)
}

/// mimeType 从 data URL 前缀提取（`data:<mime>;base64,...`）。
fn mime_from_data_url(url: &str) -> Option<String> {
    let rest = url.strip_prefix("data:")?;
    let end = rest.find(';').or_else(|| rest.find(','))?;
    if end == 0 {
        return None;
    }
    Some(rest[..end].to_string())
}

/// 预扫 assistant tool_calls 建 id→name 反查表（tool-result 的 toolName 来源）。
fn tool_name_map(req: &ChatRequest) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for m in &req.messages {
        if let Some(Value::Array(calls)) = &m.tool_calls {
            for c in calls {
                if let (Some(id), Some(name)) = (
                    c.get("id").and_then(Value::as_str),
                    c.pointer("/function/name").and_then(Value::as_str),
                ) {
                    map.insert(id.to_string(), wire_tool_name(name).to_string());
                }
            }
        }
    }
    map
}

/// OpenAI messages → CC 消息映射（§4.2 消息映射表）。
fn cc_messages(req: &ChatRequest) -> Vec<Value> {
    let names = tool_name_map(req);
    let mut out = Vec::new();
    for m in &req.messages {
        match m.role.as_str() {
            "system" | "developer" => continue,
            "assistant" => {
                // 次序强制 [reasoning, text, tool-call]（§7 坑 6）
                let mut blocks: Vec<Value> = Vec::new();
                if let Some(r) = &m.reasoning_content {
                    if !r.is_empty() {
                        blocks.push(serde_json::json!({ "type": "reasoning", "text": r }));
                    }
                }
                let t = m.text();
                if !t.is_empty() {
                    blocks.push(serde_json::json!({ "type": "text", "text": t }));
                }
                if let Some(Value::Array(calls)) = &m.tool_calls {
                    for c in calls {
                        let id = c.get("id").and_then(Value::as_str).unwrap_or_default();
                        let name = c
                            .pointer("/function/name")
                            .and_then(Value::as_str)
                            .map(wire_tool_name)
                            .unwrap_or_default();
                        let args_raw = c
                            .pointer("/function/arguments")
                            .and_then(Value::as_str)
                            .unwrap_or("{}");
                        let input: Value = serde_json::from_str(args_raw)
                            .unwrap_or(Value::String(args_raw.to_string()));
                        blocks.push(serde_json::json!({
                            "type": "tool-call", "toolCallId": id, "toolName": name, "input": input
                        }));
                    }
                }
                out.push(serde_json::json!({ "role": "assistant", "content": blocks }));
            }
            "tool" => {
                let id = m.tool_call_id.clone().unwrap_or_default();
                let mut tr = Map::new();
                tr.insert("type".into(), Value::String("tool-result".into()));
                tr.insert("toolCallId".into(), Value::String(id));
                // 查不到名字时省略 name 键（§7 坑 7：硬塞空名上游报错）
                if let Some(n) = m.tool_call_id.as_deref().and_then(|i| names.get(i)) {
                    tr.insert("toolName".into(), Value::String(n.clone()));
                }
                tr.insert(
                    "output".into(),
                    serde_json::json!({ "type": "text", "value": m.text() }),
                );
                out.push(serde_json::json!({ "role": "tool", "content": [Value::Object(tr)] }));
            }
            // user 与未知 role 兜底归一为 user（§4.2）
            _ => {
                let blocks: Vec<Value> = match &m.content {
                    Value::Array(parts) => parts
                        .iter()
                        .map(|p| match p.get("type").and_then(Value::as_str) {
                            Some("image_url") => {
                                let url = p
                                    .pointer("/image_url/url")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default();
                                let mut b = Map::new();
                                b.insert("type".into(), Value::String("image".into()));
                                b.insert("image".into(), Value::String(url.to_string()));
                                if let Some(mime) = mime_from_data_url(url) {
                                    b.insert("mimeType".into(), Value::String(mime));
                                }
                                Value::Object(b)
                            }
                            _ => p.clone(),
                        })
                        .collect(),
                    other => vec![serde_json::json!({ "type": "text", "text": other.as_str().unwrap_or_default() })],
                };
                out.push(serde_json::json!({ "role": "user", "content": blocks }));
            }
        }
    }
    out
}

/// OpenAI tools → CC 线格 `{name, description, input_schema}`（无 type 字段）。
fn cc_tools(req: &ChatRequest) -> Value {
    let mut decls: Vec<Value> = Vec::new();
    if let Some(Value::Array(tools)) = req.raw.get("tools") {
        for t in tools {
            let Some(f) = t.get("function") else { continue };
            let mut d = Map::new();
            if let Some(n) = f.get("name").and_then(Value::as_str) {
                d.insert("name".into(), Value::String(wire_tool_name(n).into()));
            }
            if let Some(desc) = f.get("description") {
                d.insert("description".into(), desc.clone());
            }
            if let Some(p) = f.get("parameters") {
                d.insert("input_schema".into(), p.clone());
            }
            decls.push(Value::Object(d));
        }
    }
    Value::Array(decls)
}

/// tool_choice 映射：required→any；{type:'function'}→{type:'tool',name}。
fn cc_tool_choice(req: &ChatRequest) -> Option<Value> {
    let tc = req.raw.get("tool_choice")?;
    match tc {
        Value::String(s) => Some(Value::String(match s.as_str() {
            "required" => "any".into(),
            other => other.into(),
        })),
        Value::Object(o) => {
            if o.get("type").and_then(Value::as_str) == Some("function") {
                let name = tc.pointer("/function/name").cloned().unwrap_or(Value::Null);
                Some(serde_json::json!({ "type": "tool", "name": name }))
            } else {
                Some(tc.clone())
            }
        }
        _ => Some(tc.clone()),
    }
}

/// NDJSON 聚合结果。
struct NdjsonOut {
    text: String,
    reasoning: String,
    /// (toolCallId, toolName, args JSON 串)
    tool_calls: Vec<(String, String, String)>,
    finish: Option<(String, Usage)>,
    upstream_error: Option<ProviderError>,
}

fn parse_ndjson(body: &str) -> NdjsonOut {
    let mut out = NdjsonOut {
        text: String::new(),
        reasoning: String::new(),
        tool_calls: Vec::new(),
        finish: None,
        upstream_error: None,
    };
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        match v.get("type").and_then(Value::as_str).unwrap_or_default() {
            "text-delta" => {
                let t = v.get("text").or_else(|| v.get("delta")).and_then(Value::as_str).unwrap_or_default();
                out.text.push_str(t);
            }
            "reasoning-delta" => {
                let t = v.get("text").and_then(Value::as_str).unwrap_or_default();
                out.reasoning.push_str(t);
            }
            "tool-call" => {
                let id = v.get("toolCallId").and_then(Value::as_str).unwrap_or_default().to_string();
                let name = v.get("toolName").and_then(Value::as_str).unwrap_or_default().to_string();
                let args = match v.get("input") {
                    Some(Value::String(s)) => s.clone(),
                    Some(other) => other.to_string(),
                    None => "{}".to_string(),
                };
                out.tool_calls.push((id, name, args));
            }
            "finish" => {
                let reason = v.get("finishReason").and_then(Value::as_str).unwrap_or_default();
                let usage = v.get("totalUsage").map(cc_usage).unwrap_or_default();
                if is_upstream_error_reason(reason) {
                    out.upstream_error = Some(ProviderError::Upstream(format!("upstream error finish: {reason}")));
                } else {
                    out.finish = Some((map_finish_reason(reason), usage));
                }
            }
            "error" => {
                let e = v.get("error").cloned().unwrap_or(Value::Null);
                let msg = e.get("message").and_then(Value::as_str).unwrap_or_default().to_string();
                let sc = e.get("statusCode").and_then(Value::as_u64).map(|s| s as u16);
                let status = status_from_parts(&msg, sc);
                // 记录不发 finish（§7 坑 11：避免谎报正常结束）
                out.upstream_error = Some(map_status_error(status, msg));
            }
            _ => {} // 信号事件静默（start/text-start/tool-input-*/provider-metadata…）
        }
    }
    out
}

pub struct CommandcodeProvider {
    base: String,
    client: reqwest::Client,
    /// sessionId（UUID 形态）+ 过期时刻；12h + rand(0..1h)。
    session: std::sync::Mutex<Option<(String, u64)>>,
    /// per-key 上报节奏：account_id → next_init_at（成功后 8h+rand(0..2h)）。
    init_state: std::sync::Mutex<std::collections::HashMap<String, u64>>,
}

impl CommandcodeProvider {
    pub fn new(base: String) -> Self {
        Self {
            base,
            client: reqwest::Client::new(),
            session: std::sync::Mutex::new(None),
            init_state: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub fn production() -> Self {
        Self::new(DEFAULT_BASE.into())
    }

    fn ensure_session(&self) -> String {
        let now = now_ts_secs();
        let mut s = self.session.lock().unwrap();
        if let Some((id, exp)) = s.as_ref() {
            if now < *exp {
                return id.clone();
            }
        }
        let id = random_uuid();
        let ttl = 12 * 3600 + rand::Rng::random_range(&mut rand::rng(), 0..3600);
        *s = Some((id.clone(), now + ttl));
        id
    }

    fn build_cc_request(&self, session_id: &str, route: &Route, req: &ChatRequest) -> Value {
        let mut params = Map::new();
        params.insert("model".into(), Value::String(route.model.clone()));
        params.insert("messages".into(), Value::Array(cc_messages(req)));
        let mt = req
            .raw
            .get("max_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_MAX_TOKENS)
            .min(MAX_TOKENS_CAP);
        params.insert("max_tokens".into(), Value::from(mt));
        params.insert("stream".into(), Value::Bool(true));
        // system 块数组；无 system 发空格占位（issue #17：阻止上游注入默认提示词）
        let sys_texts: Vec<String> = req
            .messages
            .iter()
            .filter(|m| m.role == "system" || m.role == "developer")
            .map(|m| m.text())
            .filter(|t| !t.is_empty())
            .collect();
        let system: Vec<Value> = if sys_texts.is_empty() {
            vec![serde_json::json!({ "type": "text", "text": " " })]
        } else {
            let last = sys_texts.len() - 1;
            sys_texts
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let text = if i < last { format!("{t}\n") } else { t.clone() };
                    serde_json::json!({ "type": "text", "text": text })
                })
                .collect()
        };
        params.insert("system".into(), Value::Array(system));
        if let Some(t) = req.raw.get("temperature") {
            params.insert("temperature".into(), t.clone());
        }
        if let Some(e) = req.raw.get("reasoning_effort") {
            params.insert("reasoning_effort".into(), e.clone());
        }
        // tools 恒下发（空数组与缺键在 wire 上可观测，§7 坑 5）
        params.insert("tools".into(), cc_tools(req));
        if let Some(tc) = cc_tool_choice(req) {
            params.insert("tool_choice".into(), tc);
        }
        if let Some(p) = req.raw.get("parallel_tool_calls") {
            params.insert("parallel_tool_calls".into(), p.clone());
        }

        let mut root = Map::new();
        root.insert(
            "config".into(),
            serde_json::json!({
                "workingDir": DEFAULT_PROJECT_DIR,
                "date": today_str(),
                "environment": "win32",
                "structure": [],
                "isGitRepo": false,
                "currentBranch": "",
                "mainBranch": "",
                "gitStatus": "",
                "recentCommits": []
            }),
        );
        root.insert("memory".into(), Value::Null);
        root.insert("taste".into(), Value::Null);
        root.insert("skills".into(), Value::Null);
        root.insert("permissionMode".into(), Value::String("standard".into()));
        root.insert("threadId".into(), Value::String(session_id.to_string()));
        root.insert("mode".into(), Value::String("agent".into()));
        root.insert("params".into(), Value::Object(params));
        Value::Object(root)
    }

    async fn send(
        &self,
        cred: &Credential,
        route: &Route,
        req: &ChatRequest,
    ) -> Result<(u16, Vec<u8>), ProviderError> {
        if !is_user_key(&cred.secret) {
            return Err(ProviderError::Credential(format!(
                "commandcode 凭据须为 user_ 形状：{}…",
                cred.secret.chars().take(6).collect::<String>()
            )));
        }
        self.ensure_initialized(cred).await;
        let session_id = self.ensure_session();
        let body = self.build_cc_request(&session_id, route, req);
        let traceparent = format!(
            "00-{}-{}-01",
            crate::key::random_id(16),
            crate::key::random_id(8)
        );
        let mut h = reqwest::header::HeaderMap::new();
        let ins = reqwest::header::HeaderValue::from_str;
        let _ = ins("cli").map(|v| h.insert("user-agent", v));
        let _ = ins(CC_VERSION).map(|v| h.insert("x-command-code-version", v));
        let _ = ins("production").map(|v| h.insert("x-cli-environment", v));
        let _ = ins(&slugify(DEFAULT_PROJECT_DIR)).map(|v| h.insert("x-project-slug", v));
        let _ = ins("false").map(|v| h.insert("x-taste-learning", v));
        let _ = ins(&session_id).map(|v| h.insert("x-session-id", v));
        let _ = ins(&format!("Bearer {}", cred.secret)).map(|v| h.insert("authorization", v));
        let _ = ins(&traceparent).map(|v| h.insert("traceparent", v));

        let resp = self
            .client
            .post(format!("{}{}", self.base, GENERATE_PATH))
            .headers(h)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?
            .to_vec();
        Ok((status, bytes))
    }

    /// 初始化上报头（generate 头的子集：无 UA/slug/session/traceparent）。
    fn init_headers(cred: &Credential) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        let ins = reqwest::header::HeaderValue::from_str;
        let _ = ins("production").map(|v| h.insert("x-cli-environment", v));
        let _ = ins(CC_VERSION).map(|v| h.insert("x-command-code-version", v));
        let _ = ins(&format!("Bearer {}", cred.secret)).map(|v| h.insert("authorization", v));
        h
    }

    /// 设备指纹上报：thumbmark + components（逐信号哈希，空值省略键）。
    async fn report_fingerprint(&self, cred: &Credential) -> Result<(), ProviderError> {
        let profile = derive_device_profile(&cred.secret, "");
        let mut components = Map::new();
        for (k, v) in [
            ("machineId", Some(profile.machine_id.clone())),
            ("macs", Some(profile.macs.join(","))),
            ("hostname", Some(profile.hostname.clone())),
            ("cpuModel", Some(profile.cpu_model.clone())),
            ("memoryGb", Some(profile.memory_gb.to_string())),
            ("timezone", Some(profile.timezone.clone())),
            ("osUser", Some(profile.os_user.clone())),
            ("gitEmail", Some(profile.git_email.clone())),
        ] {
            if let Some(hv) = v.as_deref().and_then(fingerprint_hash) {
                components.insert(k.into(), Value::String(hv));
            }
        }
        let body = serde_json::json!({
            "thumbmark": thumbmark(&profile.machine_id, &profile.macs, &profile.hostname, &profile.cpu_model),
            "components": Value::Object(components),
        });
        let resp = self
            .client
            .post(format!("{}{}", self.base, "/alpha/fingerprint/record"))
            .headers(Self::init_headers(cred))
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(ProviderError::Upstream(format!("fingerprint record {}", resp.status())));
        }
        Ok(())
    }

    /// 生命周期上报：cli_session_exists（metadata 的 mode 是 cliSessionMode
    /// 枚举 interactive/non-interactive，与信封顶层 mode 不同——坑 2）。
    async fn report_lifecycle(&self, cred: &Credential) -> Result<(), ProviderError> {
        let body = serde_json::json!({
            "eventType": "cli_session_exists",
            "metadata": {
                "sessionId": format!("sess_{}", crate::key::random_id(8)),
                "cliVersion": CC_VERSION,
                "mode": "interactive",
                "os": "win32-x64",
            }
        });
        let resp = self
            .client
            .post(format!("{}{}", self.base, "/alpha/lifecycle-events"))
            .headers(Self::init_headers(cred))
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(ProviderError::Upstream(format!("lifecycle events {}", resp.status())));
        }
        Ok(())
    }

    /// 首次请求前并行上报指纹+生命周期；各自失败仅告警不阻塞主请求，
    /// 成功后 8h + rand(0..2h) 内不再上报（失败下次重试）。
    async fn ensure_initialized(&self, cred: &Credential) {
        let now = now_ts_secs();
        {
            let m = self.init_state.lock().unwrap();
            if let Some(next) = m.get(&cred.account_id) {
                if now < *next {
                    return;
                }
            }
        }
        let (a, b) = tokio::join!(self.report_fingerprint(cred), self.report_lifecycle(cred));
        if a.is_ok() && b.is_ok() {
            let next = now + 8 * 3600 + rand::Rng::random_range(&mut rand::rng(), 0..7200);
            self.init_state.lock().unwrap().insert(cred.account_id.clone(), next);
        }
    }

    /// `/provider/v1/models` 动态目录（§4.5）：头为初始化子集（不带 zdr），
    /// 10s 超时；`data.data[].id` 映射为目录项。失败显式报错，
    /// 调用方回退 `catalog()` 静态表（硬编码清单手册未载明，仅缺省模型）。
    pub async fn fetch_models(&self, cred: &Credential) -> Result<Vec<ModelInfo>, ProviderError> {
        let resp = self
            .client
            .get(format!("{}{}", self.base, "/provider/v1/models"))
            .headers(Self::init_headers(cred))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?;
        if status != 200 {
            return Err(ProviderError::Upstream(format!("models {status}")));
        }
        let v: Value = serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Upstream(format!("models 响应非 JSON: {e}")))?;
        let models = v
            .pointer("/data/data")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.get("id").and_then(Value::as_str))
                    .map(|id| ModelInfo { id: id.to_string() })
                    .collect()
            })
            .unwrap_or_default();
        Ok(models)
    }

    /// NDJSON 聚合 + 终态校验（零输出/无 finish），complete 与 stream 共用。
    async fn collect(&self, cred: &Credential, route: &Route, req: &ChatRequest) -> Result<NdjsonOut, ProviderError> {
        let (status, bytes) = self.send(cred, route, req).await?;
        if status != 200 {
            return Err(map_status_error(status, String::from_utf8_lossy(&bytes).to_string()));
        }
        let out = parse_ndjson(&String::from_utf8_lossy(&bytes));
        if let Some(err) = out.upstream_error {
            return Err(err);
        }
        let Some(_) = out.finish else {
            // 无 finish = 截断，绝不伪造完成（§7 坑 11）
            return Err(ProviderError::Upstream("no finish event (truncated stream)".into()));
        };
        if out.text.is_empty() && out.reasoning.is_empty() && out.tool_calls.is_empty() {
            // 零输出 → 429 rate_limit retry 10（§4.4.7）
            return Err(ProviderError::RateLimited {
                retry_after_secs: Some(10),
                msg: "zero output from upstream".into(),
            });
        }
        Ok(out)
    }
}

fn openai_tool_calls(calls: &[(String, String, String)]) -> Value {
    Value::Array(
        calls
            .iter()
            .map(|(id, name, args)| {
                serde_json::json!({
                    "id": id, "type": "function",
                    "function": { "name": name, "arguments": args }
                })
            })
            .collect(),
    )
}

#[async_trait]
impl Provider for CommandcodeProvider {
    fn id(&self) -> &str {
        "commandcode"
    }

    fn catalog(&self) -> ProviderCatalog {
        ProviderCatalog {
            id: "commandcode".into(),
            models: vec![
                ModelInfo { id: "deepseek/deepseek-v4-flash".into() },
            ],
        }
    }

    async fn complete(&self, cred: &Credential, route: &Route, req: &ChatRequest) -> Result<ChatCompletion, ProviderError> {
        let out = self.collect(cred, route, req).await?;
        let (reason, usage) = out.finish.expect("collect 已校验 finish 存在");
        let mut completion = ChatCompletion::new(route.composite(), out.text, usage);
        if !out.tool_calls.is_empty() {
            completion.choices[0].message.tool_calls = Some(openai_tool_calls(&out.tool_calls));
            completion.choices[0].finish_reason = Some(reason);
        } else if reason != "stop" {
            completion.choices[0].finish_reason = Some(reason);
        }
        Ok(completion)
    }

    async fn stream(&self, cred: &Credential, route: &Route, req: &ChatRequest) -> Result<ChunkStream, ProviderError> {
        let out = self.collect(cred, route, req).await?;
        let (reason, usage) = out.finish.expect("collect 已校验 finish 存在");
        let mut queue: std::collections::VecDeque<Result<StreamChunk, ProviderError>> =
            std::collections::VecDeque::new();
        queue.push_back(Ok(StreamChunk::Role));
        if !out.reasoning.is_empty() {
            queue.push_back(Ok(StreamChunk::Reasoning(out.reasoning)));
        }
        if !out.text.is_empty() {
            queue.push_back(Ok(StreamChunk::Content(out.text)));
        }
        for (index, (id, name, args)) in out.tool_calls.iter().enumerate() {
            queue.push_back(Ok(StreamChunk::ToolCallDelta {
                index: index as u64,
                id: Some(id.clone()),
                name: Some(name.clone()),
                arguments: args.clone(),
            }));
        }
        queue.push_back(Ok(StreamChunk::Finish { reason, usage }));
        Ok(Box::pin(futures::stream::iter(queue)))
    }
}

// ───────────────────────── 设备指纹（§4.3，T4.3b） ─────────────────────────

pub const FP_SALT: &str = "command-code:device-fingerprint:v1";

fn sha256_hex(parts: &[&str]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
    }
    format!("{:x}", h.finalize())
}

/// 哈希层（CLI 逐字对齐）：`sha256_hex(FP_SALT ‖ "\0" ‖ lower(trim(v)))`；
/// 空值返回 None（JSON 序列化时省略键）。
pub fn fingerprint_hash(v: &str) -> Option<String> {
    let t = v.trim().to_lowercase();
    if t.is_empty() {
        return None;
    }
    Some(sha256_hex(&[FP_SALT, "\0", &t]))
}

/// thumbmark：`sha256_hex(FP_SALT ‖ "\0machine\0" ‖ join('|', parts))`；
/// parts = [machineId, macs.join(','), (machineId 空才带) hostname/cpu]，
/// 空 parts 以 'unknown' 兜底。
pub fn thumbmark(machine_id: &str, macs: &[String], hostname: &str, cpu_model: &str) -> String {
    let mut parts: Vec<String> = vec![machine_id.to_string()];
    if !macs.is_empty() {
        parts.push(macs.join(","));
    }
    if machine_id.is_empty() {
        parts.push(hostname.to_string());
        parts.push(cpu_model.to_string());
    }
    parts.retain(|p| !p.is_empty());
    let joined = if parts.is_empty() { "unknown".to_string() } else { parts.join("|") };
    sha256_hex(&[FP_SALT, "\0machine\0", &joined])
}

/// 派生层 digest：`sha256(salt ‖ "\0" ‖ apiKey ‖ "\0" ‖ field)`。
/// salt 只影响"伪造出哪台机器"，不影响哈希层（坑 28）。
fn fp_digest(salt: &str, api_key: &str, field: &str) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(salt.as_bytes());
    h.update([0u8]);
    h.update(api_key.as_bytes());
    h.update([0u8]);
    h.update(field.as_bytes());
    h.finalize().to_vec()
}

// 候选池：内存与 MAC 数逐字来自手册；CPU/时区/用户名/邮箱域手册只给数量与
// 特征（源码行 69–108），具体值未逐字载明——先放特征相符的占位集，
// 拿到原文后逐字替换（选择机制与确定性不受池内容影响）。
const CPU_POOL: [&str; 15] = [
    "12th Gen Intel(R) Core(TM) i5-12400 (12 cores)",
    "12th Gen Intel(R) Core(TM) i7-12700 (20 cores)",
    "12th Gen Intel(R) Core(TM) i9-12900K (24 cores)",
    "13th Gen Intel(R) Core(TM) i5-13400 (16 cores)",
    "13th Gen Intel(R) Core(TM) i7-13700 (24 cores)",
    "13th Gen Intel(R) Core(TM) i9-13900K (32 cores)",
    "Intel(R) Core(TM) Ultra 5 125H (18 cores)",
    "Intel(R) Core(TM) Ultra 7 155H (22 cores)",
    "Intel(R) Core(TM) Ultra 9 185H (24 cores)",
    "AMD Ryzen 5 5600X (12 cores)",
    "AMD Ryzen 5 7600X (12 cores)",
    "AMD Ryzen 7 5800X (16 cores)",
    "AMD Ryzen 7 7700X (16 cores)",
    "AMD Ryzen 9 5900X (24 cores)",
    "AMD Ryzen 9 7950X (32 cores)",
];
const MEMORY_POOL: [u32; 6] = [8, 16, 24, 32, 48, 64];
const TIMEZONE_POOL: [&str; 15] = [
    "Asia/Shanghai", "Asia/Tokyo", "Asia/Singapore", "Asia/Hong_Kong", "Asia/Seoul",
    "Europe/London", "Europe/Berlin", "Europe/Paris", "Europe/Moscow",
    "America/New_York", "America/Chicago", "America/Los_Angeles", "America/Sao_Paulo",
    "Australia/Sydney", "UTC",
];
const MAC_COUNT_POOL: [u32; 4] = [2, 3, 4, 5];
const OSUSER_POOL: [&str; 6] = ["dev", "alex", "sam", "jordan", "taylor", "casey"];
const MAIL_DOMAIN_POOL: [&str; 4] = ["gmail.com", "outlook.com", "yahoo.com", "proton.me"];

/// 伪造的设备档案：全部由 apiKey 确定性派生（坑 28：换指纹本身是可疑信号）。
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceProfile {
    pub machine_id: String,
    pub macs: Vec<String>,
    pub hostname: String,
    pub cpu_model: String,
    pub memory_gb: u32,
    pub timezone: String,
    pub os_user: String,
    pub git_email: String,
}

fn hex_uuid(bytes: &[u8]) -> String {
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

/// 派生一台"机器"。候选池选择按 `fpDigest(apiKey, field‖"\0"‖label)` 的
/// digest **字节序取最大**（往池里加候选只影响恰好胜出的 key，不会全体换设备）。
pub fn derive_device_profile(api_key: &str, salt: &str) -> DeviceProfile {
    let pick = |field: &str, pool: &[&str]| -> String {
        pool.iter()
            .map(|c| (fp_digest(salt, api_key, &format!("{field}\0{c}")).to_vec(), *c))
            .max_by(|a, b| a.0.cmp(&b.0))
            .map(|(_, c)| c.to_string())
            .unwrap_or_default()
    };
    let pick_u32 = |field: &str, pool: &[u32]| -> u32 {
        pool.iter()
            .map(|c| (fp_digest(salt, api_key, &format!("{field}\0{c}")).to_vec(), *c))
            .max_by(|a, b| a.0.cmp(&b.0))
            .map(|(_, c)| c)
            .unwrap_or(pool[0])
    };
    let machine_id = hex_uuid(&fp_digest(salt, api_key, "machineId")[..16]);
    let mac_count = pick_u32("macCount", &MAC_COUNT_POOL) as usize;
    let mut macs: Vec<String> = (0..mac_count)
        .map(|i| {
            fp_digest(salt, api_key, &format!("mac{i}"))[..6]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join(":")
        })
        .collect();
    macs.sort();
    macs.dedup();
    let hostname = format!(
        "DESKTOP-{}",
        fp_digest(salt, api_key, "hostname")[..4]
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<String>()
    );
    let os_user = pick("osUser", &OSUSER_POOL);
    let domain = pick("mailDomain", &MAIL_DOMAIN_POOL);
    let email_hex: String = fp_digest(salt, api_key, "gitEmail")[..3]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    DeviceProfile {
        machine_id,
        macs,
        hostname,
        cpu_model: pick("cpuModel", &CPU_POOL),
        memory_gb: pick_u32("memoryGb", &MEMORY_POOL),
        timezone: pick("timezone", &TIMEZONE_POOL),
        git_email: format!("{os_user}.{email_hex}@{domain}"),
        os_user,
    }
}
