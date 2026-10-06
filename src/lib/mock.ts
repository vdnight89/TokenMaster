/**
 * Round 02 原型 mock 数据（移植自 design/round-02/app.js 的 DATA 部分）。
 * 模拟数据仅用于驱动 UI 演示，跨页保持一致；所有密钥均为掩码占位。
 * 后续任务替换为 IPC 真数据时保持本文件的类型形状。
 */

/* ---------- Provider ---------- */
export const PROVIDERS = {
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
} as const;

export type PvKey = keyof typeof PROVIDERS;
export const PV_KEYS = Object.keys(PROVIDERS) as PvKey[];

export const MODEL_COLOR: Record<string, string> = {
  "glm-4.7": "#3b82f6",
  "claude-sonnet-4-5": "#06b6d4",
  "gemini-3-pro": "#10b981",
  "qwen3-coder-plus": "#8b5cf6",
  "claude-opus-4-1": "#14b8a6",
  "deepseek-v3.1": "#0ea5e9",
  "MiniMax-M2": "#ec4899",
};

/* ---------- 账号 ---------- */
export type AccState = "ok" | "cool" | "exp" | "dead" | "off";

export const STATE_LAB: Record<AccState, readonly [string, string]> = {
  ok: ["b-ok", "正常"],
  cool: ["b-warn", "限流冷却"],
  exp: ["b-warn", "临期"],
  dead: ["b-err", "失效"],
  off: ["b-gray", "停用"],
};

/** 账号卡提示条：结构化段落（替代原型 note 里的 <b> 标记，由 JSX 渲染） */
export interface NoteSeg {
  text: string;
  /** 加粗 */
  b?: boolean;
  /** 等宽字体 */
  mono?: boolean;
  /** mm:ss 实时倒计时（限流冷却） */
  timer?: boolean;
}
export interface AccNote {
  kind: "warn" | "err";
  icon: string;
  segs: NoteSeg[];
}

export interface Quota {
  m: string;
  p: number;
  /** 剩余百分比对应的重置/过期时间；生成号为原型遗留的数字形态 */
  r: string | number;
}

export interface Account {
  pv: PvKey;
  name: string;
  state: AccState;
  tier: string;
  cur?: boolean;
  q: Quota[];
  when: string;
  note?: AccNote;
}

const q = (m: string, p: number, r: string | number): Quota => ({ m, p, r });

