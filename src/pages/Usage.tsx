/** 用量页：时间分段 + 汇总卡 + 趋势/构成/占比/Top/延迟/热力/聚合表（对应原型 #page-usage / renderUsage） */
import { useState } from "react";
import { Ic } from "../lib/icons";
import { hasModelBrand, hasProviderGlyph, ModelBrandIcon, ProviderGlyphIcon } from "../lib/brand";
import { DAYS7, DONUT, HEAT, LAT, MODEL_COLOR, PROVIDERS, TOK7, TOPACCT, USAGE_SETS } from "../lib/mock";
import type { UsageRange } from "../lib/mock";
import { fmt } from "../lib/fmt";
import { Trend } from "../charts/Trend";
import { BARS_SERIES, Bars } from "../charts/Bars";
import { Donut } from "../charts/Donut";
import { Hbars } from "../charts/Hbars";
import { Heat } from "../charts/Heat";
import { Histo } from "../charts/Histo";

const RANGES: readonly UsageRange[] = ["今日", "近 7 天", "近 30 天"];

export default function Usage() {
  const [range, setRange] = useState<UsageRange>("近 7 天");
  const set = USAGE_SETS[range];
  const mult = range === "今日" ? 1 : range === "近 7 天" ? 6.9 : 29.4;

  return (
    <div className="page-wrap">
      <div className="page-h">
        <div>
          <h1>用量</h1>
          <div className="sub">每次请求记入本地账本（JSONL）· 时间 / Provider / 账号 / 模型 / tokens / 耗时</div>
        </div>
        <div className="actions">
          <button className="btn">
            <Ic name="download" />
            导出 JSONL
          </button>
        </div>
      </div>

      <div className="toolbar">
        <div className="seg">
          {RANGES.map(r => (
            <button key={r} className={range === r ? "on" : ""} onClick={() => setRange(r)}>
              {r}
            </button>
          ))}
        </div>
        <span className="grow"></span>
        <span className="count-chip">数据目录 ~/.tokenmaster/ledger/</span>
      </div>

      <div className="gstat-grid">
        <div className="gstat g-blue">
          <div className="lab">总请求</div>
          <div className="num">{fmt(1247 * mult)}</div>
          <div className="delta" style={{ color: "#34d399" }}>▲ 12.4% vs 上期</div>
        </div>
        <div className="gstat g-violet">
          <div className="lab">输入 Tokens</div>
          <div className="num">{(1.62 * mult).toFixed(1)}M</div>
          <div className="delta" style={{ color: "#34d399" }}>▲ 9.1%</div>
        </div>
        <div className="gstat g-cyan">
          <div className="lab">输出 Tokens</div>
          <div className="num">{(0.51 * mult).toFixed(1)}M</div>
          <div className="delta" style={{ color: "#34d399" }}>▲ 14.8%</div>
        </div>
        <div className="gstat g-ok">
          <div className="lab">缓存命中</div>
          <div className="num">{(0.18 * mult).toFixed(2)}M</div>
          <div className="delta" style={{ color: "var(--t4)" }}>占输入 15.4%</div>
        </div>
        <div className="gstat g-warn">
          <div className="lab">平均 TTFB</div>
          <div className="num">412ms</div>
          <div className="delta" style={{ color: "#34d399" }}>▼ 36ms</div>
        </div>
        <div className="gstat g-rose">
          <div className="lab">成功率</div>
          <div className="num">{range === "今日" ? "98.4%" : "98.2%"}</div>
          <div className="delta" style={{ color: "var(--t4)" }}>失败 71 次</div>
        </div>
      </div>

      <div className="card" style={{ marginTop: 12 }}>
        <div className="card-h">
          <span className="chipic c-blue">
            <Ic name="activity" />
          </span>
          <span className="ttl">用量趋势 · 按模型堆叠</span>
          <span className="grow"></span>
          <span className="legend" style={{ margin: 0 }}>
            {Object.keys(set.models).map(k => (
              <span className="li" key={k}>
                {hasModelBrand(k) ? (
                  <ModelBrandIcon model={k} size={12} />
                ) : (
                  <i style={{ background: MODEL_COLOR[k] }}></i>
                )}
                {k}
              </span>
            ))}
          </span>
        </div>
        <Trend data={set} h={210} />
      </div>

      <div className="grid2" style={{ marginTop: 12 }}>
        <div className="card">
          <div className="card-h">
            <span className="chipic c-violet">
              <Ic name="db" />
            </span>
            <span className="ttl">Token 构成 · 输入 / 缓存 / 输出</span>
            <span className="grow"></span>
            <span className="legend" style={{ margin: 0 }}>
              {BARS_SERIES.map(s => (
                <span className="li" key={s[0]}>
                  <i style={{ background: s[2] }}></i>
                  {s[0]}
                </span>
              ))}
            </span>
          </div>
          <Bars h={210} rows={DAYS7.map((d, i) => ({ d, cin: TOK7[i].cin, cout: TOK7[i].cout, cch: TOK7[i].cch }))} />
        </div>
        <div className="card">
          <div className="card-h">
            <span className="chipic c-ok">
              <Ic name="gauge" />
            </span>
            <span className="ttl">Provider 占比</span>
          </div>
          <div style={{ display: "flex", gap: 18, alignItems: "center" }}>
            <Donut data={DONUT} />
            <div className="legend" style={{ flexDirection: "column", alignItems: "flex-start", gap: 8, margin: 0 }}>
              {DONUT.map(d => (
                <span className="li" key={d.k}>
                  {hasProviderGlyph(d.k) ? (
                    <ProviderGlyphIcon provider={d.k} size={12} />
                  ) : (
                    <i style={{ background: PROVIDERS[d.k].color }}></i>
                  )}
                  {PROVIDERS[d.k].name} <b style={{ color: "var(--t2)" }}>{d.v}%</b>
                </span>
              ))}
            </div>
          </div>
        </div>
      </div>

      <div className="grid2" style={{ marginTop: 12 }}>
        <div className="card">
          <div className="card-h">
            <span className="chipic c-cyan">
              <Ic name="users" />
            </span>
            <span className="ttl">账号消耗 Top 6</span>
            <span className="more">万 tokens</span>
          </div>
          <Hbars data={TOPACCT} />
        </div>
        <div className="card">
          <div className="card-h">
            <span className="chipic c-warn">
              <Ic name="clock" />
            </span>
            <span className="ttl">响应延迟分布</span>
          </div>
          <Histo data={LAT} />
        </div>
      </div>

      <div className="card" style={{ marginTop: 12 }}>
        <div className="card-h">
          <span className="chipic c-blue">
            <Ic name="grid" />
          </span>
          <span className="ttl">请求热力图 · 星期 × 小时</span>
        </div>
        <Heat data={HEAT} />
      </div>

      <div className="card" style={{ marginTop: 12, padding: 0 }}>
        <div className="card-h" style={{ padding: "14px 16px 2px", marginBottom: 4 }}>
          <span className="chipic c-violet">
            <Ic name="list" />
          </span>
          <span className="ttl">按日聚合</span>
        </div>
        <div style={{ padding: "0 10px 10px" }}>
          <table className="tbl" style={{ border: 0 }}>
            <thead>
              <tr>
                <th>日期</th>
                <th className="num">请求</th>
                <th className="num">输入</th>
                <th className="num">输出</th>
                <th className="num">缓存</th>
                <th className="num">失败</th>
                <th className="num">平均 TTFB</th>
              </tr>
            </thead>
            <tbody>
              {TOK7.map(r => (
                <tr key={r.d}>
                  <td className="mono">{r.d}</td>
                  <td className="num">{fmt(r.req)}</td>
                  <td className="num">{r.cin.toFixed(2)}M</td>
                  <td className="num">{r.cout.toFixed(2)}M</td>
                  <td className="num">{r.cch.toFixed(2)}M</td>
                  <td className="num" style={{ color: r.fail > 12 ? "var(--warn)" : "var(--t3)" }}>
                    {r.fail}
                  </td>
                  <td className="num">{r.ttfb}ms</td>
                </tr>
              ))}
              <tr style={{ color: "var(--t4)" }}>
                <td className="mono">合计</td>
                <td className="num">7,191</td>
                <td className="num">8.90M</td>
                <td className="num">2.84M</td>
                <td className="num">0.89M</td>
                <td className="num">71</td>
                <td className="num">434ms</td>
              </tr>
            </tbody>
          </table>
        </div>
      </div>
    </div>
  );
}
