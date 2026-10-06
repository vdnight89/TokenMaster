# TokenMaster

TokenMaster 是一个 Windows 桌面客户端：把多家 AI 编程服务（Provider）的账号凭据聚合成本地 OpenAI 协议兼容网关，供各类 AI 编程客户端统一接入。

## Language

### 上游与账号

**Provider（上游服务）**：
一家提供 AI 编程模型额度/订阅的服务（如 zcode、gemini、trae、qoder 等 15 家），TokenMaster 从它获取模型能力。
_Avoid_: 平台、渠道（作此义时）、vendor

**账号（Account）**：
用户在一家 Provider 处的一份可独立使用的凭据（OAuth token、API key 或会话凭据）。一份凭据即一个账号，与真实自然人无关。
_Avoid_: 渠道、凭据（作整体概念时）、key（作此义时）

**令牌池（Token Pool）**：
同一家 Provider 下全部可用账号的集合，配合轮询、失效切换与用量统计对外提供服务。
_Avoid_: 账号列表、池子

### 网关

**网关（Gateway）**：
TokenMaster 在本地回环地址上暴露的 OpenAI 协议兼容 HTTP 服务，是外部 AI 编程客户端的唯一入口。
_Avoid_: 代理（作整体概念时）、服务端

**模型路由（Model Routing）**：
网关根据请求中的模型名（可带 Provider 前缀）选择目标 Provider 与令牌池的规则。
_Avoid_: 转发、分发

**网关密钥（Gateway Key）**：
调用网关所需的本地 API key（sk- 前缀），由 GUI 生成与管理，防止局域网内误用。
_Avoid_: API key（泛指上游的 key 时）、令牌

**接入说明（Setup Guide）**：
GUI 中展示给用户的、把外部客户端指向本地网关所需的配置示例（base_url + 网关密钥）。TokenMaster 不代写客户端配置文件。
_Avoid_: 一键同步、cli_sync

### 账号运营

**领取（Claim）**：
从 Provider 上游主动领取可用套餐/额度的操作（目前仅 zcode 有此流程）。
_Avoid_: mint、套餐领取（冗余）

**签到（Check-in）**：
Provider 的每日签到积分操作（codearts、buddy 系等多家支持）。

**验证码载体（Captcha Carrier）**：
为满足上游验证码要求而拉起的浏览器过码环境（Tauri WebView 子窗口），仅在 zcode 上游要求验证码时启用。
_Avoid_: 打码、chromium

**导入（Import）**：
把外部来源（本机 zcode-pool 账号目录、手动粘贴的 token/key）转换为 TokenMaster 账号的过程。
_Avoid_: 迁移、同步

**出站代理（Outbound Proxy）**：
TokenMaster 访问上游 Provider 时使用的 http/socks5 代理；支持全局默认、按 Provider、按账号三级覆盖。
_Avoid_: 认证代理、relay proxy

### 提供范围

**v1 Provider 清单**：
codearts、buddy、workbuddy、lobsterai、qoder、qodercn、trae、cline、loomy、raccoon、minimax、zcode、opencode、gemini（含 Antigravity 反代）、commandcode。
