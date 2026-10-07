//! Provider 实现族：每家上游一个模块（M4）。
//! 对照基准：docs/reference/ 各手册（deepseek-harness-codearts / commandcode-proxy 等）。

pub mod buddy;
pub mod cline;
pub mod codearts;
pub mod commandcode;
pub mod trae;
pub mod gemini;
pub mod lobsterai;
pub mod loomy;
pub mod minimax;
pub mod qoder;
pub mod raccoon;
pub mod opencode;
pub mod qodercn;
pub mod workbuddy;
#[cfg(feature = "wasm")]
pub mod qoder_wasm;
pub mod zcode;
