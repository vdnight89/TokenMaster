/** 响应延迟直方图（含 P50 / P95；移植 chartHisto） */
import { fmt, sum } from "../lib/fmt";
import type { LatData } from "../lib/mock";

export function Histo({ data }: { data: LatData }) {
  const max = Math.max(...data.counts);
  return (
    <div>
      <div className="histo">
        {data.buckets.map((b, i) => (
          <div className={"b" + (i >= 4 ? " hot" : "")} key={b}>
            <em>{data.counts[i] ? fmt(data.counts[i]) : ""}</em>
            <i style={{ height: Math.max(2, (data.counts[i] / max) * 100) + "%" }}></i>
            <span>{b}</span>
          </div>
        ))}
      </div>
      <div className="mt-s" style={{ fontSize: 11, color: "var(--t4)", fontFamily: "var(--mono)" }}>
        {fmt(sum(data.counts))} 次请求 · 中位 <b style={{ color: "var(--t2)" }}>{data.p50}ms</b> · P95{" "}
        <b style={{ color: "#fbbf24" }}>{fmt(data.p95)}ms</b>
      </div>
    </div>
  );
}
