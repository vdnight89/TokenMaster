/**
 * Provider / 模型品牌图标单一出口（T5.12）。
 *
 * 品牌图标来自 @lobehub/icons（MIT，LobeHub；见 README 署名说明），
 * 仅做具名导入并只用 .Color / Mono 纯 SVG 变体（Avatar/Combine 变体依赖
 * antd-style / @lobehub/ui，会把整包 antd 拖进产物，超出体积预算）。
 *
 * lobehub 未收录的 provider 兜底为原有样式的彩色字母徽章
 * （PROVIDERS[pv].color + .pv 同款圆角方形），保持视觉统一；
 * 全部为内联 SVG，离线自包含，不联网下载图片。
 */
import type { CSSProperties } from "react";
// 深路径导入：包入口的 CompoundedIcon 会把每个品牌的 .Avatar/.Combine/.Text
// 一并挂到导出对象上（属性赋值无法被 tree-shaking），进而拖入 @lobehub/ui
// 与 antd-style（产物 +~700KB）。只引纯 SVG 的 Color/Mono 组件文件可整段避免。
// CommandCode 组件虽有 components/Color.js，但包入口未挂 .Color，故走 Mono 白字。
import ZAIMono from "@lobehub/icons/es/ZAI/components/Mono";
import ZhipuColor from "@lobehub/icons/es/Zhipu/components/Color";
import GeminiColor from "@lobehub/icons/es/Gemini/components/Color";
import ClaudeColor from "@lobehub/icons/es/Claude/components/Color";
import QwenColor from "@lobehub/icons/es/Qwen/components/Color";
import DeepSeekColor from "@lobehub/icons/es/DeepSeek/components/Color";
import MinimaxColor from "@lobehub/icons/es/Minimax/components/Color";
import OpenAIMono from "@lobehub/icons/es/OpenAI/components/Mono";
import MoonshotMono from "@lobehub/icons/es/Moonshot/components/Mono";
import HuaweiColor from "@lobehub/icons/es/Huawei/components/Color";
import CodeBuddyColor from "@lobehub/icons/es/CodeBuddy/components/Color";
import ClineMono from "@lobehub/icons/es/Cline/components/Mono";
import OpenCodeMono from "@lobehub/icons/es/OpenCode/components/Mono";
import TraeColor from "@lobehub/icons/es/Trae/components/Color";
import QoderColor from "@lobehub/icons/es/Qoder/components/Color";
import CommandCodeMono from "@lobehub/icons/es/CommandCode/components/Mono";
import { PROVIDERS } from "./mock";
import type { PvKey } from "./mock";

/** lobehub 图标组件（Mono/Color 均为此签名；size/color 足够本用法） */
type IconComp = (props: { size?: number | string; color?: string; className?: string }) => React.ReactNode;

/* ---------- provider → 品牌（lobehub 覆盖 12 / 15） ----------
 * zcode→ZAI（z.ai 官方 "Z" 标，与 @z.ai 域名一致）
 * qoder/qodercn→Qoder（Qoder 官方标，优于泛化的 Qwen/Alibaba）
 * buddy/workbuddy→CodeBuddy（腾讯 CodeBuddy，两档即个人版/企业版）
 * codearts→Huawei、commandcode→CommandCode、其余同名直映
 * 未覆盖（兜底字母徽章）：lobsterai / loomy / raccoon                          */
const PV_COLOR: Partial<Record<PvKey, IconComp>> = {
  gemini: GeminiColor,
  trae: TraeColor,
  qoder: QoderColor,
  qodercn: QoderColor,
  minimax: MinimaxColor,
  codearts: HuaweiColor,
  buddy: CodeBuddyColor,
  workbuddy: CodeBuddyColor,
};

/** 无 .Color 变体的品牌（官方即单色标）：Mono + 白色字形，适配暗色界面 */
const PV_MONO: Partial<Record<PvKey, IconComp>> = {
  zcode: ZAIMono,
  opencode: OpenCodeMono,
  cline: ClineMono,
  commandcode: CommandCodeMono,
};

