//! T1.3 模型路由（单元级）：`provider/model` 前缀强制 + 裸名映射表优先级。
//! 行为来源：spec「路由与协议」——裸模型名按映射表路由，前缀强制指定。

use gateway_core::route::{resolve_model, Route};

fn map() -> Vec<(String, String)> {
    vec![
        ("glm-4.7".into(), "zcode/glm-4.7".into()),
        ("claude-sonnet-4-5".into(), "gemini/claude-sonnet-4-5".into()),
    ]
}

#[test]
fn provider_prefix_forces_routing_without_map() {
    let r: Route = resolve_model("zcode/glm-4.7", &[]).expect("prefix should force");
    assert_eq!(r.provider, "zcode");
    assert_eq!(r.model, "glm-4.7");
}

#[test]
fn bare_name_resolves_through_map_in_priority_order() {
    let r = resolve_model("glm-4.7", &map()).expect("mapped");
    assert_eq!(r.provider, "zcode");
    assert_eq!(r.model, "glm-4.7");
}

#[test]
fn unknown_bare_name_is_model_not_found() {
    let err = resolve_model("no-such-model", &map()).unwrap_err();
    let body = gateway_core::error::error_json(&err);
    assert_eq!(body["error"]["code"], "model_not_found");
}

#[test]
fn empty_provider_prefix_is_rejected() {
    assert!(resolve_model("/glm-4.7", &map()).is_err());
}
