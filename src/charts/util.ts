/** 图表共享工具：容器测宽、稳定 SVG id、调色板 */
import { useEffect, useRef, useState } from "react";
import type { RefObject } from "react";

/** AM 图表 15 色调色板（TokenStats 同款） */
export const PALETTE = ["#3b82f6", "#06b6d4", "#10b981", "#8b5cf6", "#ec4899", "#f59e0b", "#f43f5e", "#6366f1"];

/** 测量容器宽度；隐藏页（宽度 0）期间沿用 fallback，可见后由 ResizeObserver 校正 */
export function useWidth(fallback: number): readonly [RefObject<HTMLDivElement | null>, number] {
  const ref = useRef<HTMLDivElement | null>(null);
  const [w, setW] = useState(fallback);
  useEffect(() => {
    const el = ref.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(entries => {
      for (const e of entries) {
        if (e.contentRect.width > 0) setW(e.contentRect.width);
      }
    });
    ro.observe(el);
    if (el.clientWidth > 0) setW(el.clientWidth);
    return () => ro.disconnect();
  }, []);
  return [ref, w] as const;
}

let svgSeq = 0;

/** 稳定的 SVG 渐变 id：组件实例内不变，实例间唯一 */
export function useSvgId(prefix: string): string {
  const ref = useRef("");
  if (!ref.current) ref.current = prefix + ++svgSeq;
  return ref.current;
}
