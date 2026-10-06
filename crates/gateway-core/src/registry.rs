//! Provider 模型目录注册表：/v1/models 聚合与路由校验的数据源。
//! M4 起 Provider trait 实现方填充各自的静态表 + 远端目录。

#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub id: String,
}

#[derive(Debug, Clone)]
pub struct ProviderCatalog {
    pub id: String,
    pub models: Vec<ModelInfo>,
}

#[derive(Debug, Clone, Default)]
pub struct Registry {
    pub providers: Vec<ProviderCatalog>,
}

impl Registry {
    pub fn with(catalog: ProviderCatalog) -> Self {
        Self { providers: vec![catalog] }
    }

    /// 全部 (provider_id, model_id) 复合目录。
    pub fn all_models(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for p in &self.providers {
            for m in &p.models {
                out.push((p.id.clone(), m.id.clone()));
            }
        }
        out
    }

    pub fn provider_ids(&self) -> Vec<String> {
        self.providers.iter().map(|p| p.id.clone()).collect()
    }
}
