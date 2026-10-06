//! 账号存储：`~/.tokenmaster/accounts/{id}.json`，凭据 enc:v1 加密，
//! 原子写（tmp + rename，Windows 下 rename 目标存在时用 replace）。
//! 数据目录可被 `TOKENMASTER_HOME` 环境变量重定向（测试沙箱/便携模式）。

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::crypto;
use crate::key::random_id;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountRecord {
    pub id: String,
    pub provider: String,
    pub label: String,
    pub enabled: bool,
    /// 凭据原文（内存中即明文；落盘前加密）。
    pub credential: String,
    pub created_at: u64,
    pub updated_at: u64,
}

impl AccountRecord {
    pub fn new(provider: &str, label: &str, credential: &str) -> Self {
        let now = crate::openai::now_ts();
        Self {
            id: format!("acct-{}", random_id(8)),
            provider: provider.into(),
            label: label.into(),
            enabled: true,
            credential: credential.into(),
            created_at: now,
            updated_at: now,
        }
    }
}

/// 落盘形态：credential 为 enc:v1 密文。
#[derive(Serialize, Deserialize)]
struct AccountFile {
    id: String,
    provider: String,
    label: String,
    enabled: bool,
    credential_enc: String,
    created_at: u64,
    updated_at: u64,
}

pub struct Store {
    root: PathBuf,
}

impl Store {
    /// 默认数据目录：`$TOKENMASTER_HOME` 或 `~/.tokenmaster`。
    pub fn open_default() -> io::Result<Store> {
        let root = match std::env::var("TOKENMASTER_HOME") {
            Ok(p) => PathBuf::from(p),
            Err(_) => {
                let home = std::env::var("USERPROFILE")
                    .or_else(|_| std::env::var("HOME"))
                    .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "no home directory"))?;
                PathBuf::from(home).join(".tokenmaster")
            }
        };
        Ok(Self::open_at(&root))
    }

    pub fn open_at(root: &Path) -> Store {
        Store { root: root.to_path_buf() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn accounts_dir(&self) -> PathBuf {
        self.root.join("accounts")
    }

    /// 保存账号（upsert by id）：加密 → 原子写。
    pub fn save_account(&self, rec: &AccountRecord) -> io::Result<()> {
        let dir = self.accounts_dir();
        std::fs::create_dir_all(&dir)?;
        let file = AccountFile {
            id: rec.id.clone(),
            provider: rec.provider.clone(),
            label: rec.label.clone(),
            enabled: rec.enabled,
            credential_enc: crypto::encrypt(&rec.credential),
            created_at: rec.created_at,
            updated_at: crate::openai::now_ts(),
        };
        let path = dir.join(format!("{}.json", rec.id));
        atomic_write_json(&path, &file)
    }

    /// 读取全部账号；无法解密/解析的单条记录跳过（不拖垮整库）。
    pub fn load_accounts(&self) -> io::Result<Vec<AccountRecord>> {
        let dir = self.accounts_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&dir)? {
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(&path) else { continue };
            let Ok(file) = serde_json::from_str::<AccountFile>(&raw) else { continue };
            let credential = match crypto::decrypt(&file.credential_enc) {
                Ok(c) => c,
                Err(_) => continue, // 被篡改/密钥不符：跳过该条
            };
            out.push(AccountRecord {
                id: file.id,
                provider: file.provider,
                label: file.label,
                enabled: file.enabled,
                credential,
                created_at: file.created_at,
                updated_at: file.updated_at,
            });
        }
        out.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        Ok(out)
    }

    pub fn delete_account(&self, id: &str) -> io::Result<()> {
        let path = self.accounts_dir().join(format!("{id}.json"));
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// 原子写 JSON：同目录 tmp 文件 → rename/replace。
/// Windows 上 rename 到已存在目标会失败，因此带 replace 重试一次。
fn atomic_write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    let data = serde_json::to_vec_pretty(value)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    std::fs::write(&tmp, data)?;
    // Windows 共享冲突（杀毒/索引扫描短暂占用）容忍一次 40ms 重试
    for attempt in 0..2 {
        match std::fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            Err(e) if attempt == 0 && e.kind() == io::ErrorKind::PermissionDenied => {
                std::thread::sleep(std::time::Duration::from_millis(40));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                // rename 跨锁或目标存在：退回 replace 语义
                match std::fs::copy(&tmp, path) {
                    Ok(_) => return Ok(()),
                    Err(_) => return Err(e),
                }
            }
        }
    }
    std::fs::rename(&tmp, path)
}
