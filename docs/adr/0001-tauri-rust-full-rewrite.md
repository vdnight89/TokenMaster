# 技术栈：Tauri 2 + Rust 全量实现，不做 TS sidecar

TokenMaster 的桌面壳与 provider 适配层全部用 Rust 实现（Tauri 2 + axum + reqwest + tokio），不采用「Tauri 壳 + Node/TS sidecar 内嵌 deepseek-harness-codearts」方案。理由：单进程单语言、分发体积小、内存占用低，且 Antigravity-Manager（同为 Tauri 2 + Rust）已验证该路线可行并提供了大量可借鉴的 Rust 实现；deepseek-harness-codearts 的 TypeScript 适配器降级为协议逻辑参考（endpoint、鉴权、SSE 细节以它为准逐家核对）。

## Considered Options

- Tauri 壳 + TS 适配层 sidecar：复用度最高，但双运行时、进程管理复杂、包体大，放弃。
- Electron + Node：集成最直接，但安装包与内存占用大，放弃。

## Consequences

- provider 协议逻辑需要从 TS 翻译为 Rust，v1 工作量主要在此；协议细节以 deepseek-harness-codearts 的实现与单测为对照基准。
- 界面参考 zcode-pool 的功能与布局设计，前端技术选型在实现阶段确定（Tauri 前端可自由选框架）。