/* 手写有戏的号 + 生成的普通号，共 42 */
const NAMED: Account[] = [
  { pv: "zcode", name: "zc-wang.dev@z.ai", state: "ok", tier: "PRO", cur: false, q: [q("glm-4.7", 68, "5h12m"), q("glm-4.7-air", 91, "12h40m")], when: "10-06 13:02" },
  {
    pv: "zcode", name: "zc-moonton@z.ai", state: "cool", tier: "PRO",
    note: { kind: "warn", icon: "clock", segs: [{ text: "glm-4.7 限流冷却 " }, { text: "04:32", b: true, mono: true, timer: true }, { text: " · 429" }] },
    q: [q("glm-4.7", 0, "04:32"), q("glm-4.7-air", 84, "6h20m")], when: "10-06 12:47",
  },
  {
    pv: "zcode", name: "zc-qingfeng@z.ai", state: "exp", tier: "PRO",
    note: { kind: "warn", icon: "gift", segs: [{ text: "积分 " }, { text: "1,240", b: true, mono: true }, { text: " 将于 " }, { text: "3 天后", b: true }, { text: "作废 · 已优先消耗" }] },
    q: [q("glm-4.7", 42, "2d18h"), q("glm-4.7-air", 66, "2d18h")], when: "10-06 11:31",
  },
  { pv: "gemini", name: "sabrina.schell@gmail.com", state: "ok", tier: "ULTRA", cur: true, q: [q("gemini-3-pro", 84, "1h15m"), q("claude-sonnet-4-5", 100, "4h22m")], when: "10-06 13:10" },
  { pv: "gemini", name: "kkevin.lee@gmail.com", state: "ok", tier: "PRO", q: [q("gemini-3-pro", 91, "2h40m"), q("claude-sonnet-4-5", 78, "2h40m")], when: "10-06 09:54" },
  { pv: "trae", name: "dev.zhang@trae.ai", state: "ok", tier: "PRO", q: [q("claude-sonnet-4-5", 61, "2h08m")], when: "10-06 12:18" },
  { pv: "trae", name: "solo.wang@trae.ai", state: "off", tier: "PRO", q: [q("claude-sonnet-4-5", 33, "—")], when: "10-05 21:40" },
  { pv: "qoder", name: "q.lin@qoder.com", state: "ok", tier: "PRO", q: [q("qwen3-coder-plus", 45, "3h30m")], when: "10-06 10:26" },
  {
    pv: "minimax", name: "mm.chen@minimax.io", state: "exp", tier: "PRO",
    note: { kind: "warn", icon: "key", segs: [{ text: "access token " }, { text: "22h", b: true, mono: true }, { text: " 后过期 · 将自动刷新" }] },
    q: [q("MiniMax-M2", 39, "6h05m")], when: "10-06 08:12",
  },
  { pv: "codearts", name: "ca.liu@huawei.com", state: "ok", tier: "PRO", q: [q("deepseek-v3.1", 66, "4h50m")], when: "10-06 13:00" },
  {
    pv: "loomy", name: "lm.guo@loomy.app", state: "dead", tier: "FREE",
    note: { kind: "err", icon: "alertr", segs: [{ text: "会话已失效 · 不可续期，需" }, { text: "重新登录", b: true }] },
    q: [q("loomy-v3", 0, "—")], when: "10-04 17:22",
  },
  { pv: "buddy", name: "bd.sun@buddy.ai", state: "ok", tier: "PRO", q: [q("claude-opus-4-1", 52, "1h47m")], when: "10-06 12:55" },
];

const NAME_POOL = ["chr", "jam", "mik", "sar", "leo", "ann", "tom", "eva", "kai", "liu", "yan", "qi", "hao", "fei", "yun", "han", "xin", "lin", "ke", "rou", "shen", "meng", "du", "lu", "cao", "pan", "xu", "den", "wu", "gao"];

function genAccounts(): Account[] {
  const list: Account[] = NAMED.map(a => ({ ...a }));
  const per: Record<PvKey, number> = { qodercn: 2, workbuddy: 2, lobsterai: 2, cline: 3, raccoon: 2, opencode: 3, commandcode: 1, zcode: 2, gemini: 4, trae: 2, qoder: 2, minimax: 2, codearts: 3, buddy: 1, loomy: 1 };
  let i = 0;
  for (const [pv, n] of Object.entries(per) as [PvKey, number][]) {
    for (let k = 0; k < n; k++) {
      const m = PROVIDERS[pv].models[0];
      const nm = NAME_POOL[i % NAME_POOL.length] + "." + NAME_POOL[(i + 7) % NAME_POOL.length] + "@" + pv.split(/(?=[A-Z])/)[0].toLowerCase() + ".io";
      list.push({ pv, name: nm, state: "ok", tier: PROVIDERS[pv].tier, q: [q(m[0], 30 + ((i * 17) % 65), m[1])], when: "10-0" + (1 + (i % 6)) + " 0" + (1 + (i % 9)) + ":1" + (i % 6) });
      i++;
    }
  }
  return list.slice(0, 42);
}

export const ACCOUNTS: Account[] = genAccounts();

/* ---------- 今日待办 ---------- */
export interface Todo {
  id: string;
  pv: PvKey;
  icon: string;
  t: string;
  d: string;
  act: string;
}

export const TODOS: Todo[] = [
  { id: "claim", pv: "zcode", icon: "gift", t: "zcode 可领套餐：GLM Coding Pro · 14 天", d: "已检测到可领套餐 · 领取需过验证码（将拉起验证码载体）", act: "领取" },
  { id: "ci-codearts", pv: "codearts", icon: "cal", t: "codearts 每日签到", d: "连续 12 天 · 今日 +50 积分", act: "签到" },
  { id: "ci-buddy", pv: "buddy", icon: "cal", t: "buddy 每日签到", d: "连续 5 天 · 今日 +30 积分", act: "签到" },
  { id: "exp-gem", pv: "gemini", icon: "clock", t: "临期额度：sabrina.schell 剩余 $4.20", d: "48 小时后作废 · 选号已按临期优先", act: "查看" },
  { id: "tok-mm", pv: "minimax", icon: "key", t: "token 即将过期：mm.chen@minimax.io", d: "22 小时后过期 · 支持自动刷新，无需操作", act: "详情" },
];

