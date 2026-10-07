# Provider 横向对比与缺口分析（对照参考源码，2026-10-07）

> 参考源码（本地，经测试的稳定实现，为权威）：`deepseek-harness-codearts`、
> `commandcode-proxy-master`、`Antigravity-Manager-main`。
> 本文件是修复路线的权威清单；行号均指参考源码。按「会硬失败的错误 >
> 行为偏差 > 值级补齐」分档。

## 总览

| Provider | 已对齐 | 会硬失败的错误 | 主要逻辑缺失 | 值级解锁 |
|---|---|---|---|---|
| zcode | 推理/SSE/登录/claim 链 | **balance 解析字段路径错误**（读不存在的 data.total） | 双通道（订阅腿）、换腿、enterprise/preview 形态 | — |
| gemini | 五身份头/信封字母序/探测失败不发推理 | OAuth 缺 client_secret、redirect_uri 形态错、quota 请求体缺 project、catalog 模型名不在准入表（必 404） | thinkingConfig/toolConfig/maxOutputTokens、usage 缓存口径、端点轮换、账号轮换冷却 | client_id/secret/六项 scope 全量 |
| commandcode | 信封/NDJSON/上报节奏骨架 | components 上报形状完全不同（易被行为分析识别）、finish-step 未处理（误判截断） | idle 超时/首字节重试、cache_control 断点、session per-key | 别名表 4 条、指纹池逐字、MODELS 26 款 |
| qoder | 信封解包/claim 骨架 | Cosy-ClientType/MachineToken 值来源错（拿不到可领活动） | 设备码登录、WASM 加密推理、refresh | WASM 文件路径已定位、clientId、加密载荷全结构 |
| trae | SOLO 推理/头族/错误冷却（刚复刻） | **balance 响应形状与参考不一致**（两形状必有一错，需 wire 核对） | ExchangeToken 续期、模型目录、签到、空响应重试 | ExchangeToken 请求体、22 functions 全表 |

## 一、zcode（免费 vs 订阅专题）

### 双通道架构（参考 zcode-transport.ts:30-129 / zcode-adapter.ts:966-1100）

| 维度 | start-plan（免费积分腿） | coding-plan（付费订阅腿） |
|---|---|---|
| 端点 | `zcode.z.ai/api/v1/zcode-plan/anthropic/v1/messages` | `api.z.ai/api/anthropic/v1/messages` |
| 鉴权 | `Bearer <zcode_jwt>`（登录直出） | `Bearer <coding_plan_key_zai ?? coding_plan_key_bigmodel>` |
| HTTP-Referer 头 | 带 | **删**（transport.ts:126） |
| key 来源 | 登录下发 | **OAuth token 三步现换**（auth.ts:638-680）：①`GET api.z.ai/api/biz/customer/getCustomerInfo` 挑「默认机构/默认项目」→ ②`GET …/projects/{proj}/api_keys` 找 `name=="zcode-api-key"`（**不是 "zcode"**，transport.ts:144）→ ③`GET …/api_keys/copy/{apiKey}` 取 `secretKey` → key = `"{apiKey}.{secret}"` |
| 承载模型 | glm-5.3-flash / glm-5.2 / glm-5-turbo | glm-5.3 / glm-5.3-flash（官方白名单仅确认两个，transport.ts:51-57） |
| 额度池 | 免费积分（**两通道额度独立**，adapter:1085） | 订阅资源包（`1113`=无可用资源包） |
| 登录后处理 | — | **顺手换 key**（auth.ts:592-613）；换不到不报错（没订阅是常态），按登录渠道写 `coding_plan_key_zai`/`coding_plan_key_bigmodel` |
| 通道选择 | 模型同属两通道时**优先**（transport.ts:86-92 resolveChannelFor） | 模型仅订阅承载、或积分腿不可用时 |

### 换腿规则（transport.ts:296-311，关键纪律）

