//! gemini Provider（Google Cloud Code Assist 免费线，Antigravity 客户端身份伪装）。
//!
//! 上游：`POST {base}/v1internal:streamGenerateContent?alt=sse`（SSE 帧）。
//! 协议要点（对照 deepseek-harness-codearts gemini*.ts，参考为权威）：
//! - 双层信封**每层键字母序**（Go map 序列化语义；serde_json preserve_order 下
//!   显式按序重建，`alphabetize`）。
//! - 五个身份头**写死**（x-machine-id/x-vscode-sessionid 是占位串，不生成随机值）；
//!   流式请求刻意不带 `Accept`；不发 `x-goog-api-key`。
//! - 模型名是准入键：必须带 `-low/-medium/-high/-tiered` 档位后缀，
//!   对外只暴露主名，出站默认补 `-medium`；未知主名**本地拒绝**
//!   （gemini.ts:537-546：上游对未知名静默回落 3.8，用户拿到错误模型的答案
//!   且无任何征兆，比报错难查得多）。
//! - system 抽到顶层 `systemInstruction`；assistant 角色 → `model`。
//! - SSE 帧可能是 `{"response":{…}}` 信封**或**裸 `Response`（先试信封再试裸，
//!   gemini-messages.ts:634-637）；`candidates` 为空的帧是纯 usage 收尾帧，
//!   只记用量不算内容；usage 取「见过的最大 totalTokenCount 的那一份」
//!   （上游多帧重复播报，早期帧偏小，gemini-messages.ts:705-712）。
//! - 端点轮换：推理 daily 先、sandbox 兜底（429/403/404/可切换 400 时换一次，
//!   gemini.ts:75 / gemini-adapter.ts:690-711）；loadCodeAssist 与配额**固定走
//!   sandbox**（原版 `baseFor(path)` 路由，gemini-credits.ts:31-33）。
//!
//! project 动态探测（loadCodeAssist，两级取值）与配额（retrieveUserQuotaSummary，
//! 请求体必须带 project）见 `detect_project`/`quota_summary`；thoughtSignature
//! 跨轮回填（只取 functionCall part 自身字段）、sessionId 升代自愈、
//! sigstore 2000 条上限淘汰一半 + 原子写均见对应结构。
//!
//! TODO(真流式/空闲超时)：`send()` 目前整体缓冲响应体后一次性解析（伪流式）。
//! 参考实现按字节流增量消费并对每次 read 施加 120s 空闲超时
//! （gemini-messages.ts:578-761，GEMINI_IDLE_TIMEOUT_MS）——上游生成大
//! functionCall 参数期间可能长时间不 flush 任何字节，裸读会无限期挂起
//! （表现为「发消息后永远转圈」）。接入 `bytes_stream` 后补：帧级增量下发 +
//! tokio::time::timeout 空闲看门狗（首 token 与 chunk 间同一预算）。

use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::openai::{ChatCompletion, ChatRequest, Usage};
use crate::provider::{ChunkStream, Credential, Provider, ProviderError, StreamChunk};
use crate::registry::{ModelInfo, ProviderCatalog};
use crate::route::Route;
use crate::sse::SseParser;

/// 主端点（client.go 的 EndpointDaily）。
pub const DEFAULT_BASE: &str = "https://daily-cloudcode-pa.googleapis.com";
/// 沙箱端点（client.go 的 EndpointSandbox；LCA/配额固定走它，推理兜底）。
pub const SANDBOX_BASE: &str = "https://daily-cloudcode-pa.sandbox.googleapis.com";
const GENERATE_PATH: &str = "/v1internal:streamGenerateContent?alt=sse";
const LOAD_PATH: &str = "/v1internal:loadCodeAssist";
const QUOTA_PATH: &str = "/v1internal:retrieveUserQuotaSummary";
const CLIENT_UA: &str = "antigravity/4.3.0 (cmdc-pak)";
const DEFAULT_EFFORT_SUFFIX: &str = "-medium";
/// 上游唯一准入主名（gemini.ts:412/460-470；其余必 404 或被静默回落）。
pub const UPSTREAM_MODEL: &str = "gemini-3.8-flash";
/// 信封 userAgent 字段是短串 'antigravity'（gemini-messages.ts:340），
/// 与 HTTP UA（含版本）不同。
const ENVELOPE_UA: &str = "antigravity";
const DEFAULT_MAX_OUTPUT_TOKENS: u64 = 64_000;
/// 请求体上限（发送前真实检查，gemini.ts:704 / gemini-adapter.ts:613-623）：
/// 超限时上游要么回措辞不稳定的 400 要么直接断连，都不如本地判定可查。
/// 归类走 ContextWindowExceeded（触发压缩重试）而不是 BadRequest（死路）。
const MAX_REQUEST_BODY_BYTES: usize = 64 * 1024 * 1024;

/// 思考档位 → thinkingBudget（gemini.ts:414-418；tiered=-1 不发 budget）。
fn thinking_budget(tier: &str) -> i64 {
    match tier {
        "low" => 1_000,
        "high" => 10_000,
        "tiered" => -1,
        _ => 4_000,
    }
}

/// 剥掉已知档位后缀得目录裸名（gemini.ts:513-518）。
fn canonical_model(model: &str) -> &str {
    for tier in ["-low", "-medium", "-high", "-tiered"] {
        if let Some(stripped) = model.strip_suffix(tier) {
            return stripped;
        }
    }
    model
}

