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
