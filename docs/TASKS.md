# TokenMaster v1 任务板

> 循环协议见 [dev-loop.md](./dev-loop.md)，执行历史见 [dev-log.md](./dev-log.md)。
> 状态：`todo` → `doing` → `review` → `done`；异常标 `blocked(原因)`。
> 领任务规则：取**最靠前**且依赖全部 `done` 的 `todo` 项；一次只做一个任务。

## M0 脚手架

| # | 任务 | 缝 | 状态 | 依赖 |
|---|---|---|---|---|
| T0.1 | Vite + React 19 + TS 前端骨架，原型 CSS/图标/格式化工具迁入，`pnpm build` 通过 | — | done | — |
| T0.2 | Cargo workspace：`crates/gateway-core`（纯库）+ `src-tauri`（壳）骨架，`cargo test --workspace` 空passes | — | done(壳待T0.3) | rustup |
| T0.3 | Tauri 2 壳接通：窗口起得来、前端加载、IPC ping/pong | — | todo | T0.1, T0.2 |
| T0.4 | 前端 mock 数据层（移植原型 DATA：15 provider/42 账号/用量/日志），供 GUI 先行开发 | — | doing(agent) | T0.1 |

## M1 网关核心 · OpenAI/Anthropic 面（缝 1：HTTP 黑盒）

| # | 任务 | 缝 | 状态 | 依赖 |
|---|---|---|---|---|
| T1.1 | 领域类型：ChatRequest/Message/ToolCall/Usage/StreamChunk + serde | 1 | todo | T0.2 |
| T1.2 | 网关密钥：生成(sk-tm-)、持久化、鉴权中间件（Bearer；禁用模式；回环放行策略可配） | 1 | done(持久化待T2.1) | T0.2 |
| T1.3 | 模型路由：`provider/model` 前缀强制 + 裸名映射表（可增删排序） | 1 | todo | T1.1 |
| T1.4 | `GET /v1/models`：聚合各 provider 模型目录（provider/model 复合 id） | 1 | todo | T1.3 |
| T1.5 | `POST /v1/chat/completions` 非流式（先接 mock provider 打通） | 1 | todo | T1.2, T1.3 |
| T1.6 | SSE 流式：chunk 序列、`[DONE]`、usage 统计、客户端断开中止上游 | 1 | todo | T1.5 |
| T1.7 | think 标签拆分 + reasoning 双名归一 + 工具参数分片合并（消费层） | 1 | todo | T1.6 |
| T1.8 | `POST /v1/messages`（Anthropic 面）：请求/响应/SSE 双向转换，`x-api-key` 鉴权 | 1 | todo | T1.6 |

## M2 令牌池（缝 1 观察行为）

| # | 任务 | 缝 | 状态 | 依赖 |
|---|---|---|---|---|
| T2.1 | 账号/凭据模型 + AES-256-GCM 加密落盘（原子写；`~/.tokenmaster/`） | 1 | todo | T0.2 |
| T2.2 | 选号管道：候选过滤(启用+模型未限流) → 策略(先到期优先/剩余最多，可切换) | 1 | todo | T2.1 |
| T2.3 | 失败分类：401/402/403 换号重试(上限4)；429 解析 Retry-After 记冷却再换号；超时可重试 | 1 | todo | T2.2, T1.5 |
| T2.4 | 限流状态按 Provider×模型×账号；「全部限流」与「无可用账号」返回不同错误 | 1 | todo | T2.3 |
| T2.5 | 刷新调度：30min 周期 + 启动即刷 + 入池补刷；续期不看 enabled；不可续期只探测 | 2 | todo | T2.1 |

## M3 账本（缝 1 观察行为）

| # | 任务 | 缝 | 状态 | 依赖 |
|---|---|---|---|---|
| T3.1 | 账本：内存流水 + JSONL 滚动落盘（32MB .old） | 1 | todo | T1.5 |
| T3.2 | 聚合查询：按天/Provider/账号/模型（供 GUI 的 IPC 用） | 1 | todo | T3.1 |

## M4 Provider 适配器（缝 2：stub 上游回放，默认跳过真实 e2e）

> 每家一个任务，模板：product 静态表 → 登录/刷新流 → 推理 adapter → credits/签到 → stub 协议测试。
> 顺序按参考实现充分度排：zcode → gemini → commandcode → trae → qoder(WASM) → qodercn → cline → minimax → buddy → workbuddy → lobsterai → codearts → loomy → raccoon → opencode。

