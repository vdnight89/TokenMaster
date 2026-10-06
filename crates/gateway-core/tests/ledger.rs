//! T3.1/T3.2 账本：内存环形（500 条）+ JSONL 滚动落盘（32MB → .old）；
//! record 绝不抛错；按天/Provider/账号/模型聚合。
//! 行为来源：spec「用量与账本」+ reference dsh token-ledger（record 不抛错）
//! 与 zcode-pool usage.jsonl（32MB 滚动）。

use gateway_core::ledger::{Ledger, UsageRecord};

fn rec(provider: &str, account: &str, model: &str, status: u16) -> UsageRecord {
    UsageRecord {
        ts: 1_764_000_000,
        provider: provider.into(),
        account_id: account.into(),
        model: model.into(),
        prompt_tokens: 100,
        completion_tokens: 50,
        status,
        ttfb_ms: Some(420),
        duration_ms: 1200,
        proto: "openai",
    }
}

fn tmpdir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("tm-ledger-{tag}-{}", gateway_core::key::random_id(6)));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn records_are_written_as_jsonl_with_expected_fields() {
    let dir = tmpdir("jsonl");
    let ledger = Ledger::open(Some(&dir.join("usage.jsonl")));
    ledger.record(rec("zcode", "a1", "glm-4.7", 200));
    ledger.record(rec("gemini", "g1", "gemini-3-pro", 429));
    ledger.flush();
    let raw = std::fs::read_to_string(dir.join("usage.jsonl")).unwrap();
    let lines: Vec<&str> = raw.lines().collect();
    assert_eq!(lines.len(), 2);
    let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(first["provider"], "zcode");
    assert_eq!(first["account_id"], "a1");
    assert_eq!(first["model"], "glm-4.7");
    assert_eq!(first["status"], 200);
    assert_eq!(first["prompt_tokens"], 100);
    assert_eq!(first["proto"], "openai");
}

#[test]
fn rolling_moves_file_to_old_when_threshold_reached() {
    let dir = tmpdir("roll");
    // 阈值 256 字节：几条记录后必然滚动（单代 .old 语义：新一代覆盖上一代 .old）
    let ledger = Ledger::with_max_bytes(Some(&dir.join("usage.jsonl")), 256);
    for i in 0..20 {
        ledger.record(rec("zcode", &format!("acct-{i}"), "glm-4.7", 200));
    }
    ledger.flush();
    assert!(dir.join("usage.jsonl").exists(), "滚动后主文件重新开始");
    assert!(dir.join("usage.jsonl.old").exists(), "旧文件转 .old");
    let old_len = std::fs::read_to_string(dir.join("usage.jsonl.old")).unwrap().lines().count();
    let new_len = std::fs::read_to_string(dir.join("usage.jsonl")).unwrap().lines().count();
    assert!(old_len >= 1 && new_len >= 1, "两代各保留完整记录：old={old_len} new={new_len}");
}

#[test]
fn record_never_panics_on_unwritable_path() {
    let bad = std::path::PathBuf::from("Z:/nonexistent-drive-xyz/usage.jsonl");
    let ledger = Ledger::open(Some(&bad));
    ledger.record(rec("zcode", "a", "m", 200)); // 不 panic 即通过
    assert_eq!(ledger.recent(10).len(), 1, "内存环形仍然记录");
}

#[test]
fn aggregation_by_provider_and_model_and_day() {
    let ledger = Ledger::open(None);
    ledger.record(rec("zcode", "a1", "glm-4.7", 200));
    ledger.record(rec("zcode", "a2", "glm-4.7", 429));
    ledger.record(rec("gemini", "g1", "gemini-3-pro", 200));

    let by_pv = ledger.aggregate_by_provider();
    assert_eq!(by_pv.len(), 2);
    let z = by_pv.iter().find(|b| b.key == "zcode").unwrap();
    assert_eq!(z.requests, 2);
    assert_eq!(z.failures, 1);
    assert_eq!(z.completion_tokens, 100);

    let by_model = ledger.aggregate_by_model();
    assert_eq!(by_model.len(), 2);
    assert!(by_model.iter().any(|b| b.key == "glm-4.7" && b.requests == 2));

    let by_day = ledger.aggregate_by_day();
    assert_eq!(by_day.len(), 1, "同一天聚到一桶");
    assert_eq!(by_day[0].requests, 3);
}

#[test]
fn in_memory_ring_keeps_last_500() {
    let ledger = Ledger::open(None);
    for i in 0..600 {
        ledger.record(rec("zcode", &format!("a{i}"), "m", 200));
    }
    let recent = ledger.recent(1000);
    assert_eq!(recent.len(), 500);
    assert_eq!(recent.last().unwrap().account_id, "a599");
    assert_eq!(recent.first().unwrap().account_id, "a100");
}