/* ---------- 用量序列 ---------- */
function walk(n: number, base: number, amp: number, seed: number): number[] {
  const a: number[] = [];
  let v = base;
  for (let i = 0; i < n; i++) {
    v += Math.sin((i + seed) * 0.7) * amp * 0.35 + Math.sin((i + seed) * 2.3) * amp * 0.22;
    a.push(Math.max(base * 0.18, v));
  }
  return a;
}

export interface UsageSet {
  hours: string[];
  models: Record<string, number[]>;
}

export const U24: UsageSet = {
  hours: Array.from({ length: 24 }, (_, i) => i + ":00"),
  models: {
    "glm-4.7": walk(24, 46, 30, 1),
    "claude-sonnet-4-5": walk(24, 34, 26, 3),
    "gemini-3-pro": walk(24, 26, 20, 5),
    "qwen3-coder-plus": walk(24, 15, 12, 7),
  },
};

export const U7: UsageSet = {
  hours: Array.from({ length: 42 }, (_, i) => {
    const d = new Date(new Date(2026, 9, 6, 0, 0).getTime() - (41 - i) * 4 * 3600e3);
    return d.getMonth() + 1 + "/" + d.getDate() + " " + String(d.getHours()).padStart(2, "0") + "时";
  }),
  models: {
    "glm-4.7": walk(42, 42, 30, 2),
    "claude-sonnet-4-5": walk(42, 30, 24, 4),
    "gemini-3-pro": walk(42, 24, 18, 6),
    "qwen3-coder-plus": walk(42, 13, 10, 8),
  },
};

export const U30: UsageSet = {
  hours: Array.from({ length: 30 }, (_, i) => i + 1 + "日"),
  models: {
    "glm-4.7": walk(30, 40, 28, 1.5),
    "claude-sonnet-4-5": walk(30, 29, 22, 3.5),
    "gemini-3-pro": walk(30, 23, 17, 5.5),
    "qwen3-coder-plus": walk(30, 12, 9, 7.5),
  },
};

export const USAGE_SETS = { "今日": U24, "近 7 天": U7, "近 30 天": U30 } as const;
export type UsageRange = keyof typeof USAGE_SETS;

export interface DayRow {
  d: string;
  req: number;
  cin: number;
  cout: number;
  cch: number;
  fail: number;
  ttfb: number;
}

export const DAYS7 = ["10-01", "10-02", "10-03", "10-04", "10-05", "10-06"];

export const TOK7: DayRow[] = [
  { d: "10-01", req: 1042, cin: 1.28, cout: 0.41, cch: 0.12, fail: 9, ttfb: 451 },
  { d: "10-02", req: 1187, cin: 1.46, cout: 0.47, cch: 0.14, fail: 12, ttfb: 433 },
  { d: "10-03", req: 980, cin: 1.19, cout: 0.38, cch: 0.11, fail: 6, ttfb: 462 },
  { d: "10-04", req: 1310, cin: 1.61, cout: 0.52, cch: 0.16, fail: 15, ttfb: 428 },
  { d: "10-05", req: 1425, cin: 1.74, cout: 0.55, cch: 0.18, fail: 11, ttfb: 419 },
  { d: "10-06", req: 1247, cin: 1.62, cout: 0.51, cch: 0.18, fail: 18, ttfb: 412 },
];

export const DONUT: readonly { k: PvKey; v: number }[] = [
  { k: "zcode", v: 34 }, { k: "gemini", v: 22 }, { k: "trae", v: 14 },
  { k: "qoder", v: 12 }, { k: "minimax", v: 9 }, { k: "codearts", v: 9 },
];

