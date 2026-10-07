/** 设置页：通用 / 出站代理 / 数据 / 关于（对应原型 #page-settings，开关为原型 data-toggle 行为） */
import { useState } from "react";
import { Ic } from "../lib/icons";
import { t } from "../lib/i18n";
import type { Lang } from "../lib/i18n";
import { toast } from "../lib/toast";
import { ProviderBrandIcon } from "../lib/brand";

export default function Settings({ lang, setLang }: { lang: Lang; setLang: (l: Lang) => void }) {
  const [autoStart, setAutoStart] = useState(false);
  const [trayMin, setTrayMin] = useState(true);

  return (
    <div className="page-wrap">
      <div className="page-h">
        <div>
          <h1>{t("settings.title")}</h1>
          <div className="sub">Language / Tray / Autostart · Proxy: Global → Provider → Account</div>
        </div>
      </div>

      <div className="grid2">
        <div className="card">
          <div className="card-h">
            <span className="chipic c-blue">
              <Ic name="gear" />
            </span>
            <span className="ttl">通用</span>
          </div>
          <div className="set-row">
            <div className="si">
              <div className="st">界面语言</div>
              <div className="sd">切换即时生效，分享给不同语言习惯的同事</div>
            </div>
            <div className="ctrl">
              <div className="seg">
                <button className={lang === "zh" ? "on" : ""} onClick={() => setLang("zh")}>{t("settings.lang.zh")}</button>
                <button className={lang === "en" ? "on" : ""} onClick={() => setLang("en")}>{t("settings.lang.en")}</button>
              </div>
            </div>
          </div>
          <div className="set-row">
            <div className="si">
              <div className="st">开机自启</div>
              <div className="sd">登录 Windows 后自动启动并最小化到托盘</div>
            </div>
            <div className="ctrl">
              <button className={"sw" + (autoStart ? " on" : "")} onClick={() => setAutoStart(v => !v)}></button>
            </div>
          </div>
          <div className="set-row">
            <div className="si">
              <div className="st">关闭窗口时最小化到托盘</div>
              <div className="sd">网关继续在后台运行；托盘菜单可退出</div>
            </div>
            <div className="ctrl">
              <button className={"sw" + (trayMin ? " on" : "")} onClick={() => setTrayMin(v => !v)}></button>
            </div>
          </div>
          <div className="set-row">
            <div className="si">
              <div className="st">单实例</div>
              <div className="sd">二次启动唤起已有窗口，不开新实例（始终开启）</div>
            </div>
            <div className="ctrl">
              <span className="bdg b-gray">内置</span>
            </div>
          </div>
          <div className="set-row">
            <div className="si">
              <div className="st">凭据加密</div>
              <div className="sd">AES-256-GCM · zcode 凭据保持与官方 enc:v1 格式兼容</div>
            </div>
            <div className="ctrl">
              <span className="bdg b-ok">已启用</span>
            </div>
          </div>
        </div>

        <div className="card">
          <div className="card-h">
            <span className="chipic c-violet">
              <Ic name="globe" />
            </span>
            <span className="ttl">出站代理</span>
            <span className="grow"></span>
            <button className="btn xs" onClick={() => toast("ok", "代理连通", "socks5://127.0.0.1:7890 → gemini 上游 236ms")}>
              测试连接
            </button>
          </div>
          <div className="kv">
            <span className="k">全局默认</span>
            <select className="inp" style={{ width: 110 }}>
              <option>直连</option>
              <option>HTTP</option>
              <option>SOCKS5</option>
            </select>
            <input className="inp mono" style={{ flex: 1 }} placeholder="http://127.0.0.1:7890" disabled />
          </div>
          <div className="tblwrap" style={{ marginTop: 8 }}>
            <table className="tbl">
              <thead>
                <tr>
                  <th>覆盖范围</th>
                  <th>代理</th>
                  <th style={{ width: 50 }}></th>
                </tr>
              </thead>
              <tbody>
                <tr>
                  <td>
                    <span style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
                      <ProviderBrandIcon provider="gemini" size={18} />
                      gemini · 全部账号
                    </span>
                  </td>
                  <td className="mono" style={{ fontSize: "11.5px" }}>
                    socks5://127.0.0.1:7890
                  </td>
                  <td>
                    <button className="ibtn st-gear">
                      <Ic name="gear" />
                    </button>
                  </td>
                </tr>
                <tr>
                  <td>
                    <span style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
                      <ProviderBrandIcon provider="codearts" size={18} />
                      codearts · 全部账号
                    </span>
                  </td>
                  <td className="mono" style={{ fontSize: "11.5px", color: "var(--t4)" }}>
                    直连（继承全局）
                  </td>
                  <td>
                    <button className="ibtn st-gear">
                      <Ic name="gear" />
                    </button>
                  </td>
                </tr>
                <tr>
                  <td colSpan={3} style={{ textAlign: "center" }}>
                    <button className="btn xs">+ 按 Provider 添加覆盖</button>
                  </td>
                </tr>
              </tbody>
            </table>
          </div>
          <div className="mnote mt-s">账号级覆盖在账号详情里设置，优先级：账号 &gt; Provider &gt; 全局。</div>
        </div>
      </div>

      <div className="grid2" style={{ marginTop: 12 }}>
        <div className="card">
          <div className="card-h">
            <span className="chipic c-cyan">
              <Ic name="folder" />
            </span>
            <span className="ttl">数据</span>
          </div>
          <div className="kv">
            <span className="k">数据目录</span>
            <span className="mono" style={{ fontSize: 12 }}>
              C:\Users\me\.tokenmaster\
            </span>
          </div>
          <div className="kv">
            <span className="k">用量账本</span>
            <span className="mono" style={{ fontSize: 12 }}>
              ledger.jsonl · 18.2 MB · 32MB 滚动
            </span>
          </div>
          <div className="kv">
            <span className="k">账号文件</span>
            <span className="mono" style={{ fontSize: 12 }}>
              accounts/*.json · 原子写
            </span>
          </div>
          <div className="mt-s">
            <button className="btn sm">
              <Ic name="folder" />
              打开数据目录
            </button>
          </div>
        </div>
        <div className="card">
          <div className="card-h">
            <span className="chipic c-ok">
              <Ic name="info" />
            </span>
            <span className="ttl">关于</span>
          </div>
          <div className="kv">
            <span className="k">版本</span>
            <span className="mono" style={{ fontSize: 12 }}>
              0.1.0-dev · Tauri 2 + Rust (axum) · NSIS
            </span>
          </div>
          <div className="kv">
            <span className="k">许可</span>
            <span className="mono" style={{ fontSize: 12 }}>
              CC-BY-NC-SA-4.0（复用 Antigravity-Manager）
            </span>
          </div>
          <div className="kv">
            <span className="k">致谢</span>
            <span className="muted" style={{ fontSize: 12 }}>
              Antigravity-Manager · zcode-pool · deepseek-harness · commandcode-proxy · 品牌图标 LobeHub Icons（MIT）
            </span>
          </div>
        </div>
      </div>
    </div>
  );
}
