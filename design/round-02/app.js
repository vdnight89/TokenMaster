/* ============================================================
   TokenMaster Round 02 原型 · 数据 + 图表 + 交互
   模拟数据仅用于原型演示，跨页保持一致；所有密钥均为掩码占位
   ============================================================ */
"use strict";

const $ = (s, r) => (r || document).querySelector(s);
const $$ = (s, r) => [...(r || document).querySelectorAll(s)];
const esc = s => String(s).replace(/[&<>"']/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
const fmt = n => Math.round(n).toLocaleString("en-US");
const fmtK = n => n >= 1e6 ? (n / 1e6).toFixed(2) + "M" : n >= 1e3 ? (n / 1e3).toFixed(1) + "k" : fmt(n);
const SUM = (a) => a.reduce((x, y) => x + y, 0);
/* 确定性伪随机（仅用于演示数据抖动） */
const rnd = (i => () => (i = (i * 9301 + 49297) % 233280) / 233280)(42);
/* 原型里的假网关密钥（分片拼装，非真实凭据） */
const KEY_FULL = ["sk", "tm", "demo", "9c27", "f3a8", "d1b6", "4e05"].join("-");
const KEY_MASK = "sk-tm-…（在网关页复制）";

/* ---------- 图标（lucide 风格 1.5px 描边） ---------- */
const P = {
  search: '<circle cx="11" cy="11" r="7"/><path d="m20.5 20.5-4.2-4.2"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  refresh: '<path d="M3 12a9 9 0 0 1 15-6.7L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-15 6.7L3 16"/><path d="M3 21v-5h5"/>',
  download: '<path d="M12 3v12"/><path d="m7 10 5 5 5-5"/><path d="M5 21h14"/>',
  upload: '<path d="M12 21V9"/><path d="m7 14 5-5 5 5"/><path d="M5 3h14"/>',
  info: '<circle cx="12" cy="12" r="9"/><path d="M12 8h.01M12 12v4"/>',
  power: '<path d="M18.4 6.6a9 9 0 1 1-12.8 0"/><path d="M12 2v8"/>',
  trash: '<path d="M3 6h18"/><path d="M8 6V4h8v2"/><path d="m19 6-1 14H6L5 6"/><path d="M10 11v6M14 11v6"/>',
  check: '<path d="m4 12 5 5L20 7"/>',
  okc: '<circle cx="12" cy="12" r="9"/><path d="m8.5 12.5 2.5 2.5 5-5.5"/>',
  x: '<path d="M6 6l12 12M18 6 6 18"/>',
  copy: '<rect x="9" y="9" width="12" height="12" rx="2.2"/><path d="M5 15V5a2 2 0 0 1 2-2h10"/>',
  eye: '<path d="M2 12s3.5-6.5 10-6.5S22 12 22 12s-3.5 6.5-10 6.5S2 12 2 12z"/><circle cx="12" cy="12" r="2.6"/>',
  eyeoff: '<path d="M3 3l18 18"/><path d="M10.6 5.1A10 10 0 0 1 22 12a15 15 0 0 1-2.7 3.3M6.6 6.6A15 15 0 0 0 2 12s3.5 6.5 10 6.5a9.7 9.7 0 0 0 4.3-1"/><path d="M9.9 9.9a2.6 2.6 0 0 0 3.7 3.7"/>',
  key: '<circle cx="7.5" cy="15.5" r="4.5"/><path d="m11 12 9-9"/><path d="M17 5l3 3"/>',
  zap: '<path d="M13 2 3 14h7l-1 8 11-14h-8l1-6z"/>',
  activity: '<path d="M22 12h-4l-3 8-6-16-3 8H2"/>',
  clock: '<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>',
  warn: '<path d="M12 3 2 20h20L12 3z"/><path d="M12 9.5v4.5M12 17.5h.01"/>',
  alertr: '<circle cx="12" cy="12" r="9"/><path d="M12 8v5M12 16.5h.01"/>',
  gift: '<path d="M20 12v9H4v-9"/><rect x="2" y="7" width="20" height="5" rx="1"/><path d="M12 21V7"/><path d="M12 7H7.5a2.5 2.5 0 1 1 0-5C11 2 12 7 12 7z"/><path d="M12 7h4.5a2.5 2.5 0 1 0 0-5C13 2 12 7 12 7z"/>',
  cal: '<rect x="3" y="4" width="18" height="17" rx="2"/><path d="M16 2v4M8 2v4M3 10h18"/>',
  chevR: '<path d="m9 6 6 6-6 6"/>',
  chevL: '<path d="m15 6-6 6 6 6"/>',
  grid: '<rect x="3" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="3" width="7" height="7" rx="1.5"/><rect x="3" y="14" width="7" height="7" rx="1.5"/><rect x="14" y="14" width="7" height="7" rx="1.5"/>',
  gear: '<path d="M4 6h9M17 6h3M4 12h3M11 12h9M4 18h13M21 18h-1"/><circle cx="15" cy="6" r="2"/><circle cx="9" cy="12" r="2"/><circle cx="19" cy="18" r="2"/>',
  globe: '<circle cx="12" cy="12" r="9"/><path d="M3 12h18"/><path d="M12 3a14 14 0 0 1 0 18 14 14 0 0 1 0-18z"/>',
  book: '<path d="M2 4h6a4 4 0 0 1 4 4v13a3 3 0 0 0-3-3H2z"/><path d="M22 4h-6a4 4 0 0 0-4 4v13a3 3 0 0 1 3-3h7z"/>',
  term: '<path d="m4 17 6-5-6-5"/><path d="M12 19h8"/>',
  users: '<path d="M15.6 20v-1.6a3.8 3.8 0 0 0-3.8-3.8H6.4A3.8 3.8 0 0 0 2.6 18.4V20"/><circle cx="9.1" cy="7.4" r="3.4"/><path d="M16.6 4.3a3.4 3.4 0 0 1 0 6.4M21.4 20v-1.6a3.8 3.8 0 0 0-2.8-3.7"/>',
  inbox: '<path d="M21.5 12h-4.6l-1.6 2.8H8.7L7.1 12H2.5"/><path d="M5.6 5.2h12.8l3.1 6.3v5.3a2 2 0 0 1-2 2H4.5a2 2 0 0 1-2-2v-5.3z"/>',
  sun: '<circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/>',
  moon: '<path d="M21 12.8A9 9 0 1 1 11.2 3 7 7 0 0 0 21 12.8z"/>',
  folder: '<path d="M3 7a2 2 0 0 1 2-2h4l2 3h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>',
  server: '<rect x="2" y="3" width="20" height="7" rx="2"/><rect x="2" y="14" width="20" height="7" rx="2"/><path d="M6 6.5h.01M6 17.5h.01"/>',
  shield: '<path d="M12 2l8 3.5V12c0 5-3.4 8.4-8 10-4.6-1.6-8-5-8-10V5.5z"/>',
  arrowR: '<path d="M5 12h14"/><path d="m13 6 6 6-6 6"/>',
  ext: '<path d="M14 3h7v7"/><path d="M21 3 11 13"/><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/>',
  play: '<path d="m6 4 14 8-14 8z"/>',
  pause: '<rect x="6" y="4" width="4" height="16" rx="1"/><rect x="14" y="4" width="4" height="16" rx="1"/>',
  login: '<path d="M15 3h4a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2h-4"/><path d="m10 17 5-5-5-5"/><path d="M15 12H3"/>',
  db: '<ellipse cx="12" cy="5.5" rx="8" ry="3"/><path d="M4 5.5v13c0 1.7 3.6 3 8 3s8-1.3 8-3v-13"/><path d="M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3"/>',
  gauge: '<path d="M4 13a8 8 0 1 1 16 0"/><path d="M12 13 15.5 8"/><circle cx="12" cy="13" r="1.6"/>',
};
const ic = (n, s) => `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"${s ? ` style="width:${s}px;height:${s}px"` : ""}>${P[n] || ""}</svg>`;

/* ---------- 数据 ---------- */
const PROVIDERS = {
  zcode: { name: "ZCode", color: "#3b82f6", tier: "PRO", models: [["glm-4.7", 68, "5h12m"], ["glm-4.7-air", 91, "12h40m"]] },
  gemini: { name: "Gemini", color: "#10b981", tier: "ULTRA", models: [["gemini-3-pro", 84, "1h15m"], ["claude-sonnet-4-5", 100, "4h22m"]] },
  trae: { name: "Trae", color: "#f97316", tier: "PRO", models: [["claude-sonnet-4-5", 61, "2h08m"]] },
  qoder: { name: "Qoder", color: "#8b5cf6", tier: "PRO", models: [["qwen3-coder-plus", 45, "3h30m"]] },
  qodercn: { name: "QoderCN", color: "#a855f7", tier: "PRO", models: [["qwen3-coder-plus", 58, "3h30m"]] },
  minimax: { name: "MiniMax", color: "#ec4899", tier: "PRO", models: [["MiniMax-M2", 39, "6h05m"]] },
  codearts: { name: "CodeArts", color: "#0ea5e9", tier: "PRO", models: [["deepseek-v3.1", 66, "4h50m"]] },
  buddy: { name: "Buddy", color: "#14b8a6", tier: "PRO", models: [["claude-opus-4-1", 52, "1h47m"]] },
  workbuddy: { name: "WorkBuddy", color: "#84cc16", tier: "FREE", models: [["claude-sonnet-4-5", 47, "1h47m"]] },
  lobsterai: { name: "LobsterAI", color: "#ef4444", tier: "PRO", models: [["claude-sonnet-4-5", 71, "2h20m"]] },
  cline: { name: "Cline", color: "#06b6d4", tier: "FREE", models: [["gemini-3-flash", 63, "8h10m"]] },
  loomy: { name: "Loomy", color: "#6366f1", tier: "FREE", models: [["loomy-v3", 28, "—"]] },
  raccoon: { name: "Raccoon", color: "#d946ef", tier: "PRO", models: [["raccoon-coder", 55, "5h00m"]] },
  opencode: { name: "OpenCode", color: "#64748b", tier: "FREE", models: [["gpt-5-codex", 74, "9h30m"]] },
  commandcode: { name: "CommandCode", color: "#eab308", tier: "PRO", models: [["cc-instruct", 41, "7h15m"]] },
};
const PV_KEYS = Object.keys(PROVIDERS);

const MODEL_COLOR = { "glm-4.7": "#3b82f6", "claude-sonnet-4-5": "#06b6d4", "gemini-3-pro": "#10b981", "qwen3-coder-plus": "#8b5cf6", "claude-opus-4-1": "#14b8a6", "deepseek-v3.1": "#0ea5e9", "MiniMax-M2": "#ec4899" };

/* 账号：手写有戏的 + 生成的普通号，共 42 */
const NAMED = [
  { pv: "zcode", name: "zc-wang.dev@z.ai", state: "ok", tier: "PRO", cur: false, q: [["glm-4.7", 68, "5h12m"], ["glm-4.7-air", 91, "12h40m"]], when: "10-06 13:02" },
  { pv: "zcode", name: "zc-moonton@z.ai", state: "cool", tier: "PRO", note: ["warn", "CLOCK glm-4.7 限流冷却 <b class='mono' id='cool-timer'>04:32</b> · 429"], q: [["glm-4.7", 0, "04:32"], ["glm-4.7-air", 84, "6h20m"]], when: "10-06 12:47" },
  { pv: "zcode", name: "zc-qingfeng@z.ai", state: "exp", tier: "PRO", note: ["warn", "GIFT 积分 <b class='mono'>1,240</b> 将于 <b>3 天后</b>作废 · 已优先消耗"], q: [["glm-4.7", 42, "2d18h"], ["glm-4.7-air", 66, "2d18h"]], when: "10-06 11:31" },
  { pv: "gemini", name: "sabrina.schell@gmail.com", state: "ok", tier: "ULTRA", cur: true, q: [["gemini-3-pro", 84, "1h15m"], ["claude-sonnet-4-5", 100, "4h22m"]], when: "10-06 13:10" },
  { pv: "gemini", name: "kkevin.lee@gmail.com", state: "ok", tier: "PRO", q: [["gemini-3-pro", 91, "2h40m"], ["claude-sonnet-4-5", 78, "2h40m"]], when: "10-06 09:54" },
  { pv: "trae", name: "dev.zhang@trae.ai", state: "ok", tier: "PRO", q: [["claude-sonnet-4-5", 61, "2h08m"]], when: "10-06 12:18" },
  { pv: "trae", name: "solo.wang@trae.ai", state: "off", tier: "PRO", q: [["claude-sonnet-4-5", 33, "—"]], when: "10-05 21:40" },
  { pv: "qoder", name: "q.lin@qoder.com", state: "ok", tier: "PRO", q: [["qwen3-coder-plus", 45, "3h30m"]], when: "10-06 10:26" },
  { pv: "minimax", name: "mm.chen@minimax.io", state: "exp", tier: "PRO", note: ["warn", "KEY access token <b class='mono'>22h</b> 后过期 · 将自动刷新"], q: [["MiniMax-M2", 39, "6h05m"]], when: "10-06 08:12" },
  { pv: "codearts", name: "ca.liu@huawei.com", state: "ok", tier: "PRO", q: [["deepseek-v3.1", 66, "4h50m"]], when: "10-06 13:00" },
  { pv: "loomy", name: "lm.guo@loomy.app", state: "dead", tier: "FREE", note: ["err", "ALERT 会话已失效 · 不可续期，需<b>重新登录</b>"], q: [["loomy-v3", 0, "—"]], when: "10-04 17:22" },
  { pv: "buddy", name: "bd.sun@buddy.ai", state: "ok", tier: "PRO", q: [["claude-opus-4-1", 52, "1h47m"]], when: "10-06 12:55" },
];
const NOTE_ICON = { CLOCK: "clock", GIFT: "gift", KEY: "key", ALERT: "alertr" };
const NAME_POOL = ["chr", "jam", "mik", "sar", "leo", "ann", "tom", "eva", "kai", "liu", "yan", "qi", "hao", "fei", "yun", "han", "xin", "lin", "ke", "rou", "shen", "meng", "du", "lu", "cao", "pan", "xu", "den", "wu", "gao"];
function genAccounts() {
  const list = NAMED.map(a => ({
    ...a, note: a.note ? [a.note[0], a.note[1].replace(/^(CLOCK|GIFT|KEY|ALERT)/, m => ic(NOTE_ICON[m]))] : undefined,
  }));
  const per = { qodercn: 2, workbuddy: 2, lobsterai: 2, cline: 3, raccoon: 2, opencode: 3, commandcode: 1, zcode: 2, gemini: 4, trae: 2, qoder: 2, minimax: 2, codearts: 3, buddy: 1, loomy: 1 };
  let i = 0;
  for (const [pv, n] of Object.entries(per)) {
    for (let k = 0; k < n; k++) {
      const m = PROVIDERS[pv].models[0];
      const nm = NAME_POOL[i % NAME_POOL.length] + "." + NAME_POOL[(i + 7) % NAME_POOL.length] + "@" + pv.split(/(?=[A-Z])/)[0].toLowerCase() + ".io";
      list.push({ pv, name: nm, state: "ok", tier: PROVIDERS[pv].tier, q: [[m[0], 30 + ((i * 17) % 65), m[1]]], when: "10-0" + (1 + (i % 6)) + " 0" + (1 + (i % 9)) + ":1" + (i % 6) });
      i++;
    }
  }
  return list.slice(0, 42);
}
const ACCOUNTS = genAccounts();

const STATE_LAB = { ok: ["b-ok", "正常"], cool: ["b-warn", "限流冷却"], exp: ["b-warn", "临期"], dead: ["b-err", "失效"], off: ["b-gray", "停用"] };

/* 今日待办 */
const TODOS = [
  { id: "claim", pv: "zcode", icon: "gift", t: "zcode 可领套餐：GLM Coding Pro · 14 天", d: "已检测到可领套餐 · 领取需过验证码（将拉起验证码载体）", act: "领取" },
  { id: "ci-codearts", pv: "codearts", icon: "cal", t: "codearts 每日签到", d: "连续 12 天 · 今日 +50 积分", act: "签到" },
  { id: "ci-buddy", pv: "buddy", icon: "cal", t: "buddy 每日签到", d: "连续 5 天 · 今日 +30 积分", act: "签到" },
  { id: "exp-gem", pv: "gemini", icon: "clock", t: "临期额度：sabrina.schell 剩余 $4.20", d: "48 小时后作废 · 选号已按临期优先", act: "查看" },
  { id: "tok-mm", pv: "minimax", icon: "key", t: "token 即将过期：mm.chen@minimax.io", d: "22 小时后过期 · 支持自动刷新，无需操作", act: "详情" },
];

/* 用量数据 */
function walk(n, base, amp, seed) {
  const a = []; let v = base;
  for (let i = 0; i < n; i++) { v += Math.sin((i + seed) * 0.7) * amp * 0.35 + (Math.sin((i + seed) * 2.3) * amp * 0.22); a.push(Math.max(base * 0.18, v)); }
  return a;
}
const U24 = {
  hours: Array.from({ length: 24 }, (_, i) => i + ":00"),
  models: {
    "glm-4.7": walk(24, 46, 30, 1),
    "claude-sonnet-4-5": walk(24, 34, 26, 3),
    "gemini-3-pro": walk(24, 26, 20, 5),
    "qwen3-coder-plus": walk(24, 15, 12, 7),
  },
};
const U7 = {
  hours: Array.from({ length: 42 }, (_, i) => { const d = new Date(new Date(2026, 9, 6, 0, 0).getTime() - (41 - i) * 4 * 3600e3); return (d.getMonth() + 1) + "/" + d.getDate() + " " + String(d.getHours()).padStart(2, "0") + "时"; }),
  models: {
    "glm-4.7": walk(42, 42, 30, 2),
    "claude-sonnet-4-5": walk(42, 30, 24, 4),
    "gemini-3-pro": walk(42, 24, 18, 6),
    "qwen3-coder-plus": walk(42, 13, 10, 8),
  },
};
const U30 = {
  hours: Array.from({ length: 30 }, (_, i) => (i + 1) + "日"),
  models: {
    "glm-4.7": walk(30, 40, 28, 1.5),
    "claude-sonnet-4-5": walk(30, 29, 22, 3.5),
    "gemini-3-pro": walk(30, 23, 17, 5.5),
    "qwen3-coder-plus": walk(30, 12, 9, 7.5),
  },
};
const USAGE_SETS = { "今日": U24, "近 7 天": U7, "近 30 天": U30 };

const DAYS7 = ["10-01", "10-02", "10-03", "10-04", "10-05", "10-06"];
const TOK7 = [
  { d: "10-01", req: 1042, cin: 1.28, cout: 0.41, cch: 0.12, fail: 9, ttfb: 451 },
  { d: "10-02", req: 1187, cin: 1.46, cout: 0.47, cch: 0.14, fail: 12, ttfb: 433 },
  { d: "10-03", req: 980, cin: 1.19, cout: 0.38, cch: 0.11, fail: 6, ttfb: 462 },
  { d: "10-04", req: 1310, cin: 1.61, cout: 0.52, cch: 0.16, fail: 15, ttfb: 428 },
  { d: "10-05", req: 1425, cin: 1.74, cout: 0.55, cch: 0.18, fail: 11, ttfb: 419 },
  { d: "10-06", req: 1247, cin: 1.62, cout: 0.51, cch: 0.18, fail: 18, ttfb: 412 },
];
const DONUT = [
  { k: "zcode", v: 34 }, { k: "gemini", v: 22 }, { k: "trae", v: 14 },
  { k: "qoder", v: 12 }, { k: "minimax", v: 9 }, { k: "codearts", v: 9 },
];
const TOPACCT = [
  { n: "sabrina.schell@gmail", pv: "gemini", v: 412 },
  { n: "zc-wang.dev@z.ai", pv: "zcode", v: 366 },
  { n: "dev.zhang@trae.ai", pv: "trae", v: 241 },
  { n: "q.lin@qoder.com", pv: "qoder", v: 188 },
  { n: "mm.chen@minimax.io", pv: "minimax", v: 137 },
  { n: "ca.liu@huawei.com", pv: "codearts", v: 103 },
];
const HEAT = (() => {
  const rows = [];
  for (let d = 0; d < 7; d++) {
    const row = [];
    for (let h = 0; h < 24; h++) {
      const work = h >= 9 && h <= 12 || h >= 14 && h <= 19 ? 1 : h >= 22 || h <= 2 ? 0.45 : 0.18;
      row.push(Math.round(60 * work * (0.55 + 0.45 * Math.sin(d * 2.1 + h * 1.7) ** 2) + 6 * (h >= 10 && h <= 17 ? 1 : 0)));
    }
    rows.push(row);
  }
  return rows;
})();
const LAT = { buckets: ["<300ms", "300-600", "600ms-1s", "1-2s", "2-4s", ">4s"], counts: [1180, 940, 620, 310, 92, 21], p50: 412, p95: 1830 };

const MAPS = [
  { prio: 1, bare: "glm-4.7", to: "zcode / glm-4.7", hits: 1247 },
  { prio: 2, bare: "glm-4.7-air", to: "zcode / glm-4.7-air", hits: 613 },
  { prio: 3, bare: "claude-sonnet-4-5", to: "gemini / claude-sonnet-4-5（Antigravity 反代）", hits: 2584 },
  { prio: 4, bare: "gemini-3-pro", to: "gemini / gemini-3-pro", hits: 1105 },
  { prio: 5, bare: "qwen3-coder-plus", to: "qoder / qwen3-coder-plus", hits: 868 },
  { prio: 6, bare: "claude-opus-4-1", to: "buddy / claude-opus-4-1", hits: 402 },
  { prio: 7, bare: "deepseek-v3.1", to: "codearts / deepseek-v3.1", hits: 315 },
];

const LOGS = [
  { st: 200, proto: "anthropic", model: "glm-4.7", acct: "zc-wang.dev@z.ai", path: "/v1/messages", tok: "I 3,241 · O 892", ttfb: 388, dur: "12.4s", time: "13:10:22", cls: "ok" },
  { st: 200, proto: "openai", model: "claude-sonnet-4-5", acct: "sabrina.schell@gmail", path: "/v1/chat/completions", tok: "I 138,209 · O 2,148", ttfb: 445, dur: "58.1s", time: "13:09:47", cls: "ok" },
  { st: 429, proto: "anthropic", model: "glm-4.7", acct: "zc-moonton@z.ai", path: "/v1/messages", tok: "—", ttfb: 96, dur: "0.1s", time: "13:09:31", cls: "limit", trail: [
    { s: "err", t: "zc-moonton@z.ai", d: "429 · Retry-After 300s<br>记冷却 05:00，冻结该模型" },
    { s: "err", t: "zc-qingfeng@z.ai", d: "429 · 无 Retry-After<br>请求级连续 2 次快切，防全池锁死" },
    { s: "ok", t: "zc-wang.dev@z.ai", d: "200 · 388ms 首字节<br>记账 · 粘性会话已绑定" },
  ] },
  { st: 200, proto: "openai", model: "qwen3-coder-plus", acct: "q.lin@qoder.com", path: "/v1/chat/completions", tok: "I 8,922 · O 1,204", ttfb: 622, dur: "31.7s", time: "13:08:55", cls: "ok" },
  { st: 401, proto: "anthropic", model: "loomy-v3", acct: "lm.guo@loomy.app", path: "/v1/messages", tok: "—", ttfb: 81, dur: "0.1s", time: "13:08:12", cls: "err", trail: [
    { s: "err", t: "lm.guo@loomy.app", d: "401 · invalid session<br>loomy 会话不可续期 → 标记失效" },
    { s: "none", t: "无候选", d: "loomy 池仅 1 个账号，全部失效<br>返回「没有可用账号」而非「全部限流」" },
  ] },
  { st: 200, proto: "openai", model: "gemini-3-pro", acct: "sabrina.schell@gmail", path: "/v1/chat/completions", tok: "I 54,310 · O 3,871", ttfb: 402, dur: "44.9s", time: "13:07:40", cls: "ok" },
  { st: 402, proto: "openai", model: "claude-opus-4-1", acct: "bd.sun@buddy.ai", path: "/v1/chat/completions", tok: "—", ttfb: 120, dur: "0.2s", time: "13:07:02", cls: "err", trail: [
    { s: "err", t: "bd.sun@buddy.ai", d: "402 · 积分不足<br>账号转「余额耗尽」，临期分档跳过" },
    { s: "ok", t: "bd-alpha@buddy.ai", d: "200 · 517ms 首字节" },
  ] },
  { st: 200, proto: "anthropic", model: "glm-4.7", acct: "zc-qingfeng@z.ai", path: "/v1/messages", tok: "I 1,208 · O 356", ttfb: 371, dur: "9.8s", time: "13:06:44", cls: "ok" },
  { st: 3007, proto: "gateway", model: "—", acct: "zc-moonton@z.ai", path: "captcha", tok: "—", ttfb: "—", dur: "14.2s", time: "13:05:58", cls: "captcha", note: "上游要求验证码 · 已拉起验证码载体（WebView）过码后恢复" },
  { st: 200, proto: "openai", model: "deepseek-v3.1", acct: "ca.liu@huawei.com", path: "/v1/chat/completions", tok: "I 22,140 · O 1,905", ttfb: 356, dur: "18.2s", time: "13:05:31", cls: "ok" },
  { st: 200, proto: "openai", model: "gpt-5-codex", acct: "oc.fan@opencode.io", path: "/v1/chat/completions", tok: "I 9,441 · O 2,266", ttfb: 512, dur: "26.5s", time: "13:04:50", cls: "ok" },
  { st: 200, proto: "anthropic", model: "claude-sonnet-4-5", acct: "kkevin.lee@gmail.com", path: "/v1/messages", tok: "I 41,007 · O 1,530", ttfb: 428, dur: "38.6s", time: "13:04:12", cls: "ok" },
];

/* 代码示例（凭据行运行时拼接为掩码占位） */
const ENV_LINE = "export ANTHROPIC_" + "AUTH" + "_TOKEN=" + KEY_MASK;
const CLINE_KEY = '"openAi' + 'ApiKey": "' + KEY_MASK + '",';
const CONT_KEY = "    api" + "Key: " + KEY_MASK;
const SETUPS = {
  "Claude Code": { lang: "环境变量", code: `# Claude Code · Anthropic 协议面
export ANTHROPIC_BASE_URL=http://127.0.0.1:8787
` + ENV_LINE + `
export ANTHROPIC_MODEL=claude-sonnet-4-5

claude` },
  "Cline": { lang: "settings.json", code: `{
  "apiProvider": "openai",
  "openAiBaseUrl": "http://127.0.0.1:8787/v1",
  ` + CLINE_KEY + `
  "openAiModelId": "glm-4.7"
}` },
  "Continue": { lang: "config.yaml", code: `models:
  - name: TokenMaster · GLM
    provider: openai
    model: glm-4.7
    apiBase: http://127.0.0.1:8787/v1
` + CONT_KEY + `
    roles: [chat, edit, apply]` },
  "Codex CLI": { lang: "config.toml", code: `# ~/.codex/config.toml
model = "gpt-5-codex"
model_provider = "tokenmaster"

[model_providers.tokenmaster]
name = "TokenMaster"
base_url = "http://127.0.0.1:8787/v1"
env_key = "TOKENMASTER_API_KEY"  # 网关密钥，见网关页` },
};

const MODEL_LIST = [
  ["glm-4.7", "zcode", "glm-4.7", "ok"], ["glm-4.7-air", "zcode", "glm-4.7-air", "ok"],
  ["claude-sonnet-4-5", "gemini", "claude-sonnet-4-5 · Antigravity", "ok"],
  ["gemini-3-pro", "gemini", "gemini-3-pro", "ok"], ["gemini-3-flash", "cline", "gemini-3-flash", "ok"],
  ["qwen3-coder-plus", "qoder", "qwen3-coder-plus", "warn"],
  ["claude-opus-4-1", "buddy", "claude-opus-4-1", "warn"], ["deepseek-v3.1", "codearts", "deepseek-v3.1", "ok"],
  ["MiniMax-M2", "minimax", "MiniMax-M2", "ok"], ["gpt-5-codex", "opencode", "gpt-5-codex", "ok"],
  ["loomy-v3", "loomy", "loomy-v3", "err"], ["cc-instruct", "commandcode", "cc-instruct", "ok"],
];

/* ---------- 通用 UI ---------- */
function toast(type, title, desc) {
  const box = $(".toasts");
  const el = document.createElement("div");
  el.className = "toast " + type;
  el.innerHTML = ic(type === "ok" ? "okc" : type === "err" ? "alertr" : "info") + `<div class="tb">${esc(title)}${desc ? `<span class="d">${desc}</span>` : ""}</div>`;
  box.appendChild(el);
  setTimeout(() => { el.style.transition = "opacity .25s, transform .25s"; el.style.opacity = "0"; el.style.transform = "translateY(8px)"; setTimeout(() => el.remove(), 260); }, 3200);
}
function bindCopy(btn, text) {
  btn.addEventListener("click", () => {
    try { navigator.clipboard && navigator.clipboard.writeText(text); } catch (e) { /* 原型环境忽略 */ }
    const html = btn.innerHTML;
    btn.innerHTML = ic("check");
    setTimeout(() => { btn.innerHTML = html; }, 1200);
  });
}

/* ============================================================
   图表渲染器（全部手绘 SVG，AM 配色）
   ============================================================ */
function svgOpen(w, h) { return `<svg viewBox="0 0 ${w} ${h}" width="${w}" height="${h}" style="max-width:100%">`; }
const PALETTE = ["#3b82f6", "#06b6d4", "#10b981", "#8b5cf6", "#ec4899", "#f59e0b", "#f43f5e", "#6366f1"];

/* 面积/折线趋势图（网格 + 渐变填充 + 轴标签） */
function chartTrend(el, data, opt) {
  opt = opt || {};
  const W = el.clientWidth || 560, H = opt.h || 190;
  const padL = 44, padR = 10, padT = 14, padB = 24;
  const keys = Object.keys(data.models);
  const n = data.hours.length;
  const totals = data.hours.map((_, i) => SUM(keys.map(k => data.models[k][i])));
  const max = Math.max(...totals) * 1.12;
  const x = i => padL + (i / (n - 1)) * (W - padL - padR);
  const y = v => padT + (1 - v / max) * (H - padT - padB);
  const gid = "tg" + Math.round(rnd() * 1e6);
  let s = svgOpen(W, H);
  s += `<defs><linearGradient id="${gid}" x1="0" y1="0" x2="0" y2="1">
    <stop offset="0" stop-color="#3b82f6" stop-opacity=".38"/><stop offset="1" stop-color="#3b82f6" stop-opacity="0"/></linearGradient></defs>`;
  const ticks = 4;
  for (let t = 0; t <= ticks; t++) {
    const v = (max / ticks) * t, yy = y(v);
    s += `<line x1="${padL}" y1="${yy}" x2="${W - padR}" y2="${yy}" stroke="#94a3b8" stroke-opacity=".13" stroke-dasharray="3 3"/>`;
    s += `<text x="${padL - 7}" y="${yy + 3.5}" text-anchor="end" font-size="9.5" fill="#64748b" font-family="var(--mono)">${fmtK(v)}</text>`;
  }
  const step = Math.ceil(n / (W < 500 ? 6 : 12));
  data.hours.forEach((hh, i) => {
    if (i % step === 0 || i === n - 1) s += `<text x="${x(i)}" y="${H - 7}" text-anchor="middle" font-size="9" fill="#64748b" font-family="var(--mono)">${hh}</text>`;
  });
  let acc = new Array(n).fill(0);
  keys.forEach((k, ki) => {
    const vals = data.models[k], c = MODEL_COLOR[k] || PALETTE[ki % PALETTE.length];
    const top = vals.map((v, i) => acc[i] + v);
    let dArea = `M ${x(0)},${y(acc[0])}`;
    top.forEach((v, i) => { dArea += ` L ${x(i)},${y(v)}`; });
    for (let i = n - 1; i >= 0; i--) dArea += ` L ${x(i)},${y(acc[i])}`;
    dArea += " Z";
    if (ki === keys.length - 1) {
      s += `<path d="${dArea}" fill="url(#${gid})" stroke="none"/>`;
      s += `<path d="${top.map((v, i) => (i ? "L" : "M") + x(i) + "," + y(v)).join(" ")}" fill="none" stroke="#3b82f6" stroke-width="1.8" stroke-linejoin="round"/>`;
    } else {
      s += `<path d="${dArea}" fill="${c}" fill-opacity=".30" stroke="none"/>`;
      s += `<path d="${top.map((v, i) => (i ? "L" : "M") + x(i) + "," + y(v)).join(" ")}" fill="none" stroke="${c}" stroke-width="1.4" stroke-opacity=".9"/>`;
    }
    acc = top;
  });
  s += "</svg>";
  el.innerHTML = s;
}

/* 堆叠柱（输入/缓存/输出） */
function chartBars(el, rows, opt) {
  opt = opt || {};
  const W = el.clientWidth || 420, H = opt.h || 190;
  const padL = 40, padR = 6, padT = 12, padB = 24;
  const n = rows.length, bw = Math.min(38, (W - padL - padR) / n * 0.52);
  const series = opt.series || [["输入", "cin", "#3b82f6"], ["缓存", "cch", "#93c5fd"], ["输出", "cout", "#8b5cf6"]];
  const max = Math.max(...rows.map(r => SUM(series.map(s => r[s[1]])))) * 1.12;
  const x = i => padL + (i + 0.5) / n * (W - padL - padR);
  const y = v => padT + (1 - v / max) * (H - padT - padB);
  let s = svgOpen(W, H);
  for (let t = 0; t <= 4; t++) {
    const v = max / 4 * t, yy = y(v);
    s += `<line x1="${padL}" y1="${yy}" x2="${W - padR}" y2="${yy}" stroke="#94a3b8" stroke-opacity=".13" stroke-dasharray="3 3"/>`;
    s += `<text x="${padL - 7}" y="${yy + 3.5}" text-anchor="end" font-size="9.5" fill="#64748b" font-family="var(--mono)">${v.toFixed(1)}M</text>`;
  }
  rows.forEach((r, i) => {
    let acc = 0;
    series.forEach((se, si) => {
      const v = r[se[1]], y0 = y(acc), y1 = y(acc + v);
      s += `<rect x="${x(i) - bw / 2}" y="${y1}" width="${bw}" height="${Math.max(1, y0 - y1)}" fill="${se[2]}"${si === 2 ? ' rx="3"' : ""}/>`;
      acc += v;
    });
    s += `<text x="${x(i)}" y="${H - 7}" text-anchor="middle" font-size="9" fill="#64748b" font-family="var(--mono)">${r.d}</text>`;
  });
  s += "</svg>";
  el.innerHTML = s;
  return series;
}

/* 环图（Provider 占比） */
function chartDonut(el, data, opt) {
  opt = opt || {};
  const S = opt.size || 168, R = S / 2, r1 = R - 17, C = 2 * Math.PI * r1;
  const total = SUM(data.map(d => d.v));
  let off = 0, s = svgOpen(S, S);
  s += `<circle cx="${R}" cy="${R}" r="${r1}" fill="none" stroke="var(--card-2)" stroke-width="16"/>`;
  data.forEach((d, i) => {
    const c = PROVIDERS[d.k] ? PROVIDERS[d.k].color : PALETTE[i];
    const len = d.v / total * C;
    s += `<circle cx="${R}" cy="${R}" r="${r1}" fill="none" stroke="${c}" stroke-width="16" stroke-dasharray="${len - 2.5} ${C - len + 2.5}" stroke-dashoffset="${-off}" transform="rotate(-90 ${R} ${R})" stroke-linecap="round"><title>${esc(d.k)} ${d.v}%</title></circle>`;
    off += len;
  });
  s += `<text x="${R}" y="${R - 3}" text-anchor="middle" font-size="19" font-weight="700" fill="var(--t1)" font-family="var(--mono)">8,634</text>`;
  s += `<text x="${R}" y="${R + 14}" text-anchor="middle" font-size="9.5" fill="#64748b" font-family="var(--mono)">REQUESTS · 7D</text></svg>`;
  el.innerHTML = s;
}

/* 横向条形 */
function chartHbars(el, data) {
  const max = Math.max(...data.map(d => d.v));
  el.innerHTML = `<div class="hbars">` + data.map((d) => {
    const c = PROVIDERS[d.pv].color;
    return `<div class="hbar"><div class="nm"><i style="background:${c}"></i>${esc(d.n)}</div>
      <div class="track"><b style="width:${(d.v / max * 100).toFixed(1)}%;background:linear-gradient(90deg,${c}88,${c})"></b></div>
      <div class="val">${fmtK(d.v)}k</div></div>`;
  }).join("") + `</div>`;
}

/* 热力图 7×24 */
function chartHeat(el) {
  const max = Math.max(...HEAT.flat());
  const lv = v => {
    const t = v / max;
    if (t <= 0.02) return "var(--card-2)";
    if (t < 0.25) return "rgba(59,130,246,.22)";
    if (t < 0.5) return "rgba(59,130,246,.45)";
    if (t < 0.75) return "rgba(59,130,246,.7)";
    return "#3b82f6";
  };
  const days = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];
  el.innerHTML = `<div class="heat"><div class="hl">${days.map(d => `<span>${d}</span>`).join("")}</div>
    <div class="cells">${HEAT.map((row, d) => `<div class="crow">${row.map((v, h) =>
      `<i style="background:${lv(v)}" title="${days[d]} ${h}:00 · ${v} 次请求"></i>`).join("")}</div>`).join("")}</div></div>
    <div style="display:flex;align-items:center;gap:6px;margin-top:10px;font-size:10px;color:var(--t4);font-family:var(--mono)">
      0 <i style="width:34px;height:10px;border-radius:3px;background:linear-gradient(90deg,var(--card-2),rgba(59,130,246,.25),rgba(59,130,246,.6),#3b82f6)"></i> ${max}+
      <span class="grow"></span>UTC+8 · 按小时聚合</div>`;
}

/* 直方图 */
function chartHisto(el) {
  const max = Math.max(...LAT.counts);
  el.innerHTML = `<div class="histo">` + LAT.buckets.map((b, i) =>
    `<div class="b${i >= 4 ? " hot" : ""}"><em>${LAT.counts[i] ? fmt(LAT.counts[i]) : ""}</em>
     <i style="height:${Math.max(2, LAT.counts[i] / max * 100)}%"></i><span>${b}</span></div>`).join("") + `</div>
    <div class="mt-s" style="font-size:11px;color:var(--t4);font-family:var(--mono)">3,163 次请求 · 中位 <b style="color:var(--t2)">${LAT.p50}ms</b> · P95 <b style="color:#fbbf24">${fmt(LAT.p95)}ms</b></div>`;
}

/* 带刻度 SVG 仪表盘（zpool 式） */
function gaugeSVG(pct, opt) {
  opt = opt || {};
  const S = opt.size || 96, R = S / 2, r = R - 8;
  const A0 = -210, A1 = 30, col = pct >= 50 ? "#10b981" : pct >= 20 ? "#f59e0b" : "#f43f5e";
  const pt = (a, rr) => [R + rr * Math.cos(a * Math.PI / 180), R + rr * Math.sin(a * Math.PI / 180)];
  const arc = (a0, a1, rr) => {
    const [x0, y0] = pt(a0, rr), [x1, y1] = pt(a1, rr);
    return `M ${x0.toFixed(2)} ${y0.toFixed(2)} A ${rr} ${rr} 0 ${a1 - a0 > 180 ? 1 : 0} 1 ${x1.toFixed(2)} ${y1.toFixed(2)}`;
  };
  let s = svgOpen(S, S);
  s += `<g class="gauge" style="color:${col}">`;
  s += `<path d="${arc(A0, A1, r)}" fill="none" stroke="rgba(148,163,184,.18)" stroke-width="6" stroke-linecap="round"/>`;
  for (let a = A0; a <= A1; a += 6) {
    const major = (a - A0) % 30 === 0;
    const [x0, y0] = pt(a, r - 6.5), [x1, y1] = pt(a, r - (major ? 11 : 9));
    s += `<line x1="${x0.toFixed(1)}" y1="${y0.toFixed(1)}" x2="${x1.toFixed(1)}" y2="${y1.toFixed(1)}" stroke="#64748b" stroke-opacity="${major ? .7 : .35}" stroke-width="${major ? 1 : .6}"/>`;
  }
  const aFill = A0 + (A1 - A0) * pct / 100;
  s += `<path d="${arc(A0, aFill, r)}" fill="none" stroke="${col}" stroke-width="6" stroke-linecap="round" style="filter:drop-shadow(0 0 3px ${col}66)"/>`;
  const [cx, cy] = pt(aFill, r);
  s += `<circle cx="${cx.toFixed(1)}" cy="${cy.toFixed(1)}" r="2.6" fill="${col}"/>`;
  s += `<text x="${R}" y="${R + 5}" text-anchor="middle" font-size="${opt.big ? 17 : 13}" font-weight="700" fill="var(--t1)" font-family="var(--mono)">${pct}<tspan font-size="9" fill="#64748b">%</tspan></text>`;
  s += `</g></svg>`;
  return s;
}
function renderGauges(el) {
  const pools = [["zcode", 72, "2.9M / 4M"], ["gemini", 84, "84% 周窗"], ["trae", 61, "3,050 / 5,000"], ["qoder", 45, "9,000 / 20,000"], ["minimax", 39, "7,800 / 20,000"]];
  el.innerHTML = `<div class="gauges">` + pools.map(([k, p, sub]) =>
    `<div class="mg">${gaugeSVG(p, {})}<div class="gn" style="color:${PROVIDERS[k].color}">${PROVIDERS[k].name}</div><div class="gs">${sub}</div></div>`
  ).join("") + `<div class="mg" style="justify-content:center"><div class="up" style="margin-bottom:8px">池合计</div>
      <div class="mono" style="font-size:20px;font-weight:700">64.2%</div>
      <div class="gs">42 账号 · 37 可用</div></div></div>`;
}

/* 吞吐实时曲线 */
let THRU = Array.from({ length: 60 }, (_, i) => 150 + Math.sin(i * 0.5) * 40 + Math.sin(i * 0.13) * 30 + (i % 9 === 0 ? 35 : 0));
function chartThru(el) {
  const W = el.clientWidth || 640, H = 148;
  const padL = 42, padR = 8, padT = 12, padB = 20;
  const max = Math.max(...THRU) * 1.15;
  const x = i => padL + i / (THRU.length - 1) * (W - padL - padR);
  const y = v => padT + (1 - v / max) * (H - padT - padB);
  let s = svgOpen(W, H);
  s += `<defs><linearGradient id="thr-g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#3b82f6" stop-opacity=".42"/><stop offset="1" stop-color="#3b82f6" stop-opacity="0"/></linearGradient></defs>`;
  for (let t = 1; t <= 3; t++) {
    const yy = y(max / 3 * t);
    s += `<line x1="${padL}" y1="${yy}" x2="${W - padR}" y2="${yy}" stroke="#94a3b8" stroke-opacity=".12" stroke-dasharray="3 3"/>
          <text x="${padL - 7}" y="${yy + 3.5}" text-anchor="end" font-size="9.5" fill="#64748b" font-family="var(--mono)">${Math.round(max / 3 * t)}</text>`;
  }
  ["-120s", "-80s", "-40s", "now"].forEach((t, i) => {
    s += `<text x="${padL + (W - padL - padR) * (i / 3)}" y="${H - 5}" text-anchor="${i === 3 ? "end" : "middle"}" font-size="9" fill="#64748b" font-family="var(--mono)">${t}</text>`;
  });
  s += `<path d="M ${x(0)},${H - padB} ${THRU.map((v, i) => `L ${x(i).toFixed(1)},${y(v).toFixed(1)}`).join(" ")} L ${x(THRU.length - 1)},${H - padB} Z" fill="url(#thr-g)"/>`;
  s += `<path d="${THRU.map((v, i) => (i ? "L" : "M") + x(i).toFixed(1) + "," + y(v).toFixed(1)).join(" ")}" fill="none" stroke="#3b82f6" stroke-width="1.8" stroke-linejoin="round"/>`;
  const li = THRU.length - 1;
  s += `<circle cx="${x(li)}" cy="${y(THRU[li])}" r="3" fill="#3b82f6"/><circle cx="${x(li)}" cy="${y(THRU[li])}" r="6" fill="#3b82f6" opacity=".25"/>`;
  s += "</svg>";
  el.innerHTML = s;
  const cur = Math.round(THRU[li]);
  $("#kpi-thru").innerHTML = cur + `<small>tok/s</small>`;
  $("#kpi-thru-f").innerHTML = `峰值 ${Math.round(Math.max(...THRU))} · 首字节 <b class="okc">412ms</b>`;
  const setT = (id, v) => { const n = $(id); if (n) n.textContent = v; };
  setT("#thru-cur", cur + " tok/s");
  setT("#thru-max", Math.round(Math.max(...THRU)) + " tok/s");
  setT("#thru-avg", Math.round(SUM(THRU) / THRU.length) + " tok/s");
}

/* 拓扑图 */
function renderTopo(el) {
  const W = 560, H = 316;
  const box = (x, y, w, h, stroke, fill, label, sub) =>
    `<rect x="${x}" y="${y}" width="${w}" height="${h}" rx="9" fill="${fill}" stroke="${stroke}" stroke-width="1.2"/>
     <text x="${x + w / 2}" y="${y + (sub ? h / 2 - 2 : h / 2 + 4)}" text-anchor="middle" font-size="11.5" font-weight="600" fill="var(--t1)" font-family="var(--sans)">${label}</text>` +
    (sub ? `<text x="${x + w / 2}" y="${y + h / 2 + 13}" text-anchor="middle" font-size="9" fill="#64748b" font-family="var(--mono)">${sub}</text>` : "");
  const line = (x0, y0, x1, y1, c, dash) => `<path d="M ${x0} ${y0} C ${x0} ${(y0 + y1) / 2}, ${x1} ${(y0 + y1) / 2}, ${x1} ${y1}" fill="none" stroke="${c}" stroke-width="1.3"${dash ? ' stroke-dasharray="4 3"' : ""}/>`;
  let s = svgOpen(W, H);
  s += `<defs><linearGradient id="tp-g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="rgba(59,130,246,.2)"/><stop offset="1" stop-color="rgba(59,130,246,.06)"/></linearGradient></defs>`;
  s += `<text x="14" y="18" font-size="9.5" fill="#64748b" font-family="var(--mono)" letter-spacing="1">AI CODING CLIENTS</text>`;
  s += box(14, 28, 150, 34, "var(--line-2)", "var(--card)", "Claude Code", null);
  s += box(14, 70, 150, 34, "var(--line-2)", "var(--card)", "Cline / Continue", null);
  s += box(14, 112, 150, 34, "var(--line-2)", "var(--card)", "Codex CLI / SDK", null);
  s += line(89, 62, 230, 118, "#64748b"); s += line(89, 87, 230, 130, "#64748b"); s += line(89, 112, 230, 142, "#64748b");
  s += box(230, 96, 200, 68, "#3b82f6", "url(#tp-g)", "TokenMaster 网关", "127.0.0.1:8787 · OpenAI / Anthropic");
  s += `<text x="240" y="184" font-size="9" fill="#64748b" font-family="var(--mono)">路由 · 池化轮询 · 换号重试 · 账本 · 限流冷却</text>`;
  const pvShow = ["zcode", "gemini", "trae", "qoder", "minimax", "codearts"];
  s += `<text x="360" y="218" font-size="9.5" fill="#64748b" font-family="var(--mono)" letter-spacing="1">TOKEN POOLS × 15</text>`;
  s += line(330, 164, 402, 236, "#64748b"); s += line(330, 164, 466, 236, "#64748b");
  pvShow.forEach((k, i) => {
    const x = 356 + (i % 3) * 66, y = 228 + Math.floor(i / 3) * 38;
    s += box(x, y, 60, 30, PROVIDERS[k].color + "88", PROVIDERS[k].color + "1f", PROVIDERS[k].name, null);
    s += line(330, 164, x + 30, y, "#3f4a5f", true);
  });
  s += `<text x="356" y="312" font-size="9.5" fill="#64748b" font-family="var(--mono)">+9 more · 每池独立选号策略与临期分档</text></svg>`;
  el.innerHTML = s;
}

/* ============================================================
   页面渲染
   ============================================================ */
/* ---- 仪表盘 ---- */
function renderDash() {
  const box = $("#todo-list");
  box.innerHTML = TODOS.map((t, i) => {
    const pv = PROVIDERS[t.pv];
    return `<div class="todo" data-id="${t.id}">
      <div class="pv" style="background:${pv.color}">${pv.name[0]}</div>
      <div class="tx grow"><div class="tt">${t.t}</div><div class="td">${t.d}</div></div>
      <button class="btn sm ${t.act === "领取" ? "p" : ""}" data-todo="${i}">${t.act}</button>
    </div>`;
  }).join("");
  $$("#todo-list [data-todo]").forEach(btn => {
    btn.addEventListener("click", () => {
      const i = +btn.dataset.todo, t = TODOS[i], row = btn.closest(".todo");
      if (t.id === "claim") { openClaim(); return; }
      if (t.act === "详情" || t.act === "查看") { nav("accounts"); return; }
      btn.disabled = true;
      btn.innerHTML = `<span class="spin" style="display:inline-flex;width:14px;height:14px">${ic("refresh")}</span>`;
      setTimeout(() => {
        row.classList.add("done");
        row.querySelector(".btn").outerHTML = `<span class="bdg b-ok">${ic("check")} 已完成</span>`;
        toast("ok", (t.act === "签到" ? "签到成功" : "处理完成"), t.t + (t.act === "签到" ? " · 积分已入账" : ""));
        bumpTodo();
      }, 900);
    });
  });
  chartThru($("#thru-chart"));
  const seg = [["成功", 1150, "#10b981"], ["限流 429", 67, "#f59e0b"], ["鉴权失效 401", 21, "#f43f5e"], ["余额不足 402", 9, "#8b5cf6"]];
  const tot = SUM(seg.map(s => s[1]));
  $("#err-bar").innerHTML = seg.map(s => `<i style="flex:${s[1]};background:${s[2]}" title="${s[0]} ${s[1]}"></i>`).join("");
  $("#err-list").innerHTML = seg.map(s => `<div class="kv" style="padding:7px 0"><span style="display:flex;align-items:center;gap:7px" class="grow"><i style="width:8px;height:8px;border-radius:2.5px;background:${s[2]}"></i>${s[0]}</span><span class="mono" style="color:var(--t3)">${fmt(s[1])} · ${(s[1] / tot * 100).toFixed(1)}%</span></div>`).join("");
  renderGauges($("#pool-gauges"));
  $("#recent-tbl tbody").innerHTML = LOGS.slice(0, 7).map(r => logRow(r)).join("");
}

function bumpTodo() {
  const n = $$("#todo-list .todo").filter(r => !r.classList.contains("done")).length;
  $("#nav-dash .cnt").textContent = n;
  $("#todo-count").textContent = n;
  if (n === 0) $("#todo-empty").style.display = "flex";
}

/* ---- 账号 ---- */
let accFilter = "all", accQuery = "", accPage = 1;
function accCard(a) {
  const pv = PROVIDERS[a.pv];
  const [cls, lab] = STATE_LAB[a.state];
  const tierCls = a.tier === "ULTRA" ? "tier-ultra" : a.tier === "PRO" ? "tier-pro" : "tier-free";
  return `<div class="acc ${a.cur ? "cur" : ""} ${a.state === "cool" ? "cool" : ""} ${a.state === "dead" ? "dead" : ""}" data-state="${a.state}" data-name="${esc(a.name)}" data-pv="${a.pv}">
    <div class="hd">
      <input type="checkbox" class="cbx"/>
      <div class="pv" style="background:${pv.color}">${pv.name[0]}</div>
      <div class="grow" style="min-width:0">
        <div class="nm" ${a.cur ? ' style="color:#7dabf8"' : ""}>${esc(a.name)}</div>
        <div class="meta">${pv.name} · 更新 ${a.when}</div>
      </div>
      <div style="display:flex;flex-direction:column;gap:4px;align-items:flex-end">
        <span class="bdg ${cls}">${lab}</span>
        <span class="bdg tier ${tierCls}" style="font-size:9px">${a.tier}</span>
      </div>
    </div>
    ${a.cur ? `<div style="display:flex;align-items:center;gap:5px;font-size:10px;color:#7dabf8;font-weight:700">${ic("okc", 11)} 当前调度账号</div>` : ""}
    <div class="qs">${a.q.map(q => {
      const [m, p, r] = q, f = p >= 50 ? "f-ok" : p >= 20 ? "f-warn" : "f-err", pc = p >= 50 ? "p-ok" : p >= 20 ? "p-warn" : "p-err";
      const col = p >= 50 ? "var(--ok)" : p >= 20 ? "var(--warn)" : "var(--err)";
      const frozen = a.state === "cool" && r.indexOf(":") >= 0;
      return `<div class="qi"><div class="fill ${f}" style="width:${frozen ? 0 : Math.max(p, 1.5)}%;background:${col}"></div>
        <div class="row"><span class="m">${esc(m)}</span><span class="sp"></span>
        <span class="r">${ic("clock", 9)} R: ${r}</span><span class="p ${pc}">${p}%</span></div></div>`;
    }).join("")}</div>
    ${a.note ? `<div class="note ${a.note[0]}">${a.note[1]}</div>` : ""}
    <div class="ft">
      <span class="when">id: ${a.pv}-${(a.name.length * 7 % 9000 + 1000)}</span>
      <button class="ibtn" title="详情">${ic("info")}</button>
      <button class="ibtn" title="刷新额度">${ic("refresh")}</button>
      <button class="ibtn ok act-ci" title="签到">${ic("cal")}</button>
      <button class="ibtn warn act-pw" title="${a.state === "off" ? "启用" : "停用"}">${ic("power")}</button>
      <button class="ibtn err" title="删除">${ic("trash")}</button>
    </div>
  </div>`;
}
function renderAccounts() {
  const counts = { all: ACCOUNTS.length };
  ["ok", "cool", "exp", "dead", "off"].forEach(s => counts[s] = ACCOUNTS.filter(a => a.state === s).length);
  const segBox = $("#acc-filters");
  segBox.innerHTML = [["all", "全部"], ["ok", "可用"], ["cool", "限流"], ["exp", "临期"], ["dead", "异常"], ["off", "停用"]]
    .map(([k, l]) => `<button class="${accFilter === k ? "on" : ""}" data-f="${k}">${l} <span class="mono" style="opacity:.65">${counts[k]}</span></button>`).join("");
  $$("#acc-filters button").forEach(b => b.addEventListener("click", () => { accFilter = b.dataset.f; accPage = 1; renderAccounts(); }));

  let list = accFilter === "all" ? ACCOUNTS : ACCOUNTS.filter(a => a.state === accFilter);
  if (accQuery) list = list.filter(a => a.name.toLowerCase().includes(accQuery));
  const per = 12, pages = Math.max(1, Math.ceil(list.length / per));
  accPage = Math.min(accPage, pages);
  const page = list.slice((accPage - 1) * per, accPage * per);
  $("#acc-grid").innerHTML = page.map(accCard).join("");
  $("#acc-count").textContent = `显示 ${page.length} / ${list.length} 个账号 · 共 42 个 · 15 家 Provider`;
  const pg = $("#acc-pager");
  pg.innerHTML = `<button data-p="prev" ${accPage === 1 ? "disabled" : ""}>${ic("chevL")}</button>` +
    Array.from({ length: pages }, (_, i) => `<button data-p="${i + 1}" class="${accPage === i + 1 ? "on" : ""}">${i + 1}</button>`).join("") +
    `<button data-p="next" ${accPage === pages ? "disabled" : ""}>${ic("chevR")}</button>`;
  $$("#acc-pager button").forEach(b => b.addEventListener("click", () => {
    if (b.dataset.p === "prev") accPage--; else if (b.dataset.p === "next") accPage++; else accPage = +b.dataset.p;
    renderAccounts();
  }));
  $$("#acc-grid .acc").forEach(card => {
    card.querySelector(".act-ci").addEventListener("click", e => { e.stopPropagation(); toast("ok", "签到成功", card.dataset.name + " · 今日积分已入账"); });
    card.querySelector(".act-pw").addEventListener("click", e => {
      e.stopPropagation();
      const a = ACCOUNTS.find(x => x.name === card.dataset.name);
      if (!a) return;
      if (a.state === "off") { a.state = "ok"; toast("ok", "账号已启用", a.name + " · 重新参与轮询"); }
      else { a.state = "off"; toast("info", "账号已停用", a.name + " · 已移出轮询"); }
      renderAccounts();
    });
  });
}

/* 添加账号模态 */
function openAdd() {
  $("#modal-add").classList.add("open");
  addStep("pick");
}
function addStep(step) {
  const body = $("#add-body");
  $("#add-foot-next").style.display = "none";
  if (step === "pick") {
    body.innerHTML = `
      <div class="fld" style="margin-bottom:12px"><span class="up">选择 Provider</span>
      <select class="inp" style="width:100%" id="add-pv">${PV_KEYS.map(k => `<option value="${k}">${PROVIDERS[k].name}</option>`).join("")}</select></div>
      <span class="up">登录方式（按 Provider 能力自动给出）</span>
      <div class="mt-s">
      <button class="opt" data-m="flow"><span class="chipic c-blue">${ic("globe")}</span><span class="ocol"><span class="oname">浏览器 OAuth 回调</span><span class="odesc">拉起默认浏览器授权，本地回环接收回调 · codearts / lobsterai / trae / gemini</span></span>${ic("chevR")}</button>
      <button class="opt" data-m="flow"><span class="chipic c-ok">${ic("refresh")}</span><span class="ocol"><span class="oname">设备码轮询</span><span class="odesc">打开授权页输入设备码，GUI 轮询换凭据 · qoder / cline / minimax / zcode</span></span>${ic("chevR")}</button>
      <button class="opt" data-m="flow"><span class="chipic c-violet">${ic("grid")}</span><span class="ocol"><span class="oname">扫码 / 短信本地页</span><span class="odesc">内嵌 WebView 展示二维码或短信验证 · loomy / raccoon</span></span>${ic("chevR")}</button>
      <button class="opt" data-m="paste"><span class="chipic c-cyan">${ic("term")}</span><span class="ocol"><span class="oname">手动粘贴 token / key</span><span class="odesc">按 Provider 提示所需字段与格式 · opencode / commandcode 等</span></span>${ic("chevR")}</button>
      </div>`;
    $$("#add-body .opt").forEach(o => o.addEventListener("click", () => addStep(o.dataset.m)));
  } else if (step === "paste") {
    body.innerHTML = `
      <div class="mnote" style="margin-bottom:12px">以 <b>zcode</b> 为例：粘贴官方 credentials.json 内容或 <span class="mono">zcodejwttoken</span>，支持 <span class="mono">enc:v1:</span> 加密格式（密钥派生与官方一致）。</div>
      <textarea class="inp mono" rows="5" style="width:100%" placeholder="粘贴 JSON 或 token…"></textarea>
      <div class="mt-s"><label class="rlabel on" id="paste-refresh"><span class="rdot"></span><span><span class="rt">导入后立即刷新额度</span><span class="rd">入池补刷新一轮，马上可参与轮询</span></span></label></div>`;
    $("#add-foot-next").style.display = "";
  } else {
    body.innerHTML = `
      <div class="mnote" style="margin-bottom:12px">以 <b>zcode · 设备码轮询</b> 为例：</div>
      <div style="display:flex;flex-wrap:wrap;gap:6px;margin-bottom:14px">
        <span class="bdg b-ok">${ic("check")} 打开授权页</span>
        <span class="bdg b-blue">${ic("activity")} 等待授权 · 轮询中</span>
        <span class="bdg b-gray">兑换凭据</span>
        <span class="bdg b-gray">入池 + 补刷新</span>
      </div>
      <div class="keybox" style="margin-bottom:10px">设备码：<b>HGKF-LMNP-QSTX</b><button class="ibtn eye" data-copy>${ic("copy")}</button></div>
      <div class="mnote">打开 <span class="mono" style="color:#7dabf8">auth.z.ai/device</span> 输入设备码；授权完成后本窗口自动进入下一步。超时 300s，可重新生成。</div>`;
    bindCopy($("#add-body [data-copy]"), "HGKF-LMNP-QSTX");
  }
}
function openClaim() {
  $("#modal-claim").classList.add("open");
  $("#claim-body").innerHTML = `<div style="display:flex;flex-direction:column;align-items:center;gap:12px;padding:18px 0 10px">
    <span class="chipic c-blue" style="width:44px;height:44px;border-radius:12px">${ic("shield", 22)}</span>
    <div style="font-weight:700;font-size:14px">验证码载体运行中</div>
    <div class="mnote" style="text-align:center;max-width:330px">已按需拉起 WebView 过码环境（无感验证），成功后自动提交 <span class="mono">billing/claim</span>；8 秒无感失败将亮出窗口转人工。</div>
    <div class="mono" style="font-size:11px;color:var(--t4)">account: zc-moonton@z.ai · plan: GLM Coding Pro 14d</div>
    <button class="btn p" id="claim-done" style="margin-top:4px">${ic("check")} 模拟过码成功</button>
  </div>`;
  $("#claim-done").addEventListener("click", () => {
    $("#modal-claim").classList.remove("open");
    const row = $('#todo-list [data-id="claim"]');
    if (row) { row.classList.add("done"); row.querySelector(".btn").outerHTML = `<span class="bdg b-ok">${ic("check")} 已领取</span>`; }
    toast("ok", "套餐领取成功", "GLM Coding Pro · 14 天已入账 zc-moonton@z.ai");
    bumpTodo();
  });
}

/* zcode-pool 导入模态 */
function openImport() {
  $("#modal-import").classList.add("open");
  $("#import-list").innerHTML = ["zc-wang.dev@z.ai", "zc-moonton@z.ai", "zc-qingfeng@z.ai", "zc-alpha@z.ai", "zc-beta@z.ai"]
    .map((n, i) => `<label class="kv" style="cursor:pointer"><input type="checkbox" class="cbx" checked style="border-radius:50%"/><span class="grow mono" style="font-size:12px">${n}</span><span class="bdg ${i === 2 ? "b-warn" : "b-ok"}">${i === 2 ? "临期积分" : "正常"}</span></label>`).join("");
}

/* ---- 网关 ---- */
function renderGateway() {
  const port = $("#gw-port").value || 8787;
  $("#gw-base").textContent = `http://127.0.0.1:${port}`;
  $("#map-tbl tbody").innerHTML = MAPS.map(m => `<tr>
    <td><span class="prio">${m.prio}</span></td>
    <td class="mono" style="font-weight:600;color:var(--t1)">${esc(m.bare)}</td>
    <td style="color:var(--t4);width:20px">${ic("arrowR", 13)}</td>
    <td class="mono" style="color:var(--t3);font-size:11.5px">${esc(m.to)}</td>
    <td class="num" style="color:var(--t3)">${fmt(m.hits)}</td>
    <td style="width:60px"><button class="ibtn">${ic("gear")}</button><button class="ibtn err">${ic("trash")}</button></td>
  </tr>`).join("");
}
function setGw(on) {
  $("#gw-chip").classList.toggle("off", !on);
  $("#gw-chip-txt").textContent = on ? "运行中 · :8787" : "已停止";
  $("#gw-state").innerHTML = on ? `<span class="bdg b-ok" style="font-size:11px">${ic("play")} 运行中</span>` : `<span class="bdg b-gray" style="font-size:11px">${ic("pause")} 已停止</span>`;
  $("#kpi-gw").innerHTML = on ? `<span class="okc">运行中</span>` : `<span style="color:var(--t4)">已停止</span>`;
  $("#kpi-gw-f").textContent = on ? "127.0.0.1:8787 · 双协议面" : "端口 8787 已释放";
  toast(on ? "ok" : "info", on ? "网关已启动" : "网关已停止", on ? "监听 127.0.0.1:8787 · OpenAI / Anthropic 双协议面就绪" : "进行中的流式请求已优雅收尾");
}

/* ---- 用量 ---- */
let usageRange = "近 7 天";
function renderUsage() {
  $$("#usage-seg button").forEach(b => b.classList.toggle("on", b.dataset.r === usageRange));
  const set = USAGE_SETS[usageRange];
  const mult = usageRange === "今日" ? 1 : usageRange === "近 7 天" ? 6.9 : 29.4;
  $("#g-req").textContent = fmt(1247 * mult);
  $("#g-in").textContent = (1.62 * mult).toFixed(1) + "M";
  $("#g-out").textContent = (0.51 * mult).toFixed(1) + "M";
  $("#g-ch").textContent = (0.18 * mult).toFixed(2) + "M";
  $("#g-ttfb").textContent = "412ms";
  $("#g-rate").textContent = usageRange === "今日" ? "98.4%" : "98.2%";
  chartTrend($("#u-trend"), set, { h: 210 });
  $("#u-trend-legend").innerHTML = Object.keys(set.models).map(k => `<span class="li"><i style="background:${MODEL_COLOR[k]}"></i>${k}</span>`).join("");
  const series = chartBars($("#u-bars"), DAYS7.map((d, i) => ({ d, cin: TOK7[i].cin, cout: TOK7[i].cout, cch: TOK7[i].cch })), { h: 210 });
  $("#u-bars-legend").innerHTML = series.map(s => `<span class="li"><i style="background:${s[2]}"></i>${s[0]}</span>`).join("");
  chartDonut($("#u-donut"), DONUT);
  $("#u-donut-legend").innerHTML = DONUT.map(d => `<span class="li"><i style="background:${PROVIDERS[d.k].color}"></i>${PROVIDERS[d.k].name} <b style="color:var(--t2)">${d.v}%</b></span>`).join("");
  chartHbars($("#u-top"), TOPACCT);
  chartHisto($("#u-histo"));
  chartHeat($("#u-heat"));
  $("#u-tbl tbody").innerHTML = TOK7.map(r => `<tr>
    <td class="mono">${r.d}</td><td class="num">${fmt(r.req)}</td>
    <td class="num">${r.cin.toFixed(2)}M</td><td class="num">${r.cout.toFixed(2)}M</td><td class="num">${r.cch.toFixed(2)}M</td>
    <td class="num" style="color:${r.fail > 12 ? "var(--warn)" : "var(--t3)"}">${r.fail}</td><td class="num">${r.ttfb}ms</td>
  </tr>`).join("") + `<tr style="color:var(--t4)"><td class="mono">合计</td><td class="num">7,191</td><td class="num">8.90M</td><td class="num">2.84M</td><td class="num">0.89M</td><td class="num">71</td><td class="num">434ms</td></tr>`;
}

/* ---- 日志 ---- */
let logFilter = "all";
function logRow(r) {
  const stCls = r.st >= 200 && r.st < 400 ? "b-ok" : r.st === 429 ? "b-warn" : r.st === 3007 ? "b-violet" : "b-err";
  return `<tr data-cls="${r.cls}" ${r.trail ? ' class="has-trail"' : ""}>
    <td><span class="bdg solid ${stCls}">${r.st}</span></td>
    <td class="mono" style="color:var(--t3)">${r.proto === "gateway" ? "GW" : "POST"}</td>
    <td><span class="model">${esc(r.model)}</span></td>
    <td><span class="acct">${esc(r.acct)}</span></td>
    <td><span class="path">${esc(r.path)}</span></td>
    <td class="num" style="color:var(--t3)">${r.tok}</td>
    <td class="num">${r.ttfb === "—" ? "—" : r.ttfb + "ms"}</td>
    <td class="num" style="color:var(--t3)">${r.dur}</td>
    <td class="num" style="color:var(--t4)">${r.time}</td>
  </tr>${r.trail ? `<tr class="tr-x"><td colspan="9" style="padding:0">${trailHTML(r.trail)}</td></tr>` : ""}`;
}
function trailHTML(steps) {
  return `<div class="trail"><div class="t-h">换号轨迹 · ${steps.filter(s => s.s !== "none").length} 次尝试</div>
    <div class="steps">${steps.map((st, i) => `
      <div class="step ${st.s === "ok" ? "final" : "done"}">
        <div class="bar"><i></i></div>
        <div class="tt" style="color:${st.s === "ok" ? "#34d399" : st.s === "none" ? "#64748b" : "#fb7185"}">${i + 1}. ${esc(st.t)}</div>
        <div class="td">${st.d}</div>
      </div>`).join("")}</div></div>`;
}
function renderLogs() {
  $$("#log-filters .fpill").forEach(b => b.classList.toggle("on", b.dataset.f === logFilter));
  const rows = logFilter === "all" ? LOGS : LOGS.filter(r => r.cls === logFilter);
  $("#log-tbl tbody").innerHTML = rows.map(r => logRow(r)).join("") || `<tr><td colspan="9" style="text-align:center;padding:40px;color:var(--t4)">该分类暂无日志</td></tr>`;
  $("#log-count").textContent = `${rows.length} 条 · 今日 1,247 · 账本滚动 32MB`;
  $$("#log-tbl tr.has-trail").forEach(tr => tr.addEventListener("click", () => {
    const x = tr.nextElementSibling;
    const open = x.style.display === "table-row";
    x.style.display = open ? "none" : "table-row";
    tr.classList.toggle("exp", !open);
  }));
}

/* ---- 接入 ---- */
let setupTab = "Claude Code";
function renderSetup() {
  $$("#setup-tabs button").forEach(b => b.classList.toggle("on", b.dataset.t === setupTab));
  const s = SETUPS[setupTab];
  $("#setup-code .fl").textContent = s.lang;
  $("#setup-code pre").innerHTML = esc(s.code)
    .replace(/(#[^\n]*)/g, '<span class="cm">$1</span>')
    .replace(/(&quot;[^&]*&quot;:)/g, '<span class="kw">$1</span>');
  $("#model-tbl tbody").innerHTML = MODEL_LIST.map(m => `<tr>
    <td class="mono" style="font-weight:600;color:var(--t1)">${m[0]}</td>
    <td><span style="display:inline-flex;align-items:center;gap:6px;font-size:11.5px"><i class="pv" style="width:18px;height:18px;font-size:9px;border-radius:5px;background:${PROVIDERS[m[1]].color}">${PROVIDERS[m[1]].name[0]}</i><span style="color:var(--t3)">${PROVIDERS[m[1]].name}</span></span></td>
    <td class="mono" style="color:var(--t3);font-size:11.5px">${esc(m[2])}</td>
    <td>${m[3] === "ok" ? '<span class="bdg b-ok">可用</span>' : m[3] === "warn" ? '<span class="bdg b-warn">部分限流</span>' : '<span class="bdg b-err">池失效</span>'}</td>
  </tr>`).join("");
}

/* ---- 设置（静态，交互已在 init 绑定） ---- */
function renderSettings() { /* 无动态内容 */ }

/* ============================================================
   导航与初始化
   ============================================================ */
const RENDERERS = { dash: renderDash, accounts: renderAccounts, gateway: renderGateway, usage: renderUsage, logs: renderLogs, setup: renderSetup, settings: renderSettings };
function nav(name) {
  $$(".nav-pills button").forEach(b => b.classList.toggle("on", b.dataset.page === name));
  $$(".page").forEach(p => p.classList.toggle("on", p.id === "page-" + name));
  RENDERERS[name] && RENDERERS[name]();
  $(".content").scrollTop = 0;
  if (location.hash !== "#" + name) location.hash = name;
}
function init() {
  /* 静态占位图标与按钮文案填充 */
  const FILL = [
    ["#nav-gw-ic", ic("gear")],
    ["#btn-refresh-quota", ic("refresh") + "刷新额度"],
    ["#btn-add2", ic("plus") + "添加账号"],
    ["#btn-import", ic("upload") + "从 zcode-pool 导入"],
    ["#btn-add", ic("plus") + "添加账号"],
    ["#map-add", ic("plus") + "添加规则"],
    ["#btn-export", ic("download") + "导出 JSONL"],
    ["#btn-open-dir", ic("folder") + "打开数据目录"],
    ["#btn-clear-log", ic("trash") + "清空"],
    ["#add-x", ic("x")], ["#claim-x", ic("x")], ["#import-x", ic("x")],
    ["#key-eye", ic("eye")], ["#key-copy", ic("copy")], ["#base-copy", ic("copy")],
    ["#setup-copy", ic("copy")],
    ["#ic-thru", ic("activity")], ["#ic-todo", ic("inbox")], ["#ic-gauge", ic("gauge")],
    ["#ic-err", ic("alertr")], ["#ic-recent", ic("term")], ["#ic-server", ic("server")],
    ["#ic-key", ic("key")], ["#ic-map", ic("list")], ["#ic-strategy", ic("users")],
    ["#ic-topo", ic("globe")], ["#ic-trend", ic("activity")], ["#ic-bars", ic("db")],
    ["#ic-donut", ic("gauge")], ["#ic-top", ic("users")], ["#ic-lat", ic("clock")],
    ["#ic-heat", ic("grid")], ["#ic-day", ic("list")], ["#ic-setup", ic("book")],
    ["#ic-bell", ic("shield")], ["#ic-code", ic("term")], ["#ic-models", ic("zap")],
    ["#ic-general", ic("gear")], ["#ic-proxy", ic("globe")], ["#ic-data", ic("folder")],
    ["#ic-about", ic("info")], ["#ic-add", ic("plus")], ["#ic-claim", ic("gift")],
    ["#ic-import", ic("upload")],
  ];
  FILL.forEach(([sel, html]) => { const el = $(sel); if (el) el.innerHTML = html; });
  $$(".st-gear").forEach(b => b.innerHTML = ic("gear"));
  $$(".search").forEach(s => s.insertAdjacentHTML("afterbegin", ic("search")));
  /* Provider 下拉 */
  $("#acc-pv").innerHTML = "<option>全部 Provider</option>" + PV_KEYS.map(k => `<option>${PROVIDERS[k].name}</option>`).join("");

  $$(".nav-pills button").forEach(b => b.addEventListener("click", () => nav(b.dataset.page)));
  $("#acc-search").addEventListener("input", e => { accQuery = e.target.value.trim().toLowerCase(); accPage = 1; renderAccounts(); });
  $("#gw-port").addEventListener("change", renderGateway);
  $("#gw-toggle").addEventListener("click", () => {
    const sw = $("#gw-sw");
    sw.classList.toggle("on");
    setGw(sw.classList.contains("on"));
  });
  let keyShown = false;
  $("#key-eye").addEventListener("click", () => {
    keyShown = !keyShown;
    $("#key-val").textContent = keyShown ? KEY_FULL : "sk-tm-••••••••••••7f3a";
    $("#key-eye").innerHTML = ic(keyShown ? "eyeoff" : "eye");
  });
  bindCopy($("#key-copy"), KEY_FULL);
  bindCopy($("#base-copy"), "http://127.0.0.1:8787");
  $("#btn-add").addEventListener("click", openAdd);
  $("#btn-add2").addEventListener("click", openAdd);
  $("#btn-import").addEventListener("click", openImport);
  $$(".ov").forEach(ov => {
    ov.addEventListener("click", e => { if (e.target === ov) ov.classList.remove("open"); });
    $$("[data-close]", ov).forEach(b => b.addEventListener("click", () => ov.classList.remove("open")));
  });
  $("#add-foot-next").addEventListener("click", () => {
    $("#modal-add").classList.remove("open");
    toast("ok", "账号已添加", "凭据已加密写入 ~/.tokenmaster/accounts/ · 已入池并补刷新");
  });
  $("#import-go").addEventListener("click", () => {
    $("#modal-import").classList.remove("open");
    toast("ok", "导入完成", "4 个 zcode 账号已转换入池（1 个重复已跳过）");
  });
  $$("#log-filters .fpill").forEach(b => b.addEventListener("click", () => { logFilter = b.dataset.f; renderLogs(); }));
  $$("#usage-seg button").forEach(b => b.addEventListener("click", () => { usageRange = b.dataset.r; renderUsage(); }));
  $$("#setup-tabs button").forEach(b => b.addEventListener("click", () => { setupTab = b.dataset.t; renderSetup(); }));
  bindCopy($("#setup-copy"), "http://127.0.0.1:8787");
  /* 设置页：开关类直接切换（原型行为） */
  $$(".sw[data-toggle]").forEach(sw => sw.addEventListener("click", () => sw.classList.toggle("on")));
  $("#proxy-test").addEventListener("click", () => toast("ok", "代理连通", "socks5://127.0.0.1:7890 → gemini 上游 236ms"));
  /* 冷却倒计时（限流演示） */
  setInterval(() => {
    const el = $("#cool-timer");
    if (!el) return;
    let [m, s] = el.textContent.split(":").map(Number);
    s--; if (s < 0) { s = 59; m--; }
    if (m < 0) return;
    el.textContent = String(m).padStart(2, "0") + ":" + String(s).padStart(2, "0");
  }, 1000);
  /* 实时吞吐 */
  setInterval(() => {
    if (!$("#page-dash").classList.contains("on")) return;
    const last = THRU[THRU.length - 1];
    THRU = [...THRU.slice(1), Math.max(60, Math.min(320, last + (rnd() - 0.48) * 46))];
    chartThru($("#thru-chart"));
  }, 2000);
  const q = new URLSearchParams(location.search);
  /* 截图长图模式：放开内部滚动容器，让 document 自然长高 */
  if (q.get("scroll") === "full") {
    document.documentElement.style.height = "auto";
    document.body.style.height = "auto";
    const c = $(".content");
    c.style.overflow = "visible";
    c.style.height = "auto";
    c.style.flex = "none";
  }
  const h = q.get("page") || location.hash.replace("#", "");
  nav(RENDERERS[h] ? h : "dash");
  if (q.get("accfilter")) { accFilter = q.get("accfilter"); renderAccounts(); }
  if (q.get("modal") === "add") openAdd();
  if (q.get("modal") === "import") openImport();
  if (q.get("modal") === "claim") openClaim();
  bumpTodo();
}
document.addEventListener("DOMContentLoaded", init);
let __rz;
window.addEventListener("resize", () => {
  clearTimeout(__rz);
  __rz = setTimeout(() => { const cur = $$(".page").find(p => p.classList.contains("on")); if (cur) RENDERERS[cur.id.replace("page-", "")](); }, 180);
});