/// Gemini schema 白名单（gemini.ts GEMINI_SCHEMA_KEYS，19 键）；白名单外键
/// 上游硬 400。递归清洗：properties/items/anyOf 深入；type 数组收敛（滤
/// 'null' 取首个非空 + 出现过 'null' 才补 nullable）；enum 含非字符串值整删
/// （Gemini 只接受字符串枚举）。
fn sanitize_schema(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            const KEYS: [&str; 19] = [
                "type", "format", "description", "nullable", "enum", "items", "minItems",
                "maxItems", "properties", "required", "minProperties", "maxProperties",
                "minLength", "maxLength", "pattern", "anyOf", "propertyOrdering", "minimum",
                "maximum",
            ];
            let mut out = Map::new();
            for (k, val) in m {
                if !KEYS.contains(&k.as_str()) {
                    continue;
                }
                match k.as_str() {
                    "properties" => {
                        let mut props = Map::new();
                        if let Some(pm) = val.as_object() {
                            for (name, child) in pm {
                                if child.is_object() {
                                    props.insert(name.clone(), sanitize_schema(child));
                                }
                            }
                        }
                        out.insert("properties".into(), Value::Object(props));
                    }
                    "items" => {
                        if let Value::Array(a) = val {
                            let cleaned: Vec<Value> =
                                a.iter().filter(|i| i.is_object()).map(sanitize_schema).collect();
                            out.insert("items".into(), Value::Array(cleaned));
                        } else if val.is_object() {
                            out.insert("items".into(), sanitize_schema(val));
                        }
                    }
                    "anyOf" => {
                        if let Some(a) = val.as_array() {
                            out.insert(
                                "anyOf".into(),
                                Value::Array(a.iter().filter(|i| i.is_object()).map(sanitize_schema).collect()),
                            );
                        }
                    }
                    "type" => {
                        if let Some(arr) = val.as_array() {
                            // 数组形态（如 ["string","null"]）：滤 'null' 取首个，
                            // 出现过 'null' 才补 nullable（gemini.ts:656-668）
                            let types: Vec<&str> = arr.iter().filter_map(Value::as_str).collect();
                            let non_null: Vec<&str> =
                                types.iter().copied().filter(|s| *s != "null").collect();
                            if let Some(first) = non_null.first() {
                                out.insert("type".into(), Value::String((*first).to_string()));
                            }
                            if non_null.len() != types.len() {
                                out.insert("nullable".into(), Value::Bool(true));
                            }
                        } else if val.is_string() {
                            out.insert("type".into(), val.clone());
                        }
                    }
                    "enum" => {
                        let all_str = val
                            .as_array()
                            .map(|a| a.iter().all(|x| x.is_string()))
                            .unwrap_or(false);
                        if all_str {
                            out.insert("enum".into(), val.clone());
                        }
                    }
                    _ => {
                        out.insert(k.clone(), val.clone());
                    }
                }
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// tool_choice → toolConfig（gemini-messages.ts:360-392）。有工具就**恒发**
/// （DSH 无 toolChoice 字段时参考实现也发默认 AUTO），mode 归一：
/// none→NONE；any/required/{type:any|tool}→ANY；其余 AUTO。
fn build_tool_config(tool_choice: Option<&Value>) -> Value {
    let mode = match tool_choice {
        Some(Value::String(s)) => match s.as_str() {
            "none" => "NONE",
            "any" | "required" => "ANY",
            _ => "AUTO",
        },
        Some(Value::Object(o)) => match o.get("type").and_then(Value::as_str).unwrap_or("") {
            "none" => "NONE",
            "any" | "tool" | "function" => "ANY",
            _ => "AUTO",
        },
        _ => "AUTO",
    };
    let mut fc = Map::new();
    fc.insert("mode".into(), Value::String(mode.into()));
    if mode == "ANY" {
        let name = tool_choice
            .and_then(Value::as_object)
            .and_then(|o| {
                o.get("function")
                    .and_then(|f| f.get("name"))
                    .or_else(|| o.get("tool").and_then(|t| t.get("name")))
                    .or_else(|| o.get("name"))
                    .and_then(Value::as_str)
                    .filter(|n| !n.is_empty())
            })
            .map(str::to_string);
        if let Some(n) = name {
            fc.insert("allowedFunctionNames".into(), serde_json::json!([n]));
        }
    }
    serde_json::json!({ "functionCallingConfig": Value::Object(fc) })
}

pub struct GeminiProvider {
    base: String,
    /// 备用（sandbox）端点：None = 单端点模式（测试桩兼容），生产注入
    /// SANDBOX_BASE。推理轮换序 [base, sandbox]；LCA/配额固定 sandbox。
    sandbox_base: Option<String>,
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

/// 缓存条数上限（gemini-sigstore.ts:49）：签名串平均 300~800 字节，
/// 2000 条约 1MB 量级，足够覆盖一个工作日的会话。
pub const SIG_MAX_ENTRIES: usize = 2_000;

/// thoughtSignature 跨轮状态（gemini-sigstore.ts 权威口径）。
///
/// - 精确键 = sha256("tool:"+名 + NUL + canonicalArgs 前 512 字符).hex[..16]，
///   精确 miss 时按工具名最近一次兜底（参数漂移在长任务里是必然事件，
///   兜底缺失会让每轮都走去签重试、丢掉整条思考链）；
/// - **只记录 functionCall part 自身的签名**（思考分片上的签名回传时被上游
///   忽略，存了纯属噪音还会挤爆缓存，gemini-messages.ts:683-686）；
/// - 写入 trim、空签名丢弃；上限 2000 条淘汰最旧一半；原子写（tmp+rename）。
///
/// 生产落盘 `~/.tokenmaster/gemini-sigs.json` 独立文件（共享文档整体替换会
/// 抹掉未知字段的教训——签名状态单独成文件，不进账号存储）。
#[derive(Default)]
pub struct SigStore {
    /// 插入序维护（同键覆盖并挪到末尾，等价参考实现的 Map 插入序语义）。
    entries: std::sync::Mutex<Vec<SigEntry>>,
    path: Option<std::path::PathBuf>,
    /// 脏标记：写入置位，flush 落盘后复位（参考实现的 dirty 字段语义——
    /// 每请求结束冲刷一次，而不是每条记录全量重写）。
    dirty: std::sync::atomic::AtomicBool,
}

#[derive(Clone)]
struct SigEntry {
    key: String,
    sig: String,
    /// 写入时刻（秒）——淘汰按它取最旧一半（秒级精度，同秒内按插入序）。
    at: u64,
    /// 工具名（持久化：键是哈希不可逆，不存名字就没法重建按名兜底索引）。
    name: String,
}

impl SigStore {
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// 当前条目数（诊断/测试用）。
    pub fn len(&self) -> usize {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
    }

    /// 读已有文件恢复（缺失/损坏降级为空表，推理不因此中断——纯缓存）。
    /// 落盘形状 `{"<key16>":{"sig","at","name"}}`（gemini-sigstore.ts:203-215）。
    pub fn load_or_create(path: std::path::PathBuf) -> Self {
        let store = Self { path: Some(path.clone()), ..Self::default() };
        let Ok(txt) = std::fs::read_to_string(&path) else { return store };
        let Ok(v) = serde_json::from_str::<Value>(&txt) else { return store };
        let Some(m) = v.as_object() else { return store };
        let mut entries = Vec::new();
        for (k, e) in m {
            let Some(sig) = e.get("sig").and_then(Value::as_str) else { continue };
            if sig.is_empty() {
                continue;
            }
            let at = e.get("at").and_then(Value::as_u64).unwrap_or(0);
            let name = e.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
            entries.push(SigEntry { key: k.clone(), sig: sig.to_string(), at, name });
        }
        *store.entries.lock().unwrap_or_else(|e| e.into_inner()) = entries;
        store
    }

    fn key(name: &str, canonical_args: &str) -> String {
        // gemini-sigstore.ts:57/80-94：sha256("tool:"+名+NUL+canonical前512).hex[..16]
        use sha2::{Digest, Sha256};
        let body: String = canonical_args.chars().take(512).collect();
        let mut h = Sha256::new();
        h.update(format!("tool:{name}\0{body}"));
        format!("{:x}", h.finalize())[..16].to_string()
    }

    /// 记录一次「响应中 functionCall (name, args) ↔ 签名」。
    pub fn record(&self, name: &str, args: &Value, sig: &str) {
        let canonical = alphabetize(args).to_string();
        self.record_with_canonical(name, &canonical, sig);
    }

    pub(crate) fn record_with_canonical(&self, name: &str, canonical_args: &str, sig: &str) {
        let sig = sig.trim();
        // 空签名/空工具名丢弃（gemini-sigstore.ts:97-99/169-171）：上游偶尔下发
        // 空串占位，存进去会让回填侧以为「有签名」而发一个空字段。
        if sig.is_empty() || name.is_empty() {
            return;
        }
        let key = Self::key(name, canonical_args);
        let at = now_ts_secs();
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.retain(|e| e.key != key); // 覆盖并挪到末尾（Map 插入序语义）
        entries.push(SigEntry { key, sig: sig.to_string(), at, name: name.to_string() });
        if entries.len() > SIG_MAX_ENTRIES {
            Self::evict(&mut entries);
        }
        drop(entries);
        self.dirty.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// 淘汰最旧的一半（按 at 稳定排序，gemini-sigstore.ts:185-192：不做精确
    /// LRU，只「少留一点空间」；同秒内按插入序，与参考实现的稳定排序等价）。
    fn evict(entries: &mut Vec<SigEntry>) {
        let mut order: Vec<usize> = (0..entries.len()).collect();
        order.sort_by_key(|&i| entries[i].at);
        let half = order.len() / 2;
        let evict_keys: std::collections::HashSet<&usize> = order[..half].iter().collect();
        let mut idx = 0;
        entries.retain(|_| {
            let keep = !evict_keys.contains(&idx);
            idx += 1;
            keep
        });
    }

    /// 精确键优先，miss 时按工具名最近一次兜底；从未见过返回 None
    /// （「最近」按插入序，不用秒级时间戳——同秒两次写入无法区分，实测踩过）。
    pub fn lookup(&self, name: &str, args: &Value) -> Option<String> {
        let canonical = alphabetize(args).to_string();
        let key = Self::key(name, &canonical);
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = entries.iter().rev().find(|e| e.key == key && !e.sig.is_empty()) {
            return Some(e.sig.clone());
        }
        entries
            .iter()
            .rev()
            .find(|e| e.name == name && !e.sig.is_empty())
            .map(|e| e.sig.clone())
    }

    /// 冲刷落盘：请求结束调用（参考实现 stream 的 finally 冲刷）。无脏改动
    /// 或纯内存模式为 no-op。
    pub fn flush(&self) {
        if !self.dirty.swap(false, std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let Some(p) = &self.path else { return };
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let mut obj = Map::new();
        for e in entries.iter() {
            obj.insert(
                e.key.clone(),
                serde_json::json!({ "sig": e.sig, "at": e.at, "name": e.name }),
            );
        }
        drop(entries);
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut tmp = p.clone().into_os_string();
        tmp.push(".tmp");
        if std::fs::write(&tmp, Value::Object(obj).to_string()).is_ok() {
            let _ = std::fs::rename(&tmp, p);
        }
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

/// 上下文超限专属判据（gemini-adapter.ts:154-155 GEMINI_TOKEN_COUNT_OVERFLOW：
/// "token count" 后短距 "exceed(s)" + "the maximum"，句式无 context 字样，
/// 通用判据认不出）。大小写不敏感；exceed 搜索窗限定在 "token count" 后
/// 80 字符内，避免长正文里的无关共现误判。
pub fn is_context_exceeded(text: &str) -> bool {
    let lower = text.to_lowercase();
    let Some(i) = lower.find("token count") else { return false };
    let window_end = (i + 80).min(lower.len());
    let Some(j) = lower[i..window_end].find("exceed") else { return false };
    lower[i + j..].contains("the maximum")
}

/// 服务端按 sessionId 累计超 1M 的专属句式（gemini-adapter.ts:123-125，
/// 判据逐字抄自 Antigravity-Manager 的 [FIX session-1M]）。必须**先于**
/// is_quota_text 判定：换端点/换号解决不了服务端会话累计。
pub fn is_session_overflow(text: &str) -> bool {
    text.to_lowercase().contains("exceeds the maximum number of tokens")
}

/// 签名被上游拒绝（gemini-adapter.ts:84-88）。必须**宽**：上游文案不统一
/// （thought_signature / Invalid signature / signature is required…），漏判的
/// 后果是用户看到一次莫名其妙的 400；误判只是多发一次不带签名的请求
/// （那条路径本来就要试）。
pub fn is_signature_error(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("signature") || (lower.contains("thought") && lower.contains("invalid"))
}

/// 400 正文指向配额/权限/工程号类问题（gemini-adapter.ts:91-94，决定先换端点）。
fn is_switchable_text(text: &str) -> bool {
    let lower = text.to_lowercase();
    ["quota", "permission", "unsupported", "project"].iter().any(|w| lower.contains(w))
}

/// 400 正文明确是配额耗尽（gemini-adapter.ts:97-101，归类限流交编排层换号）。
pub fn is_quota_text(text: &str) -> bool {
    let lower = text.to_lowercase();
    ["quota", "resource_exhausted", "resource exhausted", "rate limit", "exceeded"]
        .iter()
        .any(|w| lower.contains(w))
}

impl GeminiProvider {
    pub fn new(base: String, fallback_project: String) -> Self {
        Self {
            base,
            sandbox_base: None,
            fallback_project,
            session: std::sync::Mutex::new(SessionState::default()),
            client: reqwest::Client::new(),
            resolved_project: std::sync::Mutex::new(None),
            sigs: std::sync::Arc::new(SigStore::in_memory()),
        }
    }

    /// 注入备用（sandbox）端点：生产装配用；不注入则单端点模式
    /// （LCA/配额回落主端点，推理不轮换），便于单桩测试。
    pub fn with_sandbox_base(mut self, sandbox: String) -> Self {
        self.sandbox_base = Some(sandbox);
        self
    }

    /// sessionId 确定性派生（gemini.ts:186-203）：FNV-1a 64（有符号十进制），
    /// 输入 `project\0lane\0firstUserText`（generation>0 再追加 "\0"+gen；
    /// 分隔符是 NUL 不是空格，generation==0 不拼代数段——必须与升代前同值，
    /// 否则升级这个功能本身就会让所有进行中的对话换一次 id 丢 prompt cache）。
    /// lane 是业务路径字面量：推理 `'infer'` / 冒烟 `'smoke'`（与端点无关）。
    pub fn derive_session_id(project: &str, first_user: &str, lane: &str) -> String {
        Self::derive_session_id_gen(project, first_user, lane, 0)
    }

    pub fn derive_session_id_gen(project: &str, first_user: &str, lane: &str, generation: u32) -> String {
        let mut input = format!("{project}\0{lane}\0{first_user}");
        if generation > 0 {
            input.push_str(&format!("\0{generation}"));
        }
        let mut hash: u64 = 0xcbf29ce484222325;
        for b in input.bytes() {
            hash ^= b as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        (hash as i64).to_string()
    }

    /// 首条 user 文本口径（gemini-messages.ts:178-186：按构造后的 contents 取
    /// 首个 user 角色条目、只看其中第一个非空 text part——开头的 assistant/
    /// tool 之外角色不参与；后续文本也不进哈希）。
    fn first_user_text(req: &ChatRequest) -> String {
        req.messages
            .iter()
            .find(|m| m.role == "user" || m.role == "tool")
            .map(|m| match &m.content {
                Value::Array(parts) => parts
                    .iter()
                    .filter_map(|p| p.get("text").and_then(Value::as_str))
                    .find(|t| !t.is_empty())
                    .map(str::to_string)
                    .unwrap_or_default(),
                _ => m.text(),
            })
            .unwrap_or_default()
    }

    /// 会话解析：派生因子变化 → 切换新会话（gen 归零）；返回当前代 sessionId。
    fn current_session_id(&self, project: &str, req: &ChatRequest) -> String {
        let base = Self::derive_session_id(project, &Self::first_user_text(req), "infer");
        let mut s = self.session.lock().unwrap_or_else(|e| e.into_inner());
        if s.base != base {
            *s = SessionState { base, gen: 0 };
        }
        Self::derive_session_id_gen(project, &Self::first_user_text(req), "infer", s.gen)
    }

    /// 升代：gen+1 换新 id（服务端按 id 累计，削本地历史无用）。
    fn bump_generation(&self, project: &str, req: &ChatRequest) -> String {
        let base = Self::derive_session_id(project, &Self::first_user_text(req), "infer");
        let mut s = self.session.lock().unwrap_or_else(|e| e.into_inner());
        if s.base != base {
            *s = SessionState { base, gen: 0 };
        }
        s.gen += 1;
        Self::derive_session_id_gen(project, &Self::first_user_text(req), "infer", s.gen)
    }

    pub fn production() -> Self {
        let mut p = Self::new(DEFAULT_BASE.into(), "aicode-consumers".into());
        p.sandbox_base = Some(SANDBOX_BASE.into());
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

    /// LCA 探测端点序（sandbox 优先、daily 兜底，gemini-project.ts:100-106/134：
    /// 原版 baseFor 把 loadCodeAssist 固定路由 sandbox）。
    fn probe_endpoints(&self) -> Vec<String> {
        match &self.sandbox_base {
            Some(s) => vec![s.clone(), self.base.clone()],
            None => vec![self.base.clone()],
        }
    }

    /// LCA/配额固定端点（gemini-credits.ts:31-33；单端点模式回落主端点）。
    fn meta_base(&self) -> &str {
        self.sandbox_base.as_deref().unwrap_or(&self.base)
    }

    /// loadCodeAssist：逐端点探测，全部失败才报错（gemini-project.ts:142-172）。
    /// 逐字常量请求体（gemini.ts:97；多字段可能被识别为非官方客户端）。
    async fn load_code_assist(&self, cred: &Credential) -> Result<Value, ProviderError> {
        let mut last_err = String::from("loadCodeAssist 探测失败");
        for ep in self.probe_endpoints() {
            let attempt = self
                .client
                .post(format!("{ep}{LOAD_PATH}"))
                .headers(self.identity_headers(cred))
                .json(&serde_json::json!({ "metadata": { "ideType": "ANTIGRAVITY" } }))
                .send()
                .await;
            let resp = match attempt {
                Ok(r) => r,
                Err(e) => {
                    last_err = e.to_string();
                    continue;
                }
            };
            let status = resp.status().as_u16();
            let bytes = match resp.bytes().await {
                Ok(b) => b,
                Err(e) => {
                    last_err = e.to_string();
                    continue;
                }
            };
            if status != 200 {
                last_err = format!("loadCodeAssist HTTP {status}");
                continue;
            }
            match serde_json::from_slice(&bytes) {
                Ok(v) => return Ok(v),
                Err(e) => last_err = format!("loadCodeAssist 响应非 JSON: {e}"),
            }
        }
        Err(ProviderError::Upstream(last_err))
    }

    /// project 动态探测（每次真探测并更新缓存；GUI/入池可手动刷新）。
    /// 失败 ≠ 探测到空：HTTP/解析失败返回 Err，调用方不得发推理；
    /// 200 但无 `cloudaicompanionProject` 时用兜底值（免费档常态）。
    pub async fn detect_project(&self, cred: &Credential) -> Result<String, ProviderError> {
        let v = self.load_code_assist(cred).await?;
        // 两级取值：顶层 → currentTier 嵌套（gemini-project.ts:72-83）
        let detected = v
            .get("cloudaicompanionProject")
            .and_then(Value::as_str)
            .or_else(|| v.pointer("/currentTier/cloudaicompanionProject").and_then(Value::as_str))
            .unwrap_or("");
        let project = if detected.is_empty() {
            self.fallback_project.clone()
        } else {
            detected.to_string()
        };
        *self.resolved_project.lock().unwrap_or_else(|e| e.into_inner()) = Some(project.clone());
        Ok(project)
    }

    /// 推理路径：缓存优先，miss 时探测一次。
    async fn resolve_project(&self, cred: &Credential) -> Result<String, ProviderError> {
        if let Some(p) = self.resolved_project.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            return Ok(p);
        }
        self.detect_project(cred).await
    }

    /// 配额摘要（5h/周双窗口百分比，单位 '%' 不参与积分归一）。
    /// **固定走 sandbox 端点**（原版 baseFor 路由）；请求体必须带 `project`
    /// （字段名是 project 不是 cloudaicompanionProject，空对象对部分账号回
    /// 403 SUBSCRIPTION_REQUIRED——gemini-credits.ts:12-29）。响应 schema 参考
    /// 实现未在手册给出完整字段表，原样透传 JSON 供 GUI 接线时以真实响应校准。
    pub async fn quota_summary(&self, cred: &Credential, project: &str) -> Result<Value, ProviderError> {
        let resp = self
            .client
            .post(format!("{}{}", self.meta_base(), QUOTA_PATH))
            .headers(self.identity_headers(cred))
            .json(&serde_json::json!({ "project": project }))
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

    /// 构造双层信封（gemini-messages.ts translateGeminiRequest）。
    /// `with_sigs=false` 供去签重试（参考实现重进循环时跳过签名查表）。
    fn build_envelope(
        &self,
        project: &str,
        session_id: &str,
        route: &Route,
        req: &ChatRequest,
        with_sigs: bool,
    ) -> Result<Value, ProviderError> {
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
            // tool 消息的结果只走 functionResponse.response.content，不另发 text part
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
                            // 解析失败/非对象退化为 {}（gemini-messages.ts:405-416：
                            // 不编造参数，也不因畸形历史打死整轮请求）
                            let args: Value = match serde_json::from_str(args_raw) {
                                Ok(v @ Value::Object(_)) => v,
                                _ => Value::Object(Map::new()),
                            };
                            let mut fc = Map::new();
                            fc.insert("args".into(), args.clone());
                            fc.insert("name".into(), Value::String(name.to_string()));
                            // 跨轮签名回填：精确键优先，同工具名最近一次兜底
                            if with_sigs {
                                if let Some(sig) = self.sigs.lookup(name, &args) {
                                    fc.insert("thoughtSignature".into(), Value::String(sig));
                                }
                            }
                            parts.push(serde_json::json!({ "functionCall": Value::Object(fc) }));
                        }
                    }
                }
                "tool" => {
                    // name 必须来自 tool_use 映射：解析不到整块丢弃（上游按 name
                    // 配对，空 name 会 400；丢了顶多少一轮工具上下文，不打死
                    // 整轮——gemini-messages.ts:240-245）
                    if let Some(id) = m.tool_call_id.as_deref() {
                        if let Some(name) = name_map.get(id) {
                            parts.push(serde_json::json!({
                                "functionResponse": { "name": name, "response": { "content": t } }
                            }));
                        }
                    }
                }
                _ => {}
            }
            // 空消息整条跳过（gemini-messages.ts:278-279；发空 parts 上游 400）
            if parts.is_empty() {
                continue;
            }
            contents.push(serde_json::json!({ "parts": parts, "role": role }));
        }
        if contents.is_empty() {
            return Err(ProviderError::BadRequest(
                "gemini: messages 里没有可用内容（全部为空/孤儿 tool 结果）".into(),
            ));
        }
        let mut request = Map::new();
        request.insert("contents".into(), Value::Array(contents));
        let mut generation = Map::new();
        let max_out = req.raw.get("max_tokens").and_then(Value::as_u64).unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS);
        generation.insert("maxOutputTokens".into(), Value::from(max_out));
        let wire = Self::wire_model(&route.model);
        let tier = wire.rsplit('-').next().unwrap_or("medium");
        let mut thinking = Map::new();
        // includeThoughts 恒 true（「关闭思考」是假关：关掉照样思考照样计费）
        thinking.insert("includeThoughts".into(), Value::Bool(true));
        let budget = thinking_budget(tier);
        if budget >= 0 {
            thinking.insert("thinkingBudget".into(), Value::from(budget));
        }
        generation.insert("thinkingConfig".into(), Value::Object(thinking));
        if let Some(t) = req.raw.get("temperature") {
            generation.insert("temperature".into(), t.clone());
        }
        request.insert("generationConfig".into(), Value::Object(generation));
        request.insert("sessionId".into(), Value::String(session_id.to_string()));
        if !system_parts.is_empty() {
            request.insert(
                "systemInstruction".into(),
                serde_json::json!({ "role": "system", "parts": [{ "text": system_parts.join("\n\n") }] }),
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
                if let Some(desc) = f.get("description") {
                    d.insert("description".into(), desc.clone());
                }
                if let Some(p) = f.get("parameters") {
                    d.insert("parameters".into(), sanitize_schema(p));
                }
                decls.push(Value::Object(d));
            }
            if !decls.is_empty() {
                request.insert(
                    "tools".into(),
                    serde_json::json!([{ "functionDeclarations": decls }]),
                );
                // 有工具就恒发 toolConfig（gemini-messages.ts:332-333/360-375：
                // 参考实现无 toolChoice 时也发默认 AUTO，不做条件省略）
                request.insert("toolConfig".into(), build_tool_config(req.raw.get("tool_choice")));
            }
        }
        let mut root = Map::new();
        root.insert("model".into(), Value::String(wire));
        root.insert("project".into(), Value::String(project.to_string()));
        root.insert("request".into(), Value::Object(request));
        // requestId 形态 agent/{ms}/{8hex}（gemini.ts:400-402），每请求随机——
        // 重试（去签/升代/换端点）也重新生成，与参考「每轮重构信封」一致
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        root.insert(
            "requestId".into(),
            Value::String(format!("agent/{ms}/{}", crate::key::random_id(4))),
        );
        root.insert("userAgent".into(), Value::String(ENVELOPE_UA.into()));
        Ok(alphabetize(&Value::Object(root)))
    }

    /// 单次推理 POST（流式刻意不带 Accept，抓包一致；信封已字母序序列化）。
    async fn post_generate(
        &self,
        base: &str,
        wire: &str,
        cred: &Credential,
    ) -> Result<reqwest::Response, ProviderError> {
        let mut h = self.identity_headers(cred);
        h.insert(
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static("application/json"),
        );
        self.client
            .post(format!("{base}{GENERATE_PATH}"))
            .headers(h)
            .body(wire.to_string())
            .send()
            .await
            .map_err(|e| ProviderError::Upstream(e.to_string()))
    }

    /// 发送推理请求：返回 (HTTP 状态, 响应体)。
    ///
    /// 失败处置升级式（gemini-adapter.ts:660-758 的单号内部分）：
    /// 1. 签名被拒（宽判据）→ 去签重试一次（同端点同号）；
    /// 2. 400 服务端会话累计超 1M → 升代换新 sessionId 重试一次（先于端点/
    ///    配额判定——换端点换号都救不了服务端累计）；再超限归
    ///    ContextWindowExceeded（本地历史太大时升代没用，交客户端压缩）；
    /// 3. 403/404/400(quota|permission|unsupported|project) 或 429 首次 →
    ///    先换端点（廉价兜底，一次）；
    /// 4. 换端点救不了 → 分类上抛：429→RateLimited(60s)（编排层冷却+换号）、
    ///    401→Credential（编排层续期/换号）、403/404→Upstream（编排层换号）。
    async fn send(
        &self,
        cred: &Credential,
        route: &Route,
        req: &ChatRequest,
    ) -> Result<(u16, Vec<u8>), ProviderError> {
        // 模型准入门（gemini.ts:537-546，纯本地零网络）：名字是上游准入键，
        // 放行的后果是上游静默回落 3.8，用户拿到错误模型的答案且无任何征兆
        if canonical_model(&route.model) != UPSTREAM_MODEL {
            return Err(ProviderError::BadRequest(format!(
                "gemini: 模型 {} 不在本 provider 目录中（仅支持 {UPSTREAM_MODEL}）",
                route.model
            )));
        }
        // 探测失败不发推理（失败 ≠ 探测到空）
        let project = self.resolve_project(cred).await?;
        let endpoints = self.inference_endpoints();
        let mut endpoint_idx = 0usize;
        let mut endpoint_switched = false;
        let mut dropped_signatures = false;
        let mut session_bumped = false;
        let mut sid = self.current_session_id(&project, req);
        loop {
            let body = self.build_envelope(&project, &sid, route, req, !dropped_signatures)?;
            // 请求体上限发送前检查（gemini-adapter.ts:613-623）
            let wire = body.to_string();
            if wire.len() > MAX_REQUEST_BODY_BYTES {
                return Err(ProviderError::ContextWindowExceeded(format!(
                    "gemini: 请求体 {} 字节超过上限 {MAX_REQUEST_BODY_BYTES} 字节",
                    wire.len()
                )));
            }
            let resp = self
                .post_generate(&endpoints[endpoint_idx], &wire, cred)
                .await?;
            let status = resp.status().as_u16();
            let bytes = resp.bytes().await.map_err(|e| ProviderError::Upstream(e.to_string()))?.to_vec();
            if status == 200 {
                return Ok((status, bytes));
            }
            let text = String::from_utf8_lossy(&bytes).to_string();
            // 1) 签名被拒 → 去签重试一次（仅一次，同端点同号）
            if is_signature_error(&text) && !dropped_signatures && wire.contains("thoughtSignature") {
                dropped_signatures = true;
                continue;
            }
            // 2) 服务端会话累计超 1M → 升代（一次请求最多一代；再超限走 4) 的
            //    ContextWindowExceeded 归类）
            if status == 400 && is_session_overflow(&text) && !session_bumped {
                session_bumped = true;
                sid = self.bump_generation(&project, req);
                continue;
            }
            // 3) 先换端点（一次；单端点模式无备选直接走分类）
            let endpoint_first =
                status == 403 || status == 404 || (status == 400 && is_switchable_text(&text));
            if (endpoint_first || status == 429) && !endpoint_switched && endpoints.len() > 1 {
                endpoint_switched = true;
                endpoint_idx = 1;
                continue;
            }
            // 4) 分类上抛（401 续期、429 冷却换号由编排层承接）
            return Err(map_status_error(status, text));
        }
    }

    /// 推理端点序（gemini.ts:75 GEMINI_ENDPOINTS：daily 先，sandbox 兜底；
    /// 单端点模式只有主端点）。
    fn inference_endpoints(&self) -> Vec<String> {
        match &self.sandbox_base {
            Some(s) => vec![self.base.clone(), s.clone()],
            None => vec![self.base.clone()],
        }
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

/// HTTP 状态 + 正文 → ProviderError（公开供单测）。
///
/// 归类顺序有讲究（gemini-adapter.ts:774-794）：上下文超限**最先**判——
/// is_quota_text 的关键词表含 "exceeded"，超限报文先落 quota 会被误归限流，
/// 丢掉客户端压缩重试的机会（归错码的代价不对称：多判一次溢出只是多一次
/// 无效压缩尝试，漏判则长会话每轮报废）。
pub fn map_status_error(status: u16, msg: String) -> ProviderError {
    if status == 400 && is_context_exceeded(&msg) {
        return ProviderError::ContextWindowExceeded(format!("400 context exceeded: {msg}"));
    }
    if status == 400 && is_quota_text(&msg) {
        // 400 配额文案（RESOURCE_EXHAUSTED 等）语义等同限流：编排层冷却换号
        return ProviderError::RateLimited { retry_after_secs: Some(60), msg: format!("400 quota: {msg}") };
    }
    match status {
        401 => ProviderError::Credential(format!("401 token rejected: {msg}")),
        429 => ProviderError::RateLimited { retry_after_secs: Some(60), msg: format!("429 quota: {msg}") },
        // 403/404 归 Upstream（参考 SERVER 分类，gemini-adapter.ts:788-791）：
        // Cloud Code 上这两个码多是「入口/模型注册表不认」，归凭据错误会把
        // 用户引向「去重新登录」这个无效动作
        403 | 404 => ProviderError::Upstream(format!("http {status}: {msg}")),
        400 => ProviderError::BadRequest(format!("400 rejected: {msg}")),
        code => ProviderError::Upstream(format!("http {code}: {msg}")),
    }
}

/// 一帧 SSE data 的聚合结果。
struct FrameData {
    text: String,
    /// `thought:true` 分片的文本（思考链，stream 走 Reasoning chunk、非流式
    /// 丢弃——OutMessage 无 reasoning 字段，与 commandcode 同口径）
    reasoning: String,
    /// functionCall 调用 (name, args JSON 串)
    calls: Vec<(String, String)>,
    /// 签名事件 (工具名, canonical args, signature)——只来自 functionCall
    /// part 自身的 thoughtSignature 字段
    sig_events: Vec<(String, String, String)>,
    usage: Option<Usage>,
    finish_reason: Option<String>,
}

/// 解析一帧 SSE data（gemini-messages.ts processFrame）。
///
/// 帧可能是 `{"response":{…}}` 信封**或**裸 `Response`——先试信封再试裸
/// （:634-637）；`candidates` 缺失/为空是纯 usage 收尾帧，只带用量。
/// `thought:true` 的文本 part 是思考链（includeThoughts 恒 true，必有），
/// 与正文分开聚合（:683-694）。
fn parse_frame(v: &Value) -> FrameData {
    let inner = match v.get("response") {
        Some(Value::Object(_)) => v.get("response").unwrap_or(v),
        _ => v,
    };
    let candidate = inner
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|c| c.first());
    let parts = candidate
        .and_then(|c| c.pointer("/content/parts"))
        .and_then(Value::as_array);
    let mut text = String::new();
    let mut reasoning = String::new();
    if let Some(parts) = parts {
        for p in parts {
            let Some(t) = p.get("text").and_then(Value::as_str) else { continue };
            if p.get("thought").and_then(Value::as_bool) == Some(true) {
                reasoning.push_str(t);
            } else {
                text.push_str(t);
            }
        }
    }
    let mut calls = Vec::new();
    let mut sig_events = Vec::new();
    if let Some(parts) = parts {
        for p in parts {
            let Some(fc) = p.get("functionCall").and_then(Value::as_object) else { continue };
            let name = fc.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
            let args = fc.get("args").cloned().unwrap_or(Value::Object(Map::new()));
            // 只取 functionCall part 自身的签名（gemini-messages.ts:651-657）：
            // 思考分片上的签名回传时被上游忽略，存了纯属噪音（sigstore 头注）
            if let Some(sig) = p
                .get("thoughtSignature")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                sig_events.push((name.clone(), alphabetize(&args).to_string(), sig.to_string()));
            }
            calls.push((name, args.to_string()));
        }
    }
    let finish_reason = candidate
        .and_then(|c| c.get("finishReason"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let usage = inner.get("usageMetadata").map(|u| {
        let prompt = u.get("promptTokenCount").and_then(Value::as_u64).unwrap_or(0);
        let cached = u.get("cachedContentTokenCount").and_then(Value::as_u64).unwrap_or(0);
        let completion = u.get("candidatesTokenCount").and_then(Value::as_u64).unwrap_or(0);
        let total = u.get("totalTokenCount").and_then(Value::as_u64).unwrap_or(0);
        Usage {
            // 互斥口径：input 扣除缓存命中（否则双重计费，gemini-messages.ts:499-523）
            prompt_tokens: prompt.saturating_sub(cached),
            completion_tokens: completion,
            total_tokens: total,
        }
    });
    FrameData { text, reasoning, calls, sig_events, usage, finish_reason }
}

/// 多帧 usage 聚合器：保留「totalTokenCount 最大」的那一份（上游多帧重复
/// 播报 usage，早期帧数字偏小——末帧覆盖会少计，gemini-messages.ts:705-712）。
#[derive(Default)]
struct UsageAgg {
    usage: Usage,
    best_total: i64,
}

impl UsageAgg {
    fn note(&mut self, u: Usage) {
        if (u.total_tokens as i64) >= self.best_total {
            self.best_total = u.total_tokens as i64;
            self.usage = u;
        }
    }
}

/// 上游 finishReason → OpenAI finish_reason（mapGeminiFinish :489-497 的
/// OpenAI 词汇版：未知/缺失一律 stop，不编造成错误；有工具调用优先）。
fn map_finish(saw_tool_call: bool, finish_reason: Option<&str>) -> &'static str {
    if saw_tool_call {
        return "tool_calls";
    }
    if finish_reason == Some("MAX_TOKENS") {
        return "length";
    }
    "stop"
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
            // 上游准入表只认 gemini-3.8-flash（gemini.ts:460-554，其余必 404）
            models: vec![ModelInfo { id: UPSTREAM_MODEL.into() }],
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
        let mut usage = UsageAgg::default();
        let mut finish_reason: Option<String> = None;
        while let Some(data) = parser.next_data() {
            let Ok(v) = serde_json::from_str::<Value>(&data) else { continue };
            let fd = parse_frame(&v);
            text.push_str(&fd.text);
            calls.extend(fd.calls);
            for (name, canonical, sig) in fd.sig_events {
                self.sigs.record_with_canonical(&name, &canonical, &sig);
            }
            if let Some(fr) = fd.finish_reason {
                finish_reason = Some(fr);
            }
            if let Some(u2) = fd.usage {
                usage.note(u2);
            }
        }
        // 请求结束冲刷签名缓存（参考实现 stream finally；纯缓存，失败吞掉）
        self.sigs.flush();
        let mut out = ChatCompletion::new(route.composite(), text, usage.usage);
        out.choices[0].finish_reason =
            Some(map_finish(!calls.is_empty(), finish_reason.as_deref()).into());
        if !calls.is_empty() {
            out.choices[0].message.tool_calls = Some(openai_tool_calls(&calls));
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
        let mut usage = UsageAgg::default();
        let mut saw_tool_call = false;
        let mut finish_reason: Option<String> = None;
        while let Some(data) = parser.next_data() {
            let Ok(v) = serde_json::from_str::<Value>(&data) else { continue };
            let fd = parse_frame(&v);
            if !fd.reasoning.is_empty() {
                queue.push_back(Ok(StreamChunk::Reasoning(fd.reasoning)));
            }
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
            if let Some(fr) = fd.finish_reason {
                finish_reason = Some(fr);
            }
            if let Some(u2) = fd.usage {
                usage.note(u2);
            }
        }
        // 请求结束冲刷签名缓存（参考实现 stream finally；纯缓存，失败吞掉）
        self.sigs.flush();
        // 上游一次性返回帧流；帧尽即完成
        let reason = map_finish(saw_tool_call, finish_reason.as_deref());
        queue.push_back(Ok(StreamChunk::Finish { reason: reason.into(), usage: usage.usage }));
        Ok(Box::pin(futures::stream::iter(queue)))
    }
}

/// Gemini OAuth（gemini-oauth.ts）：授权 `accounts.google.com/o/oauth2/v2/auth` +
/// token `oauth2.googleapis.com/token`；**refresh_token 会轮换，刷新后必须
/// 立即回写**（响应未带新 rt 时保留旧值）。
///
/// 凭据 secret 形态：JSON `{access_token, refresh_token, expires_at}`；
/// 推理 Authorization 取其中 access_token（裸串回退兼容）。
#[derive(Clone)]
pub struct GeminiOAuth {
    auth_url: String,
    token_url: String,
    client_id: String,
    client_secret: String,
    scopes: Vec<String>,
    client: reqwest::Client,
}

/// 公开 OAuth 客户端 id（gemini-oauth.ts:36；client_secret 除外——那个走 env）。
pub const DEFAULT_CLIENT_ID: &str =
    "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";

/// 六项 scope 逐字清单（gemini-oauth.ts:59-66，空格连接下发）。
pub const SCOPES: [&str; 6] = [
    "openid",
    "https://www.googleapis.com/auth/cloud-platform",
    "https://www.googleapis.com/auth/userinfo.email",
    "https://www.googleapis.com/auth/userinfo.profile",
    "https://www.googleapis.com/auth/cclog",
    "https://www.googleapis.com/auth/experimentsandconfigs",
];

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
    pub fn new(
        auth_url: String,
        token_url: String,
        client_id: String,
        client_secret: String,
        scopes: Vec<String>,
    ) -> Self {
        Self { auth_url, token_url, client_id, client_secret, scopes, client: reqwest::Client::new() }
    }

    /// 生产配置：Google 固定端点；client_id 经 `CMDC_PAK_GOOGLE_CLIENT_ID`
    /// 注入（未配置时 refresh 会被上游以凭据错误拒绝）。
    pub fn production() -> Self {
        Self::new(
            "https://accounts.google.com/o/oauth2/v2/auth".into(),
            "https://oauth2.googleapis.com/token".into(),
            std::env::var("CMDC_PAK_GOOGLE_CLIENT_ID")
                .unwrap_or_else(|_| DEFAULT_CLIENT_ID.into()),
            // client_secret 不进源码（红线/平台密钥拦截）：经 env 注入。
            // 值的出处见 docs/provider-gap-analysis.md §二。
            std::env::var("CMDC_PAK_GOOGLE_CLIENT_SECRET").unwrap_or_default(),
            SCOPES.iter().map(|s| s.to_string()).collect(),
        )
    }

    /// 授权 URL（response_type=code + access_type=offline 换 refresh_token；
    /// prompt=consent 保证离线授权下发；include_granted_scopes=true 与参考一致）。
    pub fn login_url(&self, redirect_uri: &str, state: &str) -> String {
        format!(
            "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&state={}&access_type=offline&include_granted_scopes=true&prompt=consent",
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
        // expires_in 来自 OAuth 端点响应：饱和加法防巨数溢出 panic
        let expires_at = now_ts_secs().saturating_add(v.get("expires_in").and_then(Value::as_u64).unwrap_or(3600));
        let secret = serde_json::json!({
            "access_token": at,
            "refresh_token": rt,
            "expires_at": expires_at,
        })
        .to_string();
        Ok(Credential { account_id: account_id.into(), secret })
    }

    /// 本地回环回调登录：随机端口起 listener，返回 (授权 URL, 凭据接收端)。
    /// 浏览器完成授权后 Google 重定向到 `http://localhost:{port}?code=…&state=…`，
    /// 校验 state（防 CSRF）→ 交换 token → oneshot 回传 Credential。
    pub async fn start_login(
        &self,
    ) -> (String, tokio::sync::oneshot::Receiver<Result<Credential, ProviderError>>) {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind loopback callback");
        let port = listener.local_addr().unwrap().port();
        // redirect_uri 必须逐字 localhost + /oauth-callback（client 注册值，
        // 用 127.0.0.1 或无路径会 redirect_uri_mismatch——gemini-oauth.ts:41-44）
        let redirect_uri = format!("http://localhost:{port}/oauth-callback");
        let state = crate::key::random_id(12);
        let url = self.login_url(&redirect_uri, &state);
        let (tx, rx) = tokio::sync::oneshot::channel();
        let tx = std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));
        let this = self.clone();
        let expected_state = state.clone();
        let app = axum::Router::new().route(
            "/oauth-callback",
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
                        let res = this.exchange_code(code, &format!("http://localhost:{port}/oauth-callback")).await;
                        if let Some(tx) = tx.lock().unwrap_or_else(|e| e.into_inner()).take() {
                            let _ = tx.send(res);
                        }
                        (axum::http::StatusCode::OK, "TokenMaster: 登录完成，可关闭此页")
                    }
                },
            ),
        );
        let app_v6 = app.clone();
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("callback serve");
        });
        // IPv6 兜底监听（gemini-oauth.ts:424-463）：浏览器常把 localhost 解析成
        // ::1，只听 v4 会「页面打不开/授权完没反应」；[::1] 绑定失败可容忍
        //（机器没开 IPv6 时 v4 仍可用），端口同 v4。
        if let Ok(v6) = tokio::net::TcpListener::bind(("[::1]", port)).await {
            tokio::spawn(async move {
                axum::serve(v6, app_v6).await.expect("callback serve v6");
            });
        }
        (url, rx)
    }

    /// 授权码交换（account_id 由调用方落库时分配，此处沿用传入值）。
    pub async fn exchange_code(&self, code: &str, redirect_uri: &str) -> Result<Credential, ProviderError> {
        let v = self
            .post_token(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
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
                ("client_secret", self.client_secret.as_str()),
            ])
            .await?;
        self.credential_from(&v, Some(&rt), &cred.account_id)
    }
}
