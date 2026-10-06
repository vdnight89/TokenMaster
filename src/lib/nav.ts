import { useEffect, useState } from "react";

/** 页面标识与 round-02 原型的 data-page 一致 */
export const PAGES = [
  { key: "dash", label: "仪表盘" },
  { key: "accounts", label: "账号" },
  { key: "gateway", label: "网关" },
  { key: "usage", label: "用量" },
  { key: "logs", label: "日志" },
  { key: "setup", label: "接入" },
  { key: "settings", label: "设置" },
] as const;

export type PageKey = (typeof PAGES)[number]["key"];

export function useHashPage(): [PageKey, (p: PageKey) => void] {
  const read = (): PageKey => {
    const h = location.hash.replace("#", "");
    return (PAGES.find((p) => p.key === h)?.key ?? "dash") as PageKey;
  };
  const [page, setPage] = useState<PageKey>(read);
  useEffect(() => {
    const on = () => setPage(read());
    window.addEventListener("hashchange", on);
    return () => window.removeEventListener("hashchange", on);
  }, []);
  const go = (p: PageKey) => {
    if (location.hash !== "#" + p) location.hash = p;
    setPage(p);
  };
  return [page, go];
}
