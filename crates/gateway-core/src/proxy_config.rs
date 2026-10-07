//! T6.5 出站代理：全局 → Provider → 账号三级覆盖（http/socks5）。
//!
//! 优先级：账号级 > Provider 级 > 全局（最高到最低查找）。
//! 值为 URL 形态：`http://host:port`、`socks5://host:port`。
//! 空串 = 显式禁用（覆盖上级为「不用代理」）。

use serde_json::Value;

/// 三级代理配置。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProxyConfig {
    /// 全局默认（最低优先级）。
    pub global: Option<String>,
    /// Provider 级覆盖。
    pub provider: Option<String>,
    /// 账号级覆盖（最高优先级）。
    pub account: Option<String>,
}

impl ProxyConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn global(mut self, url: &str) -> Self {
        self.global = Some(url.to_string());
        self
    }

    pub fn provider(mut self, url: &str) -> Self {
        self.provider = Some(url.to_string());
        self
    }

    pub fn account(mut self, url: &str) -> Self {
        self.account = Some(url.to_string());
        self
    }

    /// 解析生效代理：账号 > Provider > 全局。
    /// `Some("")` = 显式禁用（返回 None = 直连）。
    pub fn resolve(&self) -> Option<String> {
        let first = self
            .account
            .as_ref()
            .or(self.provider.as_ref())
            .or(self.global.as_ref());
        match first {
            Some(url) if url.is_empty() => None, // 显式禁用
            Some(url) => Some(url.clone()),
            None => None, // 全部未配置
        }
    }

    /// 从凭据 JSON 读取账号级代理。
    pub fn from_credential(cred_secret: &str) -> Option<String> {
        let v: Value = serde_json::from_str(cred_secret).ok()?;
        v.get("proxy_url")
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// 构造 reqwest Client 的代理（返回 None = 直连）。
pub fn apply_proxy(builder: reqwest::ClientBuilder, proxy_url: Option<&str>) -> Result<reqwest::ClientBuilder, String> {
    if let Some(url) = proxy_url {
        let proxy = reqwest::Proxy::all(url).map_err(|e| format!("代理 URL 无效 {url}: {e}"))?;
        Ok(builder.proxy(proxy))
    } else {
        Ok(builder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_level_priority() {
        // 只有全局
        let pc = ProxyConfig::new().global("http://g:1");
        assert_eq!(pc.resolve(), Some("http://g:1".to_string()));

        // Provider 覆盖全局
        let pc = ProxyConfig::new().global("http://g:1").provider("http://p:2");
        assert_eq!(pc.resolve(), Some("http://p:2".to_string()));

        // 账号覆盖一切
        let pc = ProxyConfig::new().global("http://g:1").provider("http://p:2").account("http://a:3");
        assert_eq!(pc.resolve(), Some("http://a:3".to_string()));

        // 只有账号
        let pc = ProxyConfig::new().account("socks5://a:3");
        assert_eq!(pc.resolve(), Some("socks5://a:3".to_string()));
    }

    #[test]
    fn empty_string_disables() {
        // 账号级空串显式禁用（覆盖 Provider 与全局）
        let pc = ProxyConfig::new().global("http://g:1").provider("http://p:2").account("");
        assert_eq!(pc.resolve(), None);

        // Provider 级空串覆盖全局
        let pc = ProxyConfig::new().global("http://g:1").provider("");
        assert_eq!(pc.resolve(), None);
    }

    #[test]
    fn no_config_means_direct() {
        let pc = ProxyConfig::new();
        assert_eq!(pc.resolve(), None);
    }

    #[test]
    fn credential_proxy_read() {
        let secret = r#"{"access_token":"t","proxy_url":"socks5://127.0.0.1:1080"}"#;
        assert_eq!(ProxyConfig::from_credential(secret), Some("socks5://127.0.0.1:1080".to_string()));
        // 无字段
        let secret2 = r#"{"access_token":"t"}"#;
        assert_eq!(ProxyConfig::from_credential(secret2), None);
    }
}
