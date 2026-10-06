//! 网关配置。默认鉴权为禁用模式——GUI 首次启动会生成密钥并设为
//! Required；禁用仅是用户显式选择（spec：网关对外契约）。

use crate::registry::Registry;

#[derive(Clone, Debug, Default)]
pub struct GatewayConfig {
    /// 监听端口；None = 随机端口（测试用）。
    pub port: Option<u16>,
    /// 网关密钥鉴权模式。
    pub auth: AuthMode,
    /// 模型名路由表（裸模型名 → "provider/model"，按优先级排序）。
    pub model_map: Vec<(String, String)>,
    /// Provider 模型目录注册表。
    pub registry: Registry,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum AuthMode {
    /// 放行所有请求（用户显式禁用密钥时）。
    #[default]
    Disabled,
    /// 必须携带 `Authorization: Bearer <key>` 或 `x-api-key: <key>`。
    Required(String),
}
