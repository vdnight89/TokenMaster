//! 凭据加密：AES-256-GCM，`enc:v1:<nonce>.<tag>.<ct>`（URL_SAFE_NO_PAD）。
//!
//! 密钥派生与 zcode-pool zcrypto 同思路：机器属性派生（混淆级保护，
//! 防的是误分享/顺手翻看，不等同硬件级加密）；`TOKENMASTER_SECRET`
//! 环境变量可覆盖派生盐。zcode 官方凭据的**兼容解密**在 zcompat
//! 模块（导入功能用，密钥派生公式与官方完全一致），不复用本模块。

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};

const PREFIX: &str = "enc:v1:";

fn derive_key() -> [u8; 32] {
    let salt = std::env::var("TOKENMASTER_SECRET").unwrap_or_else(|_| "tokenmaster-credential".into());
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    let user = std::env::var("USERNAME").unwrap_or_default();
    let mut h = Sha256::new();
    h.update(format!("{salt}:{home}:{user}"));
    h.finalize().into()
}

pub fn encrypt(plain: &str) -> String {
    let key = Key::<Aes256Gcm>::from(derive_key());
    let cipher = Aes256Gcm::new(&key);
    let mut nonce_bytes = [0u8; 12];
    use rand::RngCore;
    rand::rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ct = cipher
        .encrypt(nonce, Payload { msg: plain.as_bytes(), aad: b"tokenmaster" })
        .expect("aes-gcm encrypt");
    // aes-gcm 0.10 的输出已含 16 字节 tag 尾部；拆成 nonce.tag.ct 三段
    let (body, tag) = ct.split_at(ct.len() - 16);
    format!(
        "{PREFIX}{}.{}.{}",
        URL_SAFE_NO_PAD.encode(nonce_bytes),
        URL_SAFE_NO_PAD.encode(tag),
        URL_SAFE_NO_PAD.encode(body)
    )
}

pub fn decrypt(enc: &str) -> Result<String, CryptoError> {
    let rest = enc.strip_prefix(PREFIX).ok_or(CryptoError::BadFormat)?;
    let mut parts = rest.split('.');
    let (n, t, c) = (
        parts.next().ok_or(CryptoError::BadFormat)?,
        parts.next().ok_or(CryptoError::BadFormat)?,
        parts.next().ok_or(CryptoError::BadFormat)?,
    );
    if parts.next().is_some() {
        return Err(CryptoError::BadFormat);
    }
    let nonce_bytes = URL_SAFE_NO_PAD.decode(n).map_err(|_| CryptoError::BadFormat)?;
    let tag = URL_SAFE_NO_PAD.decode(t).map_err(|_| CryptoError::BadFormat)?;
    let body = URL_SAFE_NO_PAD.decode(c).map_err(|_| CryptoError::BadFormat)?;
    if nonce_bytes.len() != 12 || tag.len() != 16 {
        return Err(CryptoError::BadFormat);
    }
    let mut ct = body;
    ct.extend_from_slice(&tag);
    let key = Key::<Aes256Gcm>::from(derive_key());
    let cipher = Aes256Gcm::new(&key);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let plain = cipher
        .decrypt(nonce, Payload { msg: &ct, aad: b"tokenmaster" })
        .map_err(|_| CryptoError::Auth)?;
    String::from_utf8(plain).map_err(|_| CryptoError::Auth)
}

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("bad enc:v1 format")]
    BadFormat,
    #[error("ciphertext auth failed")]
    Auth,
}
