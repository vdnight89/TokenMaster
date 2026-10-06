# v1 功能边界：明确不做的事

与参考项目能力对照后划定的 v1 边界（用户逐项确认）：

- **不做** auto 跨池智能路由（deepseek-harness 的 `jet-hub-auto/auto` 虚拟模型）——仅按模型名/Provider 前缀路由。
- **不做** zcode-pool 的邮箱池、批量自动注册（Outlook 收信 + 注入驱动的自动注册链）。
- **移植**领取（zcode 套餐领取）与签到（各 Provider 每日签到积分）功能。
- **不做** Web 管理台（zcode-pool 由网关吐 proxy.html 的做法不采用），管理界面只在桌面 GUI 内。
- **不做**客户端配置一键写入（AM 的 cli_sync），GUI 仅展示接入说明（base_url + 网关密钥的配置示例）。
- **不做**无头/CLI/服务化运行（见 ADR-0004），**不做**多虚拟 key 分发（见 ADR-0003）。
- 数据导入仅覆盖：本机 zcode-pool 账号目录一键导入 + 各 Provider 手动粘贴 token/key；不兼容 Antigravity-Manager / dsh 的数据格式。

## Consequences

- 这些「不做」均为可后续追加的功能（架构上不封死），追加时在本文件追加 superseded 记录而非删除条目。
- 账号获取走各 Provider 的正常登录流程（浏览器 OAuth / 设备码 / 扫码 / 手动粘贴），产品定位是凭据池网关而非账号工厂。
