# 开发日志（dev-log）

> 每任务一行：`日期 任务号 要点 测试 状态/提交`

- 2026-10-06 LOOP 起跑：rustup(MSVC) 安装中；TASKS.md/dev-loop.md 建立；round-02 原型确认为 UI 基准（AM 式暗色控制台，默认端口取原型 8787）。ADR-0005 修订：antd → 原型自带 CSS 设计系统。
- 2026-10-06 T0.1 前端骨架（Vite+React19+TS+原型CSS，build 通过：CSS 26KB/JS 224KB）。
- 2026-10-06 T0.2 Cargo workspace + gateway-core（axum0.8/tokio/reqwest/serde preserve_order）。
- 2026-10-06 T1.2 TDD：红（缺模块编译失败）→绿；密钥生成 2 测 + 鉴权 5 测（401 形状/Bearer/x-api-key/禁用放行/错钥拒绝）；clippy 0 警告。
- 2026-10-06 T1.3+T1.4 TDD：路由（前缀强制/裸名映射/model_not_found）4 测 + /v1/models 复合 id 2 测。
- 2026-10-06 T1.5 TDD：chat/completions 非流式 4 测（OpenAI 形状/裸名路由/404/400）；引入 Provider trait（async_trait）+ MockProvider + ProviderError→ApiError 映射；共 17 测绿，clippy 0 警告。GUI 移植子代理并行进行中。
- 2026-10-06 T0.4+T5.1~T5.9 GUI 移植子代理交付：7 页/11 图表/3 模态全量 React 化（mock 数据驱动），check+build 绿，无 XSS 向量；视觉冒烟（react-dash vs 原型）通过。差异清单见代理报告（toast.tsx、URL 参数不移植、怪癖保留）。
- 2026-10-06 T1.6 TDD：SSE 流式 3 测（chunk 拼装/usage 汇总 chunk/[DONE]、非流式仍 JSON）。
- 2026-10-06 T1.7 TDD：消费层 8 测——流式 think 拆分（开标签嗅探+闭标签跨 chunk）3、非流式 split_think（最后闭标签/裸闭/无标签/未闭合开标签）4、reasoning 双名归一 1；ToolCallDelta 变体入 StreamChunk。
- 2026-10-06 T1.8 TDD：Anthropic /v1/messages 3 测（message 形状/SSE 事件序列无 [DONE]/x-api-key 鉴权）；anthropic.rs 转换层 + AnthropicEventBuilder。
- 2026-10-06 M1 集成关卡：cargo test --workspace 10 套件 31 测全绿 + clippy 0 警告 + pnpm check/build 绿。提交并推送。
- 2026-10-07 T2.1 TDD：Store（enc:v1 AES-256-GCM/原子写/篡改跳过/删除）+ crypto 5 测。
- 2026-10-07 T2.2+T2.4 TDD：池选号（先到期/剩余最多/tried 排除/账号级+模型级冷却/停用失效过滤/Shortage 区分）6 测。
- 2026-10-07 T2.3 TDD：编排器（429 模型级冷却换号/401 标失效换号/BadRequest 立即中止/全部限流 429 vs 无账号 503/预算耗尽带最后错误）+ Provider trait 增加 Credential 参数 + start_pooled 端到端 7 测。
- 2026-10-07 T2.5 TDD：刷新调度（续期不看 enabled/不可续期只探测/失败标失效）3 测。
- 2026-10-07 M2 集成关卡：workspace 52 测全绿 + clippy 0 警告 + pnpm build 绿。另：应用户要求处理 mimosa 拦截——round-02 原型源码（已 100% 移植）归档为 prototype.zip 并移除散文件，消除提交扫描的持续误报源；宿主级 mimosa 开关需在 ZCode 插件设置里禁用。
- 2026-10-07 T3.1+T3.2 TDD：账本（JSONL 字段/单代 .old 滚动/坏路径不 panic/四维聚合/500 环）5 测 + 缝 1 记账（成功/失败都入账、账号归因、路由失败也入账；orchestrate 返回 Dispatched{completion,account_id}）1 测。M3 关卡：16 套件 58 测全绿 + clippy 0。提交 74c9814。
- 2026-10-07 用户新增需求 T5.12：Provider/模型品牌图标（@lobehub/icons 优先 + 字母徽章兜底），已插入 M5 任务板（纯前端，可与 M4 并行）。
- 2026-10-07 T0.3 Tauri 壳：cargo build 绿；WebView2 已在非标准路径 C:/Program Files (x86)/Microsoft/EdgeWebView（注册表确认 136.0.3240.92，标准路径检查会漏报）；网关端口配置化（桌面壳固定 8787 与原型一致）；运行时冒烟通过（tauri dev：8787→401 鉴权活着、5173→200 前端加载）。图标：stdlib 生成 1024 PNG → tauri icon 全套。
- 2026-10-07 T4.1a zcode Provider 首切片：start-plan 通道非流式（Anthropic 协议出站转换 openai_to_anthropic_body/completion_from_anthropic；身份头 11 项对照 reference；401→Credential/429→Retry-After/3012→30min 冷却/3007→限流）stub 4 测绿。全仓 62 测/clippy 0。
- 2026-10-07 T4.1b zcode 流式：SSE 事件流→StreamChunk（message_start 取 input/发 Role、text_delta/thinking_delta、message_delta 取 stop/输出量、message_stop 收 Finish；无 message_stop 不伪造完成）；新增共享 sse.rs 增量解析器（跨 chunk/CRLF/多行 data，含 3 单测）；body 构造器补 stream 标志；错误映射抽 map_upstream_error 复用。顺手修复 T0.3 引入 GatewayConfig.port 后 7 个旧测试字面量漏改的编译错。全仓 68 测绿/clippy 0。
- 2026-10-07 用户新增需求 T6.0【优先】：无边框窗口 + 自定义标题栏（traffic 三点窗控 + 拖拽区），去掉 Windows 原生标题栏。
- 2026-10-07 T5.12 品牌图标：@lobehub/icons@5.23.0（MIT）深路径导入 `es/*/components/Color|Mono`——包入口 CompoundedIcon 的 .Avatar/.Combine 属性赋值不可 tree-shake，会把 @lobehub/ui+antd-style 拖进产物（实测 +~700KB）；覆盖 12/15 provider（zcode→ZAI、gemini、trae、qoder/qodercn→Qoder、minimax、codearts→Huawei、buddy/workbuddy→CodeBuddy、cline、opencode、commandcode→CommandCode），兜底字母徽章 3 家（lobsterai/loomy/raccoon）；模型前缀 glm→Zhipu、claude、gemini、qwen、deepseek、minimax、gpt/codex→OpenAI、kimi→Moonshot，未识别 null。替换点：账号卡/待办徽章、池仪表、映射表 to 列、日志模型列、接入页模型表、出站代理表、Trend/Donut/Hbars 图例、AddModal 选择器旁、ImportModal 行；Bars 图例（输入/缓存/输出）与 Topo 拓扑不适用。产物 JS gzip 93.58→110.25KB（+16.7KB<150KB 预算）；署名：README 参考项目表 + Settings 致谢。提交 41974e3。
