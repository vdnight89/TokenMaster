# 前端采用 React + antd

GUI 层用 React 19 + antd（配合 vite 构建），不沿用 zcode-pool 的无框架 Vanilla JS 路线。理由：TokenMaster 的界面规模（15 家 Provider 的账号池、用量账本、网关设置、日志）远超 zcode-pool 的三页结构，Antigravity-Manager 已用同栈实现过同类复杂界面，其账号池/网关面板的信息架构可直接参考；antd 的表格/表单/抽屉组件能显著降低手写成本。zcode-pool 的暗色设计变量、i18n 词条结构、页面交互流程仍作为设计参考。

## 修订（2026-10-06，Round 02 之后）

用户在 `design/round-02/` 交付了完整 UI 原型（AM 式暗色控制台，7 页 + 11 类手绘 SVG 图表 + 32KB 自包含 CSS 设计系统），并被指定为实现基准。据此修订：**React 19 + vite 保留；放弃 antd，直接以原型 CSS 为设计系统、组件全部按原型手写**（11 类图表为纯 SVG 组件，antd 组件反而与原型视觉冲突）。双语仍沿用「平铺词条表 + t()」模式。

## Consequences

- 包体比 Vanilla 路线大（React runtime ~140KB gzip 级），可接受。
- 双语沿用「平铺 key-value 词条表 + t(key) 占位替换」模式，配词条完整性检查脚本（参考 zcode-pool `scripts/check-i18n-keys.mjs`）。