- **只认 429 + 业务码 1005（余额不足）/ 1113（无资源包）** → 换另一条通道重试一次；
- **不认** 401/1002（换通道也是同一份凭据，救不了）、3012（风控重试会加重冷却：30min→24h→停用）、3009（并发限流走退避）；
- 换腿前确认另一通道真可用（describeChannels）；**不重产 captcha**（param 一次性，仅在无 captcha 场景换腿）。

### 余额真实形状（upstream.ts:256-330，**我们的解析是错的**）

```
GET zcode.z.ai/api/v1/zcode-plan/billing/balance
→ { code, data: { displayMode, balances: [ { plan_id, show_name,
    unit_type: "token", meter: "model_usage",
    total_units, used_units, remaining_units, available_units, expires_at } ], plans? } }
```
- `displayMode=="enterprise"` → 不下发额度数字（remaining/total=0，无 preview）；
- 剩余 = Σ(`available_units` ?? `remaining_units`)；总量 = Σ(`total_units`)；最早 expires_at；
- **单位是 token 不是积分**（实测 1 亿 tokens 量级）；
- **每日赠送不在 balances 里，只在 preview.plans**（同一账号 balances 空、preview 有 start-plan-trust-1003，只读 buckets 面板显示 0）——余额展示必须合并 preview 可领摘要。

### 我们的缺口（按优先级）

1. balance 解析重写（buckets/enterprise/单位）+ preview 可领摘要合并；
2. coding-plan 通道：key 三步现换（登录后顺手、失败吞掉）+ 端点/头切换 + 凭据字段扩展（zai/bigmodel_access_token、coding_plan_key_*）；
3. 模型→通道归属（CHANNEL_MODELS）与 resolveChannelFor；
4. 换腿（429+1005/1113，一次，带前置可用性检查）。

## 二、gemini

### 值级解锁（gemini-oauth.ts:36-66，Antigravity oauth.rs:4-5 双源一致）

- client_id `1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com`
- **client_secret 必须随 token 请求下发（缺失硬失败）**：值见参考源码 gemini-oauth.ts:37（与 Antigravity-Manager oauth.rs:5 一致）；实现时经配置/env 注入，**不硬编码进本仓库**（触发平台密钥拦截）
- 六项 scope：`openid` / `…/auth/cloud-platform` / `…/auth/userinfo.email` / `…/auth/userinfo.profile` / `…/auth/cclog` / `…/auth/experimentsandconfigs`
- redirect_uri：**`http://localhost:{port}/oauth-callback`**（localhost 非 127.0.0.1，路径逐字）；授权 URL 加 `include_granted_scopes=true`；总预算 6min；身份从 id_token 解 sub/email。

### 会硬失败的错误

1. **catalog 模型名不在上游准入表**：参考只允许 `gemini-3.8-flash(-tier)`（gemini.ts:460-554）；我们暴露 gemini-3-pro/3-flash → 补后缀后必 404。
2. **quota 请求体必须 `{"project": …}`**：空对象对部分账号 403 SUBSCRIPTION_REQUIRED（gemini-credits.ts:12-29）——与我们「固定空对象」注释直接冲突。
3. LCA 请求体应为逐字常量 `{"metadata":{"ideType":"ANTIGRAVITY"}}`；响应两级取值（顶层 → currentTier 嵌套）；固定走 sandbox 端点。
4. thoughtSignature：**只取 functionCall part 自身的字段**（不收集思考块签名）；键 = `sha256("tool:"+名 + '\0' + canonicalArgs前512字符).hex[..16]`；写入 trim、上限 2000 淘汰一半、原子写。
5. `functionResponse.response` 用 `{"content": …}`（非 result），isError 另加 error:true。
6. 信封 `userAgent` 短串 `'antigravity'`；requestId 形态 `agent/{ms}/{8hex}`。

### 行为偏差

