//! commandcode Provider（CLI 私有协议，缝 2）。
//!
//! 上游：`POST {base}/alpha/generate`，响应 200 + NDJSON（每行一个 JSON 事件）。
//! 协议要点（对照 docs/reference/commandcode-proxy.md §4/§7 与 proxy.mjs）：
//! - 私有信封固定键序 `config, memory, taste, skills, permissionMode,
//!   threadId, mode, params`（promptCache 从不生成）；threadId 仅 UUID 形态
//!   注入，否则整键省略（toWireThreadId）；`params.stream` 恒 true（CC API
//!   只有流式，非流式是代理自己缓冲）。
//! - 凭据必须是 `user_[a-zA-Z0-9_-]+` 形状，Bearer 原样透传全部端点。
//! - 无 system 时发 `[{type:'text',text:' '}]` 占位，阻止上游注入 ~7.5K
//!   token 默认提示词（issue #17）。
//! - assistant 内容块次序强制 `[reasoning, text, tool-call]`（thinking 模式
//!   校验 reasoning 随历史带回，丢弃/乱序被上游拒绝）；reasoning 取
//!   reasoning_content 字段优先，客户端放在 content 数组里的 reasoning 块
//!   在无字段时透传。
//! - 流尽无 finish 绝不伪造完成（截断=可重试错误）；finish-step 也是完成
//!   信号（reason/usage 待 finish 合并：显式 finish 优先，双方缺省 stop）；
//!   error 事件状态采纳链 `message <NNN> 前缀 > error.statusCode > 502`。
//! - 零输出（无 text/reasoning/tool-call 且 finish 到达）→ 429 rate_limit。
//! - 重试口径：只有传输层闪断与"截断/upstream-error finishReason"在未吐字
//!   前重试（max 2、退避 400ms×n）；HTTP 状态错/error 事件/超时/零输出
//!   不重试。
//!
//! 设备指纹（fpDigest 派生+哈希层+上报节奏）与初始化上报（成败都写
//! nextInitAt，8h+抖动节流）；`/provider/v1/models` 动态目录失败回退
//! 静态 26 款 MODELS 表。参考实现是增量流式 + 下游 SSE keepalive 注释行
//! （proxy.mjs:1514-1517）；本实现缓冲整条 NDJSON 后一次性产出，下游不存
//! 在"静默期"，keepalive 在此架构下不适用（idle 看门狗以总超时近似）。

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

/// 静态模型回退表（proxy.mjs:457-494 逐字 26 款）。
const MODELS_FALLBACK: [&str; 26] = [
    // Anthropic
    "claude-sonnet-4-6",
    "claude-opus-4-8",
    "claude-opus-4-7",
    "claude-haiku-4-5-20251001",
    // OpenAI
    "gpt-5.5",
    "gpt-5.4",
    "gpt-5.4-mini",
    "gpt-5.3-codex",
    // DeepSeek
    "deepseek/deepseek-v4-pro",
    "deepseek/deepseek-v4-flash",
    // Kimi
    "moonshotai/Kimi-K2.6",
    "moonshotai/Kimi-K2.5",
    // GLM
    "zai-org/GLM-5.1",
    "zai-org/GLM-5",
    // MiniMax
    "MiniMaxAI/MiniMax-M3",
    "MiniMaxAI/MiniMax-M2.7",
    "MiniMaxAI/MiniMax-M2.5",
    // Qwen
    "Qwen/Qwen3.6-Max-Preview",
    "Qwen/Qwen3.6-Plus",
    "Qwen/Qwen3.7-Max",
    // Step
    "stepfun/Step-3.7-Flash",
    "stepfun/Step-3.5-Flash",
    // Xiaomi
    "xiaomi/mimo-v2.5-pro",
    "xiaomi/mimo-v2.5",
    // Gemini
    "google/gemini-3.5-flash",
    "google/gemini-3.1-flash-lite",
];

