/**
 * 全局 toast 系统（React 状态版，替代原型 app.js 的 DOM append 实现）。
 * 用法：任意组件内 `toast("ok", "标题", "描述")`；App 挂一次 <ToastHost />。
 */
import { useEffect, useState, useSyncExternalStore } from "react";
import { Ic } from "./icons";

export type ToastKind = "ok" | "err" | "info";

export interface ToastItem {
  id: number;
  kind: ToastKind;
  title: string;
  desc?: string;
}

let items: readonly ToastItem[] = [];
let seq = 0;
const listeners = new Set<() => void>();

function emit(): void {
  for (const l of listeners) l();
}
function subscribe(l: () => void): () => void {
  listeners.add(l);
  return () => {
    listeners.delete(l);
  };
}
function snapshot(): readonly ToastItem[] {
  return items;
}
function remove(id: number): void {
  items = items.filter(t => t.id !== id);
  emit();
}

/** 弹出一条 toast：3.2s 后淡出并移除（与原型节奏一致） */
export function toast(kind: ToastKind, title: string, desc?: string): void {
  items = [...items, { id: ++seq, kind, title, desc }];
  emit();
}

export function ToastHost() {
  const list = useSyncExternalStore(subscribe, snapshot);
  return <div className="toasts">{list.map(t => <ToastRow key={t.id} item={t} />)}</div>;
}

function ToastRow({ item }: { item: ToastItem }) {
  const [out, setOut] = useState(false);
  useEffect(() => {
    const t1 = window.setTimeout(() => setOut(true), 3200);
    const t2 = window.setTimeout(() => remove(item.id), 3460);
    return () => {
      window.clearTimeout(t1);
      window.clearTimeout(t2);
    };
  }, [item.id]);
  return (
    <div
      className={`toast ${item.kind}`}
      style={out ? { transition: "opacity .25s, transform .25s", opacity: 0, transform: "translateY(8px)" } : undefined}
    >
      <Ic name={item.kind === "ok" ? "okc" : item.kind === "err" ? "alertr" : "info"} />
      <div className="tb">
        {item.title}
        {item.desc ? <span className="d">{item.desc}</span> : null}
      </div>
    </div>
  );
}
