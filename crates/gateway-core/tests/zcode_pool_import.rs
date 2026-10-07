//! T6.2 zcode-pool 导入：enc:v1 兼容解析（缝 2）。
//! 行为来源：zcode-pool/src-tauri/src/zcrypto.rs + store.rs——
//! - 账号文件 `~/.zcode-pool/store/accounts/{id}.json`
//! - 凭据字段可能带 `enc:v1:` 前缀（AES-256-GCM：
//!   `enc:v1:{nonce_b64url}.{tag_b64url}.{ct_b64url}`）
//! - 密钥 = sha256(secret)，secret =
//!   env `ZCODE_CREDENTIAL_SECRET` 或 `zcode-credential-fallback:{platform}:{home}:{username}`
//! - platform 映射：windows→win32 / macos→darwin
//! - 非加密字段直接读 JSON

use gateway_core::zcode_pool_import::{
    decrypt_enc_v1, default_secret, import_zcode_pool_accounts, is_encrypted,
};

// ── enc:v1 前缀检测 ──

#[test]
fn is_encrypted_checks_prefix() {
    assert!(is_encrypted("enc:v1:abc.def.ghi"));
    assert!(!is_encrypted("plain-text"));
    assert!(!is_encrypted(""));
    assert!(!is_encrypted("enc:v2:abc"));
}

// ── default_secret 合成 ──

#[test]
fn default_secret_composition() {
    // 平台映射：windows→win32
    let s = default_secret(Some("win32"), "/home/test", "user1");
    assert!(s.contains("zcode-credential-fallback:"));
    assert!(s.contains("win32"));
    assert!(s.contains("/home/test"));
    assert!(s.contains("user1"));
    // macos→darwin
    let s2 = default_secret(Some("darwin"), "/Users/bob", "bob");
    assert!(s2.contains("darwin"));
}

// ── 解密往返（用我们自己的加密格式构造密文） ──

#[test]
fn decrypt_enc_v1_roundtrip() {
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::Aes256Gcm;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use sha2::{Digest, Sha256};

    let secret = "test-secret";
    let plaintext = r#"{"access_token":"jwt-abc","refresh_token":"rt-1"}"#;

    let key = {
        let d = Sha256::digest(secret.as_bytes());
        let mut k = [0u8; 32];
        k.copy_from_slice(&d);
        k
    };
    let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
    let nonce_bytes = [7u8; 12];
    let ct = cipher.encrypt(aes_gcm::Nonce::from_slice(&nonce_bytes), plaintext.as_bytes()).unwrap();
    let (ciphertext, tag) = ct.split_at(ct.len() - 16);

    let encoded = format!(
        "enc:v1:{}.{}.{}",
        URL_SAFE_NO_PAD.encode(nonce_bytes),
        URL_SAFE_NO_PAD.encode(tag),
        URL_SAFE_NO_PAD.encode(ciphertext),
    );

    assert!(is_encrypted(&encoded));
    let decrypted = decrypt_enc_v1(&encoded, secret).unwrap();
    assert_eq!(decrypted, plaintext);
}

#[test]
fn decrypt_wrong_secret_fails() {
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::Aes256Gcm;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use sha2::{Digest, Sha256};

    let secret = "correct-secret";
    let plaintext = "hello";
    let key = {
        let d = Sha256::digest(secret.as_bytes());
        let mut k = [0u8; 32];
        k.copy_from_slice(&d);
        k
    };
    let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
    let nonce_bytes = [1u8; 12];
    let ct = cipher.encrypt(aes_gcm::Nonce::from_slice(&nonce_bytes), plaintext.as_bytes()).unwrap();
    let (c, t) = ct.split_at(ct.len() - 16);
    let encoded = format!(
        "enc:v1:{}.{}.{}",
        URL_SAFE_NO_PAD.encode(nonce_bytes),
        URL_SAFE_NO_PAD.encode(t),
        URL_SAFE_NO_PAD.encode(c),
    );

    let result = decrypt_enc_v1(&encoded, "wrong-secret");
    assert!(result.is_err(), "错误密钥应解密失败");
}

// ── 从目录导入 ──

#[test]
fn import_from_directory_reads_json_accounts() {
    use std::fs;
    let tmp = std::env::temp_dir().join(format!("tm-zpool-test-{}", std::process::id()));
    let accounts_dir = tmp.join("store").join("accounts");
    fs::create_dir_all(&accounts_dir).unwrap();

    // 写两个账号（一个带凭据、一个只有名称）
    fs::write(
        accounts_dir.join("acc-001.json"),
        r#"{"id":"acc-001","name":"test-1","created_at":"2026-01-01","updated_at":"2026-01-01","hash":"","credentials":{"access_token":"jwt-token","refresh_token":"rt"}}"#,
    ).unwrap();
    fs::write(
        accounts_dir.join("acc-002.json"),
        r#"{"id":"acc-002","name":"test-2","created_at":"2026-01-02","updated_at":"2026-01-02","hash":"","credentials":{"access_token":"jwt-2","refresh_token":"rt-2"}}"#,
    ).unwrap();
    // 非 JSON 文件跳过
    fs::write(accounts_dir.join("readme.txt"), "skip me").unwrap();

    let accounts = import_zcode_pool_accounts(&tmp, None).unwrap();
    assert_eq!(accounts.len(), 2, "应读入 2 个 JSON 账号");
    assert!(accounts.iter().any(|a| a.name == "test-1"));
    assert!(accounts.iter().any(|a| a.name == "test-2"));

    fs::remove_dir_all(&tmp).ok();
}

#[test]
fn import_nonexistent_dir_returns_empty() {
    let accounts = import_zcode_pool_accounts(std::path::Path::new("/nonexistent/path"), None).unwrap();
    assert!(accounts.is_empty());
}

#[test]
fn import_corrupt_json_skipped() {
    use std::fs;
    let tmp = std::env::temp_dir().join(format!("tm-zpool-corrupt-{}", std::process::id()));
    let accounts_dir = tmp.join("store").join("accounts");
    fs::create_dir_all(&accounts_dir).unwrap();
    fs::write(accounts_dir.join("bad.json"), "{not valid json").unwrap();
    fs::write(
        accounts_dir.join("good.json"),
        r#"{"id":"g","name":"ok","created_at":"","updated_at":"","hash":"","credentials":{}}"#,
    ).unwrap();

    let accounts = import_zcode_pool_accounts(&tmp, None).unwrap();
    assert_eq!(accounts.len(), 1, "坏 JSON 跳过，好 JSON 保留");

    fs::remove_dir_all(&tmp).ok();
}