/* ---------- 模型家族前缀 → 品牌（忽略大小写） ---------- */
const MODEL_COLOR: readonly (readonly [string, IconComp])[] = [
  ["glm", ZhipuColor],
  ["claude", ClaudeColor],
  ["gemini", GeminiColor],
  ["qwen", QwenColor],
  ["deepseek", DeepSeekColor],
  ["minimax", MinimaxColor],
];

const MODEL_MONO: readonly (readonly [string, IconComp])[] = [
  ["gpt", OpenAIMono],
  ["codex", OpenAIMono],
  ["kimi", MoonshotMono],
];

/** 模型名是否命中已知品牌（未命中时调用方保留原色点，保证图例对齐） */
export function hasModelBrand(model: string): boolean {
  const m = model.toLowerCase();
  return (
    MODEL_COLOR.some(([p]) => m.startsWith(p)) || MODEL_MONO.some(([p]) => m.startsWith(p))
  );
}

/** provider 是否有 lobehub 品牌图标 */
function pvIcon(pv: PvKey): { Comp: IconComp; mono: boolean } | null {
  const c = PV_COLOR[pv];
  if (c) return { Comp: c, mono: false };
  const m = PV_MONO[pv];
  if (m) return { Comp: m, mono: true };
  return null;
}

/** 徽章圆角方形容器（与页面 .pv 同形态，尺寸随 size 缩放） */
function badgeBox(size: number, bg: string): CSSProperties {
  return {
    width: size,
    height: size,
    borderRadius: Math.max(4, Math.round(size * 0.31)),
    background: bg,
    flex: "none",
  };
}

const BI = "pv-bi"; // 样式见 styles.css：inline-flex 居中

/**
 * Provider 徽章：lobehub 品牌字形装在 provider 色的浅底圆角方形里；
 * 未收录 provider 为实色底 + 白色首字母（原 .pv 样式）。
 */
export function ProviderBrandIcon({ provider, size = 26 }: { provider: PvKey; size?: number }) {
  const pv = PROVIDERS[provider];
  const hit = pvIcon(provider);
  if (hit) {
    const s = Math.round(size * 0.62);
    return (
      <span className={BI} style={badgeBox(size, pv.color + "1f")} title={pv.name}>
        {hit.mono ? <hit.Comp size={s} color="#fff" /> : <hit.Comp size={s} />}
      </span>
    );
  }
  return (
    <span
      className={BI}
      style={{
        ...badgeBox(size, pv.color),
        color: "#fff",
        fontSize: Math.max(8, Math.round(size * 0.42)),
        fontWeight: 800,
      }}
      title={pv.name}
    >
      {pv.name[0]}
    </span>
  );
}

/** provider 是否有 lobehub 品牌字形（无则调用方保留原色点/徽章） */
export function hasProviderGlyph(provider: PvKey): boolean {
  return pvIcon(provider) !== null;
}

/** provider 单色小图标（无底、随行文字对齐），用于仪表/表格/图例等紧凑位置 */
export function ProviderGlyphIcon({ provider, size = 13 }: { provider: PvKey; size?: number }) {
  const hit = pvIcon(provider);
  if (!hit) return null;
  return hit.mono ? <hit.Comp size={size} color="#fff" /> : <hit.Comp size={size} />;
}

/** 模型品牌小图标（按家族前缀匹配）；未识别返回 null，调用方不渲染 */
export function ModelBrandIcon({ model, size = 13 }: { model: string; size?: number }) {
  const m = model.toLowerCase();
  for (const [p, Comp] of MODEL_COLOR) if (m.startsWith(p)) return <Comp size={size} />;
  for (const [p, Comp] of MODEL_MONO) if (m.startsWith(p)) return <Comp size={size} color="#fff" />;
  return null;
}
