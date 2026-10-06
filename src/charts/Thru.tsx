/** 号池吞吐实时曲线（2 秒采样推进；移植 chartThru + 实时 setInterval） */
import { useEffect, useState } from "react";
import { makeRnd } from "../lib/fmt";
import { THRU0 } from "../lib/mock";
import { useSvgId, useWidth } from "./util";

const thruRnd = makeRnd(42);

/** 吞吐序列：active 时每 2s 推进一格（60..320 随机游走） */
export function useThru(active: boolean): number[] {
  const [data, setData] = useState<number[]>(THRU0);
  useEffect(() => {
    if (!active) return;
    const id = window.setInterval(() => {
      setData(prev => {
        const last = prev[prev.length - 1];
        return [...prev.slice(1), Math.max(60, Math.min(320, last + (thruRnd() - 0.48) * 46))];
      });
    }, 2000);
    return () => window.clearInterval(id);
  }, [active]);
  return data;
}

export function ThruChart({ data }: { data: readonly number[] }) {
  const [ref, W] = useWidth(640);
  const gid = useSvgId("thr");
  const H = 148, padL = 42, padR = 8, padT = 12, padB = 20;
  const max = Math.max(...data) * 1.15;
  const x = (i: number) => padL + (i / (data.length - 1)) * (W - padL - padR);
  const y = (v: number) => padT + (1 - v / max) * (H - padT - padB);
  const li = data.length - 1;
  const dLine = data.map((v, i) => (i ? "L" : "M") + x(i).toFixed(1) + "," + y(v).toFixed(1)).join(" ");
  return (
    <div className="chart" ref={ref} style={{ height: 148 }}>
      <svg viewBox={`0 0 ${W} ${H}`} width={W} height={H} style={{ maxWidth: "100%" }}>
        <defs>
          <linearGradient id={gid} x1={0} y1={0} x2={0} y2={1}>
            <stop offset={0} stopColor="#3b82f6" stopOpacity={0.42} />
            <stop offset={1} stopColor="#3b82f6" stopOpacity={0} />
          </linearGradient>
        </defs>
        {[1, 2, 3].map(t => {
          const yy = y((max / 3) * t);
          return (
            <g key={t}>
              <line x1={padL} y1={yy} x2={W - padR} y2={yy} stroke="#94a3b8" strokeOpacity={0.12} strokeDasharray="3 3" />
              <text x={padL - 7} y={yy + 3.5} textAnchor="end" fontSize={9.5} fill="#64748b" fontFamily="var(--mono)">
                {Math.round((max / 3) * t)}
              </text>
            </g>
          );
        })}
        {["-120s", "-80s", "-40s", "now"].map((t, i) => (
          <text key={t} x={padL + (W - padL - padR) * (i / 3)} y={H - 5} textAnchor={i === 3 ? "end" : "middle"} fontSize={9} fill="#64748b" fontFamily="var(--mono)">
            {t}
          </text>
        ))}
        <path
          d={`M ${x(0)},${H - padB} ${data.map((v, i) => `L ${x(i).toFixed(1)},${y(v).toFixed(1)}`).join(" ")} L ${x(li)},${H - padB} Z`}
          fill={`url(#${gid})`}
        />
        <path d={dLine} fill="none" stroke="#3b82f6" strokeWidth={1.8} strokeLinejoin="round" />
        <circle cx={x(li)} cy={y(data[li])} r={3} fill="#3b82f6" />
        <circle cx={x(li)} cy={y(data[li])} r={6} fill="#3b82f6" opacity={0.25} />
      </svg>
    </div>
  );
}
