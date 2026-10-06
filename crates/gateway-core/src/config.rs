//! 网关配置。默认鉴权为禁用模式——GUI 首次启动会生成密钥并设为
//! Required；禁用仅是用户显式选择（spec：网关对外契约）。

#[derive(Clone, Debug)]
pub struct GatewayConfig {
    /// 网关密钥鉴权模式。
    pub auth: AuthMode,
    /// 模型名路由表（裸模型名 → "provider/model"）。
    pub model_map: Vec<(String, String)>,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            auth: AuthMode::Disabled,
            model_map: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum AuthMode {
    /// 放行所有请求（用户显式禁用密钥时）。
    Disabled,
    /// 必须携带 `Authorization: Bearer <key>` 或 `x-api-key: <key>`。
    Required(String),
}
