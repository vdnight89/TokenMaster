//! 真机 e2e 测试基础设施。
//!
//! 这是 e2e 测试的框架层：不需要真实凭据就能跑通框架本身；
//! 有了真实凭据后通过 env 注入即可执行真机验证。
//!
//! 用法：
//! ```bash
//! # 框架验证（无凭据，只验证测试管道）
//! cargo test -p gateway-core --test e2e_infra -- --ignored
//!
//! # 真机验证（需提供凭据，见各 provider 的 env 变量说明）
//! ZCODE_JWT=eyJ... cargo test -p gateway-core --test e2e_infra -- --ignored real
//! ```

#![cfg(test)]

use gateway_core::provider::{Provider, ProviderError};

/// 从 env 读取凭据；缺失时返回 None（e2e 跳过该用例）。
fn env_cred(key: &str) -> Option<gateway_core::Credential> {
    let secret = std::env::var(key).ok()?;
    if secret.is_empty() {
        return None;
    }
    Some(gateway_core::Credential {
        account_id: format!("e2e-{}", key.to_lowercase()),
        secret,
    })
}

/// e2e 辅助：发送一条简单消息并断言收到非空回复。
async fn assert_simple_completion(
    provider: &dyn Provider,
    cred: &gateway_core::Credential,
    model: &str,
) -> Result<(), ProviderError> {
    let route = gateway_core::route::Route {
        provider: provider.id().to_string(),
        model: model.to_string(),
    };
    let req: gateway_core::openai::ChatRequest = serde_json::from_value(serde_json::json!({
        "model": format!("{}/{}", provider.id(), model),
        "messages": [{ "role": "user", "content": "Say 'hello' and nothing else." }]
    }))
    .unwrap();

    let completion = provider.complete(cred, &route, &req).await?;
    let content = completion.choices[0].message.content.as_str();
    assert!(
        !content.is_empty(),
        "[{}] {} 真机 e2e：回复为空",
        provider.id(),
        model
    );
    assert!(
        completion.usage.total_tokens > 0,
        "[{}] {} 真机 e2e：usage 全零",
        provider.id(),
        model
    );
    Ok(())
}

// ── 框架自测（不需要凭据，验证测试管道本身） ──

#[test]
fn e2e_framework_self_test() {
    // env_cred 在无 env 时返回 None
    assert!(env_cred("NONEXISTENT_KEY").is_none());
}

// ── 真机 e2e 用例（#[ignore] 需显式 --ignored 运行 + 凭据） ──

#[tokio::test]
#[ignore = "需要 ZCODE_JWT 环境变量"]
async fn e2e_zcode_real() {
    let Some(cred) = env_cred("ZCODE_JWT") else {
        eprintln!("跳过：ZCODE_JWT 未设置");
        return;
    };
    let p = gateway_core::providers::zcode::ZcodeProvider::production();
    assert_simple_completion(&p, &cred, "glm-5.2").await.unwrap();
}

#[tokio::test]
#[ignore = "需要 GEMINI_REFRESH_TOKEN 环境变量"]
async fn e2e_gemini_real() {
    let Some(cred) = env_cred("GEMINI_CREDENTIAL_JSON") else {
        eprintln!("跳过：GEMINI_CREDENTIAL_JSON 未设置");
        return;
    };
    let p = gateway_core::providers::gemini::GeminiProvider::production();
    assert_simple_completion(&p, &cred, "gemini-3.8-flash").await.unwrap();
}

#[tokio::test]
#[ignore = "需要 TRAE_CREDENTIAL_JSON 环境变量"]
async fn e2e_trae_real() {
    let Some(cred) = env_cred("TRAE_CREDENTIAL_JSON") else {
        eprintln!("跳过：TRAE_CREDENTIAL_JSON 未设置");
        return;
    };
    let p = gateway_core::providers::trae::TraeProvider::production();
    assert_simple_completion(&p, &cred, "glm-5.2").await.unwrap();
}

#[tokio::test]
#[ignore = "需要 CLINE_CREDENTIAL_JSON 环境变量"]
async fn e2e_cline_real() {
    let Some(cred) = env_cred("CLINE_CREDENTIAL_JSON") else {
        eprintln!("跳过：CLINE_CREDENTIAL_JSON 未设置");
        return;
    };
    let p = gateway_core::providers::cline::ClineProvider::production();
    assert_simple_completion(&p, &cred, "claude-sonnet-4-6").await.unwrap();
}

#[tokio::test]
#[ignore = "需要 MINIMAX_CREDENTIAL_JSON 环境变量"]
async fn e2e_minimax_real() {
    let Some(cred) = env_cred("MINIMAX_CREDENTIAL_JSON") else {
        eprintln!("跳过：MINIMAX_CREDENTIAL_JSON 未设置");
        return;
    };
    let p = gateway_core::providers::minimax::MinimaxProvider::production();
    assert_simple_completion(&p, &cred, "MiniMax-M3").await.unwrap();
}

#[tokio::test]
#[ignore = "需要 BUDDY_CREDENTIAL_JSON 环境变量"]
async fn e2e_buddy_real() {
    let Some(cred) = env_cred("BUDDY_CREDENTIAL_JSON") else {
        eprintln!("跳过：BUDDY_CREDENTIAL_JSON 未设置");
        return;
    };
    let p = gateway_core::providers::buddy::BuddyProvider::production();
    assert_simple_completion(&p, &cred, "Deepseek-V4.1-Flash").await.unwrap();
}

#[tokio::test]
#[ignore = "需要 LOBSTERAI_CREDENTIAL_JSON 环境变量"]
async fn e2e_lobsterai_real() {
    let Some(cred) = env_cred("LOBSTERAI_CREDENTIAL_JSON") else {
        eprintln!("跳过：LOBSTERAI_CREDENTIAL_JSON 未设置");
        return;
    };
    let p = gateway_core::providers::lobsterai::LobsteraiProvider::production();
    assert_simple_completion(&p, &cred, "deepseek-v4-flash").await.unwrap();
}

#[tokio::test]
#[ignore = "需要 CODEARTS_CREDENTIAL_JSON 环境变量"]
async fn e2e_codearts_real() {
    let Some(cred) = env_cred("CODEARTS_CREDENTIAL_JSON") else {
        eprintln!("跳过：CODEARTS_CREDENTIAL_JSON 未设置");
        return;
    };
    let p = gateway_core::providers::codearts::CodeartsProvider::production();
    assert_simple_completion(&p, &cred, "glm-5.2").await.unwrap();
}
