//! T1.2 网关密钥：`sk-tm-` 前缀 + 足够熵，两次生成必不相同。

use gateway_core::key::generate_gateway_key;

#[test]
fn gateway_key_has_sk_tm_prefix_and_128bit_entropy() {
    let k = generate_gateway_key();
    assert!(k.starts_with("sk-tm-"), "key should carry sk-tm- prefix: {k}");
    // 16 字节 → 32 个十六进制字符
    assert_eq!(k.len(), "sk-tm-".len() + 32, "key length: {k}");
}

#[test]
fn two_generated_keys_differ() {
    assert_ne!(generate_gateway_key(), generate_gateway_key());
}