| # | 任务 | 缝 | 状态 | 依赖 |
|---|---|---|---|---|
| T4.1 | zcode：双通道(anthropic)、CLI 设备码登录、伪装头、3007→验证码事件、套餐领取、积分/额度 | 2 | todo | M1, M2 |
| T4.2 | gemini：CloudCode 上游、OAuth 回调、双层信封、thoughtSignature、Antigravity 身份 | 2 | todo | M1, M2 |
| T4.3 | commandcode：私有信封、每 key 设备指纹(HMAC)、SSE 转译、user_ key 透传 | 2 | todo | M1, M2 |
| T4.4 | trae：SOLO 载荷/SSE、本地回调 18080、Cloud-IDE-JWT | 2 | todo | M1, M2 |
| T4.5 | qoder：PKCE 设备码、WASM 请求加密(wasmtime)、信封解包、双推理路径 | 2 | todo | M1, M2 |
| T4.6 | qodercn：复用 qoder，仅换端点/client_id | 2 | todo | T4.5 |
| T4.7 | cline：WorkOS 设备码、`workos:` Bearer 前缀、OpenAI 兼容 | 2 | todo | M1, M2 |
| T4.8 | minimax：设备码+PKCE、Anthropic Messages 协议、refresh 轮换 | 2 | todo | M1, M2 |
| T4.9 | buddy：external-link 轮询、X-Domain、余额临期分档 | 2 | todo | M1, M2 |
| T4.10 | workbuddy：复用 buddy，换 workbuddy.ai 配置 | 2 | todo | T4.9 |
| T4.11 | lobsterai：本地回调 OAuth、OpenAI 兼容、积分作废预警 | 2 | todo | M1, M2 |
| T4.12 | codearts：IAM OAuth+PKCE、SDK-HMAC-SHA256 签名、积分签到 | 2 | todo | M1, M2 |
| T4.13 | loomy：短信/微信扫码、HMAC-SHA1、无刷新（只探测） | 2 | todo | M1, M2 |
| T4.14 | raccoon：扫码+短信本地页、refresh 轮换、10MB 限制 | 2 | todo | M1, M2 |
| T4.15 | opencode：账号槽+匿名槽、免费全槽、每账号代理、projectId 指纹 | 2 | todo | M1, M2 |

## M5 GUI 移植（round-02 原型 → React；可与 M1-M4 并行，mock 数据先行）

| # | 任务 | 缝 | 状态 | 依赖 |
|---|---|---|---|---|
| T5.1 | 布局壳：顶栏+药丸导航+7 页路由+页面切换动效+toasts | — | todo | T0.1, T0.4 |
| T5.2 | 图表库移植（11 类 SVG 图表为 React 组件） | — | todo | T0.4 |
| T5.3 | 仪表盘页：KPI 条/待办/吞吐/池仪表/错误分布/最近请求 | — | todo | T5.1, T5.2 |
| T5.4 | 账号页：筛选/搜索/分页/账号卡/行内操作/状态卡 | — | todo | T5.1 |
| T5.5 | 添加账号模态（四类登录引导）+ zcode-pool 导入模态 + 领取(验证码载体)模态 | — | todo | T5.4 |
| T5.6 | 网关页：启停/端口/base_url/密钥卡/映射表/选号策略/拓扑图 | — | todo | T5.1, T5.2 |
| T5.7 | 用量页：分段/汇总卡/四类图表/聚合表 | — | todo | T5.2 |
| T5.8 | 日志页：过滤/监控表/行展开换号轨迹 | — | todo | T5.1 |
| T5.9 | 接入页 + 设置页（代理三级覆盖 UI） | — | todo | T5.1 |
| T5.10 | i18n 双语（zh/en 词条表 + t()，与 Rust 错误词表对齐） | — | todo | T5.3-T5.9 |
| T5.11 | IPC 接线：mock 数据逐页替换为 Tauri command 真数据 | — | todo | T0.3, M1-M3, T5.3-T5.9 |

## M6 桌面集成

| # | 任务 | 缝 | 状态 | 依赖 |
|---|---|---|---|---|
| T6.1 | 托盘（菜单/tooltip）+ 单实例 + 开机自启 + 关窗最小化 | — | todo | T0.3 |
| T6.2 | zcode-pool 导入实现（enc:v1 兼容解析，读其账号目录） | 2 | todo | T2.1, T4.1 |
| T6.3 | 验证码载体：WebView 子窗口按需拉起 + 供应池退避 | 2 | todo | T0.3, T4.1 |
| T6.4 | 签到调度（手动 + 可选自动轮询） | 2 | todo | M4 |
| T6.5 | 出站代理：全局→Provider→账号三级覆盖（http/socks5） | 2 | todo | M4 |
| T6.6 | NSIS 打包（currentUser）+ 版本一致性校验脚本 | — | todo | M5, M6 |

## M7 集成收尾

| # | 任务 | 缝 | 状态 | 依赖 |
|---|---|---|---|---|
| T7.1 | 缝 1 黑盒套件全量绿 + 缝 2 全部 stub 测试绿（集成关卡） | 1,2 | todo | M1-M4 |
| T7.2 | release profile（strip/lto）+ 冷启动/包体检查 | — | todo | T7.1 |
| T7.3 | 用户文档（README 安装/接入/FAQ 更新）+ ADR 补充 | — | todo | T7.1 |

## 集成关卡（里程碑门禁）

每完成一个里程碑（M0-M7）：`cargo test --workspace` + `cargo clippy --workspace` + `pnpm check` + `pnpm build` 四项全绿才算关；失败就地修复后重跑。M5 页面每完成一页，用 oil-ui-pro 截图工具对该页做视觉自查（对照 round-02 原型同页截图）。
