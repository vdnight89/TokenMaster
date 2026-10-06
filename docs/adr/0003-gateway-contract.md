# 网关对外契约：单端口、模型名路由、OpenAI+Anthropic 双协议、可配置密钥

网关在本地回环地址上监听**单一端口**（默认值实现期定），不按 Provider 拆端口、不用路径前缀区分。路由依据请求中的模型名：裸模型名（如 `glm-4.7`）按内置映射表路由到可用 Provider；`provider/model` 前缀形式（如 `zcode/glm-4.7`）强制指定 Provider。

对外协议面：核心交付 OpenAI `/v1/chat/completions`（含 SSE 流式）与 `/v1/models`；扩展交付 Anthropic `/v1/messages`（让 Claude Code 等客户端直连）。是否补 OpenAI `/v1/responses` 视参考实现已有能力再定，不作为 v1 承诺。

鉴权：首次启动自动生成随机网关密钥（Gateway Key，sk- 前缀），GUI 可查看/重置/禁用；禁用时允许裸调用（本机/局域网自担风险）。

## Considered Options

- 每 Provider 一端口 / 路径前缀区分：隔离彻底但客户端配置繁琐，放弃。
- 仅 OpenAI 协议：最省力，但 Claude Code 生态（Anthropic 协议）是重要接入方，故纳入扩展。
- 多虚拟 key（按 key 绑定 Provider/限流/用量）：v1 不做，架构上留扩展位。

## Consequences

- 模型名冲突（多家 Provider 提供同名模型）由内置映射表 + Provider 前缀消歧，GUI 需提供映射表管理界面。
- Anthropic 协议面意味着需要 OpenAI↔Anthropic 的请求/响应/SSE 双向转换层（commandcode-proxy 与 AM 均有可复用实现）。
