# TokenMaster

Windows 桌面客户端（TokenHub）：把 15 家 AI 编程服务的账号凭据聚合为本地 **OpenAI 协议兼容网关**，配合多账号令牌池统一供给各类 AI 编程客户端使用。

> 当前状态：**设计阶段已完成**（领域术语见 [CONTEXT.md](./CONTEXT.md)，决策记录见 [docs/adr/](./docs/adr/)），实现待启动。

## 核心能力（v1 目标）

- **15 家 Provider 全量接入**：codearts、buddy、workbuddy、lobsterai、qoder、qodercn、trae、cline、loomy、raccoon、minimax、zcode、opencode、gemini（Antigravity 反代）、commandcode
- **本地网关**：单端口 + 模型名路由（裸模型名按映射表路由，`provider/model` 前缀强制指定）；OpenAI `/v1/chat/completions`（SSE 流式）+ `/v1/models` 核心，Anthropic `/v1/messages` 扩展
- **完整令牌池**：多账号轮询、429/401 自适应换号重试、失效切换、按模型限流标记、额度/用量账本
- **账号登录**：浏览器 OAuth + 本地回环回调 / 设备码轮询 / 扫码 / 手动粘贴，按各 Provider 实际协议实现
- **zcode 验证码链**：按需拉起 Tauri WebView 子窗口过码（上游 3007 要求）
- **领取与签到**：zcode 套餐领取、各 Provider 每日签到积分
- **数据导入**：本机 zcode-pool 账号目录一键导入 + 手动粘贴 token/key
- **出站代理**：全局默认 + 按 Provider + 按账号三级覆盖（http/socks5）
- **桌面体验**：中英双语、暗色主题、系统托盘、开机自启、单实例、NSIS 安装包

## 技术栈

Tauri 2（Rust：axum + reqwest + tokio）+ React 19 + antd + vite。

## 参考项目与许可

| 项目 | 用途 | 许可 |
|---|---|---|
| `deepseek-harness-codearts` | 14 家 Provider 的协议/OAuth/池化逻辑参考（TS） | MIT |
| `commandcode-proxy-master` | commandcode 协议参考（Node） | MIT |
| `zcode-pool` | GUI 信息架构、ZCode 凭据加密兼容、i18n 方案参考 | MIT |
| `Antigravity-Manager-main` | **Rust 代码直接复用**（令牌池/OAuth/重试换号等模块） | CC-BY-NC-SA-4.0 |

因直接复用了 Antigravity-Manager 的代码，本项目含其衍生代码的部分遵循 **CC-BY-NC-SA-4.0**（详见 [LICENSE](./LICENSE) 与 [ADR-0002](./docs/adr/0002-reuse-antigravity-manager-code.md)）。

## 开发环境

- Rust 工具链（rustup，含 `x86_64-pc-windows-msvc` target）— *当前机器尚未安装，实现第一步需安装*
- Node.js ≥ 22 与 pnpm（本机已具备）
- 构建：`pnpm tauri build` 产出 NSIS 安装包

## 仓库结构

```
CONTEXT.md          # 领域术语表（glossary）
docs/adr/           # 架构决策记录
LICENSE             # 许可声明
```