/// 工具名别名表（proxy.mjs:714-721 逐字，共 4 条）。
/// ⚠️ 只在 **tools 声明** 做别名；消息体 tool-call/tool-result 的 toolName
/// 用原始名（proxy.mjs:612/558-560）。
fn wire_tool_name(n: &str) -> &str {
    match n {
        "bash_output" => "shell_output",
        "task_output" => "shell_output",
        "tool_search" => "search_tools",
        "read_multiple_files" => "read_file",
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

/// finishReason 规范化（§4.4.5；proxy.mjs:930-943）：trim+lowercase，空值→stop，
/// 未知值原样返回，宁可露出不谎报 stop；pause_turn 原样保留。
pub fn map_finish_reason(r: &str) -> String {
    let lower = r.trim().to_lowercase();
    if lower.is_empty() {
        return "stop".into();
    }
    match lower.as_str() {
        "tool-calls" | "tool_calls" | "tool_use" => "tool_calls".into(),
        "length" | "max_tokens" | "max_output_tokens" | "model_context_window_exceeded" => "length".into(),
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
/// 公开供单测（error 事件状态采纳链）。
pub fn status_from_parts_pub(message: &str, status_code: Option<u16>) -> u16 {
    status_from_parts(message, status_code)
}

fn status_from_parts(message: &str, status_code: Option<u16>) -> u16 {
    // 只认 message **开头**的 `<NNN>`（正则 `^<(\d{3})>` 锚定串首、不容忍前导
    // 空白，对齐 CLI readStreamErrorEvent 与 proxy.mjs:1027-1030；扫正文任意
    // 三位数会把普通数字误当状态码）
    if let Some(rest) = message.strip_prefix('<') {
        if let Some(gt) = rest.find('>') {
            if gt == 3 && rest[..gt].bytes().all(|b| b.is_ascii_digit()) {
                if let Ok(n) = rest[..gt].parse::<u16>() {
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

/// 毫秒时钟（toolCallId 兜底 `call_{ms}_{index}`，proxy.mjs:795）。
fn now_ts_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
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

/// mimeType 从 data URL 前缀提取（`^data:([^;,]+)`，proxy.mjs:576-580）；
/// 非 dataURL（http(s) 等）返回 None → 只发 `{type:'image', image: url}` 不带 mimeType。
fn mime_from_data_url(url: &str) -> Option<String> {
    let rest = url.strip_prefix("data:")?;
    let end = rest.find(';').or_else(|| rest.find(','))?;
    if end == 0 {
        return None;
    }
    Some(rest[..end].to_string())
}

/// JSON 侧的 JS 假值（`filter(Boolean)` / `||` 语义）：null/false/0/""。
fn json_falsy(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Bool(b) => !*b,
        Value::Number(n) => n.as_f64() == Some(0.0),
        Value::String(s) => s.is_empty(),
        _ => false,
    }
}

/// JS `String(v ?? '')` 语义：null→""、字符串原样、数字/布尔 → 字面量。
fn json_to_string(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// tool 消息输出值（toWireToolOutputValue，proxy.mjs:724-730）：
/// 字符串原样；数组只取 text 块用 `\n` 拼接；null→""；其他 String 化。
fn tool_output_value(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter(|p| p.get("type").and_then(Value::as_str) == Some("text"))
            .map(|p| p.get("text").and_then(Value::as_str).unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n"),
        other => json_to_string(other),
    }
}

/// 合法 UUID 形态（8-4-4-4-12 hex，大小写不敏感）——threadId 只有此形态才进信封
/// （proxy.mjs:1307-1315 toWireThreadId）。
fn is_uuid_str(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// 预扫 assistant tool_calls 建 id→name 反查表（tool-result 的 toolName 来源，
/// proxy.mjs:553-561：只登记非空 id）。
fn tool_name_map(req: &ChatRequest) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for m in &req.messages {
        if let Some(Value::Array(calls)) = &m.tool_calls {
            for c in calls {
                if let (Some(id), Some(name)) = (
                    c.get("id").and_then(Value::as_str),
                    c.pointer("/function/name").and_then(Value::as_str),
                ) {
                    if !id.is_empty() {
                        map.insert(id.to_string(), name.to_string());
                    }
                }
            }
        }
    }
    map
}

/// OpenAI messages → CC 消息映射（§4.2 消息映射表；proxy.mjs:566-628）。
fn cc_messages(req: &ChatRequest) -> Vec<Value> {
    let names = tool_name_map(req);
    let mut out = Vec::new();
    for m in &req.messages {
        match m.role.as_str() {
            "system" | "developer" => continue,
            "assistant" => {
                // 次序强制 [reasoning, text, tool-call]（§7 坑 6；proxy.mjs:586-617）。
                // reasoning 来源二选一：reasoning_content 字段优先；客户端把
                // reasoning 块直接放 content 数组时透传（有字段则不重复）。
                let has_reasoning_field = m.reasoning_content.as_deref().map(|r| !r.is_empty()).unwrap_or(false);
                let mut blocks: Vec<Value> = Vec::new();
                if has_reasoning_field {
                    blocks.push(serde_json::json!({
                        "type": "reasoning", "text": m.reasoning_content.as_deref().unwrap()
                    }));
                }
                match &m.content {
                    // 非空字符串 → 单 text 块（空串不发块，对齐 if (msg.content)）
                    Value::String(t) if !t.is_empty() => {
                        blocks.push(serde_json::json!({ "type": "text", "text": t }));
                    }
                    // 数组：text 块原样透传；reasoning 块仅在无 reasoning_content
                    // 字段时透传；其余类型（与参考一致）静默丢弃
                    Value::Array(parts) => {
                        for p in parts {
                            if json_falsy(p) {
                                continue;
                            }
                            match p.get("type").and_then(Value::as_str) {
                                Some("text") => blocks.push(p.clone()),
                                Some("reasoning") if !has_reasoning_field => blocks.push(p.clone()),
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
                if let Some(Value::Array(calls)) = &m.tool_calls {
                    for c in calls {
                        let id = c.get("id").and_then(Value::as_str).unwrap_or_default();
                        // 消息体 toolName 用原始名（别名只在 tools 声明做）
                        let name = c
                            .pointer("/function/name")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        // arguments：字符串 → tryParseJSON（失败发 {}）；非字符串
                        // 假值 → {}；对象/数组原样（proxy.mjs:614-616）
                        let input: Value = match c.pointer("/function/arguments") {
                            Some(Value::String(s)) => {
                                serde_json::from_str(s).unwrap_or_else(|_| serde_json::json!({}))
                            }
                            Some(other) if json_falsy(other) => serde_json::json!({}),
                            Some(other) => other.clone(),
                            None => serde_json::json!({}),
                        };
                        let mut tc = Map::new();
                        tc.insert("type".into(), Value::String("tool-call".into()));
                        // toolCallId 缺省时省键（JS undefined 不进 JSON；proxy.mjs:611）
                        if !id.is_empty() {
                            tc.insert("toolCallId".into(), Value::String(id.to_string()));
                        }
                        tc.insert("toolName".into(), Value::String(name.to_string()));
                        tc.insert("input".into(), input);
                        blocks.push(Value::Object(tc));
                    }
                }
                out.push(serde_json::json!({ "role": "assistant", "content": blocks }));
            }
            "tool" => {
                let id = m.tool_call_id.clone().unwrap_or_default();
                let mut tr = Map::new();
                tr.insert("type".into(), Value::String("tool-result".into()));
                tr.insert("toolCallId".into(), Value::String(id));
                // 查不到名字发空串（参考 proxy.mjs:625：toolNameMap[id] || msg.name || ''；
                // 本 crate 的 ChatMessage 不含 name 字段，后者不可达）
                let name = m
                    .tool_call_id
                    .as_deref()
                    .and_then(|i| names.get(i))
                    .cloned()
                    .unwrap_or_default();
                tr.insert("toolName".into(), Value::String(name));
                tr.insert(
                    "output".into(),
                    serde_json::json!({ "type": "text", "value": tool_output_value(&m.content) }),
                );
                out.push(serde_json::json!({ "role": "tool", "content": [Value::Object(tr)] }));
            }
            // user 与未知 role 兜底归一为 user（§4.2；proxy.mjs:583-584）
            _ => {
                let blocks: Vec<Value> = match &m.content {
                    Value::Array(parts) => parts
                        .iter()
                        // .filter(Boolean)：null/false/0/"" 块丢弃（proxy.mjs:582）
                        .filter(|p| !json_falsy(p))
                        .map(|p| {
                            if p.get("type").and_then(Value::as_str) == Some("image_url") {
                                let url = p
                                    .pointer("/image_url/url")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default();
                                let mut b = Map::new();
                                b.insert("type".into(), Value::String("image".into()));
                                b.insert("image".into(), Value::String(url.to_string()));
                                // mimeType 只在 dataURL 形态下发；http(s) 等原样透传不带
                                if let Some(mime) = mime_from_data_url(url) {
                                    b.insert("mimeType".into(), Value::String(mime));
                                }
                                Value::Object(b)
                            } else {
                                p.clone()
                            }
                        })
                        .collect(),
                    other => vec![serde_json::json!({ "type": "text", "text": json_to_string(other) })],
                };
                out.push(serde_json::json!({ "role": "user", "content": blocks }));
            }
        }
    }
    out
}

/// OpenAI tools → CC 线格 `{name, description, input_schema}`（无 type 字段）。
/// 参考支持裸工具形态（无 function 包装时回退顶层 name/description/
/// input_schema），且三键恒下发：缺 name → ''、缺 schema → 空对象骨架；
/// 回退链是 JS `||`（假值继续回退）（proxy.mjs:678-684）。
fn cc_tools(req: &ChatRequest) -> Value {
    let mut decls: Vec<Value> = Vec::new();
    if let Some(Value::Array(tools)) = req.raw.get("tools") {
        for t in tools {
            let f = t.get("function");
            let name = f
                .and_then(|f| f.get("name"))
                .or_else(|| t.get("name"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .unwrap_or_default();
            let desc = f
                .and_then(|f| f.get("description"))
                .or_else(|| t.get("description"))
                .filter(|v| !json_falsy(v))
                .cloned()
                .unwrap_or(Value::String(String::new()));
            let schema = f
                .and_then(|f| f.get("parameters"))
                .or_else(|| t.get("input_schema"))
                .filter(|v| !json_falsy(v))
                .cloned()
                .unwrap_or_else(|| serde_json::json!({ "type": "object", "properties": {} }));
            let mut d = Map::new();
            d.insert("name".into(), Value::String(wire_tool_name(name).into()));
            d.insert("description".into(), desc);
            d.insert("input_schema".into(), schema);
            decls.push(Value::Object(d));
        }
    }
    Value::Array(decls)
}

/// tool_choice 映射（proxy.mjs:697-711）：字符串 → `{type: mapped}`（required→any、
/// 未知→auto）；`{type:'function'}` → `{type:'tool', name}`（name 缺省省键）；
/// 其他对象/标量原样透传。
fn cc_tool_choice(req: &ChatRequest) -> Option<Value> {
    let tc = req.raw.get("tool_choice")?;
    match tc {
        Value::String(s) => {
            let mapped = match s.as_str() {
                "auto" | "none" => s.as_str(),
                "required" => "any",
                _ => "auto",
            };
            Some(serde_json::json!({ "type": mapped }))
        }
        Value::Object(o) => {
            if o.get("type").and_then(Value::as_str) == Some("function") {
                let mut m = Map::new();
                m.insert("type".into(), Value::String("tool".into()));
                if let Some(name) = tc.pointer("/function/name").and_then(Value::as_str) {
                    m.insert("name".into(), Value::String(name.to_string()));
                }
                Some(Value::Object(m))
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
    /// error **事件**（语义错误，有意的信号，不可重试）。
    upstream_error: Option<ProviderError>,
    /// finish/finish-step 携带的 upstream-error 家族 finishReason（连接类
    /// 闪断，与截断同等对待，未吐字前可重试）。
    error_finish: Option<String>,
    /// 是否见过任何完成信号（finish 或 finish-step）。
    saw_finish_signal: bool,
}

fn parse_ndjson(body: &str) -> NdjsonOut {
    let mut out = NdjsonOut {
        text: String::new(),
        reasoning: String::new(),
        tool_calls: Vec::new(),
        finish: None,
        upstream_error: None,
        error_finish: None,
        saw_finish_signal: false,
    };
    // finish-step 的原始 reason/usage（流式参考 proxy.mjs:807-817 同样记变量；
    // finish 到达时按非流式口径"后到覆盖"，仅 finish 缺 reason 时才回退它）
    let mut step_reason_raw: Option<String> = None;
    let mut step_usage: Option<Usage> = None;
    for line in body.lines() {
        let line = line.trim();
        // 空行 / '[DONE]' / ':' 注释行（上游 keepalive）一律跳过（proxy.mjs:757）
        if line.is_empty() || line == "[DONE]" || line.starts_with(':') {
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
                // 缺 toolCallId 兜底 call_{ms}_{index}（proxy.mjs:795）
                let id = v
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("call_{}_{}", now_ts_ms(), out.tool_calls.len()));
                let name = v.get("toolName").and_then(Value::as_str).unwrap_or_default().to_string();
                // input：字符串原样；假值（null/""/0/false）→ "{}"；对象/数组 stringify
                let args = match v.get("input") {
                    Some(Value::String(s)) => s.clone(),
                    Some(other) if json_falsy(other) => "{}".to_string(),
                    Some(other) => other.to_string(),
                    None => "{}".to_string(),
                };
                out.tool_calls.push((id, name, args));
            }
            // finish-step 也是完成信号（proxy.mjs:807-817）：sawFinish 置位，
            // reason/usage 记下待 finish 合并（无 finish 到达时兜底生效）。
            "finish-step" => {
                out.saw_finish_signal = true;
                let reason = v.get("finishReason").and_then(Value::as_str).unwrap_or_default().to_string();
                if !reason.is_empty() {
                    step_reason_raw = Some(reason);
                }
                if let Some(u) = v.get("usage") {
                    step_usage = Some(cc_usage(u));
                }
            }
            "finish" => {
                out.saw_finish_signal = true;
                let fin_raw = v.get("finishReason").and_then(Value::as_str).unwrap_or_default().to_string();
                // 显式 finish 的 reason 优先，缺省回退 finish-step 的，再缺省
                // stop（本缓冲架构对齐参考**非流式**聚合路径 proxy.mjs:1686-1691
                // 的"后到覆盖"；流式翻译器 :819-821 是 step 优先——两种口径参
                // 考里并存，此处取与测试约定一致的一种）
                let merged_raw = if !fin_raw.is_empty() {
                    fin_raw
                } else {
                    step_reason_raw.clone().unwrap_or_else(|| "stop".to_string())
                };
                // totalUsage 缺失时回退 finish-step 的 usage（proxy.mjs:822）
                let usage = v
                    .get("totalUsage")
                    .map(cc_usage)
                    .or(step_usage)
                    .unwrap_or_default();
                if is_upstream_error_reason(&merged_raw) {
                    // provider 报 network/connection/upstream-error → 未完成（可重试 502）
                    out.error_finish = Some(merged_raw);
                } else {
                    out.finish = Some((map_finish_reason(&merged_raw), usage));
                }
            }
            "error" => {
                let e = v.get("error").cloned().unwrap_or(Value::Null);
                // message 采纳链：error.message > 顶层 message > 'Unknown CC error'
                // （proxy.mjs:1019）
                let msg = e
                    .get("message")
                    .and_then(Value::as_str)
                    .or_else(|| v.get("message").and_then(Value::as_str))
                    .unwrap_or("Unknown CC error")
                    .to_string();
                let sc = e.get("statusCode").and_then(Value::as_u64).map(|s| s as u16);
                let status = status_from_parts(&msg, sc);
                // 记录不发 finish（§7 坑 11：避免谎报正常结束）
                out.upstream_error = Some(map_status_error(status, msg));
            }
            _ => {} // 信号事件静默（start/text-start/tool-input-*/provider-metadata…）
        }
    }
    // 只有 finish-step（不带 finishReason）且无显式 finish：也算正常完成，
    // reason 兜底 stop、usage 用 step 的（流式 sawFinish 口径，proxy.mjs:950-956）
    if out.finish.is_none() && out.upstream_error.is_none() && out.error_finish.is_none() && out.saw_finish_signal {
        if let Some(raw) = step_reason_raw {
            if is_upstream_error_reason(&raw) {
                out.error_finish = Some(raw);
            } else {
                out.finish = Some((map_finish_reason(&raw), step_usage.unwrap_or_default()));
            }
        } else {
            out.finish = Some(("stop".to_string(), step_usage.unwrap_or_default()));
        }
    }
    out
}

pub struct CommandcodeProvider {
    base: String,
    client: reqwest::Client,
    /// 伪造的项目目录（config.workingDir / x-project-slug 同源；参考
    /// proxy.mjs:104-108 的 CFG.deviceProjectDir，可按部署配置）。
    project_dir: String,
    /// 上游读超时（秒）。参考 idle 语义（流 30s/非流 90s）在缓冲架构下
    /// 以总超时近似；超时 → 429 rate_limit + retry 5（不可重试）。
    timeout_secs: u64,
    /// 首字节前重试上限（共 retry_max+1 次尝试；退避 400ms×attempt）。
    retry_max: u32,
    /// per-key sessionId（UUID 形态）+ 过期时刻；12h + rand(0..1h)。
    session: std::sync::Mutex<std::collections::HashMap<String, (String, u64)>>,
    /// per-key 上报节奏：account_id → next_init_at（上报后 8h+rand(0..2h)，成败都写）。
    init_state: std::sync::Mutex<std::collections::HashMap<String, u64>>,
}

impl CommandcodeProvider {
    pub fn new(base: String) -> Self {
        Self {
            base,
            client: reqwest::Client::new(),
            project_dir: DEFAULT_PROJECT_DIR.to_string(),
            session: std::sync::Mutex::new(std::collections::HashMap::new()),
            timeout_secs: 90,
            retry_max: 2,
            init_state: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub fn production() -> Self {
        Self::new(DEFAULT_BASE.into())
    }

    /// 覆盖伪造的项目目录（workingDir 与 x-project-slug 随之联动）。
    pub fn with_project_dir(mut self, dir: impl Into<String>) -> Self {
        self.project_dir = dir.into();
        self
    }

    /// 注入读超时与重试上限（测试）。
    pub fn with_timeout(mut self, secs: u64, retry_max: u32) -> Self {
        self.timeout_secs = secs;
        self.retry_max = retry_max;
        self
    }

    fn ensure_session(&self, cred: &Credential, req: &ChatRequest) -> String {
        // 客户端 prompt_cache_key ≥8 字符优先采信（proxy.mjs:363-375）
        if let Some(pk) = req.raw.get("prompt_cache_key").and_then(Value::as_str) {
            if pk.len() >= 8 {
                return pk.to_string();
            }
        }
        let now = now_ts_secs();
        let mut s = self.session.lock().unwrap();
        if let Some((id, exp)) = s.get(&cred.account_id) {
            if now < *exp {
                return id.clone();
            }
        }
        let id = random_uuid();
        let ttl = 12 * 3600 + rand::Rng::random_range(&mut rand::rng(), 0..3600);
        s.insert(cred.account_id.clone(), (id.clone(), now + ttl));
        id
    }

    fn build_cc_request(&self, session_id: &str, route: &Route, req: &ChatRequest) -> Value {
        let mut params = Map::new();
        params.insert("model".into(), Value::String(route.model.clone()));
        let msgs = cc_messages(req);
        params.insert("messages".into(), Value::Array(msgs.clone()));
        // `max_tokens || 64000`：0/缺省都用默认值，再钳 200000（proxy.mjs:668）
        let mt = req
            .raw
            .get("max_tokens")
            .and_then(Value::as_u64)
            .filter(|v| *v > 0)
            .unwrap_or(DEFAULT_MAX_TOKENS)
            .min(MAX_TOKENS_CAP);
        params.insert("max_tokens".into(), Value::from(mt));
        params.insert("stream".into(), Value::Bool(true));
        // system 块数组；无 system 发空格占位（issue #17）。块归一化为
        // {type:'text', text, cache_control?}（text 缺省回退 content 键，空 text
        // 但带 cache_control 的块保留）；客户端全程未在任何块上标断点但有
        // prompt_cache_key 且 system 非空时，把 {type:'ephemeral'} 落在最后一
        // 块（缓存按前缀计算，system 是最前缀；proxy.mjs:530-649）。
        let mut system: Vec<Value> = Vec::new();
        let mut saw_cache_control = false;
        for m in req.messages.iter().filter(|m| m.role == "system" || m.role == "developer") {
            match &m.content {
                Value::Array(blocks) => {
                    for b in blocks {
                        let has_cc = b.get("cache_control").is_some();
                        if has_cc {
                            saw_cache_control = true;
                        }
                        let text = json_to_string(b.get("text").or_else(|| b.get("content")).unwrap_or(&Value::Null));
                        // 空 text 且无 cache_control 的块跳过（proxy.mjs:540）
                        if text.is_empty() && !has_cc {
                            continue;
                        }
                        let mut blk = Map::new();
                        blk.insert("type".into(), Value::String("text".into()));
                        blk.insert("text".into(), Value::String(text));
                        if let Some(cc) = b.get("cache_control") {
                            blk.insert("cache_control".into(), cc.clone());
                        }
                        system.push(Value::Object(blk));
                    }
                }
                Value::String(t) if !t.is_empty() => {
                    system.push(serde_json::json!({ "type": "text", "text": t }));
                }
                Value::Null => {}
                other => {
                    // 非字符串非数组非 null 的 content → String 化（proxy.mjs:547-549）
                    system.push(serde_json::json!({ "type": "text", "text": json_to_string(other) }));
                }
            }
        }
        if system.len() > 1 {
            let last = system.len() - 1;
            for blk in system.iter_mut().take(last) {
                let t = blk.get("text").and_then(Value::as_str).map(str::to_string);
                if let Some(t) = t {
                    if let Some(obj) = blk.as_object_mut() {
                        obj.insert("text".into(), Value::String(format!("{t}\n")));
                    }
                }
            }
        }
        // 断点标记看全量消息块（system + 任意消息的 content 块，proxy.mjs:634-638）
        let has_cache_marker = saw_cache_control
            || msgs.iter().any(|m| {
                m.get("content")
                    .and_then(Value::as_array)
                    .map(|parts| parts.iter().any(|p| p.get("cache_control").is_some()))
                    .unwrap_or(false)
            });
        if system.is_empty() {
            system = vec![serde_json::json!({ "type": "text", "text": " " })];
        } else if !has_cache_marker {
            // prompt_cache_key 非空即触发（proxy.mjs:639-641，无长度门槛）
            if let Some(pk) = req.raw.get("prompt_cache_key").and_then(Value::as_str) {
                if !pk.is_empty() {
                    if let Some(obj) = system.last_mut().and_then(Value::as_object_mut) {
                        obj.insert("cache_control".into(), serde_json::json!({ "type": "ephemeral" }));
                    }
                }
            }
        }
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
                "workingDir": self.project_dir.clone(),
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
        // threadId 需为合法 UUID，否则整键省略（CLI 的 toWireThreadId；
        // proxy.mjs:1307-1315）。x-session-id 头不受此限，仍携带原始 sessionId。
        if is_uuid_str(session_id) {
            root.insert("threadId".into(), Value::String(session_id.to_string()));
        }
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
        let session_id = self.ensure_session(cred, req);
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
        let _ = ins(&slugify(&self.project_dir)).map(|v| h.insert("x-project-slug", v));
        let _ = ins("false").map(|v| h.insert("x-taste-learning", v));
        let _ = ins(&session_id).map(|v| h.insert("x-session-id", v));
        let _ = ins(&format!("Bearer {}", cred.secret)).map(|v| h.insert("authorization", v));
        let _ = ins(&traceparent).map(|v| h.insert("traceparent", v));

        let resp = self
            .client
            .post(format!("{}{}", self.base, GENERATE_PATH))
            .headers(h)
            .timeout(std::time::Duration::from_secs(self.timeout_secs))
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    // 超时 → 429 rate_limit + retry 5（proxy.mjs idle 看门狗语义）
                    ProviderError::RateLimited { retry_after_secs: Some(5), msg: "upstream read timeout".into() }
                } else {
                    ProviderError::Upstream(e.to_string())
                }
            })?;
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
    /// components 键序逐字对照 proxy.mjs:171-189（preserve_order 下键序在
    /// wire 上可观测）。
    async fn report_fingerprint(&self, cred: &Credential) -> Result<(), ProviderError> {
        let profile = derive_device_profile(&cred.secret, "");
        let mut components = Map::new();
        if let Some(h) = fingerprint_hash(&profile.machine_id) {
            components.insert("machineIdHash".into(), Value::String(h));
        }
        let mac_hashes: Vec<Value> = profile
            .macs
            .iter()
            .filter_map(|m| fingerprint_hash(m).map(Value::String))
            .collect();
        components.insert("macHashes".into(), Value::Array(mac_hashes));
        if let Some(h) = fingerprint_hash(&profile.os_user) {
            components.insert("osUserHash".into(), Value::String(h));
        }
        if let Some(h) = fingerprint_hash(&profile.hostname) {
            components.insert("hostnameHash".into(), Value::String(h));
        }
        if let Some(h) = fingerprint_hash(&profile.git_email) {
            components.insert("gitEmailHash".into(), Value::String(h));
        }
        components.insert("platform".into(), Value::String("win32".into()));
        components.insert("arch".into(), Value::String("x64".into()));
        components.insert("osRelease".into(), Value::String("10.0.22631".into()));
        components.insert("cpuModel".into(), Value::String(profile.cpu_model.clone()));
        components.insert("cpuCount".into(), Value::from(profile.cpu_count));
        components.insert("memGiB".into(), Value::from(profile.memory_gb));
        components.insert("isContainer".into(), Value::Bool(false));
        components.insert("timezone".into(), Value::String(profile.timezone.clone()));
        components.insert("runtime".into(), Value::String("cli".into()));
        components.insert("collectorVersion".into(), Value::from(1));
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

    /// 首次请求前并行上报指纹+生命周期；**无论成败都写 nextInitAt**（对齐
    /// proxy.mjs:417-451：单项 !ok/网络错在参考里只 log warn，Promise.all
    /// 照常 resolve，之后无条件 `nextInitAt = now + 8h + rand(0..2h)`——本
    /// crate 无日志框架，失败静默吞掉，8h 后自然重试）。主请求不被阻塞。
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
        let _ = tokio::join!(self.report_fingerprint(cred), self.report_lifecycle(cred));
        // 用上报完成后的时刻起算（参考在 Promise.all 之后取 Date.now()）
        let next = now_ts_secs() + 8 * 3600 + rand::Rng::random_range(&mut rand::rng(), 0..7200);
        self.init_state.lock().unwrap().insert(cred.account_id.clone(), next);
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
    /// 可重试判定对齐参考（proxy.mjs:1546-1551/1752-1759）：只有**传输层闪断**
    /// 与**无 finish 截断 / upstream-error finishReason** 在未吐字前重试；
    /// HTTP 状态错、error 事件（上游有意传下的信号）、超时、零输出都不重试。
    async fn collect(&self, cred: &Credential, route: &Route, req: &ChatRequest) -> Result<NdjsonOut, ProviderError> {
        let mut attempt: u32 = 0;
        loop {
            match self.try_collect(cred, route, req).await {
                Ok(out) => return Ok(out),
                Err((e, retry)) => {
                    if !retry || attempt >= self.retry_max {
                        return Err(e);
                    }
                    // 退避 400ms × 尝试序号（proxy.mjs:259-262）
                    tokio::time::sleep(std::time::Duration::from_millis(400 * u64::from(attempt + 1))).await;
                    attempt += 1;
                }
            }
        }
    }

    async fn try_collect(
        &self,
        cred: &Credential,
        route: &Route,
        req: &ChatRequest,
    ) -> Result<NdjsonOut, (ProviderError, bool)> {
        // 传输层错误（reqwest 网络/连接失败）可重试；读超时（RateLimited）不可
        let (status, bytes) = match self.send(cred, route, req).await {
            Ok(x) => x,
            Err(e) if matches!(e, ProviderError::Upstream(_)) => return Err((e, true)),
            Err(e) => return Err((e, false)),
        };
        if status != 200 {
            // HTTP 状态错是确定性响应，不重试（参考对 !ok 直接下发）
            return Err((map_status_error(status, String::from_utf8_lossy(&bytes).to_string()), false));
        }
        let out = parse_ndjson(&String::from_utf8_lossy(&bytes));
        // error 事件：上游有意传下的语义信号，不重试（proxy.mjs:1528-1531）
        if let Some(err) = out.upstream_error {
            return Err((err, false));
        }
        let Some(_) = out.finish else {
            if let Some(raw) = &out.error_finish {
                // provider 报 network/connection/upstream-error：与截断同等，
                // 未吐字前可重试（proxy.mjs:1752-1759）
                return Err((ProviderError::Upstream(format!("upstream error finish: {raw}")), true));
            }
            // 无 finish = 截断，绝不伪造完成（§7 坑 11），可重试
            return Err((ProviderError::Upstream("no finish event (truncated stream)".into()), true));
        };
        if out.text.is_empty() && out.reasoning.is_empty() && out.tool_calls.is_empty() {
            // 零输出 → 429 rate_limit retry 10（§4.4.7），不重试
            return Err((
                ProviderError::RateLimited {
                    retry_after_secs: Some(10),
                    msg: "zero output from upstream".into(),
                },
                false,
            ));
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
            // MODELS 回退表（proxy.mjs:457-494 逐字，26 款）；动态目录
            // fetch_models 失败时由调用方回退到这里。
            models: MODELS_FALLBACK.iter().map(|id| ModelInfo { id: id.to_string() }).collect(),
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

// 候选池逐字对照 proxy.mjs:69-108。派生 field 名按参考是 cpu/mem
// （label 形如 "{model}|{cores}"）——field 名错了同 key 会派生出不同设备。
const CPU_POOL: [(&str, u32); 15] = [
    ("12th Gen Intel(R) Core(TM) i7-12650H", 10),
    ("12th Gen Intel(R) Core(TM) i5-12400F", 6),
    ("12th Gen Intel(R) Core(TM) i9-12900K", 16),
    ("13th Gen Intel(R) Core(TM) i7-13700K", 16),
    ("13th Gen Intel(R) Core(TM) i5-13600K", 14),
    ("13th Gen Intel(R) Core(TM) i9-13900K", 24),
    ("Intel(R) Core(TM) Ultra 7 155H", 16),
    ("Intel(R) Core(TM) Ultra 9 285H", 16),
    ("Intel(R) Core(TM) i9-14900K", 24),
    ("Intel(R) Core(TM) i7-14700K", 20),
    ("AMD Ryzen 7 7800X3D", 8),
    ("AMD Ryzen 9 7950X", 16),
    ("AMD Ryzen 5 7600", 6),
    ("AMD Ryzen 9 7900X", 12),
    ("AMD Ryzen 7 5800X3D", 8),
];
const MEMORY_POOL: [u32; 6] = [8, 16, 24, 32, 48, 64];
const TIMEZONE_POOL: [&str; 15] = [
    "America/New_York", "America/Chicago", "America/Los_Angeles", "America/Toronto",
    "Europe/London", "Europe/Berlin", "Europe/Paris", "Europe/Moscow",
    "Asia/Shanghai", "Asia/Tokyo", "Asia/Singapore", "Asia/Seoul", "Asia/Hong_Kong",
    "Australia/Sydney", "Pacific/Auckland",
];
const MAC_COUNT_POOL: [u32; 4] = [2, 3, 4, 5];
const OSUSER_POOL: [&str; 6] = ["dev", "user", "admin", "coder", "engineer", "work"];
const MAIL_DOMAIN_POOL: [&str; 4] = ["gmail.com", "outlook.com", "qq.com", "163.com"];

/// 伪造的设备档案：全部由 apiKey 确定性派生（坑 28：换指纹本身是可疑信号）。
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceProfile {
    pub machine_id: String,
    pub macs: Vec<String>,
    pub hostname: String,
    pub cpu_model: String,
    pub cpu_count: u32,
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
    // CPU 池 label 是 "{model}|{cores}"（proxy.mjs:140），field 名 cpu
    let cpu = CPU_POOL
        .iter()
        .map(|(m, c)| (fp_digest(salt, api_key, &format!("cpu\0{m}|{c}")).to_vec(), (*m, *c)))
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, mc)| mc)
        .unwrap_or(CPU_POOL[0]);
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
        cpu_model: cpu.0.to_string(),
        cpu_count: cpu.1,
        memory_gb: pick_u32("mem", &MEMORY_POOL),
        timezone: pick("timezone", &TIMEZONE_POOL),
        git_email: format!("{os_user}.{email_hex}@{domain}"),
        os_user,
    }
}
