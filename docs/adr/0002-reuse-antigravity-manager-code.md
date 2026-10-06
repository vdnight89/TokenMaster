# 直接拷贝 Antigravity-Manager 代码及其许可影响

四个参考项目中，zcode-pool、deepseek-harness-codearts、commandcode-proxy 均为 MIT，可自由复用；Antigravity-Manager（AM）为 CC-BY-NC-SA-4.0（署名-非商业-相同方式共享）。用户决定：**直接拷贝 AM 的 Rust 模块**（令牌池 token_manager、OAuth 回调 oauth/oauth_server、自适应重试换号、模型映射等），不回避代码级复用。

## Consequences

- TokenMaster 中包含 AM 衍生代码的部分受 CC-BY-NC-SA-4.0 约束：仓库推送到 gitee 即构成分发，衍生部分须以同许可提供，且**不得用于商业目的**。
- 建议：仓库根目录 LICENSE 采用 CC-BY-NC-SA-4.0（与上游保持一致最简单），README 注明来源项目与署名；仅从 MIT 项目复用的模块可另行标注。
- 纯粹参考协议逻辑而自行编写的代码不构成衍生作品，但为避免争议，与 AM 高度相似的模块一律视为衍生并按上述许可处理。
