# zcode 验证码链 v1 必做，用 Tauri WebView 子窗口作过码载体

zcode 上游在特定场景（错误码 3007）要求浏览器环境完成验证码，参考项目 deepseek-harness-codearts 以约 200-400MB 的常驻 chromium 作验证码载体（captcha carrier）。用户确认这是「参考项目明确说明必须做的」能力，v1 必须实现；但 TokenMaster 不内置独立 chromium，而是按需拉起 Tauri WebView（系统 WebView2）子窗口承载过码页，配合退避与验证码供应池（参考 captcha-supply/captcha-backoff 的调度逻辑）。

## Considered Options

- v1 降级（遇 3007 提示手动重登）：工作量最小，被用户否决。
- 随包携带常驻 chromium sidecar（照抄参考实现）：内存与包体代价过大，放弃。

## Consequences

- 过码页在 WebView2 而非 chromium 中渲染，需对照参考项目验证上游对环境指纹的兼容性；这是 zcode 通道联调时的重点风险项。
- 验证码供应池按需生产，无常驻进程，空闲时零开销。
