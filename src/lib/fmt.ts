/** 数字与时间格式化（与 round-02 原型 app.js 的 fmt/fmtK 语义一致） */

export const fmt = (n: number): string => Math.round(n).toLocaleString("en-US");

export const fmtK = (n: number): string =>
  n >= 1e6 ? (n / 1e6).toFixed(2) + "M" : n >= 1e3 ? (n / 1e3).toFixed(1) + "k" : fmt(n);

export const sum = (a: number[]): number => a.reduce((x, y) => x + y, 0);

/** 确定性伪随机（演示数据抖动用，与原型同种子行为） */
export function makeRnd(seed: number): () => number {
  let i = seed;
  return () => {
    i = (i * 9301 + 49297) % 233280;
    return i / 233280;
  };
}
