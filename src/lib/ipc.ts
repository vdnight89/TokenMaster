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
): { data: T; loading: boolean; isReal: boolean } {
  const [data, setData] = useState<T>(fallback);
  const [loading, setLoading] = useState(isTauri());
  const [isReal, setIsReal] = useState(false);

  useEffect(() => {
    if (!isTauri()) return;
    let cancelled = false;
    invoke<T>(command)
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
  }, [command]);

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