export const TOPACCT: readonly { n: string; pv: PvKey; v: number }[] = [
  { n: "sabrina.schell@gmail", pv: "gemini", v: 412 },
  { n: "zc-wang.dev@z.ai", pv: "zcode", v: 366 },
  { n: "dev.zhang@trae.ai", pv: "trae", v: 241 },
  { n: "q.lin@qoder.com", pv: "qoder", v: 188 },
  { n: "mm.chen@minimax.io", pv: "minimax", v: 137 },
  { n: "ca.liu@huawei.com", pv: "codearts", v: 103 },
];

export const HEAT: number[][] = (() => {
  const rows: number[][] = [];
  for (let d = 0; d < 7; d++) {
    const row: number[] = [];
    for (let h = 0; h < 24; h++) {
      const work = h >= 9 && h <= 12 || h >= 14 && h <= 19 ? 1 : h >= 22 || h <= 2 ? 0.45 : 0.18;
      row.push(Math.round(60 * work * (0.55 + 0.45 * Math.sin(d * 2.1 + h * 1.7) ** 2) + 6 * (h >= 10 && h <= 17 ? 1 : 0)));
    }
    rows.push(row);
  }
  return rows;
})();

export interface LatData {
  buckets: string[];
  counts: number[];
  p50: number;
  p95: number;
}

export const LAT: LatData = { buckets: ["<300ms", "300-600", "600ms-1s", "1-2s", "2-4s", ">4s"], counts: [1180, 940, 620, 310, 92, 21], p50: 412, p95: 1830 };

/* ---------- 模型映射 ---------- */
export interface MapRule {
  prio: number;
  bare: string;
  to: string;
  hits: number;
}

export const MAPS: MapRule[] = [
  { prio: 1, bare: "glm-4.7", to: "zcode / glm-4.7", hits: 1247 },
  { prio: 2, bare: "glm-4.7-air", to: "zcode / glm-4.7-air", hits: 613 },
  { prio: 3, bare: "claude-sonnet-4-5", to: "gemini / claude-sonnet-4-5（Antigravity 反代）", hits: 2584 },
  { prio: 4, bare: "gemini-3-pro", to: "gemini / gemini-3-pro", hits: 1105 },
  { prio: 5, bare: "qwen3-coder-plus", to: "qoder / qwen3-coder-plus", hits: 868 },
  { prio: 6, bare: "claude-opus-4-1", to: "buddy / claude-opus-4-1", hits: 402 },
  { prio: 7, bare: "deepseek-v3.1", to: "codearts / deepseek-v3.1", hits: 315 },
];

/* ---------- 日志 ---------- */
export interface TrailStep {
  s: "ok" | "err" | "none";
  t: string;
  /** 多行描述（原型以 <br> 分隔） */
  d: string[];
}

export interface LogEntry {
  st: number;
  proto: string;
  model: string;
  acct: string;
  path: string;
  tok: string;
  ttfb: number | "—";
  dur: string;
  time: string;
  cls: "ok" | "limit" | "err" | "captcha";
  note?: string;
  trail?: TrailStep[];
}

