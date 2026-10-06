/** 堆叠柱状图（输入 / 缓存 / 输出；移植 chartBars） */
import { sum } from "../lib/fmt";
import { useWidth } from "./util";

export const BARS_SERIES = [
  ["输入", "cin", "#3b82f6"],
  ["缓存", "cch", "#93c5fd"],
  ["输出", "cout", "#8b5cf6"],
] as const;

export interface BarsRow {
  d: string;
  cin: number;
  cout: number;
  cch: number;
}

export function Bars({ rows, h = 190 }: { rows: BarsRow[]; h?: number }) {
  const [ref, W] = useWidth(420);
  const padL = 40, padR = 6, padT = 12, padB = 24;
  const n = rows.length;
  const bw = Math.min(38, ((W - padL - padR) / n) * 0.52);
  const max = Math.max(...rows.map(r => sum(BARS_SERIES.map(s => r[s[1]])))) * 1.12;
  const x = (i: number) => padL + ((i + 0.5) / n) * (W - padL - padR);
  const y = (v: number) => padT + (1 - v / max) * (h - padT - padB);

  const stacks = rows.map(r => {
    let acc = 0;
    const segs = BARS_SERIES.map((se, si) => {
      const v = r[se[1]];
      const y0 = y(acc);
      const y1 = y(acc + v);
      acc += v;
      return { y1, hh: Math.max(1, y0 - y1), fill: se[2], rx: si === 2 ? 3 : undefined };
    });
    return { d: r.d, segs };
  });

  return (
    <div className="chart" ref={ref}>
      <svg viewBox={`0 0 ${W} ${h}`} width={W} height={h} style={{ maxWidth: "100%" }}>
        {Array.from({ length: 5 }, (_, t) => {
          const v = (max / 4) * t;
          const yy = y(v);
          return (
            <g key={t}>
              <line x1={padL} y1={yy} x2={W - padR} y2={yy} stroke="#94a3b8" strokeOpacity={0.13} strokeDasharray="3 3" />
              <text x={padL - 7} y={yy + 3.5} textAnchor="end" fontSize={9.5} fill="#64748b" fontFamily="var(--mono)">
                {v.toFixed(1)}M
              </text>
            </g>
          );
        })}
        {stacks.map((st, i) => (
          <g key={st.d}>
            {st.segs.map((sg, si) => (
              <rect key={si} x={x(i) - bw / 2} y={sg.y1} width={bw} height={sg.hh} fill={sg.fill} rx={sg.rx} />
            ))}
            <text x={x(i)} y={h - 7} textAnchor="middle" fontSize={9} fill="#64748b" fontFamily="var(--mono)">
              {st.d}
            </text>
          </g>
        ))}
      </svg>
    </div>
  );
}
