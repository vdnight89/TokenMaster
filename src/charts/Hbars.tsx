/** 横向条形排行（账号消耗 Top；移植 chartHbars） */
import { fmtK } from "../lib/fmt";
import { PROVIDERS } from "../lib/mock";
import type { PvKey } from "../lib/mock";
import { ProviderGlyphIcon, hasProviderGlyph } from "../lib/brand";

export interface HbarItem {
  n: string;
  pv: PvKey;
  v: number;
}

export function Hbars({ data }: { data: readonly HbarItem[] }) {
  const max = Math.max(...data.map(d => d.v));
  return (
    <div className="hbars">
      {data.map(d => {
        const c = PROVIDERS[d.pv].color;
        return (
          <div className="hbar" key={d.n}>
            <div className="nm">
              {hasProviderGlyph(d.pv) ? (
                <ProviderGlyphIcon provider={d.pv} size={12} />
              ) : (
                <i style={{ background: c }}></i>
              )}
              {d.n}
            </div>
            <div className="track">
              <b style={{ width: ((d.v / max) * 100).toFixed(1) + "%", background: `linear-gradient(90deg,${c}88,${c})` }}></b>
            </div>
            <div className="val">{fmtK(d.v)}k</div>
          </div>
        );
      })}
    </div>
  );
}
