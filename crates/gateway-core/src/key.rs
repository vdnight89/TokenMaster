//! 网关密钥与随机 id 生成：`sk-tm-` + 16 字节随机数的十六进制（128bit 熵）。

use rand::RngCore;

pub fn generate_gateway_key() -> String {
    let mut b = [0u8; 16];
    rand::rng().fill_bytes(&mut b);
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("sk-tm-{hex}")
}

/// 生成 `n` 字节随机数的十六进制字符串（响应 id 等用途）。
pub fn random_id(n: usize) -> String {
    let mut b = vec![0u8; n];
    rand::rng().fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}
