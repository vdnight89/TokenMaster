/** 路由拓扑图示（客户端 → 网关 → Token 池；移植 renderTopo） */
import { Fragment } from "react";
import { PROVIDERS } from "../lib/mock";

function Box({ x, y, w, h, stroke, fill, label, sub }: { x: number; y: number; w: number; h: number; stroke: string; fill: string; label: string; sub?: string }) {
  return (
    <g>
      <rect x={x} y={y} width={w} height={h} rx={9} fill={fill} stroke={stroke} strokeWidth={1.2} />
      <text x={x + w / 2} y={y + (sub ? h / 2 - 2 : h / 2 + 4)} textAnchor="middle" fontSize={11.5} fontWeight={600} fill="var(--t1)" fontFamily="var(--sans)">
        {label}
      </text>
      {sub ? (
        <text x={x + w / 2} y={y + h / 2 + 13} textAnchor="middle" fontSize={9} fill="#64748b" fontFamily="var(--mono)">
          {sub}
        </text>
      ) : null}
    </g>
  );
}

function Link({ x0, y0, x1, y1, c, dash }: { x0: number; y0: number; x1: number; y1: number; c: string; dash?: boolean }) {
  return (
    <path
      d={`M ${x0} ${y0} C ${x0} ${(y0 + y1) / 2}, ${x1} ${(y0 + y1) / 2}, ${x1} ${y1}`}
      fill="none"
      stroke={c}
      strokeWidth={1.3}
      strokeDasharray={dash ? "4 3" : undefined}
    />
  );
}

const PV_SHOW = ["zcode", "gemini", "trae", "qoder", "minimax", "codearts"] as const;

export function Topo() {
  return (
    <svg viewBox="0 0 560 316" width={560} height={316} style={{ maxWidth: "100%" }}>
      <defs>
        <linearGradient id="tp-g" x1={0} y1={0} x2={0} y2={1}>
          <stop offset={0} stopColor="rgba(59,130,246,.2)" />
          <stop offset={1} stopColor="rgba(59,130,246,.06)" />
        </linearGradient>
      </defs>
      <text x={14} y={18} fontSize={9.5} fill="#64748b" fontFamily="var(--mono)" letterSpacing={1}>
        AI CODING CLIENTS
      </text>
      <Box x={14} y={28} w={150} h={34} stroke="var(--line-2)" fill="var(--card)" label="Claude Code" />
      <Box x={14} y={70} w={150} h={34} stroke="var(--line-2)" fill="var(--card)" label="Cline / Continue" />
      <Box x={14} y={112} w={150} h={34} stroke="var(--line-2)" fill="var(--card)" label="Codex CLI / SDK" />
      <Link x0={89} y0={62} x1={230} y1={118} c="#64748b" />
      <Link x0={89} y0={87} x1={230} y1={130} c="#64748b" />
      <Link x0={89} y0={112} x1={230} y1={142} c="#64748b" />
      <Box x={230} y={96} w={200} h={68} stroke="#3b82f6" fill="url(#tp-g)" label="TokenMaster 网关" sub="127.0.0.1:8787 · OpenAI / Anthropic" />
      <text x={240} y={184} fontSize={9} fill="#64748b" fontFamily="var(--mono)">
        路由 · 池化轮询 · 换号重试 · 账本 · 限流冷却
      </text>
      <text x={360} y={218} fontSize={9.5} fill="#64748b" fontFamily="var(--mono)" letterSpacing={1}>
        TOKEN POOLS × 15
      </text>
      <Link x0={330} y0={164} x1={402} y1={236} c="#64748b" />
      <Link x0={330} y0={164} x1={466} y1={236} c="#64748b" />
      {PV_SHOW.map((k, i) => {
        const x = 356 + (i % 3) * 66;
        const y = 228 + Math.floor(i / 3) * 38;
        return (
          <Fragment key={k}>
            <Box x={x} y={y} w={60} h={30} stroke={PROVIDERS[k].color + "88"} fill={PROVIDERS[k].color + "1f"} label={PROVIDERS[k].name} />
            <Link x0={330} y0={164} x1={x + 30} y1={y} c="#3f4a5f" dash />
          </Fragment>
        );
      })}
      <text x={356} y={312} fontSize={9.5} fill="#64748b" fontFamily="var(--mono)">
        +9 more · 每池独立选号策略与临期分档
      </text>
    </svg>
  );
}
