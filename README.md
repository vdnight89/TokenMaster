<p align="center">
  <img src="./assets/readme/hero.svg" width="100%" alt="TokenMaster：把 15 家 AI 编程服务的账号聚合为本地 OpenAI 协议兼容网关 —— AI 编程客户端经网关密钥接入，网关按模型名路由到各 Provider 的令牌池，失败自动换号、限流自动冷却">
</p>

<p align="center">
  <img src="https://img.shields.io/badge/%E7%8A%B6%E6%80%81-%E6%A0%B8%E5%BF%83%E5%AE%9E%E7%8E%B0%E5%AE%8C%E6%88%90%C2%B7299%20%E6%B5%8B%E8%AF%95%E5%85%A8%E7%BB%BF-2ea043?style=flat-square" alt="状态：核心实现完成·299 测试全绿">
  <img src="https://img.shields.io/badge/%E5%B9%B3%E5%8F%B0-Windows%2010%2F11-58a6ff?style=flat-square" alt="平台：Windows 10/11">
  <img src="https://img.shields.io/badge/%E6%8A%80%E6%9C%AF%E6%A0%88-Tauri%202%20%C2%B7%20Rust%20%C2%B7%20React%2019-d29922?style=flat-square" alt="技术栈：Tauri 2 · Rust · React 19">
  <img src="https://img.shields.io/badge/%E8%AE%B8%E5%8F%AF-CC--BY--NC--SA--4.0-f85149?style=flat-square" alt="许可：CC-BY-NC-SA-4.0">
</p>

## 这是什么

如果你在多家 AI 编程服务上都有账号和订阅额度，就会遇到同样的麻烦：每家额度只能锁在它自家的客户端或 IDE 插件里用，协议互不兼容；多账号要手动切换；额度分散在各家后台，临期作废无人提醒。

TokenMaster 把这些问题收敛进一个 Windows 桌面应用：在本机回环地址运行一个 **OpenAI 协议兼容网关**（附 Anthropic 协议扩展面），把 15 家 Provider 的账号聚合成**令牌池**。任何支持自定义 `base_url` 的 AI 编程客户端——Claude Code、Cline、Codex CLI 等——只需把地址指向网关、配上一把 `sk-` 网关密钥，就能用统一的模型名消费全部额度。

## 工作原理

一次请求的生命周期：

1. **路由**：客户端按 OpenAI 或 Anthropic 协议把请求发给本地网关；模型名决定去向——裸模型名按映射表走默认 Provider，`provider/model` 前缀强制指定。
2. **选号**：网关从目标 Provider 的令牌池选一个账号（先到期优先 / 剩余额度最多，可切换；有积分有效期的 Provider 叠加余额临期分档），把请求翻译成该家上游的私有协议。
3. **兜底**：401/402/403 自动换号重试；429 解析 `Retry-After` 冷却该账号再换号；限流按「Provider × 模型 × 账号」粒度记录，一个模型的限流不拖累整个账号。
4. **记账**：每次请求记入本地账本（token 用量、缓存命中、耗时、TTFB），在 GUI 里按天 / Provider / 账号 / 模型聚合查看。

## v1 核心能力

- **15 家 Provider 全量接入**：codearts、buddy、workbuddy、lobsterai、qoder、qodercn、trae、cline、loomy、raccoon、minimax、zcode、opencode、gemini（Antigravity 反代）、commandcode
- **双协议网关**：OpenAI `/v1/chat/completions`（SSE 流式）+ `/v1/models`，Anthropic `/v1/messages`；单端口、模型名路由、`sk-` 网关密钥防局域网误用
- **完整令牌池**：多账号轮询、失效切换、按模型限流冷却、双选号策略、额度/用量账本
- **多方式登录**：浏览器 OAuth 本地回环回调 / 设备码轮询 / 扫码短信 / 手动粘贴，凭据到期前自动刷新
- **额度运营**：zcode 套餐领取（上游要求验证码时自动拉起 WebView 过码）、各 Provider 每日签到积分
- **数据导入**：本机 zcode-pool 账号目录一键导入（加密格式兼容）+ 手动粘贴 token/key
- **出站代理**：全局默认 → 按 Provider → 按账号三级覆盖（http/socks5）
- **桌面体验**：中英双语、暗色主题、系统托盘常驻、开机自启、单实例、NSIS 安装包

## v1 明确不做

边界依据 [ADR-0007](./docs/adr/0007-v1-scope-boundaries.md)：auto 跨池智能路由、邮箱池/批量注册、Web 管理台、无头/CLI/docker 运行、多虚拟 key、客户端配置一键写入、自动更新、数据导出备份、`/v1/responses` 对外面、非 Windows 平台承诺。

## 当前状态

**核心实现已完成**——15 家 Provider 适配器、双协议网关、令牌池/换号/限流/账本、签到调度、代理三级覆盖、zcode-pool 导入、i18n 双语、托盘/单实例/自关最小化全部落地。当前 **299 个测试全绿**、clippy 0 警告、TypeScript 严格模式通过。

### 已完成

