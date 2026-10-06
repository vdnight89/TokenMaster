//! T2.1 账号/凭据模型与加密落盘（缝 2 单元级：Store 公开 API）。
//!
//! 行为来源：spec「存储」——凭据 AES-256-GCM 加密、`~/.tokenmaster/`、
//! 原子写（tmp+rename，Windows 共享冲突需容忍）；格式借鉴 zcode-pool 的
//! `enc:v1:nonce.tag.ciphertext`（URL_SAFE_NO_PAD），密钥派生可用
//! 环境变量覆盖（测试沙箱用 TOKENMASTER_HOME 重定向数据目录）。

use gateway_core::store::{AccountRecord, Store};

fn rec(provider: &str, label: &str, credential: &str) -> AccountRecord {
    AccountRecord::new(provider, label, credential)
}

#[test]
fn credential_roundtrips_and_is_encrypted_at_rest() {
    let dir = tempfile_dir("t21-roundtrip");
    let store = Store::open_at(&dir);
    let r = rec("zcode", "yx-main@z.ai", "jwt-secret-token-value-0123456789");
    store.save_account(&r).unwrap();

    let file = dir.join("accounts").join(format!("{}.json", r.id));
    let raw = std::fs::read_to_string(&file).unwrap();
    assert!(!raw.contains("jwt-secret-token-value"), "凭据明文不得出现在磁盘");
    assert!(raw.contains("enc:v1:"), "密文采用 enc:v1 形态: {raw}");

    let loaded = store.load_accounts().unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].credential, "jwt-secret-token-value-0123456789");
    assert_eq!(loaded[0].provider, "zcode");
    assert_eq!(loaded[0].label, "yx-main@z.ai");
    assert!(loaded[0].enabled);
}

#[test]
fn tampered_ciphertext_fails_to_load_that_record() {
    let dir = tempfile_dir("t21-tamper");
    let store = Store::open_at(&dir);
    store.save_account(&rec("gemini", "a@gmail", "tok")).unwrap();
    let files: Vec<_> = std::fs::read_dir(dir.join("accounts"))
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    let path = files[0].path();
    let raw = std::fs::read_to_string(&path).unwrap();
    let tampered = raw.replace("enc:v1:", "enc:v1:AAAA");
    std::fs::write(&path, tampered).unwrap();
    // 被篡改的记录解不开：跳过该条并返回其余（不 panic、不丢整库）
    assert!(store.load_accounts().unwrap().is_empty());
}

#[test]
fn save_is_atomic_and_leaves_no_temp_files() {
    let dir = tempfile_dir("t21-atomic");
    let store = Store::open_at(&dir);
    store.save_account(&rec("trae", "solo-01", "c1")).unwrap();
    store.save_account(&rec("trae", "solo-02", "c2")).unwrap();
    let entries: Vec<_> = std::fs::read_dir(dir.join("accounts"))
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 2, "只应有 2 个账号文件，无 tmp 残留");
    assert!(entries.iter().all(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json")));
}

#[test]
fn delete_removes_account_file() {
    let dir = tempfile_dir("t21-delete");
    let store = Store::open_at(&dir);
    let r = rec("opencode", "acct-01", "sk-xx");
    store.save_account(&r).unwrap();
    store.delete_account(&r.id).unwrap();
    assert!(store.load_accounts().unwrap().is_empty());
}

#[test]
fn crypto_format_roundtrip_and_prefix() {
    let enc = gateway_core::crypto::encrypt("你好 token");
    assert!(enc.starts_with("enc:v1:"));
    assert_eq!(enc.matches('.').count(), 2, "nonce.tag.ct 三段");
    assert_eq!(gateway_core::crypto::decrypt(&enc).unwrap(), "你好 token");
    assert!(gateway_core::crypto::decrypt("enc:v1:bad").is_err());
}

/// 每个测试独立的临时目录。
fn tempfile_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("tm-tests-{tag}-{}", gateway_core::key::random_id(6)));
    std::fs::create_dir_all(&d).unwrap();
    d
}
