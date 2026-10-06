//! T1.4 缝 1：/v1/models 聚合行为——每个注册 provider 的模型以
//! `provider/model` 复合 id 出现（spec：路由与协议）。

use gateway_core::config::{AuthMode, GatewayConfig};
use gateway_core::registry::{ModelInfo, ProviderCatalog, Registry};

fn config_with_mock() -> GatewayConfig {
    GatewayConfig {
        port: None,
        auth: AuthMode::Disabled,
        model_map: Vec::new(),
        registry: Registry::with(ProviderCatalog {
            id: "mock".into(),
            models: vec![
                ModelInfo { id: "mock-alpha".into() },
                ModelInfo { id: "mock-beta".into() },
            ],
        }),
    }
}

#[tokio::test]
async fn models_list_contains_composite_ids() {
    let h = gateway_core::server::start(config_with_mock()).await.unwrap();
    let resp = reqwest::get(format!("http://{}/v1/models", h.addr)).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let ids: Vec<&str> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"mock/mock-alpha"));
    assert!(ids.contains(&"mock/mock-beta"));
    h.shutdown().await;
}

#[tokio::test]
async fn each_model_entry_carries_provider_and_object_fields() {
    let h = gateway_core::server::start(config_with_mock()).await.unwrap();
    let body: serde_json::Value = reqwest::get(format!("http://{}/v1/models", h.addr))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let first = &body["data"][0];
    assert_eq!(first["object"], "model");
    assert_eq!(first["owned_by"], "mock");
    h.shutdown().await;
}
