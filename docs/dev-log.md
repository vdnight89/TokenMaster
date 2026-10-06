# 开发日志（dev-log）

> 每任务一行：`日期 任务号 要点 测试 状态/提交`

- 2026-10-06 LOOP 起跑：rustup(MSVC) 安装中；TASKS.md/dev-loop.md 建立；round-02 原型确认为 UI 基准（AM 式暗色控制台，默认端口取原型 8787）。ADR-0005 修订：antd → 原型自带 CSS 设计系统。
- 2026-10-06 T0.1 前端骨架（Vite+React19+TS+原型CSS，build 通过：CSS 26KB/JS 224KB）。
- 2026-10-06 T0.2 Cargo workspace + gateway-core（axum0.8/tokio/reqwest/serde preserve_order）。
- 2026-10-06 T1.2 TDD：红（缺模块编译失败）→绿；密钥生成 2 测 + 鉴权 5 测（401 形状/Bearer/x-api-key/禁用放行/错钥拒绝）；clippy 0 警告。
- 2026-10-06 T1.3+T1.4 TDD：路由（前缀强制/裸名映射/model_not_found）4 测 + /v1/models 复合 id 2 测。
- 2026-10-06 T1.5 TDD：chat/completions 非流式 4 测（OpenAI 形状/裸名路由/404/400）；引入 Provider trait（async_trait）+ MockProvider + ProviderError→ApiError 映射；共 17 测绿，clippy 0 警告。GUI 移植子代理并行进行中。