| 里程碑 | 内容 | 状态 |
|---|---|---|
| M0 脚手架 | Tauri 2 + React 19 + TS 严格模式 + Rust workspace | ✅ |
| M1 网关核心 | OpenAI + Anthropic 双协议、SSE 流式、模型路由 | ✅ |
| M2 令牌池 | 多账号选号、失败换号、按模型限流冷却 | ✅ |
| M3 账本 | JSONL 流水、四维聚合 | ✅ |
| M4 Provider | **15 家全量**适配器 + 86 项对照修复 | ✅ |
| M5 GUI | 7 页 + 11 类图表 + 品牌图标 + i18n 双语 | ✅（IPC 接线待做） |
| M6 桌面 | 托盘/单实例/自启/关窗最小化 + 导入/签到/代理 | ✅ |
| M7 收尾 | 集成关卡 4 项全绿 + release 3.6MB | ✅ |

### 待完成

- T5.11 IPC 接线（mock 数据 → Tauri command 真数据）
- T6.3 验证码载体（WebView 子窗口）
- T6.6 NSIS 安装包

## 快速开始

### 开发环境

Lockfile is up to date, resolution step is skipped
Already up to date

╭ Warning ─────────────────────────────────────────────────────────────────────╮
│                                                                              │
│   Ignored build scripts: esbuild@0.28.2.                                     │
│   Run "pnpm approve-builds" to pick which dependencies should be allowed     │
│   to run scripts.                                                            │
│                                                                              │
╰──────────────────────────────────────────────────────────────────────────────╯
Done in 549ms using pnpm v10.33.0

> tokenmaster@0.1.0 check E:\Project\TokenHub\TokenMaster
> tsc -b --noEmit


> tokenmaster@0.1.0 build E:\Project\TokenHub\TokenMaster
> tsc -b && vite build

[36mvite v7.3.7 [32mbuilding client environment for production...[36m[39m
transforming...
[32m✓[39m 317 modules transformed.
rendering chunks...
computing gzip size...
[2mdist/[22m[32mindex.html                 [39m[1m[2m  0.41 kB[22m[1m[22m[2m │ gzip:   0.28 kB[22m
[2mdist/[22m[35massets/index-DoLojfuq.css  [39m[1m[2m 26.94 kB[22m[1m[22m[2m │ gzip:   6.15 kB[22m
[2mdist/[22m[36massets/window-D0x3tz3C.js  [39m[1m[2m 15.42 kB[22m[1m[22m[2m │ gzip:   3.95 kB[22m
[2mdist/[22m[36massets/index-h5a8Jlpq.js   [39m[1m[2m381.06 kB[22m[1m[22m[2m │ gzip: 113.67 kB[22m
[32m✓ built in 1.60s[39m

### 从源码构建安装包



### 作为 AI 编程客户端的网关使用

1. 启动 TokenMaster，在**网关**页获取  和  密钥
2. 在客户端（Claude Code / Cline / Codex CLI 等）配置：
   
3. 模型名格式：（如 ）或裸名走映射表

### 导入 zcode-pool 账号

在**账号**页点击「导入 zcode-pool」，自动读取  下的加密账号文件（enc:v1 兼容解析）。

## 文档地图

| 文档 | 内容 |
|---|---|
| [CONTEXT.md](./CONTEXT.md) | 领域术语表：Provider、账号、令牌池、网关、模型路由、领取、签到…… |
| [docs/spec/v1.md](./docs/spec/v1.md) | v1 实施规格：35 条用户故事、协议转换决策、选号管道、测试缝设计（ready-for-agent） |
| [docs/adr/0001](./docs/adr/0001-tauri-rust-full-rewrite.md) | 技术栈：Tauri 2 + Rust 全量实现，不做 TS sidecar |
| [docs/adr/0002](./docs/adr/0002-reuse-antigravity-manager-code.md) | 直接拷贝 Antigravity-Manager 代码及其许可影响 |
| [docs/adr/0003](./docs/adr/0003-gateway-contract.md) | 网关对外契约：单端口、模型名路由、OpenAI+Anthropic 双协议、可配置密钥 |
| [docs/adr/0004](./docs/adr/0004-desktop-embedded-gateway.md) | 网关仅桌面内嵌运行，不提供无头模式 |
| [docs/adr/0005](./docs/adr/0005-frontend-react-antd.md) | 前端采用 React + antd |
| [docs/adr/0006](./docs/adr/0006-zcode-captcha-webview-carrier.md) | zcode 验证码链：Tauri WebView 子窗口作过码载体 |
| [docs/adr/0007](./docs/adr/0007-v1-scope-boundaries.md) | v1 功能边界：明确不做的事 |
| [docs/reference/](./docs/reference/) | 4 份参考项目解读手册（code graph + 复用映射） |

## 参考项目与许可

| 项目 | 用途 | 许可 |
|---|---|---|
| `deepseek-harness-codearts` | 14 家 Provider 的协议/OAuth/池化逻辑参考（TS） | MIT |
| `commandcode-proxy-master` | commandcode 协议参考（Node） | MIT |
| `zcode-pool` | GUI 信息架构、ZCode 凭据加密兼容、i18n 方案参考 | MIT |
| `Antigravity-Manager-main` | **Rust 代码直接复用**（令牌池/OAuth/重试换号等模块） | CC-BY-NC-SA-4.0 |
| `@lobehub/icons` | GUI 的 Provider / 模型品牌图标（内联 SVG 子集） | MIT |

因直接复用了 Antigravity-Manager 的代码，本项目含其衍生代码的部分遵循 **CC-BY-NC-SA-4.0**（详见 [LICENSE](./LICENSE) 与 [ADR-0002](./docs/adr/0002-reuse-antigravity-manager-code.md)）。品牌图标来自 LobeHub Icons（MIT，Copyright © 2023 LobeHub；各图标商标归其各自所有者）。
