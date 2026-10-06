/** 带刻度 SVG 仪表盘（zpool 式；移植 gaugeSVG） */
export function Gauge({ pct, size = 96, big = false }: { pct: number; size?: number; big?: boolean }) {
  const R = size / 2;
  const r = R - 8;
  const A0 = -210;
  const A1 = 30;
  const col = pct >= 50 ? "#10b981" : pct >= 20 ? "#f59e0b" : "#f43f5e";
  const pt = (a: number, rr: number): [number, number] => [R + rr * Math.cos((a * Math.PI) / 180), R + rr * Math.sin((a * Math.PI) / 180)];
  const arc = (a0: number, a1: number, rr: number): string => {
    const [x0, y0] = pt(a0, rr);
    const [x1, y1] = pt(a1, rr);
    return `M ${x0.toFixed(2)} ${y0.toFixed(2)} A ${rr} ${rr} 0 ${a1 - a0 > 180 ? 1 : 0} 1 ${x1.toFixed(2)} ${y1.toFixed(2)}`;
  };
  const ticks: { x0: string; y0: string; x1: string; y1: string; major: boolean }[] = [];
  for (let a = A0; a <= A1; a += 6) {
    const major = (a - A0) % 30 === 0;
    const [x0, y0] = pt(a, r - 6.5);
    const [x1, y1] = pt(a, r - (major ? 11 : 9));
    ticks.push({ x0: x0.toFixed(1), y0: y0.toFixed(1), x1: x1.toFixed(1), y1: y1.toFixed(1), major });
  }
  const aFill = A0 + ((A1 - A0) * pct) / 100;
  const [cx, cy] = pt(aFill, r);
  return (
    <svg viewBox={`0 0 ${size} ${size}`} width={size} height={size} style={{ maxWidth: "100%" }}>
      <g className="gauge" style={{ color: col }}>
        <path d={arc(A0, A1, r)} fill="none" stroke="rgba(148,163,184,.18)" strokeWidth={6} strokeLinecap="round" />
        {ticks.map((t, i) => (
          <line
            key={i}
            x1={t.x0}
            y1={t.y0}
            x2={t.x1}
            y2={t.y1}
            stroke="#64748b"
            strokeOpacity={t.major ? 0.7 : 0.35}
            strokeWidth={t.major ? 1 : 0.6}
          />
        ))}
        <path d={arc(A0, aFill, r)} fill="none" stroke={col} strokeWidth={6} strokeLinecap="round" style={{ filter: `drop-shadow(0 0 3px ${col}66)` }} />
        <circle cx={cx.toFixed(1)} cy={cy.toFixed(1)} r={2.6} fill={col} />
        <text x={R} y={R + 5} textAnchor="middle" fontSize={big ? 17 : 13} fontWeight={700} fill="var(--t1)" fontFamily="var(--mono)">
          {pct}
          <tspan fontSize={9} fill="#64748b">
            %
          </tspan>
        </text>
      </g>
    </svg>
  );
}
