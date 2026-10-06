//! 网关密钥生成：`sk-tm-` + 16 字节随机数的十六进制（128bit 熵）。

use rand::RngCore;

pub fn generate_gateway_key() -> String {
    let mut b = [0u8; 16];
    rand::rng().fill_bytes(&mut b);
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("sk-tm-{hex}")
}