- sessionId：lane = `'infer'`/`'smoke'` 字面量（非端点 host）；哈希 FNV-1a 64 有符号十进制；generation 折进哈希输入；首条 user 文本只取 contents[0] 的第一个非空 text part。
- usage：`input = promptTokenCount − cachedContentTokenCount`（互斥口径防双计费）；多帧取 totalTokenCount 最大的一份；缺 thinkingConfig（includeThoughts 恒 true + 档位 budget）、toolConfig、maxOutputTokens(默认 64000)、tools 带 description + schema 白名单清洗。
- 错误处置：429 先换端点再换号（冷却 60s，最多 3 号）；401 先续期再换号（冷却 300s）；400 quota 文案先换端点；签名被拒按文案判定（含 signature / thought+invalid）。
- 兜底 project `aicode-consumers` 不进缓存（免费账号不会被钉死）。

## 三、commandcode

### 值级解锁（proxy.mjs）

- 别名表 4 条（:714-721）：`bash_output→shell_output`、`task_output→shell_output`、`tool_search→search_tools`、`read_multiple_files→read_file`；**只在 tools 声明做别名，消息体 toolName 不别名**。
- 指纹池（:69-108）：CPU 15 款（model 与 cores 是分离字段，派生 label 为 `{model}|{cores}`，field 名是 **`cpu`/`mem`** 而非 cpuModel/memoryGb）；时区池含 `America/Toronto`、**无** Sao_Paulo/UTC；用户名 `[dev,user,admin,coder,engineer,work]`；邮箱域 `[gmail.com,outlook.com,qq.com,163.com]`。
- MODELS 硬编码 26 款回退表（:457-494，claude/gpt/deepseek/kimi/glm/minimax/qwen/step/xiaomi/gemini 系）。

### 会硬失败的错误

1. **components 上报形状完全不同**（:170-189）：键名是 `machineIdHash/macsHashes[]/osUserHash/hostnameHash/gitEmailHash`（哈希）+ `cpuModel/timezone` 原样 + `cpuCount/memGiB` 数字 + `platform/arch/osRelease/isContainer/runtime:'cli'/collectorVersion:1`。我们 8 个全哈希字符串字段——形状级偏差，最易被行为分析识别。
2. **finish-step 事件必须当完成信号**（:808-818，含 usage 回退 totalUsage||usage）——我们不处理，会误判截断。
3. 派生 field 名不对（cpu/mem）→ 同 key 派生出与参考不同的设备。

### 行为偏差

- `pause_turn` 原样保留（不折 length）；error 状态采纳链只认 message 开头 `<NNN>` 与 statusCode；tool-call 缺 id 兜底 `call_{ms}_{index}`；toolName 查不到发空串（非省键）；assistant args 解析失败发 `{}`（非原样字符串）。
- **idle 超时（流 30s/非流 90s）+ 首字节前重试（max 2、退避 400ms×n、已写字节绝不重试）完全缺失**。
- session 应 per-key（按 apiKey 隔离），可采信客户端 session 头；cache_control/prompt_cache_key 断点透传（缓存按前缀，断点落 system 最后一块）。

## 四、qoder

### 解锁（参考 qoder*.ts）

