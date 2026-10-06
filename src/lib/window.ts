/**
 * 窗口控制桥：仅在 Tauri 壳内生效，浏览器开发环境静默 no-op
 * （T6.0 自定义标题栏：traffic 三点窗控 + 顶栏拖拽/双击最大化）。
 */
const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function withWindow(fn: (w: import("@tauri-apps/api/window").Window) => Promise<void>) {
  if (!inTauri) return;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await fn(getCurrentWindow());
}

export const winMinimize = () => withWindow(w => w.minimize());
export const winToggleMaximize = () => withWindow(w => w.toggleMaximize());
export const winClose = () => withWindow(w => w.close());
export const inDesktopShell = inTauri;
