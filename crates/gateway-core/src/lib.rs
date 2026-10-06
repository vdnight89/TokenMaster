//! TokenMaster 网关核心：把 15 家 Provider 的令牌池聚合为本地
//! OpenAI/Anthropic 协议兼容网关。纯库形态（ADR-0004），可在
//! 测试进程内直接启动（缝 1：HTTP 黑盒）。

pub mod anthropic;
pub mod config;
pub mod consume;
pub mod crypto;
pub mod error;
pub mod key;
pub mod ledger;
pub mod mock;
pub mod openai;
pub mod orchestrate;
pub mod pool;
pub mod provider;
pub mod providers;
pub mod refresh;
pub mod registry;
pub mod route;
pub mod server;
pub mod store;

pub use provider::Credential;