- **WASM 文件已定位**：`deepseek-harness-codearts/src/qoder-auth-wasm.wasm`（298KB）。加密链：`generate_runtime_auth_fields(uid/token/org…)` → `qodercontext_new(machineId, '1.1.49', userInfo, clientMeta{client_type:'5',business_product:'cli',business_type:'agent',scene:'assistant'})` → `prepareInferRequest(host, payload, modelKey, 'system')` → headers **原样透传**（覆盖 Authorization 会 403 Signature invalid）+ `Accept: text/event-stream`。
- 加密端点：`api2.qoder.sh/algo/api/v2/service/pro/sse/agent_chat_generation?FetchKeys=llm_model_result&AgentId=agent_common&Encode=1`（**不是 api2-v2**）。
- 载荷：`business:{type:"agent"}` **必填**（缺失路由到故障节点）；`tools` 顶层必发（空也要发）；model_config 官方 10 字段；session_type 国际 `qodercli`/CN `qoder_work`。
- 设备码登录：nonce/PKCE/machine_id 全本地生成；授权 URL `qoder.com/device/selectAccounts?challenge&challenge_method=S256&nonce&machine_id&client_id`（clientId `e883ade2-…`）；轮询 **GET** `openapi.qoder.sh/api/v1/deviceToken/poll?nonce&verifier&challenge_method=S256`，404=继续轮询；凭据 `security_oauth_token` 与 `access_token` 双写同值 + **user_id→uid（加密推理必需）**。
- refresh：POST `/api/v1/deviceToken/refresh` body `{refresh_token, machine_id}`。
- credits 头修正：`Cosy-ClientType: '10'`；MachineToken/Type 来自 IDE `machine_token.json` 的 `{token,type}`（非 machine_id）。

### 行为偏差

- 信封内层解析失败应判**业务错误**（我们判心跳）；error 判据补 message/statusCodeValue 单现。
- 推理错误分型：10605 排队（两形态）、409 duplicate 重发一次、105/401 才 refresh（每请求一次）、110 不可重试。

## 五、trae

### 遗漏清单

1. **ExchangeToken 续期**：POST `api.trae.com.cn/cloudide/api/v3/trae/oauth/ExchangeToken`，body `{ClientID:"en1oxy7wnw8j9n", RefreshToken, ClientSecret:"-", UserID:""}`；refreshToken 轮换立即回写；expires_at 归一毫秒字符串；machine_id/device_id/uid/nickname 不动。
2. **模型目录**：POST `{agentHost}/api/ide/v1/batch_get_detail_param`（非流式头），functions 传全部 22 个；白名单过滤 + 空档位不覆盖 + `usage=="chat_completion"`/`config_switch`/`is_invisible_to_user` 三过滤；context_window 取 `dev`；`display_contact_config` 需二次 parse 取倍率；30s TTL 缓存（空结果也缓存）。
3. **签到**：`/trae/api/v2/ug/checkin_credits/status|claim`（body `{}`）；完整签到头（设备身份按 uid 派生：X-Device-Id 15 位数字=sha256("devid:{uid}"++counter)取模、X-Market-User-ID 派生 UUID、Vscode-Sessionid 派生 64hex、X-Lscbd-Aid:787976 等）；claim 响应无积分数需补查 status；9074 冷却 300s 不换设备。
4. **空响应重试**：零可解析事件（含 metadata）→ 同账号重发一次；已收到任何事件绝不重放。
5. **余额形状待 wire 核对**：参考读 `user_entitlement_pack_list[].entitlement_base_info.quota.credits_limit` + `usage.credits_amount` + 秒级 expire_time（×1000），与我们 `data[].credits_limit` 顶层形状不一致——两种必有一错。
6. 杂项：去掉输入消息的 `reasoning_content` 键（参考不写）；system 消息确认未被丢；Max 模式字段成套下发；4001 语义为模型不可调用。

## 六、修复路线建议（优先级）

1. **P0 硬失败修复**：zcode balance 解析；gemini OAuth（secret/redirect_uri/scope/client_id）+ catalog 准入名 + quota 带 project；commandcode components 形状 + finish-step；trae balance 形状 wire 核对。
2. **P1 通道/续期**：zcode coding-plan 双通道+换腿；trae ExchangeToken；qoder 设备码登录+refresh。
3. **P1 推理补全**：gemini thinkingConfig/toolConfig/usage 口径/thoughtSignature 键格式；commandcode 指纹池逐字+派生 field 名+别名表；qoder WASM 接线（wasmtime）。
4. **P2 健壮性**：各家 idle 超时/重试/账号轮换（网关编排层统一承接）；trae 模型目录+签到；zcode preview 摘要。
