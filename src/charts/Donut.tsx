/** Provider 占比环图（移植 chartDonut） */
import { sum } from "../lib/fmt";
import { PROVIDERS } from "../lib/mock";
import type { PvKey } from "../lib/mock";
import { PALETTE } from "./util";

export function Donut({
  data,
  center = "8,634",
  centerSub = "REQUESTS · 7D",
  size = 168,
}: {
  data: readonly { k: PvKey; v: number }[];
  center?: string;
  centerSub?: string;
  size?: number;
}) {
  const R = size / 2;
  const r1 = R - 17;
  const C = 2 * Math.PI * r1;
  const total = sum(data.map(d => d.v));
  let off = 0;
  const arcs = data.map((d, i) => {
    const c = PROVIDERS[d.k] ? PROVIDERS[d.k].color : PALETTE[i];
    const len = (d.v / total) * C;
    const seg = { c, dash: `${len - 2.5} ${C - len + 2.5}`, off: -off, title: `${d.k} ${d.v}%` };
    off += len;
    return seg;
  });
  return (
    <div className="chart" style={{ flex: "none" }}>
      <svg viewBox={`0 0 ${size} ${size}`} width={size} height={size} style={{ maxWidth: "100%" }}>
        <circle cx={R} cy={R} r={r1} fill="none" stroke="var(--card-2)" strokeWidth={16} />
        {arcs.map((a, i) => (
          <circle
            key={i}
            cx={R}
            cy={R}
            r={r1}
            fill="none"
            stroke={a.c}
            strokeWidth={16}
            strokeDasharray={a.dash}
            strokeDashoffset={a.off}
            transform={`rotate(-90 ${R} ${R})`}
            strokeLinecap="round"
          >
            <title>{a.title}</title>
          </circle>
        ))}
        <text x={R} y={R - 3} textAnchor="middle" fontSize={19} fontWeight={700} fill="var(--t1)" fontFamily="var(--mono)">
          {center}
        </text>
        <text x={R} y={R + 14} textAnchor="middle" fontSize={9.5} fill="#64748b" fontFamily="var(--mono)">
          {centerSub}
        </text>
      </svg>
    </div>
  );
}
