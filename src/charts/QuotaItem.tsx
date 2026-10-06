/** QuotaItem：AM 式 22px 内嵌配额条（配额行内含重置时间与剩余百分比） */
import { Ic } from "../lib/icons";

export function QuotaItem({ m, p, r, frozen }: { m: string; p: number; r: string | number; frozen?: boolean }) {
  const f = p >= 50 ? "f-ok" : p >= 20 ? "f-warn" : "f-err";
  const pc = p >= 50 ? "p-ok" : p >= 20 ? "p-warn" : "p-err";
  const col = p >= 50 ? "var(--ok)" : p >= 20 ? "var(--warn)" : "var(--err)";
  return (
    <div className="qi">
      <div className={`fill ${f}`} style={{ width: (frozen ? 0 : Math.max(p, 1.5)) + "%", background: col }}></div>
      <div className="row">
        <span className="m">{m}</span>
        <span className="sp"></span>
        <span className="r">
          <Ic name="clock" size={9} />
          {"R: " + r}
        </span>
        <span className={`p ${pc}`}>{p}%</span>
      </div>
    </div>
  );
}
