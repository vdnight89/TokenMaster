/** 模态：添加账号（四类登录引导两步；对应原型 #modal-add / openAdd / addStep） */
import { useState } from "react";
import { Ic } from "../lib/icons";
import { toast } from "../lib/toast";
import { CopyBtn } from "../lib/ui";
import { PROVIDERS, PV_KEYS } from "../lib/mock";

type AddStep = "pick" | "paste" | "flow";

const OPTS = [
  { m: "flow", cls: "c-blue", icon: "globe", name: "浏览器 OAuth 回调", desc: "拉起默认浏览器授权，本地回环接收回调 · codearts / lobsterai / trae / gemini" },
  { m: "flow", cls: "c-ok", icon: "refresh", name: "设备码轮询", desc: "打开授权页输入设备码，GUI 轮询换凭据 · qoder / cline / minimax / zcode" },
  { m: "flow", cls: "c-violet", icon: "grid", name: "扫码 / 短信本地页", desc: "内嵌 WebView 展示二维码或短信验证 · loomy / raccoon" },
  { m: "paste", cls: "c-cyan", icon: "term", name: "手动粘贴 token / key", desc: "按 Provider 提示所需字段与格式 · opencode / commandcode 等" },
] as const;

export default function AddModal({ onClose }: { onClose: () => void }) {
  const [step, setStep] = useState<AddStep>("pick");
  return (
    <div
      className="ov open"
      onClick={e => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="modal">
        <div className="mh">
          <span className="chipic c-blue">
            <Ic name="plus" />
          </span>
          添加账号
          <button className="ibtn" style={{ marginLeft: "auto" }} onClick={onClose}>
            <Ic name="x" />
          </button>
        </div>
        <div className="mb">
          {step === "pick" && (
            <>
              <div className="fld" style={{ marginBottom: 12 }}>
                <span className="up">选择 Provider</span>
                <select className="inp" style={{ width: "100%" }}>
                  {PV_KEYS.map(k => (
                    <option key={k} value={k}>
                      {PROVIDERS[k].name}
                    </option>
                  ))}
                </select>
              </div>
              <span className="up">登录方式（按 Provider 能力自动给出）</span>
              <div className="mt-s">
                {OPTS.map(o => (
                  <button className="opt" key={o.name} onClick={() => setStep(o.m)}>
                    <span className={"chipic " + o.cls}>
                      <Ic name={o.icon} />
                    </span>
                    <span className="ocol">
                      <span className="oname">{o.name}</span>
                      <span className="odesc">{o.desc}</span>
                    </span>
                    <Ic name="chevR" />
                  </button>
                ))}
              </div>
            </>
          )}
          {step === "paste" && (
            <>
              <div className="mnote" style={{ marginBottom: 12 }}>
                以 <b>zcode</b> 为例：粘贴官方 credentials.json 内容或 <span className="mono">zcodejwttoken</span>，支持{" "}
                <span className="mono">enc:v1:</span> 加密格式（密钥派生与官方一致）。
              </div>
              <textarea className="inp mono" rows={5} style={{ width: "100%" }} placeholder="粘贴 JSON 或 token…"></textarea>
              <div className="mt-s">
                <label className="rlabel on">
                  <span className="rdot"></span>
                  <span>
                    <span className="rt">导入后立即刷新额度</span>
                    <span className="rd">入池补刷新一轮，马上可参与轮询</span>
                  </span>
                </label>
              </div>
            </>
          )}
          {step === "flow" && (
            <>
              <div className="mnote" style={{ marginBottom: 12 }}>
                以 <b>zcode · 设备码轮询</b> 为例：
              </div>
              <div style={{ display: "flex", flexWrap: "wrap", gap: 6, marginBottom: 14 }}>
                <span className="bdg b-ok">
                  <Ic name="check" />
                  打开授权页
                </span>
                <span className="bdg b-blue">
                  <Ic name="activity" />
                  等待授权 · 轮询中
                </span>
                <span className="bdg b-gray">兑换凭据</span>
                <span className="bdg b-gray">入池 + 补刷新</span>
              </div>
              <div className="keybox" style={{ marginBottom: 10 }}>
                设备码：<b>HGKF-LMNP-QSTX</b>
                <CopyBtn text="HGKF-LMNP-QSTX" className="eye" />
              </div>
              <div className="mnote">
                打开 <span className="mono" style={{ color: "#7dabf8" }}>auth.z.ai/device</span>{" "}
                输入设备码；授权完成后本窗口自动进入下一步。超时 300s，可重新生成。
              </div>
            </>
          )}
        </div>
        <div className="mf">
          <button className="btn" onClick={onClose}>
            取消
          </button>
          {step === "paste" && (
            <button
              className="btn p"
              onClick={() => {
                onClose();
                toast("ok", "账号已添加", "凭据已加密写入 ~/.tokenmaster/accounts/ · 已入池并补刷新");
              }}
            >
              导入并加入池
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
