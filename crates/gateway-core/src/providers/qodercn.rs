//! qodercn Provider（Qoder 中国版，T4.6）。
//!
//! 与国际版（qoder.rs）共用全部实现逻辑，仅换产品常量
//! （对照参考 qoder-product.ts:584-660 的 `QODER_CN`）：
//! - auth=qoder.cn、openapi=openapi.qoder.com.cn、加密推理=gateway.qoder.com.cn
//! - client_id `732aef47-…`（与国际版完全不同，用错授权后报参数无效）
//! - session_type CN=`qoder_work`（国际版=qodercli）
//! - Cosy-ClientType 同 `'10'`、UA 同 `qoder` 前缀

use crate::providers::qoder::{
    QoderDeviceSession, QoderOAuth, QoderProvider,
};
use crate::provider::{Credential, ProviderError};

pub const QODERCN_AUTH_BASE: &str = "https://qoder.cn";
pub const QODERCN_OPENAPI_BASE: &str = "https://openapi.qoder.com.cn";
pub const QODERCN_ENCRYPTED_INFER_BASE: &str = "https://gateway.qoder.com.cn";
/// CN asar `Vpe.authClientIds.prod`——与国际版**完全不同**
/// （qoder-product.ts:606-608；用错的症状是授权页 302 正常、点授权后报参数无效）。
pub const QODERCN_CLIENT_ID: &str = "732aef47-9cf2-46a2-95fe-4cebb5d0d1fa";
/// CN session_type（qoder-wasm.ts:199——国际版是 `qodercli`）。
pub const QODERCN_SESSION_TYPE: &str = "qoder_work";

/// CN 版授权 URL（auth base 换 qoder.cn，client_id 换 CN 值）。
pub fn build_qodercn_auth_url(sess: &QoderDeviceSession) -> String {
    format!(
        "{}/device/selectAccounts?challenge={}&challenge_method=S256&nonce={}&machine_id={}&client_id={}",
        QODERCN_AUTH_BASE, sess.challenge, sess.nonce, sess.machine_id, QODERCN_CLIENT_ID
    )
}

pub struct QodercnProvider {
    inner: QoderProvider,
    oauth: QoderOAuth,
}

impl QodercnProvider {
    pub fn production() -> Self {
        Self::with_openapi_base(QODERCN_OPENAPI_BASE.into())
    }

    pub fn with_openapi_base(openapi: String) -> Self {
        Self {
            inner: QoderProvider::new(openapi.clone()),
            oauth: QoderOAuth::new_for_test(openapi),
        }
    }

    /// CN 设备码登录（走 CN 端点，凭据同国际版形态）。
    pub async fn login_with_interval(
        &self,
        interval: std::time::Duration,
    ) -> Result<Credential, ProviderError> {
        let sess = self.oauth.create_device_session();
        self.oauth.poll_until_token(&sess, interval).await
    }

    /// 余额（透传到 qoder 实现——请求路径/Cosy 头与产品无关）。
    pub async fn balance(
        &self,
        cred: &Credential,
    ) -> Result<crate::providers::qoder::QoderBalance, ProviderError> {
        self.inner.balance(cred).await
    }

    /// 活动（透传）。
    pub async fn campaigns(&self, cred: &Credential) -> Result<Vec<serde_json::Value>, ProviderError> {
        self.inner.campaigns(cred).await
    }

    /// 领取（透传）。
    pub async fn claim(
        &self,
        cred: &Credential,
        campaign_id: &str,
    ) -> Result<crate::providers::qoder::ClaimOutcome, ProviderError> {
        self.inner.claim(cred, campaign_id).await
    }
}
