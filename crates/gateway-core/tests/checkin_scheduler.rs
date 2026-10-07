//! T6.4 签到调度测试。

use gateway_core::checkin::{AutoCheckin, CheckinOutcome, CheckinScheduler};
use gateway_core::provider::{Credential, ProviderError};

fn cred(id: &str) -> Credential {
    Credential { account_id: id.into(), secret: format!("{{\"token\":\"{id}\"}}") }
}

#[tokio::test]
async fn run_all_calls_all_registered_providers_and_accounts() {
    let mut sched = CheckinScheduler::new();
    sched.register("zcode", vec![cred("z1"), cred("z2")]);
    sched.register("trae", vec![cred("t1")]);

    let results = sched
        .run_all(|provider: String, cred: Credential| async move {
            match (provider.as_str(), cred.account_id.as_str()) {
                ("zcode", "z1") => Ok((false, "领取成功 +10000".to_string())),
                ("zcode", "z2") => Ok((true, "今日已领取".to_string())),
                _ => Err(ProviderError::Credential("session dead".into())),
            }
        })
        .await;

    assert_eq!(results.len(), 3);
    let z1 = results.iter().find(|r| r.account_id == "z1").unwrap();
    assert!(z1.success && !z1.already_claimed);
    let z2 = results.iter().find(|r| r.account_id == "z2").unwrap();
    assert!(z2.success && z2.already_claimed);
    let t1 = results.iter().find(|r| r.account_id == "t1").unwrap();
    assert!(!t1.success);
    assert!(t1.message.contains("session dead"));
}

#[tokio::test]
async fn run_all_empty_returns_empty() {
    let sched = CheckinScheduler::new();
    let results = sched
        .run_all(|_: String, _: Credential| async { Ok((false, String::new())) })
        .await;
    assert!(results.is_empty());
}

#[test]
fn summarize_counts() {
    let results = vec![
        CheckinOutcome { account_id: "a".into(), provider: "zcode".into(), success: true, already_claimed: false, message: String::new() },
        CheckinOutcome { account_id: "b".into(), provider: "zcode".into(), success: true, already_claimed: true, message: String::new() },
        CheckinOutcome { account_id: "c".into(), provider: "trae".into(), success: false, already_claimed: false, message: "err".into() },
    ];
    let s = CheckinScheduler::summarize(&results);
    assert!(s.contains("2/3"), "{s}");
    assert!(s.contains("1 已领取"), "{s}");
}

#[test]
fn auto_checkin_toggle() {
    let ac = AutoCheckin::new(3600);
    assert_eq!(ac.interval(), std::time::Duration::from_secs(3600));
    assert!(!ac.is_enabled());
    ac.set_enabled(true);
    assert!(ac.is_enabled());
    ac.set_enabled(false);
    assert!(!ac.is_enabled());
}
