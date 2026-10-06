/** 跨页面复用的小 UI 件（对应原型 bindCopy / #cool-timer 行为） */
import { useEffect, useState } from "react";
import { Ic } from "./icons";

export type ModalKind = "add" | "claim" | "import" | null;

/** 复制按钮：点击写剪贴板并把图标换成对勾 1.2s（对应原型 bindCopy） */
export function CopyBtn({ text, title = "复制", className = "" }: { text: string; title?: string; className?: string }) {
  const [done, setDone] = useState(false);
  return (
    <button
      className={`ibtn${className ? " " + className : ""}`}
      title={title}
      onClick={() => {
        try {
          navigator.clipboard?.writeText(text)?.catch(() => {
            /* 原型环境忽略剪贴板失败 */
          });
        } catch {
          /* 原型环境忽略 */
        }
        setDone(true);
        window.setTimeout(() => setDone(false), 1200);
      }}
    >
      <Ic name={done ? "check" : "copy"} />
    </button>
  );
}

/** 限流冷却倒计时（mm:ss 每秒递减，到 00:00 停住，对应原型 #cool-timer 定时器） */
export function CoolTimer({ initial }: { initial: string }) {
  const [t, setT] = useState(initial);
  useEffect(() => {
    const id = window.setInterval(() => {
      setT(prev => {
        const [m0, s0] = prev.split(":");
        let m = Number(m0);
        let s = Number(s0) - 1;
        if (s < 0) {
          s = 59;
          m -= 1;
        }
        if (m < 0) return prev;
        return String(m).padStart(2, "0") + ":" + String(s).padStart(2, "0");
      });
    }, 1000);
    return () => window.clearInterval(id);
  }, []);
  return <>{t}</>;
}
