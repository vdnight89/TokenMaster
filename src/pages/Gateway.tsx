/** 网关页：服务启停 / 密钥 / 模型映射 / 选号策略 / 路由拓扑（对应原型 #page-gateway / renderGateway） */
import { useState } from "react";
import { Ic } from "../lib/icons";
import { CopyBtn } from "../lib/ui";
import { KEY_FULL, KEY_MASKED, MAPS } from "../lib/mock";
import { fmt } from "../lib/fmt";
import { Topo } from "../charts/Topo";

export default function Gateway({ gwOn, onToggleGw }: { gwOn: boolean; onToggleGw: () => void }) {
  const [port, setPort] = useState("8787");
  const [keyShown, setKeyShown] = useState(false);
  const [tiering, setTiering] = useState(true);
  const [sticky, setSticky] = useState(true);
  const base = "http://127.0.0.1:" + (port || 8787);

  return (
    <div className="page-wrap">
      <div className="page-h">
        <div>
          <h1>网关</h1>
          <div className="sub">本地回环 OpenAI / Anthropic 协议网关 · 外部访问必须携带网关密钥</div>
        </div>
      </div>

      <div className="grid2">
        <div className="card">
          <div className="card-h">
            <span className="chipic c-blue">
              <Ic name="server" />
            </span>
            <span className="ttl">服务</span>
            <span className="grow"></span>
            {gwOn ? (
              <span className="bdg b-ok" style={{ fontSize: 11 }}>
                <Ic name="play" />
                运行中
              </span>
            ) : (
              <span className="bdg b-gray" style={{ fontSize: 11 }}>
                <Ic name="pause" />
                已停止
              </span>
            )}
            <button className={"sw ok" + (gwOn ? " on" : "")} style={{ pointerEvents: "none" }}></button>
            <button className="btn sm" onClick={onToggleGw}>
              启 / 停
            </button>
          </div>
          <div className="kv">
            <span className="k">监听地址</span>
            <select className="inp" style={{ width: 180 }}>
              <option>127.0.0.1（仅本机）</option>
              <option>0.0.0.0（局域网，需密钥）</option>
            </select>
          </div>
          <div className="kv">
            <span className="k">端口</span>
            <input className="inp mono w-sm" value={port} onChange={e => setPort(e.target.value)} />
          </div>
          <div className="kv">
            <span className="k">Base URL</span>
            <span className="keybox" style={{ flex: 1, padding: "6px 10px" }}>
              <span style={{ fontSize: 12 }}>{base}</span>
              <CopyBtn text={base} className="eye" />
            </span>
          </div>
          <div className="kv">
            <span className="k">协议面</span>
            <span className="bdg solid b-ok">OpenAI · /v1/chat/completions</span>
            <span className="bdg solid b-warn">Anthropic · /v1/messages</span>
          </div>
          <div className="kv">
            <span className="k">模型发现</span>
            <span className="mono muted" style={{ fontSize: 12 }}>
              GET /v1/models → provider/model 复合 id
            </span>
          </div>
        </div>

        <div className="card">
          <div className="card-h">
            <span className="chipic c-violet">
              <Ic name="key" />
            </span>
            <span className="ttl">网关密钥</span>
            <span className="grow"></span>
            <button className="btn xs">重置</button>
            <button className="btn xs danger">禁用</button>
          </div>
          <div className="keybox">
            <span>{keyShown ? KEY_FULL : KEY_MASKED}</span>
            <button
              className="ibtn eye"
              title="显示/隐藏"
              onClick={() => setKeyShown(v => !v)}
            >
              <Ic name={keyShown ? "eyeoff" : "eye"} />
            </button>
            <CopyBtn text={KEY_FULL} />
          </div>
          <div className="kv" style={{ marginTop: 6 }}>
            <span className="k">生成于</span>
            <span className="muted mono" style={{ fontSize: 12 }}>
              2026-09-28 21:14
            </span>
          </div>
          <div className="kv">
            <span className="k">最近使用</span>
            <span className="muted mono" style={{ fontSize: 12 }}>
              13:10:22 · Claude Code（本机）
            </span>
          </div>
          <div className="mnote mt-s">
            本机（回环）请求免鉴权；<b>非本机地址必须携带密钥</b>。密钥加密落盘，重置后旧密钥立即失效。
          </div>
        </div>
      </div>

      <div className="card" style={{ marginTop: 12, padding: 0 }}>
        <div className="card-h" style={{ padding: "14px 16px 2px", marginBottom: 4 }}>
          <span className="chipic c-cyan">
            <Ic name="list" />
          </span>
          <span className="ttl">模型映射</span>
          <span className="grow"></span>
          <button className="btn sm">
            <Ic name="plus" />
            添加规则
          </button>
        </div>
        <div className="mnote" style={{ padding: "0 16px 8px" }}>
          裸模型名按下列优先级路由；请求带 <b className="mono">provider/model</b> 前缀可强制指定 Provider，绕过映射。
        </div>
        <div style={{ padding: "0 10px 10px" }}>
          <table className="tbl map-tbl" style={{ border: 0 }}>
            <thead>
              <tr>
                <th style={{ width: 50 }}>优先级</th>
                <th>裸模型名</th>
                <th style={{ width: 20 }}></th>
                <th>目标 Provider / 模型</th>
                <th className="num">7 日命中</th>
                <th style={{ width: 70 }}>操作</th>
              </tr>
            </thead>
            <tbody>
              {MAPS.map(m => (
                <tr key={m.prio}>
                  <td>
                    <span className="prio">{m.prio}</span>
                  </td>
                  <td className="mono" style={{ fontWeight: 600, color: "var(--t1)" }}>
                    {m.bare}
                  </td>
                  <td style={{ color: "var(--t4)", width: 20 }}>
                    <Ic name="arrowR" size={13} />
                  </td>
                  <td className="mono" style={{ color: "var(--t3)", fontSize: "11.5px" }}>
                    {m.to}
                  </td>
                  <td className="num" style={{ color: "var(--t3)" }}>
                    {fmt(m.hits)}
                  </td>
                  <td style={{ width: 60 }}>
                    <button className="ibtn">
                      <Ic name="gear" />
                    </button>
                    <button className="ibtn err">
                      <Ic name="trash" />
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>

      <div className="grid2" style={{ marginTop: 12 }}>
        <div className="card">
          <div className="card-h">
            <span className="chipic c-ok">
              <Ic name="users" />
            </span>
            <span className="ttl">选号策略</span>
          </div>
          <div className="radio-row">
            <label className="rlabel on">
              <input type="radio" name="strategy" defaultChecked />
              <span className="rdot"></span>
              <span>
                <span className="rt">先到期优先</span>
                <span className="rd">优先消耗即将过期的账号与临期积分，减少作废浪费（推荐）</span>
              </span>
            </label>
            <label className="rlabel">
              <input type="radio" name="strategy" />
              <span className="rdot"></span>
              <span>
                <span className="rt">剩余额度最多</span>
                <span className="rd">优先使用余量充足的账号，压平各池消耗速度</span>
              </span>
            </label>
          </div>
          <div className="kv mt-s">
            <span className="k">临期分档</span>
            <span className="grow"></span>
            <span className="muted" style={{ fontSize: "11.5px", marginRight: 8 }}>
              buddy / workbuddy / lobsterai / trae 按余额到期日分档
            </span>
            <button className={"sw" + (tiering ? " on" : "")} onClick={() => setTiering(v => !v)}></button>
          </div>
          <div className="kv">
            <span className="k">换号重试上限</span>
            <span className="mono" style={{ fontSize: 12 }}>
              min(池深 × 2, 4) 次 · 429 无 Retry-After 时连续快切 ≤ 2 个账号
            </span>
          </div>
          <div className="kv">
            <span className="k">限流冷却</span>
            <span className="mono" style={{ fontSize: 12 }}>
              按 Retry-After 记录，上限 300s · 粒度 Provider × 模型 × 账号
            </span>
          </div>
          <div className="kv">
            <span className="k">粘性会话</span>
            <button className={"sw" + (sticky ? " on" : "")} onClick={() => setSticky(v => !v)} style={{ marginLeft: "auto" }}></button>
          </div>
        </div>
        <div className="card">
          <div className="card-h">
            <span className="chipic c-blue">
              <Ic name="globe" />
            </span>
            <span className="ttl">路由拓扑</span>
          </div>
          <div className="topo">
            <Topo />
          </div>
        </div>
      </div>
    </div>
  );
}
