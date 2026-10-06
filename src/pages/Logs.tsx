/** 日志页：快捷过滤 + 监控表 + 行展开换号轨迹（对应原型 #page-logs / renderLogs） */
import { Fragment, useState } from "react";
import { Ic } from "../lib/icons";
import { LOGS } from "../lib/mock";
import type { LogEntry, TrailStep } from "../lib/mock";

type LogFilter = "all" | "ok" | "limit" | "err" | "captcha";

const FILTERS: readonly (readonly [LogFilter, string])[] = [
  ["all", "全部"],
  ["ok", "成功"],
  ["limit", "限流"],
  ["err", "错误"],
  ["captcha", "验证码"],
];

export default function Logs() {
  const [filter, setFilter] = useState<LogFilter>("all");
  const [openTrail, setOpenTrail] = useState<ReadonlySet<number>>(() => new Set());
  const rows = LOGS.map((r, i) => ({ r, i })).filter(x => filter === "all" || x.r.cls === filter);
  const toggle = (i: number) => {
    setOpenTrail(prev => {
      const nx = new Set(prev);
      if (nx.has(i)) nx.delete(i);
      else nx.add(i);
      return nx;
    });
  };
  return (
    <div className="page-wrap">
      <div className="page-h">
        <div>
          <h1>日志</h1>
          <div className="sub">网关请求日志与诊断 · 点击带轨迹的行查看换号过程</div>
        </div>
        <div className="actions">
          <button className="btn danger">
            <Ic name="trash" />
            清空
          </button>
        </div>
      </div>
      <div className="toolbar">
        <div className="search">
          <Ic name="search" />
          <input className="inp" placeholder="搜索模型 / 路径 / 状态码…" />
        </div>
        <div style={{ display: "flex", gap: 6 }}>
          {FILTERS.map(([k, l]) => (
            <button key={k} className={"fpill" + (filter === k ? " on" : "")} onClick={() => setFilter(k)}>
              {l}
            </button>
          ))}
        </div>
        <span className="grow"></span>
        <span className="count-chip">{rows.length} 条 · 今日 1,247 · 账本滚动 32MB</span>
      </div>
      <div className="tblwrap">
        <table className="tbl">
          <thead>
            <tr>
              <th>状态</th>
              <th>方法</th>
              <th>模型</th>
              <th>账号</th>
              <th>路径</th>
              <th className="num">Tokens</th>
              <th className="num">TTFB</th>
              <th className="num">耗时</th>
              <th className="num">时间</th>
            </tr>
          </thead>
          <tbody>
            {rows.length === 0 ? (
              <tr>
                <td colSpan={9} style={{ textAlign: "center", padding: 40, color: "var(--t4)" }}>
                  该分类暂无日志
                </td>
              </tr>
            ) : (
              rows.map(({ r, i }) => <LogRow key={i} r={r} expanded={openTrail.has(i)} onToggle={() => toggle(i)} />)
            )}
          </tbody>
        </table>
      </div>
      <div className="mnote mt-s">账本按大小滚动（32MB），完整原始报文默认不落盘；错误记录保留上游原始错误体用于诊断。</div>
    </div>
  );
}

/** 单条日志行（含可选的换号轨迹展开行）；仪表盘「最近请求」复用 */
export function LogRow({ r, expanded, onToggle }: { r: LogEntry; expanded?: boolean; onToggle?: () => void }) {
  const stCls = r.st >= 200 && r.st < 400 ? "b-ok" : r.st === 429 ? "b-warn" : r.st === 3007 ? "b-violet" : "b-err";
  return (
    <>
      <tr className={r.trail ? "has-trail" + (expanded ? " exp" : "") : undefined} data-cls={r.cls} onClick={r.trail ? onToggle : undefined}>
        <td>
          <span className={"bdg solid " + stCls}>{r.st}</span>
        </td>
        <td className="mono" style={{ color: "var(--t3)" }}>
          {r.proto === "gateway" ? "GW" : "POST"}
        </td>
        <td>
          <span className="model">{r.model}</span>
        </td>
        <td>
          <span className="acct">{r.acct}</span>
        </td>
        <td>
          <span className="path">{r.path}</span>
        </td>
        <td className="num" style={{ color: "var(--t3)" }}>
          {r.tok}
        </td>
        <td className="num">{r.ttfb === "—" ? "—" : r.ttfb + "ms"}</td>
        <td className="num" style={{ color: "var(--t3)" }}>
          {r.dur}
        </td>
        <td className="num" style={{ color: "var(--t4)" }}>
          {r.time}
        </td>
      </tr>
      {r.trail ? (
        <tr className="tr-x" style={{ display: expanded ? "table-row" : "none" }}>
          <td colSpan={9} style={{ padding: 0 }}>
            <Trail steps={r.trail} />
          </td>
        </tr>
      ) : null}
    </>
  );
}

function Trail({ steps }: { steps: TrailStep[] }) {
  return (
    <div className="trail">
      <div className="t-h">换号轨迹 · {steps.filter(s => s.s !== "none").length} 次尝试</div>
      <div className="steps">
        {steps.map((st, i) => (
          <div className={"step " + (st.s === "ok" ? "final" : "done")} key={i}>
            <div className="bar">
              <i></i>
            </div>
            <div className="tt" style={{ color: st.s === "ok" ? "#34d399" : st.s === "none" ? "#64748b" : "#fb7185" }}>
              {i + 1}. {st.t}
            </div>
            <div className="td">
              {st.d.map((l, j) => (
                <Fragment key={j}>
                  {j > 0 && <br />}
                  {l}
                </Fragment>
              ))}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
