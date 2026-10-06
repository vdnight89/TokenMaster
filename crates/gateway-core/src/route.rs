//! 模型名路由：`provider/model` 前缀强制指定，裸模型名按映射表（有序，先匹配优先）。

use crate::error::ApiError;

#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    pub provider: String,
    pub model: String,
}

impl Route {
    /// 复合 id：`provider/model`（与 /v1/models 的 id 形态一致）。
    pub fn composite(&self) -> String {
        format!("{}/{}", self.provider, self.model)
    }
}

/// 解析请求里的模型名。
///
/// - 含 `/`：按前缀强制指定（provider 与 model 均非空）。
/// - 不含 `/`：查映射表，第一条命中生效（GUI 按优先级排序维护）。
pub fn resolve_model(input: &str, map: &[(String, String)]) -> Result<Route, ApiError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(ApiError::ModelNotFound);
    }
    if let Some((provider, model)) = input.split_once('/') {
        if provider.is_empty() || model.is_empty() {
            return Err(ApiError::ModelNotFound);
        }
        return Ok(Route { provider: provider.to_string(), model: model.to_string() });
    }
    for (bare, target) in map {
        if bare == input {
            let (provider, model) = target.split_once('/').ok_or(ApiError::ModelNotFound)?;
            return Ok(Route { provider: provider.to_string(), model: model.to_string() });
        }
    }
    Err(ApiError::ModelNotFound)
}
