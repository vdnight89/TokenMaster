# 前端采用 React + antd

GUI 层用 React 19 + antd（配合 vite 构建），不沿用 zcode-pool 的无框架 Vanilla JS 路线。理由：TokenMaster 的界面规模（15 家 Provider 的账号池、用量账本、网关设置、日志）远超 zcode-pool 的三页结构，Antigravity-Manager 已用同栈实现过同类复杂界面，其账号池/网关面板的信息架构可直接参考；antd 的表格/表单/抽屉组件能显著降低手写成本。zcode-pool 的暗色设计变量、i18n 词条结构、页面交互流程仍作为设计参考。

## Consequences

- 包体比 Vanilla 路线大（React runtime ~140KB gzip 级），可接受。
- 双语沿用「平铺 key-value 词条表 + t(key) 占位替换」模式，配词条完整性检查脚本（参考 zcode-pool `scripts/check-i18n-keys.mjs`）。
