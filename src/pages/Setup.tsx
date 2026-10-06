/** 接入页：接入信息 / 健康自检 / 客户端配置示例 / 可用模型（对应原型 #page-setup / renderSetup） */
import { useState } from "react";
import type { ReactNode } from "react";
import { Ic } from "../lib/icons";
import { CopyBtn } from "../lib/ui";
import { ProviderBrandIcon } from "../lib/brand";
import { MODEL_LIST, PROVIDERS, SETUPS } from "../lib/mock";
import type { SetupTab } from "../lib/mock";

const TABS: readonly SetupTab[] = ["Claude Code", "Cline", "Continue", "Codex CLI"];

/** 语法高亮（注释 → .cm，"…"： → .kw；以 React 节点输出，替代原型的转义 + 正则替换 innerHTML） */
function highlightCode(code: string): ReactNode[] {
  const out: ReactNode[] = [];
  const lines = code.split("\n");
  lines.forEach((line, li) => {
    let quote: string | null = null;
    let hash = -1;
    for (let i = 0; i < line.length; i++) {
      const c = line[i];
      if (quote) {
        if (c === quote) quote = null;
      } else if (c === '"') {
        quote = '"';
      } else if (c === "#") {
        hash = i;
        break;
      }
    }
    const codePart = hash >= 0 ? line.slice(0, hash) : line;
    const comment = hash >= 0 ? line.slice(hash) : null;
    const re = /"[^"]*":/g;
    let last = 0;
    let m: RegExpExecArray | null;
    let k = 0;
    while ((m = re.exec(codePart)) !== null) {
      if (m.index > last) out.push(codePart.slice(last, m.index));
      out.push(
        <span className="kw" key={`${li}-k${k++}`}>
          {m[0]}
        </span>,
      );
      last = m.index + m[0].length;
    }
    if (last < codePart.length) out.push(codePart.slice(last));
    if (comment)
      out.push(
        <span className="cm" key={`${li}-c`}>
          {comment}
        </span>,
      );
    if (li < lines.length - 1) out.push("\n");
  });
  return out;
}

export default function Setup() {
  const [tab, setTab] = useState<SetupTab>("Claude Code");
  const s = SETUPS[tab];

  return (
    <div className="page-wrap">
      <div className="page-h">
        <div>
          <h1>接入</h1>
          <div className="sub">把任意 AI 编程客户端指向本地网关 · TokenMaster 不代写客户端配置文件</div>
        </div>
      </div>

      <div className="grid2">
        <div className="card">
          <div className="card-h">
            <span className="chipic c-blue">
              <Ic name="book" />
            </span>
            <span className="ttl">接入信息</span>
          </div>
          <div className="kv">
            <span className="k">Base URL</span>
            <span className="keybox" style={{ flex: 1, padding: "6px 10px" }}>
              <span style={{ fontSize: 12 }}>http://127.0.0.1:8787</span>
              <CopyBtn text="http://127.0.0.1:8787" className="eye" />
            </span>
          </div>
          <div className="kv">
            <span className="k">网关密钥</span>
            <span className="keybox" style={{ flex: 1, padding: "6px 10px" }}>
              <span className="mono" style={{ fontSize: 12 }}>sk-tm-••••••••••••7f3a</span>
              <span className="muted" style={{ fontSize: 11 }}>网关页可复制</span>
            </span>
          </div>
          <div className="mnote mt-s">
            OpenAI 客户端用 <b className="mono">/v1</b>；Anthropic 客户端（Claude Code 等）直接指向根路径即可，密钥放{" "}
            <b className="mono">AUTH_TOKEN</b> / <b className="mono">x-api-key</b>。
          </div>
        </div>
        <div className="card">
          <div className="card-h">
            <span className="chipic c-violet">
              <Ic name="shield" />
            </span>
            <span className="ttl">健康自检</span>
          </div>
          <div className="kv">
            <span className="dot on"></span>
            <span className="grow" style={{ marginLeft: 2 }}>
              网关监听 127.0.0.1:8787
            </span>
            <span className="bdg b-ok">通过</span>
          </div>
          <div className="kv">
            <span className="dot on"></span>
            <span className="grow" style={{ marginLeft: 2 }}>
              网关密钥已配置
            </span>
            <span className="bdg b-ok">通过</span>
          </div>
          <div className="kv">
            <span className="dot on"></span>
            <span className="grow" style={{ marginLeft: 2 }}>
              至少一个 Provider 可用（34 / 42）
            </span>
            <span className="bdg b-ok">通过</span>
          </div>
          <div className="kv">
            <span className="dot warn"></span>
            <span className="grow" style={{ marginLeft: 2 }}>
              qoder 池限流中 · glm 池冷却 04:32
            </span>
            <span className="bdg b-warn">注意</span>
          </div>
          <div className="mnote mt-s">
            连通性验证可用 <span className="mono" style={{ color: "#7dabf8" }}>curl http://127.0.0.1:8787/v1/models</span>。
          </div>
        </div>
      </div>

      <div className="card" style={{ marginTop: 12 }}>
        <div className="card-h">
          <span className="chipic c-cyan">
            <Ic name="term" />
          </span>
          <span className="ttl">客户端配置示例</span>
          <span className="grow"></span>
          <div className="seg" style={{ marginRight: 8 }}>
            {TABS.map(t => (
              <button key={t} className={tab === t ? "on" : ""} onClick={() => setTab(t)}>
                {t}
              </button>
            ))}
          </div>
        </div>
        <div className="codebox">
          <div className="cb-h">
            <span className="fl">{s.lang}</span>
            <span className="grow"></span>
            <button className="ibtn" style={{ display: "none" }}></button>
          </div>
          <pre>{highlightCode(s.code)}</pre>
        </div>
      </div>

      <div className="card" style={{ marginTop: 12, padding: 0 }}>
        <div className="card-h" style={{ padding: "14px 16px 2px", marginBottom: 4 }}>
          <span className="chipic c-ok">
            <Ic name="zap" />
          </span>
          <span className="ttl">可用模型</span>
          <span className="more mono" style={{ fontWeight: 400 }}>
            GET /v1/models · 12 / 27
          </span>
        </div>
        <div style={{ padding: "0 10px 10px" }}>
          <table className="tbl" style={{ border: 0 }}>
            <thead>
              <tr>
                <th>模型 id（裸名）</th>
                <th>默认 Provider</th>
                <th>上游模型</th>
                <th>状态</th>
              </tr>
            </thead>
            <tbody>
              {MODEL_LIST.map(m => (
                <tr key={m[0]}>
                  <td className="mono" style={{ fontWeight: 600, color: "var(--t1)" }}>
                    {m[0]}
                  </td>
                  <td>
                    <span style={{ display: "inline-flex", alignItems: "center", gap: 6, fontSize: "11.5px" }}>
                      <ProviderBrandIcon provider={m[1]} size={18} />
                      <span style={{ color: "var(--t3)" }}>{PROVIDERS[m[1]].name}</span>
                    </span>
                  </td>
                  <td className="mono" style={{ color: "var(--t3)", fontSize: "11.5px" }}>
                    {m[2]}
                  </td>
                  <td>
                    {m[3] === "ok" ? (
                      <span className="bdg b-ok">可用</span>
                    ) : m[3] === "warn" ? (
                      <span className="bdg b-warn">部分限流</span>
                    ) : (
                      <span className="bdg b-err">池失效</span>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    </div>
  );
}
