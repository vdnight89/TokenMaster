/**
 * T5.11 IPC 桥：Tauri invoke 封装 + mock/real 数据切换层。
 *
 * 在 Tauri 环境下调 invoke() 拿真数据；在浏览器（vite dev）或 invoke
 * 失败时回退到 mock 数据。每页 import { useIPC } 即可透明切换。
 */

// 检测是否在 Tauri 环境
function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

// invoke 封装（Tauri 环境）
async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) throw new Error("not in tauri");
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args);
}

// ── 数据类型（与 mock.ts 形状对齐） ──

export interface ProviderInfo {
  name: string;
  color: string;
  tier: string;
  models?: readonly (readonly [string, number, string])[]; // [modelId, pct, reset]
}

export interface AccountInfo {
  id: string;
  provider: string;
  name: string;
  state: "ok" | "cool" | "exp" | "dead" | "off";
  /** Unix 秒（list_accounts IPC 下发） */
  created_at: number;
  updated_at: number;
  balance?: string;
  expires?: string;
}

export interface GatewayInfo {
  port: number;
  base_url: string;
  protocol: string;
}

export interface UsageData {
  today: { requests: number; tokens: number };
  byProvider: Record<string, number>;
  byModel: Record<string, number>;
}

// ── IPC hooks（React） ──

import { useEffect, useState } from "react";

/** 通用 IPC hook：Tauri 环境调 command，失败回退 fallback。 */
function useIPCData<T>(
  command: string,
  fallback: T,
  args?: Record<string, unknown>,
): { data: T; loading: boolean; isReal: boolean } {
  const [data, setData] = useState<T>(fallback);
  const [loading, setLoading] = useState(isTauri());
  const [isReal, setIsReal] = useState(false);
  // args 序列化进依赖：对象身份每次渲染都变会反复触发请求，用稳定串代替
  const argsKey = args === undefined ? "" : JSON.stringify(args);

  useEffect(() => {
    if (!isTauri()) return;
    let cancelled = false;
    invoke<T>(command, args)
      .then((result) => {
        if (!cancelled) {
          setData(result);
          setIsReal(true);
        }
      })
      .catch(() => {
        // Tauri 里 command 失败也回退 mock
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [command, argsKey]);

  return { data, loading, isReal };
}

/** Provider 列表 */
export function useProviders(fallback: Record<string, ProviderInfo>) {
  return useIPCData("list_providers", fallback);
}

/** 账号列表 */
export function useAccounts(fallback: AccountInfo[]) {
  return useIPCData("list_accounts", fallback);
}

/** 网关配置 */
export function useGatewayInfo(fallback: GatewayInfo) {
  return useIPCData("gateway_config", fallback);
}

/** 用量汇总 */
export function useUsage(fallback: UsageData) {
  return useIPCData("usage_summary", fallback);
}

export { isTauri, invoke };

export interface LogEntry {
  ts: number;
  provider: string;
  account: string;
  model: string;
  prompt_tokens: number;
  completion_tokens: number;
  status: number;
  ttfb_ms: number | null;
  duration_ms: number;
  proto: string;
}

/** 请求日志（limit 作为 invoke 参数传递——拼进命令名会得到不存在的 command） */
export function useLogs(fallback: LogEntry[], limit?: number) {
  const args = limit === undefined ? undefined : { limit };
  return useIPCData("list_logs", fallback, args);
}
