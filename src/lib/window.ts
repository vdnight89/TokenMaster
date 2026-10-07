/**
 * 窗口控制桥：仅在 Tauri 壳内生效，浏览器开发环境静默 no-op
 * （T6.0 自定义标题栏：traffic 三点窗控 + 顶栏拖拽/双击最大化）。
 */
const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function withWindow(fn: (w: import("@tauri-apps/api/window").Window) => Promise<void>): Promise<void> {
  if (!inTauri) return;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await fn(getCurrentWindow());
}

// 窗控是 fire-and-forget 调用：吞掉 rejection（窗口可能已销毁），避免 unhandled rejection
const safeWindow = (fn: (w: import("@tauri-apps/api/window").Window) => Promise<void>) => {
  withWindow(fn).catch(() => {});
};

export const winMinimize = () => safeWindow(w => w.minimize());
export const winToggleMaximize = () => safeWindow(w => w.toggleMaximize());
export const winClose = () => safeWindow(w => w.close());
export const inDesktopShell = inTauri;
