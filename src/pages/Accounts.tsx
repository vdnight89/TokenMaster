/** 账号页：搜索 + 状态筛选 + Provider 下拉 + 账号卡网格 + 分页（对应原型 #page-accounts / renderAccounts） */
import { useState } from "react";
import { Ic } from "../lib/icons";
import { toast } from "../lib/toast";
import { ProviderBrandIcon } from "../lib/brand";
import type { ModalKind } from "../lib/ui";
import { CoolTimer } from "../lib/ui";
import { PROVIDERS, PV_KEYS, STATE_LAB } from "../lib/mock";
import type { Account, AccNote, AccState } from "../lib/mock";
import { QuotaItem } from "../charts/QuotaItem";

type AccFilter = "all" | AccState;

const FILTERS: readonly (readonly [AccFilter, string])[] = [
  ["all", "全部"],
  ["ok", "可用"],
  ["cool", "限流"],
  ["exp", "临期"],
  ["dead", "异常"],
  ["off", "停用"],
];

export default function Accounts({
  accounts,
  onTogglePower,
  openModal,
}: {
  accounts: Account[];
  onTogglePower: (name: string) => void;
  openModal: (m: Exclude<ModalKind, null>) => void;
}) {
  const [filter, setFilter] = useState<AccFilter>("all");
  const [rawQuery, setRawQuery] = useState("");
  const [pg, setPg] = useState(1);

  const query = rawQuery.trim().toLowerCase();
  const counts: Record<AccFilter, number> = {
    all: accounts.length,
    ok: accounts.filter(a => a.state === "ok").length,
    cool: accounts.filter(a => a.state === "cool").length,
    exp: accounts.filter(a => a.state === "exp").length,
    dead: accounts.filter(a => a.state === "dead").length,
    off: accounts.filter(a => a.state === "off").length,
  };
  let list = filter === "all" ? accounts : accounts.filter(a => a.state === filter);
  if (query) list = list.filter(a => a.name.toLowerCase().includes(query));
  const per = 12;
  const pages = Math.max(1, Math.ceil(list.length / per));
  const pageNo = Math.min(pg, pages);
  const page = list.slice((pageNo - 1) * per, pageNo * per);

  return (
    <div className="page-wrap">
      <div className="page-h">
        <div>
          <h1>账号</h1>
          <div className="sub">凭据加密存储 · 到期前自动刷新 · 失效自动提示</div>
        </div>
        <div className="actions">
          <button className="btn" onClick={() => openModal("import")}>
            <Ic name="upload" />
            从 zcode-pool 导入
          </button>
          <button className="btn p" onClick={() => openModal("add")}>
            <Ic name="plus" />
            添加账号
          </button>
        </div>
      </div>
      <div className="toolbar">
        <div className="search">
          <Ic name="search" />
          <input
            className="inp"
            placeholder="搜索账号…"
            value={rawQuery}
            onChange={e => {
              setRawQuery(e.target.value);
              setPg(1);
            }}
          />
        </div>
        <div className="seg">
          {FILTERS.map(([k, l]) => (
            <button
              key={k}
              className={filter === k ? "on" : ""}
              onClick={() => {
                setFilter(k);
                setPg(1);
              }}
            >
              {l} <span className="mono" style={{ opacity: 0.65 }}>{counts[k]}</span>
            </button>
          ))}
        </div>
        <select className="inp" style={{ width: 150 }}>
          <option>全部 Provider</option>
          {PV_KEYS.map(k => (
            <option key={k}>{PROVIDERS[k].name}</option>
          ))}
        </select>
        <span className="grow"></span>
        <span className="count-chip">
          显示 {page.length} / {list.length} 个账号 · 共 42 个 · 15 家 Provider
        </span>
      </div>
      <div className="acc-grid">
        {page.map(a => (
          <AccCard
            key={a.pv + a.name}
            a={a}
            onCi={() => toast("ok", "签到成功", a.name + " · 今日积分已入账")}
            onPw={() => onTogglePower(a.name)}
          />
        ))}
      </div>
      <div className="pager">
        <button disabled={pageNo === 1} onClick={() => setPg(pageNo - 1)}>
          <Ic name="chevL" />
        </button>
        {Array.from({ length: pages }, (_, i) => (
          <button key={i + 1} className={pageNo === i + 1 ? "on" : ""} onClick={() => setPg(i + 1)}>
            {i + 1}
          </button>
        ))}
        <button disabled={pageNo === pages} onClick={() => setPg(pageNo + 1)}>
          <Ic name="chevR" />
        </button>
      </div>
    </div>
  );
}