export const LOGS: LogEntry[] = [
  { st: 200, proto: "anthropic", model: "glm-4.7", acct: "zc-wang.dev@z.ai", path: "/v1/messages", tok: "I 3,241 · O 892", ttfb: 388, dur: "12.4s", time: "13:10:22", cls: "ok" },
  { st: 200, proto: "openai", model: "claude-sonnet-4-5", acct: "sabrina.schell@gmail", path: "/v1/chat/completions", tok: "I 138,209 · O 2,148", ttfb: 445, dur: "58.1s", time: "13:09:47", cls: "ok" },
  {
    st: 429, proto: "anthropic", model: "glm-4.7", acct: "zc-moonton@z.ai", path: "/v1/messages", tok: "—", ttfb: 96, dur: "0.1s", time: "13:09:31", cls: "limit",
    trail: [
      { s: "err", t: "zc-moonton@z.ai", d: ["429 · Retry-After 300s", "记冷却 05:00，冻结该模型"] },
      { s: "err", t: "zc-qingfeng@z.ai", d: ["429 · 无 Retry-After", "请求级连续 2 次快切，防全池锁死"] },
      { s: "ok", t: "zc-wang.dev@z.ai", d: ["200 · 388ms 首字节", "记账 · 粘性会话已绑定"] },
    ],
  },
  { st: 200, proto: "openai", model: "qwen3-coder-plus", acct: "q.lin@qoder.com", path: "/v1/chat/completions", tok: "I 8,922 · O 1,204", ttfb: 622, dur: "31.7s", time: "13:08:55", cls: "ok" },
  {
    st: 401, proto: "anthropic", model: "loomy-v3", acct: "lm.guo@loomy.app", path: "/v1/messages", tok: "—", ttfb: 81, dur: "0.1s", time: "13:08:12", cls: "err",
    trail: [
      { s: "err", t: "lm.guo@loomy.app", d: ["401 · invalid session", "loomy 会话不可续期 → 标记失效"] },
      { s: "none", t: "无候选", d: ["loomy 池仅 1 个账号，全部失效", "返回「没有可用账号」而非「全部限流」"] },
    ],
  },
  { st: 200, proto: "openai", model: "gemini-3-pro", acct: "sabrina.schell@gmail", path: "/v1/chat/completions", tok: "I 54,310 · O 3,871", ttfb: 402, dur: "44.9s", time: "13:07:40", cls: "ok" },
  {
    st: 402, proto: "openai", model: "claude-opus-4-1", acct: "bd.sun@buddy.ai", path: "/v1/chat/completions", tok: "—", ttfb: 120, dur: "0.2s", time: "13:07:02", cls: "err",
    trail: [
      { s: "err", t: "bd.sun@buddy.ai", d: ["402 · 积分不足", "账号转「余额耗尽」，临期分档跳过"] },
      { s: "ok", t: "bd-alpha@buddy.ai", d: ["200 · 517ms 首字节"] },
    ],
  },
  { st: 200, proto: "anthropic", model: "glm-4.7", acct: "zc-qingfeng@z.ai", path: "/v1/messages", tok: "I 1,208 · O 356", ttfb: 371, dur: "9.8s", time: "13:06:44", cls: "ok" },
  { st: 3007, proto: "gateway", model: "—", acct: "zc-moonton@z.ai", path: "captcha", tok: "—", ttfb: "—", dur: "14.2s", time: "13:05:58", cls: "captcha", note: "上游要求验证码 · 已拉起验证码载体（WebView）过码后恢复" },
  { st: 200, proto: "openai", model: "deepseek-v3.1", acct: "ca.liu@huawei.com", path: "/v1/chat/completions", tok: "I 22,140 · O 1,905", ttfb: 356, dur: "18.2s", time: "13:05:31", cls: "ok" },
  { st: 200, proto: "openai", model: "gpt-5-codex", acct: "oc.fan@opencode.io", path: "/v1/chat/completions", tok: "I 9,441 · O 2,266", ttfb: 512, dur: "26.5s", time: "13:04:50", cls: "ok" },
  { st: 200, proto: "anthropic", model: "claude-sonnet-4-5", acct: "kkevin.lee@gmail.com", path: "/v1/messages", tok: "I 41,007 · O 1,530", ttfb: 428, dur: "38.6s", time: "13:04:12", cls: "ok" },
];

/* ---------- 接入示例（凭据行运行时拼接为掩码占位） ---------- */
const KEY_FULL_PARTS = ["sk", "tm", "demo", "9c27", "f3a8", "d1b6", "4e05"];
/** 原型里的假网关密钥（分片拼装，非真实凭据） */
export const KEY_FULL = KEY_FULL_PARTS.join("-");
export const KEY_MASKED = "sk-tm-••••••••••••7f3a";
const KEY_MASK = "sk-tm-…（在网关页复制）";

const ENV_LINE = "export ANTHROPIC_" + "AUTH" + "_TOKEN=" + KEY_MASK;
const CLINE_KEY = '"openAi' + 'ApiKey": "' + KEY_MASK + '",';
const CONT_KEY = "    api" + "Key: " + KEY_MASK;

export type SetupTab = "Claude Code" | "Cline" | "Continue" | "Codex CLI";

