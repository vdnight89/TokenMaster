/**
 * 布局壳：topbar / navbar / nav-pills / content / page 结构与 round-02 原型 index.html 对齐。
 * 七页常驻挂载、按 .page/.on 切换（与原型 DOM 行为一致，含 160ms 淡入上移动效）；
 * 网关开关 / 待办完成 / 账号启停等跨页状态在此提升，模态与 toast 全局挂载。
 */
import { useEffect, useState } from "react";
import type { ReactElement } from "react";
import { PAGES, useHashPage } from "./lib/nav";
import type { PageKey } from "./lib/nav";
import { Ic } from "./lib/icons";
import { toast, ToastHost } from "./lib/toast";
import type { ModalKind } from "./lib/ui";
import { ACCOUNTS, TODOS } from "./lib/mock";
import type { Account } from "./lib/mock";
import Dash from "./pages/Dash";
import Accounts from "./pages/Accounts";
import Gateway from "./pages/Gateway";
import Usage from "./pages/Usage";
import Logs from "./pages/Logs";
import Setup from "./pages/Setup";
import Settings from "./pages/Settings";
import AddModal from "./modals/AddModal";
import ClaimModal from "./modals/ClaimModal";
import ImportModal from "./modals/ImportModal";
import { winClose, winMinimize, winToggleMaximize } from "./lib/window";
import { useLang } from "./lib/i18n";
import { useAccounts } from "./lib/ipc";

export default function App() {
  const [page, go] = useHashPage();
  const [gwOn, setGwOn] = useState(true);
  const [lang, setLang] = useLang();
  const [accounts, setAccounts] = useState<Account[]>(ACCOUNTS);
  // Tauri 环境下用 IPC 真数据替换 mock；浏览器 dev 保持 mock
  // 类型暂时 cast（IPC 数据形状与 mock Account 对齐是 T5.11 后续工作）
  const { data: ipcAccounts, isReal: accountsLive } = useAccounts([] as never[]);
  useEffect(() => {
    if (accountsLive) setAccounts(ipcAccounts as unknown as Account[]);
  }, [ipcAccounts, accountsLive]);
  const [doneTodos, setDoneTodos] = useState<ReadonlySet<string>>(() => new Set());
  const [modal, setModal] = useState<ModalKind>(null);

  /* 切页回到顶部（对应原型 nav() 里的 scrollTop = 0） */
  useEffect(() => {
    const c = document.querySelector(".content");
    if (c) c.scrollTop = 0;
  }, [page]);

  const todoCount = TODOS.length - doneTodos.size;

  const toggleGw = () => {
    const next = !gwOn;
    setGwOn(next);
    toast(
      next ? "ok" : "info",
      next ? "网关已启动" : "网关已停止",
      next ? "监听 127.0.0.1:8787 · OpenAI / Anthropic 双协议面就绪" : "进行中的流式请求已优雅收尾",
    );
  };

  const togglePower = (name: string) => {
    const a = accounts.find(x => x.name === name);
    if (!a) return;
    if (a.state === "off") {
      setAccounts(accounts.map(x => (x.name === name ? { ...x, state: "ok" as const } : x)));
      toast("ok", "账号已启用", a.name + " · 重新参与轮询");
    } else {
      setAccounts(accounts.map(x => (x.name === name ? { ...x, state: "off" as const } : x)));
      toast("info", "账号已停用", a.name + " · 已移出轮询");
    }
  };

  const markTodoDone = (id: string) => {
    setDoneTodos(prev => {
      const nx = new Set(prev);
      nx.add(id);
      return nx;
    });
  };

  const openModal = (m: Exclude<ModalKind, null>) => setModal(m);

  const pageEl: Record<PageKey, ReactElement> = {
    dash: <Dash active={page === "dash"} gwOn={gwOn} doneTodos={doneTodos} onTodoDone={markTodoDone} openModal={openModal} nav={go} />,
    accounts: <Accounts accounts={accounts} onTogglePower={togglePower} openModal={openModal} />,
    gateway: <Gateway gwOn={gwOn} onToggleGw={toggleGw} />,
    usage: <Usage />,
    logs: <Logs />,
    setup: <Setup />,
    settings: <Settings lang={lang} setLang={setLang} />,
  };

  return (
    <>
      {/* 顶栏 = 拖拽区 + 窗控（T6.0 自定义标题栏；浏览器环境下窗控 no-op） */}
      <div className="topbar" data-tauri-drag-region onDoubleClick={winToggleMaximize}>
        <div className="traffic" onDoubleClick={e => e.stopPropagation()}>
          <button className="t-close" title="关闭" onClick={winClose}>
            <Ic name="x" size={8} />
          </button>
          <button className="t-min" title="最小化" onClick={winMinimize}>
            <Ic name="minus" size={8} />
          </button>
          <button className="t-max" title="最大化 / 还原" onClick={winToggleMaximize}>
            <Ic name="square" size={8} />
          </button>
        </div>
        <span>TokenMaster</span>
        <span className="grow"></span>
        <span className="mono">v0.1.0-dev</span>
      </div>

      {/* 导航 */}
      <nav className="navbar">
        <div className="brand">
          <div className="mark">TM</div>
          <div>
            <div className="name">TokenMaster</div>
            <div className="ver">15 providers · 42 accounts</div>
          </div>
        </div>
        <div className="nav-pills">
          {PAGES.map(p => (
            <button key={p.key} className={page === p.key ? "on" : ""} onClick={() => go(p.key as PageKey)}>
              {p.label()}
              {p.key === "dash" && <span className="cnt">{todoCount}</span>}
            </button>
          ))}
        </div>
        <div className="nav-right">
          <div className={"gw-chip" + (gwOn ? "" : " off")}>
            <span className="dot"></span>
            <span>{gwOn ? "运行中 · :8787" : "已停止"}</span>
          </div>
          <button className="iconbtn" title="网关设置" onClick={() => go("gateway")}>
            <Ic name="gear" />
          </button>
        </div>
      </nav>

      <div className="content" style={{ flex: 1, overflowY: "auto" }}>
        {PAGES.map(p => (
          <section key={p.key} id={"page-" + p.key} className={"page" + (page === p.key ? " on" : "")}>
            {pageEl[p.key]}
          </section>
        ))}
      </div>

      {modal === "add" && <AddModal onClose={() => setModal(null)} />}
      {modal === "claim" && <ClaimModal onClose={() => setModal(null)} onSuccess={() => markTodoDone("claim")} />}
      {modal === "import" && <ImportModal onClose={() => setModal(null)} />}

      <ToastHost />
    </>
  );
}
