//! TokenMaster 网关核心：把 15 家 Provider 的令牌池聚合为本地
//! OpenAI/Anthropic 协议兼容网关。纯库形态（ADR-0004），可在
//! 测试进程内直接启动（缝 1：HTTP 黑盒）。

pub mod config;
pub mod error;
pub mod key;
pub mod server;
