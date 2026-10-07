/** 仪表盘页（对应原型 #page-dash / renderDash） */
import { useState } from "react";
import { Ic } from "../lib/icons";
import { toast } from "../lib/toast";
import { ProviderBrandIcon, ProviderGlyphIcon } from "../lib/brand";
import type { ModalKind } from "../lib/ui";
import type { PageKey } from "../lib/nav";
import { ERR_SEG, LOGS, POOLS, TODOS } from "../lib/mock";
import { useProviders } from "../lib/ipc";
import { PROVIDERS as MOCK_PROVIDERS } from "../lib/mock";
import type { Todo } from "../lib/mock";
import { fmt, sum } from "../lib/fmt";
import { Gauge } from "../charts/Gauge";
import { ThruChart, useThru } from "../charts/Thru";
import { LogRow } from "./Logs";

export default function Dash({
  active,
  gwOn,
  doneTodos,
  onTodoDone,
  openModal,
  nav,
}: {
  active: boolean;
  gwOn: boolean;
  doneTodos: ReadonlySet<string>;
  onTodoDone: (id: string) => void;
  openModal: (m: Exclude<ModalKind, null>) => void;
  nav: (p: PageKey) => void;
}) {
  const thru = useThru(active);
  const cur = Math.round(thru[thru.length - 1]);
  const mx = Math.round(Math.max(...thru));
  const avg = Math.round(sum(thru) / thru.length);
  const [pending, setPending] = useState<string | null>(null);
  const { data: PROVIDERS } = useProviders(MOCK_PROVIDERS);
  const remaining = TODOS.filter(t => !doneTodos.has(t.id)).length;
  const tot = sum(ERR_SEG.map(s => s[1]));

  const onTodoAct = (t: Todo) => {
    if (t.id === "claim") {
      openModal("claim");
      return;
    }
    if (t.act === "详情" || t.act === "查看") {
      nav("accounts");
      return;
    }
    setPending(t.id);
    window.setTimeout(() => {
      setPending(null);
      onTodoDone(t.id);
      toast("ok", t.act === "签到" ? "签到成功" : "处理完成", t.t + (t.act === "签到" ? " · 积分已入账" : ""));
    }, 900);
  };

  return (
    <div className="page-wrap">
      <div className="page-h">
        <div>
          <h1>下午好 👋</h1>
          <div className="sub">
            10 月 6 日 周二 · 今天有 <b style={{ color: "var(--warn)" }}>{remaining}</b> 件该处理的事
          </div>
        </div>
        <div className="actions">
          <button className="btn">
            <Ic name="refresh" />
            刷新额度
          </button>
          <button className="btn p" onClick={() => openModal("add")}>
            <Ic name="plus" />
            添加账号
          </button>
        </div>
      </div>

      {/* KPI 状态栏 */}
      <div className="card" style={{ padding: 0, marginBottom: 12 }}>
        <div className="statusbar">
          <div className="sbc">
            <div className="k">网关服务</div>
            <div className="v">{gwOn ? <span className="okc">运行中</span> : <span style={{ color: "var(--t4)" }}>已停止</span>}</div>
            <div className="f">{gwOn ? "127.0.0.1:8787 · 双协议面" : "端口 8787 已释放"}</div>
          </div>
          <div className="sbc">
            <div className="k">今日请求</div>
            <div className="v">1,247</div>
            <div className="f">
              <span className="badc">失败 18</span> · 限流冷却 67 次
            </div>
          </div>
          <div className="sbc">
            <div className="k">今日 Tokens</div>
            <div className="v">
              2.31<small>M</small>
            </div>
            <div className="f">输入 1.62M · 输出 512K</div>
          </div>
          <div className="sbc">
            <div className="k">号池吞吐</div>
            <div className="v">
              {cur}
              <small>tok/s</small>
            </div>
            <div className="f">
              峰值 {mx} · 首字节 <b className="okc">412ms</b>
            </div>
          </div>
          <div className="sbc">
            <div className="k">平均响应</div>
            <div className="v">
              6.2<small>s</small>
            </div>
            <div className="f">整发时长 · 首字节 412ms</div>
          </div>
          <div className="sbc">
            <div className="k">池健康</div>
            <div className="v">
              37<small>/ 42</small>
            </div>
            <div className="f">1 限流 · 2 临期 · 1 失效 · 1 停用</div>
          </div>
        </div>
      </div>

      <div className="grid23">
        {/* 吞吐曲线 */}
        <div className="card" style={{ display: "flex", flexDirection: "column" }}>
          <div className="card-h">
            <span className="chipic c-blue">
              <Ic name="activity" />
            </span>
            <span className="ttl">号池吞吐 · 实时</span>
            <span className="more mono" style={{ fontWeight: 400 }}>
              2 秒采样 · 最近 60 次
            </span>
          </div>
          <ThruChart data={thru} />
          <div style={{ display: "flex", gap: 22, paddingTop: 11, marginTop: "auto", borderTop: "1px solid var(--line)" }}>
            <div>
              <div className="up">当前</div>
              <div className="mono" style={{ fontSize: 15, fontWeight: 700 }}>
                {cur} tok/s
              </div>
            </div>
            <div>
              <div className="up">峰值</div>
              <div className="mono" style={{ fontSize: 15, fontWeight: 700 }}>
                {mx} tok/s
              </div>
            </div>
            <div>
              <div className="up">均值</div>
              <div className="mono" style={{ fontSize: 15, fontWeight: 700 }}>
                {avg} tok/s
              </div>
            </div>
            <div>
              <div className="up">活跃流</div>
              <div className="mono" style={{ fontSize: 15, fontWeight: 700 }}>
                3
              </div>
            </div>
            <div style={{ marginLeft: "auto", textAlign: "right" }}>
              <div className="up">单发最大</div>
              <div className="mono" style={{ fontSize: 15, fontWeight: 700 }}>
                96 tok/s
              </div>
            </div>
          </div>
        </div>
        {/* 今日待办 */}
        <div className="card">
          <div className="card-h">
            <span className="chipic c-warn">
              <Ic name="inbox" />
            </span>
            <span className="ttl">今日待办</span>
            <span className="more" style={{ fontWeight: 400 }}>
              签到 / 领取 / 临期
            </span>
          </div>
          <div className="todos">
            {TODOS.map(t => {
              const done = doneTodos.has(t.id);
              return (
                <div className={"todo" + (done ? " done" : "")} data-id={t.id} key={t.id}>
                  <ProviderBrandIcon provider={t.pv} />
                  <div className="tx grow">
                    <div className="tt">{t.t}</div>
                    <div className="td">{t.d}</div>
                  </div>
                  {done ? (
                    <span className="bdg b-ok">
                      <Ic name="check" />
                      {t.id === "claim" ? "已领取" : "已完成"}
                    </span>
                  ) : (
                    <button
                      className={"btn sm" + (t.act === "领取" ? " p" : "")}
                      disabled={pending === t.id}
                      onClick={() => onTodoAct(t)}
                    >
                      {pending === t.id ? (
                        <span className="spin" style={{ display: "inline-flex", width: 14, height: 14 }}>
                          <Ic name="refresh" />
                        </span>
                      ) : (
                        t.act
                      )}
                    </button>
                  )}
                </div>
              );
            })}
          </div>
          {remaining === 0 && (
            <div className="empty" style={{ display: "flex", padding: 20 }}>
              全部处理完了 ✓
            </div>
          )}
        </div>
      </div>

      <div className="grid2" style={{ marginTop: 12 }}>
        {/* 池额度仪表 */}
        <div className="card">
          <div className="card-h">
            <span className="chipic c-ok">
              <Ic name="gauge" />
            </span>
            <span className="ttl">Provider 池额度</span>
            <span className="more" onClick={() => nav("accounts")}>
              查看账号 →
            </span>
          </div>
          <div className="gauges">
            {POOLS.map(([k, p, sub]) => (
              <div className="mg" key={k}>
                <Gauge pct={p} />
                <div className="gn" style={{ color: PROVIDERS[k].color, display: "flex", alignItems: "center", justifyContent: "center", gap: 4 }}>
                  <ProviderGlyphIcon provider={k} size={13} />
                  {PROVIDERS[k].name}
                </div>
                <div className="gs">{sub}</div>
              </div>
            ))}
            <div className="mg" style={{ justifyContent: "center" }}>
              <div className="up" style={{ marginBottom: 8 }}>
                池合计
              </div>
              <div className="mono" style={{ fontSize: 20, fontWeight: 700 }}>
                64.2%
              </div>
              <div className="gs">42 账号 · 37 可用</div>
            </div>
          </div>
        </div>
        {/* 错误分布 */}
        <div className="card">
          <div className="card-h">
            <span className="chipic c-err">
              <Ic name="alertr" />
            </span>
            <span className="ttl">请求结果分布</span>
            <span className="more">
              今日成功率 <b className="mono" style={{ color: "var(--ok)" }}>98.4%</b>
            </span>
          </div>
          <div style={{ display: "flex", height: 10, borderRadius: 5, overflow: "hidden", gap: 2, marginBottom: 6 }}>
            {ERR_SEG.map(s => (
              <i key={s[0]} style={{ flex: s[1], background: s[2] }} title={`${s[0]} ${s[1]}`}></i>
            ))}
          </div>
          <div>
            {ERR_SEG.map(s => (
              <div className="kv" style={{ padding: "7px 0" }} key={s[0]}>
                <span className="grow" style={{ display: "flex", alignItems: "center", gap: 7 }}>
                  <i style={{ width: 8, height: 8, borderRadius: 2.5, background: s[2] }}></i>
                  {s[0]}
                </span>
                <span className="mono" style={{ color: "var(--t3)" }}>
                  {fmt(s[1])} · {((s[1] / tot) * 100).toFixed(1)}%
                </span>
              </div>
            ))}
          </div>
        </div>
      </div>

      {/* 最近请求 */}
      <div className="card" style={{ marginTop: 12, padding: 0 }}>
        <div className="card-h" style={{ padding: "14px 16px 2px", marginBottom: 4 }}>
          <span className="chipic c-violet">
            <Ic name="term" />
          </span>
          <span className="ttl">最近请求</span>
          <span className="more" onClick={() => nav("logs")}>
            全部日志 →
          </span>
        </div>
        <div style={{ padding: "0 10px 10px" }}>
          <table className="tbl" style={{ border: 0 }}>
            <thead>
              <tr>
                <th>状态</th>
                <th>协议</th>
                <th>模型</th>
                <th>账号</th>
                <th>路径</th>
                <th className="num">Tokens</th>
                <th className="num">TTFB</th>
                <th className="num">耗时</th>
                <th className="num">时间</th>
              </tr>
            </thead>
            <tbody>{LOGS.slice(0, 7).map((r, i) => (
              <LogRow key={i} r={r} />
            ))}</tbody>
          </table>
        </div>
      </div>
    </div>
  );
}