function AccCard({ a, onCi, onPw }: { a: Account; onCi: () => void; onPw: () => void }) {
  const pv = PROVIDERS[a.pv];
  const [cls, lab] = STATE_LAB[a.state];
  const tierCls = a.tier === "ULTRA" ? "tier-ultra" : a.tier === "PRO" ? "tier-pro" : "tier-free";
  return (
    <div className={"acc" + (a.cur ? " cur" : "") + (a.state === "cool" ? " cool" : "") + (a.state === "dead" ? " dead" : "")}>
      <div className="hd">
        <input type="checkbox" className="cbx" />
        <ProviderBrandIcon provider={a.pv} />
        <div className="grow" style={{ minWidth: 0 }}>
          <div className="nm" style={a.cur ? { color: "#7dabf8" } : undefined}>
            {a.name}
          </div>
          <div className="meta">
            {pv.name} · 更新 {a.when}
          </div>
        </div>
        <div style={{ display: "flex", flexDirection: "column", gap: 4, alignItems: "flex-end" }}>
          <span className={"bdg " + cls}>{lab}</span>
          <span className={"bdg tier " + tierCls} style={{ fontSize: 9 }}>
            {a.tier}
          </span>
        </div>
      </div>
      {a.cur ? (
        <div style={{ display: "flex", alignItems: "center", gap: 5, fontSize: 10, color: "#7dabf8", fontWeight: 700 }}>
          <Ic name="okc" size={11} /> 当前调度账号
        </div>
      ) : null}
      <div className="qs">
        {a.q.map((qq, i) => (
          <QuotaItem key={i} m={qq.m} p={qq.p} r={qq.r} frozen={a.state === "cool" && String(qq.r).includes(":")} />
        ))}
      </div>
      {a.note ? (
        <div className={"note " + a.note.kind}>
          <Ic name={a.note.icon} />
          <NoteSegs note={a.note} />
        </div>
      ) : null}
      <div className="ft">
        <span className="when">id: {a.pv}-{(a.name.length * 7) % 9000 + 1000}</span>
        <button className="ibtn" title="详情">
          <Ic name="info" />
        </button>
        <button className="ibtn" title="刷新额度">
          <Ic name="refresh" />
        </button>
        <button className="ibtn ok" title="签到" onClick={onCi}>
          <Ic name="cal" />
        </button>
        <button className="ibtn warn" title={a.state === "off" ? "启用" : "停用"} onClick={onPw}>
          <Ic name="power" />
        </button>
        <button className="ibtn err" title="删除">
          <Ic name="trash" />
        </button>
      </div>
    </div>
  );
}

/** 结构化提示条渲染（替代原型 note 里的 <b>/<i> 拼接） */
function NoteSegs({ note }: { note: AccNote }) {
  return (
    <span>
      {note.segs.map((s, i) =>
        s.timer ? (
          <b className="mono" key={i}>
            <CoolTimer initial={s.text} />
          </b>
        ) : s.b && s.mono ? (
          <b className="mono" key={i}>
            {s.text}
          </b>
        ) : s.b ? (
          <b key={i}>{s.text}</b>
        ) : (
          <span key={i}>{s.text}</span>
        ),
      )}
    </span>
  );
}
