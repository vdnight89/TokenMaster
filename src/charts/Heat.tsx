/** 请求热力图 7×24（星期 × 小时；移植 chartHeat） */
const DAYS = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];

export function Heat({ data }: { data: readonly number[][] }) {
  const max = Math.max(...data.flat());
  const lv = (v: number): string => {
    const t = v / max;
    if (t <= 0.02) return "var(--card-2)";
    if (t < 0.25) return "rgba(59,130,246,.22)";
    if (t < 0.5) return "rgba(59,130,246,.45)";
    if (t < 0.75) return "rgba(59,130,246,.7)";
    return "#3b82f6";
  };
  return (
    <div>
      <div className="heat">
        <div className="hl">
          {DAYS.map(d => (
            <span key={d}>{d}</span>
          ))}
        </div>
        <div className="cells">
          {data.map((row, d) => (
            <div className="crow" key={d}>
              {row.map((v, h) => (
                <i key={h} style={{ background: lv(v) }} title={`${DAYS[d]} ${h}:00 · ${v} 次请求`}></i>
              ))}
            </div>
          ))}
        </div>
      </div>
      <div style={{ display: "flex", alignItems: "center", gap: 6, marginTop: 10, fontSize: 10, color: "var(--t4)", fontFamily: "var(--mono)" }}>
        0{" "}
        <i style={{ width: 34, height: 10, borderRadius: 3, background: "linear-gradient(90deg,var(--card-2),rgba(59,130,246,.25),rgba(59,130,246,.6),#3b82f6)" }}></i>{" "}
        {max}+
        <span className="grow"></span>UTC+8 · 按小时聚合
      </div>
    </div>
  );
}
