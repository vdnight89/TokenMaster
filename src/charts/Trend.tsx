/** 堆叠面积趋势图（网格 + 渐变填充 + 轴标签，按模型堆叠；移植 chartTrend） */
import { MODEL_COLOR } from "../lib/mock";
import { fmtK, sum } from "../lib/fmt";
import { PALETTE, useSvgId, useWidth } from "./util";

export interface TrendData {
  hours: string[];
  models: Record<string, number[]>;
}

export function Trend({ data, h = 190 }: { data: TrendData; h?: number }) {
  const [ref, W] = useWidth(560);
  const gid = useSvgId("tg");
  const padL = 44, padR = 10, padT = 14, padB = 24;
  const keys = Object.keys(data.models);
  const n = data.hours.length;
  const totals = data.hours.map((_, i) => sum(keys.map(k => data.models[k][i])));
  const max = Math.max(...totals) * 1.12;
  const x = (i: number) => padL + (i / (n - 1)) * (W - padL - padR);
  const y = (v: number) => padT + (1 - v / max) * (h - padT - padB);
  const step = Math.ceil(n / (W < 500 ? 6 : 12));

  const acc = new Array<number>(n).fill(0);
  const series = keys.map((k, ki) => {
    const vals = data.models[k];
    const c = MODEL_COLOR[k] ?? PALETTE[ki % PALETTE.length];
    const bottom = [...acc];
    const top = vals.map((v, i) => acc[i] + v);
    let dArea = `M ${x(0)},${y(bottom[0])}`;
    top.forEach((v, i) => {
      dArea += ` L ${x(i)},${y(v)}`;
    });
    for (let i = n - 1; i >= 0; i--) dArea += ` L ${x(i)},${y(bottom[i])}`;
    dArea += " Z";
    const dLine = top.map((v, i) => (i ? "L" : "M") + x(i) + "," + y(v)).join(" ");
    for (let i = 0; i < n; i++) acc[i] = top[i];
    return { k, c, last: ki === keys.length - 1, dArea, dLine };
  });

  return (
    <div className="chart" ref={ref}>
      <svg viewBox={`0 0 ${W} ${h}`} width={W} height={h} style={{ maxWidth: "100%" }}>
        <defs>
          <linearGradient id={gid} x1={0} y1={0} x2={0} y2={1}>
            <stop offset={0} stopColor="#3b82f6" stopOpacity={0.38} />
            <stop offset={1} stopColor="#3b82f6" stopOpacity={0} />
          </linearGradient>
        </defs>
        {Array.from({ length: 5 }, (_, t) => {
          const v = (max / 4) * t;
          const yy = y(v);
          return (
            <g key={t}>
              <line x1={padL} y1={yy} x2={W - padR} y2={yy} stroke="#94a3b8" strokeOpacity={0.13} strokeDasharray="3 3" />
              <text x={padL - 7} y={yy + 3.5} textAnchor="end" fontSize={9.5} fill="#64748b" fontFamily="var(--mono)">
                {fmtK(v)}
              </text>
            </g>
          );
        })}
        {data.hours.map((hh, i) => {
          if (i % step !== 0) return null;
          const xx = x(i);
          if (xx > W - padR - 30) return null; // 防止末尾标签越界或与前一标签重叠
          return (
            <text key={i} x={xx} y={h - 7} textAnchor="middle" fontSize={9} fill="#64748b" fontFamily="var(--mono)">
              {hh}
            </text>
          );
        })}
        {series.map(s =>
          s.last ? (
            <g key={s.k}>
              <path d={s.dArea} fill={`url(#${gid})`} stroke="none" />
              <path d={s.dLine} fill="none" stroke="#3b82f6" strokeWidth={1.8} strokeLinejoin="round" />
            </g>
          ) : (
            <g key={s.k}>
              <path d={s.dArea} fill={s.c} fillOpacity={0.3} stroke="none" />
              <path d={s.dLine} fill="none" stroke={s.c} strokeWidth={1.4} strokeOpacity={0.9} />
            </g>
          ),
        )}
      </svg>
    </div>
  );
}