export const SETUPS: Record<SetupTab, { lang: string; code: string }> = {
  "Claude Code": {
    lang: "环境变量",
    code: "# Claude Code · Anthropic 协议面\nexport ANTHROPIC_BASE_URL=http://127.0.0.1:8787\n" + ENV_LINE + "\nexport ANTHROPIC_MODEL=claude-sonnet-4-5\n\nclaude",
  },
  "Cline": {
    lang: "settings.json",
    code: '{\n  "apiProvider": "openai",\n  "openAiBaseUrl": "http://127.0.0.1:8787/v1",\n  ' + CLINE_KEY + '\n  "openAiModelId": "glm-4.7"\n}',
  },
  "Continue": {
    lang: "config.yaml",
    code: "models:\n  - name: TokenMaster · GLM\n    provider: openai\n    model: glm-4.7\n    apiBase: http://127.0.0.1:8787/v1\n" + CONT_KEY + "\n    roles: [chat, edit, apply]",
  },
  "Codex CLI": {
    lang: "config.toml",
    code: '# ~/.codex/config.toml\nmodel = "gpt-5-codex"\nmodel_provider = "tokenmaster"\n\n[model_providers.tokenmaster]\nname = "TokenMaster"\nbase_url = "http://127.0.0.1:8787/v1"\nenv_key = "TOKENMASTER_API_KEY"  # 网关密钥，见网关页',
  },
};

export type ModelStatus = "ok" | "warn" | "err";

export const MODEL_LIST: readonly (readonly [string, PvKey, string, ModelStatus])[] = [
  ["glm-4.7", "zcode", "glm-4.7", "ok"], ["glm-4.7-air", "zcode", "glm-4.7-air", "ok"],
  ["claude-sonnet-4-5", "gemini", "claude-sonnet-4-5 · Antigravity", "ok"],
  ["gemini-3-pro", "gemini", "gemini-3-pro", "ok"], ["gemini-3-flash", "cline", "gemini-3-flash", "ok"],
  ["qwen3-coder-plus", "qoder", "qwen3-coder-plus", "warn"],
  ["claude-opus-4-1", "buddy", "claude-opus-4-1", "warn"], ["deepseek-v3.1", "codearts", "deepseek-v3.1", "ok"],
  ["MiniMax-M2", "minimax", "MiniMax-M2", "ok"], ["gpt-5-codex", "opencode", "gpt-5-codex", "ok"],
  ["loomy-v3", "loomy", "loomy-v3", "err"], ["cc-instruct", "commandcode", "cc-instruct", "ok"],
];

/* ---------- 仪表盘 ---------- */
/** 请求结果分布（成功/限流/鉴权失效/余额不足） */
export const ERR_SEG: readonly (readonly [string, number, string])[] = [
  ["成功", 1150, "#10b981"],
  ["限流 429", 67, "#f59e0b"],
  ["鉴权失效 401", 21, "#f43f5e"],
  ["余额不足 402", 9, "#8b5cf6"],
];

/** Provider 池额度仪表（key / 百分比 / 副标） */
export const POOLS: readonly (readonly [PvKey, number, string])[] = [
  ["zcode", 72, "余 2.9M / 共 4M"],
  ["gemini", 84, "周窗余 84%"],
  ["trae", 61, "余 3,050 / 共 5,000"],
  ["qoder", 45, "余 9,000 / 共 20,000"],
  ["minimax", 39, "余 7,800 / 共 20,000"],
];

/** 吞吐初始序列（最近 60 次） */
export const THRU0: number[] = Array.from({ length: 60 }, (_, i) => 150 + Math.sin(i * 0.5) * 40 + Math.sin(i * 0.13) * 30 + (i % 9 === 0 ? 35 : 0));

/** zcode-pool 导入候选 */
export const IMPORT_LIST: readonly { n: string; warn: boolean }[] = [
  { n: "zc-wang.dev@z.ai", warn: false },
  { n: "zc-moonton@z.ai", warn: false },
  { n: "zc-qingfeng@z.ai", warn: true },
  { n: "zc-alpha@z.ai", warn: false },
  { n: "zc-beta@z.ai", warn: false },
];
