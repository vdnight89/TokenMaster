//! T6.2 zcode-pool 导入：enc:v1 兼容解析，读其账号目录。
//!
//! zcode-pool 的存储布局（src-tauri/src/store.rs:62-63）：
//! - `~/.zcode-pool/store/accounts/{id}.json`：账号文件
//! - 凭据字段可能带 `enc:v1:` 前缀（zcrypto.rs）
//!
//! enc:v1 格式：`enc:v1:{nonce_b64url}.{tag_b64url}.{ct_b64url}`
//! - 密钥 = sha256(secret)
//! - secret = env `ZCODE_CREDENTIAL_SECRET` 或
//!   `zcode-credential-fallback:{platform}:{home}:{username}`
//! - platform 映射：windows→win32 / macos→darwin

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::Aes256Gcm;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;

pub const ENC_PREFIX: &str = "enc:v1:";

/// zcode-pool 的默认存储根（`~/.zcode-pool`）。
pub fn default_pool_root() -> std::path::PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    Path::new(&home).join(".zcode-pool")
}

/// 检测 `enc:v1:` 前缀。
pub fn is_encrypted(v: &str) -> bool {
    v.starts_with(ENC_PREFIX)
}

/// 合成 fallback secret（zcrypto.rs:44-46）。
/// platform 映射：windows→win32 / macos→darwin（zcrypto.rs:9-13）。
pub fn default_secret(platform: Option<&str>, home: &str, username: &str) -> String {
    if let Ok(s) = std::env::var("ZCODE_CREDENTIAL_SECRET") {
        return s;
    }
    let os = platform.unwrap_or(std::env::consts::OS);
    let mapped = match os {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    };
    format!("zcode-credential-fallback:{mapped}:{home}:{username}")
}

/// 当前用户的默认 secret（自动探测 platform/home/username）。
pub fn current_default_secret() -> String {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    let username = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "unknown".to_string());
    default_secret(None, &home, &username)
}

fn derive_key(secret: &str) -> [u8; 32] {
    let d = Sha256::digest(secret.as_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(&d);
    out
}

/// 解密 `enc:v1:{nonce}.{tag}.{ct}` → 明文字符串。
pub fn decrypt_enc_v1(value: &str, secret: &str) -> Result<String, String> {
    let body = value.strip_prefix(ENC_PREFIX).ok_or("不是 enc:v1 格式")?;
    let parts: Vec<&str> = body.split('.').collect();
    if parts.len() != 3 {
        return Err("enc:v1 格式不正确（需三段）".into());
    }
    let nonce_b = URL_SAFE_NO_PAD.decode(parts[0]).map_err(|e| format!("nonce 解码失败：{e}"))?;
    let tag_b = URL_SAFE_NO_PAD.decode(parts[1]).map_err(|e| format!("tag 解码失败：{e}"))?;
    let ct_b = URL_SAFE_NO_PAD.decode(parts[2]).map_err(|e| format!("密文解码失败：{e}"))?;
    if nonce_b.len() != 12 {
        return Err("nonce 长度异常".into());
    }
    let key = derive_key(secret);
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| format!("密钥初始化失败：{e}"))?;
    let mut buf = ct_b;
    buf.extend_from_slice(&tag_b);
    let pt = cipher
        .decrypt(aes_gcm::Nonce::from_slice(&nonce_b), buf.as_slice())
        .map_err(|_| "解密失败（密钥不匹配或数据损坏）".to_string())?;
    Ok(String::from_utf8_lossy(&pt).to_string())
}

/// 递归解密 JSON 值中所有带 `enc:v1:` 前缀的字符串字段。
pub fn decrypt_json_fields(v: &Value, secret: &str) -> Value {
    match v {
        Value::String(s) if is_encrypted(s) => {
            match decrypt_enc_v1(s, secret) {
                Ok(plain) => Value::String(plain),
                Err(_) => Value::Null, // 解密失败置空（导入时跳过）
            }
        }
        Value::Object(m) => {
            let mut out = serde_json::Map::new();
            for (k, val) in m {
                out.insert(k.clone(), decrypt_json_fields(val, secret));
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| decrypt_json_fields(x, secret)).collect()),
        other => other.clone(),
    }
}

/// zcode-pool 账号（store.rs:78-91 的 Account 结构子集）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ImportedAccount {
    pub id: String,
    pub name: String,
    pub credentials: Value,
    pub virtual_device_mid: Option<String>,
}

/// 从 zcode-pool 根目录（`~/.zcode-pool`）导入全部账号。
/// - `pool_root`：zcode-pool 根目录（含 `store/accounts/`）
/// - `secret_override`：手动传入解密密钥（None 时用 current_default_secret）
///
/// 坏 JSON 跳过不中断；`enc:v1:` 字段解密失败置空（凭据不完整仍可导入，
/// 后续 GUI 标记需重新登录）。
pub fn import_zcode_pool_accounts(
    pool_root: &Path,
    secret_override: Option<&str>,
) -> Result<Vec<ImportedAccount>, String> {
    let accounts_dir = pool_root.join("store").join("accounts");
    if !accounts_dir.exists() {
        return Ok(Vec::new());
    }
    let secret = secret_override
        .map(str::to_string)
        .unwrap_or_else(current_default_secret);

    let mut out = Vec::new();
    let entries = std::fs::read_dir(&accounts_dir).map_err(|e| format!("读目录失败：{e}"))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&raw) else { continue };
        let id = v.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
        let name = v.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
        let credentials = v
            .get("credentials")
            .cloned()
            .map(|c| decrypt_json_fields(&c, &secret))
            .unwrap_or(Value::Null);
        let virtual_device_mid = v
            .get("virtual_device_mid")
            .and_then(Value::as_str)
            .map(str::to_string);
        out.push(ImportedAccount { id, name, credentials, virtual_device_mid });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}
